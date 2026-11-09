//! Dark native title bar on Windows (best effort).
//!
//! winit's `SetTheme(Dark)` does not darken the caption on current Windows 11
//! builds, so on Windows we call `DwmSetWindowAttribute` once after the window
//! exists: immersive dark mode plus caption / text / border colours from the
//! app palette (Windows 11 22000+ honours the colours; older builds ignore
//! them). Every failure is ignored and logged at debug level. On other
//! platforms this is a no-op.

use eframe::egui::Color32;

/// `COLORREF` (0x00BBGGRR) of an egui colour.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn colorref(c: Color32) -> u32 {
    u32::from(c.r()) | (u32::from(c.g()) << 8) | (u32::from(c.b()) << 16)
}

/// Apply the dark caption to the main window. Safe to call more than once.
#[cfg(windows)]
pub fn apply(frame: &eframe::Frame) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR,
        DWMWA_USE_IMMERSIVE_DARK_MODE,
    };

    use crate::theme::{BG, BORDER, TEXT};

    let hwnd: HWND = match frame.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get() as HWND,
        Ok(other) => {
            log::debug!("Title bar: unexpected window handle {other:?}");
            return;
        }
        Err(e) => {
            log::debug!("Title bar: no window handle ({e})");
            return;
        }
    };

    let set = |attr: i32, value: u32| {
        // SAFETY: `hwnd` is the live top-level window owned by eframe, and the
        // pointer/size describe a valid u32 (BOOL / COLORREF) on the stack.
        let hr = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                attr as u32,
                (&value as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
            )
        };
        if hr != 0 {
            log::debug!("DwmSetWindowAttribute({attr}) failed: HRESULT {hr:#010x}");
        }
    };
    set(DWMWA_USE_IMMERSIVE_DARK_MODE, 1);
    set(DWMWA_CAPTION_COLOR, colorref(BG));
    set(DWMWA_TEXT_COLOR, colorref(TEXT));
    set(DWMWA_BORDER_COLOR, colorref(BORDER));
}

/// Apply the dark caption to the main window (no-op off Windows).
#[cfg(not(windows))]
pub fn apply(_frame: &eframe::Frame) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorref_is_bgr() {
        assert_eq!(colorref(Color32::from_rgb(0xD4, 0xAF, 0x37)), 0x0037_AFD4);
        assert_eq!(colorref(Color32::from_rgb(0x1e, 0x1e, 0x1e)), 0x001E_1E1E);
        assert_eq!(colorref(Color32::from_rgb(0x3d, 0x3d, 0x3d)), 0x003D_3D3D);
    }
}
