//! RAW development with rawler (replacement for LibRaw `raw.postprocess`).
//!
//! rawler 0.8.0 API used (read from the vendored source):
//! `RawDevelop::new_with(&[ProcessingStep])` + `develop_intermediate(&RawImage)
//! -> Intermediate::{Monochrome, ThreeColor, FourColor}` with f32 data.
//! We run every step except `SRgb`, so the result is **linear** sRGB
//! (camera matrix applied, highlights soft-clipped by rawler around 1.0).
//! Then, in this module:
//!
//! 1. optional auto white balance (grey world on unclipped pixels),
//! 2. auto exposure like dcraw/LibRaw with `no_auto_bright=False`: the
//!    per-channel 99th percentile is mapped to white (1 % highlight clip),
//!    multiplied by the user `brightness` (LibRaw `bright`),
//! 3. sRGB transfer curve, clamped to 0..1,
//! 4. EXIF orientation applied to the pixels (LibRaw rotates too).

use std::sync::atomic::{AtomicBool, Ordering};

use rawler::decoders::{Decoder, RawDecodeParams};
use rawler::imgop::develop::{Intermediate, ProcessingStep, RawDevelop};
use rawler::rawsource::RawSource;
use rayon::prelude::*;

use crate::error::{ConvertError, Result};
use crate::settings::WhiteBalance;
use crate::RgbF32Image;

/// Upper bound of the auto exposure gain (dcraw has none; this guards
/// against amplifying noise of an all-black frame to grey mush).
pub const MAX_AUTO_GAIN: f32 = 16.0;
/// Fraction of pixels allowed to clip to white (dcraw: 1 %).
pub const AUTO_BRIGHT_CLIP: f32 = 0.01;

fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(ConvertError::Cancelled)
    } else {
        Ok(())
    }
}

/// Develop a RAW into a display-referred sRGB f32 image (0..1), upright.
pub fn develop(
    source: &RawSource,
    decoder: &dyn Decoder,
    white_balance: WhiteBalance,
    brightness: f32,
    cancel: &AtomicBool,
) -> Result<RgbF32Image> {
    let params = RawDecodeParams::default();
    let mut raw = decoder.raw_image(source, &params, false)?;
    check(cancel)?;

    normalize_wb(&mut raw.wb_coeffs);
    let orientation = raw.orientation.to_u16();
    log::debug!(
        "develop: {} {} {}x{} cpp={} wb={:?} orientation={}",
        raw.clean_make,
        raw.clean_model,
        raw.width,
        raw.height,
        raw.cpp,
        raw.wb_coeffs,
        orientation
    );

    let dev = RawDevelop::new_with(&[
        ProcessingStep::Rescale,
        ProcessingStep::Demosaic,
        ProcessingStep::FujiRotate,
        ProcessingStep::CropActiveArea,
        ProcessingStep::WhiteBalance,
        ProcessingStep::Calibrate,
        ProcessingStep::CropDefault,
    ]);
    let inter = dev.develop_intermediate(&raw)?;
    drop(raw);
    check(cancel)?;

    let mut img = intermediate_to_rgb(inter)?;
    check(cancel)?;

    if white_balance == WhiteBalance::Auto {
        let gains = grey_world_gains(&img);
        log::debug!("develop: auto WB gains {gains:?}");
        apply_gains(&mut img, gains);
    }

    let white = percentile_white(&img, AUTO_BRIGHT_CLIP);
    let gain = auto_gain(white, brightness);
    log::debug!("develop: auto-bright white={white:.4} gain={gain:.3} (brightness {brightness})");
    encode_srgb(&mut img, gain);
    check(cancel)?;

    Ok(crate::resize::apply_orientation(img, orientation))
}

/// Normalise white balance multipliers so the smallest of R/G/B is 1
/// (dcraw convention); NaN/invalid coefficients become neutral.
pub fn normalize_wb(wb: &mut [f32; 4]) {
    if wb[..3].iter().any(|v| !v.is_finite() || *v <= 0.0) {
        *wb = [1.0, 1.0, 1.0, 1.0];
        return;
    }
    let min = wb[..3].iter().copied().fold(f32::INFINITY, f32::min);
    if !wb[3].is_finite() || wb[3] <= 0.0 {
        wb[3] = wb[1];
    }
    for v in wb.iter_mut() {
        *v /= min;
    }
}

fn intermediate_to_rgb(inter: Intermediate) -> Result<RgbF32Image> {
    let (w, h, data): (usize, usize, Vec<f32>) = match inter {
        Intermediate::ThreeColor(px) => {
            let (w, h) = (px.width, px.height);
            (w, h, px.data.into_iter().flatten().collect())
        }
        Intermediate::Monochrome(px) => {
            let (w, h) = (px.width, px.height);
            (w, h, px.data.iter().flat_map(|&v| [v, v, v]).collect())
        }
        Intermediate::FourColor(px) => {
            let (w, h) = (px.width, px.height);
            (
                w,
                h,
                px.data.iter().flat_map(|p| [p[0], p[1], p[2]]).collect(),
            )
        }
    };
    RgbF32Image::from_raw(w as u32, h as u32, data)
        .ok_or_else(|| ConvertError::Decode(format!("unexpected developed buffer for {w}x{h}")))
}

/// Grey-world white balance gains (G stays 1), ignoring near-black and
/// near-clipped pixels.
pub fn grey_world_gains(img: &RgbF32Image) -> [f32; 3] {
    let buf: &[f32] = img.as_ref();
    let sums = buf
        .par_chunks(3 * 4096)
        .map(|c| {
            let mut s = [0f64; 3];
            for p in c.as_chunks::<3>().0 {
                let mx = p[0].max(p[1]).max(p[2]);
                let mn = p[0].min(p[1]).min(p[2]);
                if mx < 0.95 && mn > 0.002 {
                    s[0] += p[0] as f64;
                    s[1] += p[1] as f64;
                    s[2] += p[2] as f64;
                }
            }
            s
        })
        .reduce(|| [0.0; 3], |a, b| [a[0] + b[0], a[1] + b[1], a[2] + b[2]]);
    if sums.iter().any(|s| *s <= 0.0) {
        return [1.0, 1.0, 1.0];
    }
    let g = sums[1];
    [
        (g / sums[0]).clamp(0.25, 4.0) as f32,
        1.0,
        (g / sums[2]).clamp(0.25, 4.0) as f32,
    ]
}

fn apply_gains(img: &mut RgbF32Image, g: [f32; 3]) {
    let buf: &mut [f32] = img.as_mut();
    buf.par_chunks_mut(3 * 4096).for_each(|c| {
        for p in c.as_chunks_mut::<3>().0 {
            p[0] *= g[0];
            p[1] *= g[1];
            p[2] *= g[2];
        }
    });
}

const HIST_BINS: usize = 8192;

/// dcraw's auto-bright white point: for each channel, the value above which
/// `clip_fraction` of the pixels lie; the maximum over channels.
#[allow(clippy::needless_range_loop)] // channel index into a [bin][channel] histogram
pub fn percentile_white(img: &RgbF32Image, clip_fraction: f32) -> f32 {
    let buf: &[f32] = img.as_ref();
    let n = buf.len() / 3;
    if n == 0 {
        return 1.0;
    }
    let hist = buf
        .par_chunks(3 * 4096)
        .map(|c| {
            let mut h = vec![[0u32; 3]; HIST_BINS];
            for p in c.as_chunks::<3>().0 {
                for ch in 0..3 {
                    let v = p[ch].clamp(0.0, 1.0);
                    let b = ((v * (HIST_BINS - 1) as f32) as usize).min(HIST_BINS - 1);
                    h[b][ch] += 1;
                }
            }
            h
        })
        .reduce(
            || vec![[0u32; 3]; HIST_BINS],
            |mut a, b| {
                for (x, y) in a.iter_mut().zip(b) {
                    x[0] += y[0];
                    x[1] += y[1];
                    x[2] += y[2];
                }
                a
            },
        );
    let limit = (n as f64 * clip_fraction as f64).round() as u64;
    let mut white = 0usize;
    for ch in 0..3 {
        let mut total = 0u64;
        let mut bin = HIST_BINS - 1;
        while bin > 0 {
            total += hist[bin][ch] as u64;
            if total > limit {
                break;
            }
            bin -= 1;
        }
        white = white.max(bin);
    }
    ((white + 1) as f32 / HIST_BINS as f32).min(1.0)
}

/// Exposure gain from the percentile white point and the user brightness.
pub fn auto_gain(white: f32, brightness: f32) -> f32 {
    let base = if white > 0.0 {
        (1.0 / white).min(MAX_AUTO_GAIN)
    } else {
        MAX_AUTO_GAIN
    };
    (base * brightness).max(0.0)
}

/// sRGB opto-electronic transfer function.
#[inline]
pub fn srgb_oetf(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn encode_srgb(img: &mut RgbF32Image, gain: f32) {
    let buf: &mut [f32] = img.as_mut();
    buf.par_chunks_mut(4096).for_each(|c| {
        for v in c.iter_mut() {
            *v = srgb_oetf((*v * gain).clamp(0.0, 1.0));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn wb_normalisation() {
        let mut wb = [2048.0, 1024.0, 1536.0, f32::NAN];
        normalize_wb(&mut wb);
        assert_eq!(wb, [2.0, 1.0, 1.5, 1.0]);
        let mut bad = [f32::NAN, 1.0, 1.0, 1.0];
        normalize_wb(&mut bad);
        assert_eq!(bad, [1.0; 4]);
    }

    #[test]
    fn percentile_white_ignores_top_one_percent() {
        // 1000 pixels: 990 at 0.25, 10 at 1.0 -> white ~0.25
        let mut img = RgbF32Image::from_pixel(1000, 1, Rgb([0.25, 0.25, 0.25]));
        for x in 0..10 {
            img.put_pixel(x, 0, Rgb([1.0, 1.0, 1.0]));
        }
        let w = percentile_white(&img, 0.01);
        assert!((w - 0.25).abs() < 0.001, "{w}");
        // With 20 bright pixels (2 %), white is 1.0.
        for x in 0..20 {
            img.put_pixel(x, 0, Rgb([1.0, 1.0, 1.0]));
        }
        assert!((percentile_white(&img, 0.01) - 1.0).abs() < 0.001);
        assert!((auto_gain(0.25, 1.0) - 4.0).abs() < 1e-6);
        assert!((auto_gain(0.25, 0.5) - 2.0).abs() < 1e-6);
        assert_eq!(auto_gain(0.0001, 1.0), MAX_AUTO_GAIN);
    }

    #[test]
    fn grey_world_balances_cast() {
        let img = RgbF32Image::from_pixel(10, 10, Rgb([0.4, 0.2, 0.1]));
        let g = grey_world_gains(&img);
        assert!((g[0] - 0.5).abs() < 1e-6 && (g[2] - 2.0).abs() < 1e-6);
    }

    #[test]
    fn srgb_curve_endpoints() {
        assert_eq!(srgb_oetf(0.0), 0.0);
        assert!((srgb_oetf(1.0) - 1.0).abs() < 1e-6);
        assert!((srgb_oetf(0.18) - 0.4613).abs() < 1e-3);
    }
}
