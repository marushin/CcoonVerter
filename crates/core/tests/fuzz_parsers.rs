//! Deterministic mutation fuzzing of the code that handles untrusted bytes:
//! the embedded-JPEG scanner and the EXIF/TIFF parsing and patching.
//!
//! Stable Rust, no extra tools. Iteration `n` of seed `s` always produces the
//! same input, whatever the thread count, so any finding can be reproduced
//! with `CCOON_FUZZ_ONLY=<n>`. Seeds are real JPEG and TIFF structures: encoder
//! output with full EXIF, a synthetic DNG, the headers and camera JPEGs of
//! `tests/samples/` when present.
//!
//! A finding is a panic, an iteration that allocates far more than its input
//! justifies, or one that runs too long, on our own code paths. Set
//! `CCOON_FUZZ_RAWLER=1` to also exercise rawler's parsers: their panics are
//! reported separately, but rawler can still ask for an impossible allocation
//! on some damaged files, which aborts the whole process (see
//! `docs/V0.12_REPORT.md`), so that target is off by default.
//!
//! Long run (release build, all cores):
//! `CCOON_FUZZ_SECS=600 cargo test --release -p ccoonverter-core --test fuzz_parsers
//! fuzz_long -- --ignored --nocapture`

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ccoonverter_core::encode::{self, OutputMeta};
use ccoonverter_core::exif::{self, ExifInfo, GpsInfo};
use ccoonverter_core::metering;
use ccoonverter_core::rawler;
use ccoonverter_core::settings::ColorSpace;
use ccoonverter_core::testutil::{synthetic_dng, DngSpec};
use ccoonverter_core::{embedded, RgbF32Image};
use image::{Rgb, RgbImage};

// ---------------------------------------------------------------------------
// Allocation accounting (per thread, no destructors)
// ---------------------------------------------------------------------------

struct Counting;

thread_local! {
    static LIVE: Cell<usize> = const { Cell::new(0) };
    static PEAK: Cell<usize> = const { Cell::new(0) };
}

/// A single request this large aborts the process, so nothing could report
/// it afterwards: print the iteration and a backtrace first.
const HUGE_REQUEST: usize = 8 << 30;

thread_local! {
    static ITERATION: Cell<u64> = const { Cell::new(u64::MAX) };
    static REPORTING: Cell<bool> = const { Cell::new(false) };
}

fn report_huge(size: usize) {
    if REPORTING.try_with(|r| r.replace(true)).unwrap_or(true) {
        return;
    }
    let n = ITERATION.try_with(|i| i.get()).unwrap_or(u64::MAX);
    eprintln!(
        "fuzz: iteration {n} requested {size} bytes at\n{}",
        std::backtrace::Backtrace::force_capture()
    );
    let _ = REPORTING.try_with(|r| r.set(false));
}

fn note_alloc(size: usize) {
    if size >= HUGE_REQUEST {
        report_huge(size);
    }
    let _ = LIVE.try_with(|l| {
        let v = l.get().saturating_add(size);
        l.set(v);
        let _ = PEAK.try_with(|p| p.set(p.get().max(v)));
    });
}

fn note_free(size: usize) {
    let _ = LIVE.try_with(|l| l.set(l.get().saturating_sub(size)));
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_alloc(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_alloc(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note_free(layout.size());
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_free(layout.size());
        note_alloc(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

// ---------------------------------------------------------------------------
// Deterministic random numbers
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new(seed: u64, n: u64) -> Self {
        // SplitMix-style scramble so neighbouring iterations are unrelated.
        let mut z = seed ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        Self((z ^ (z >> 31)) | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

// ---------------------------------------------------------------------------
// Seeds
// ---------------------------------------------------------------------------

fn full_info() -> ExifInfo {
    ExifInfo {
        make: Some("SONY".into()),
        model: Some("ILCE-7M4".into()),
        orientation: Some(6),
        date_time_original: Some("2026:01:02 03:04:05".into()),
        exposure_time: Some((1, 250)),
        fnumber: Some((28, 10)),
        iso: Some(400),
        exposure_bias: Some((-1, 3)),
        focal_length: Some((35, 1)),
        lens_model: Some("FE 35mm".into()),
        serial_number: Some("1234".into()),
        lens_spec: Some([(35, 1), (35, 1), (18, 10), (18, 10)]),
        gps: Some(GpsInfo {
            latitude: Some([(37, 1), (33, 1), (1, 1)]),
            latitude_ref: Some("N".into()),
            longitude: Some([(126, 1), (58, 1), (2, 1)]),
            longitude_ref: Some("E".into()),
            ..Default::default()
        }),
        rating: Some(4),
        output_color_space: Some(1),
        ..Default::default()
    }
}

fn samples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/samples")
}

const MAX_INPUT: usize = 1 << 20;

fn seeds() -> Vec<Vec<u8>> {
    let info = full_info();
    let tiff = exif::build_exif(&info, 6).unwrap();
    let img = RgbImage::from_fn(48, 32, |x, y| Rgb([(x * 5) as u8, (y * 7) as u8, 90]));
    let jpeg = encode::encode_rgb8(&img, 85, Some(&tiff)).unwrap();
    let thumb = encode::encode_rgb8(&img, 40, None).unwrap();
    let mut nested = tiff.clone();
    nested.extend_from_slice(&thumb);
    let jpeg_nested = encode::encode_rgb8(&img, 90, Some(&nested)).unwrap();
    let mut with_xmp = jpeg.clone();
    with_xmp.extend_from_slice(exif::xmp_rating_xml(3).as_bytes());
    // A sidecar with the structures real ones have.
    let sidecar_seed = br#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/"
 xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" xmp:Rating="3" xmp:Label="Red" crs:Version="16.0">
<crs:ToneCurvePV2012><rdf:Seq><rdf:li>0, 0</rdf:li></rdf:Seq></crs:ToneCurvePV2012>
<xmp:Rating>2</xmp:Rating></rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end="w"?>"#
        .to_vec();

    let mut out = vec![
        sidecar_seed,
        tiff,
        jpeg.clone(),
        jpeg_nested,
        with_xmp,
        synthetic_dng(&DngSpec {
            width: 32,
            height: 24,
            preview: Some((16, 12)),
            ..Default::default()
        }),
    ];
    let mut blob = vec![0x42u8; 256];
    blob.extend_from_slice(&jpeg);
    blob.extend_from_slice(&[0xFF; 32]);
    blob.extend_from_slice(&thumb);
    out.push(blob);

    let Ok(dir) = std::fs::read_dir(samples_dir()) else {
        return out;
    };
    for entry in dir.flatten() {
        let path = entry.path();
        let is_raw = path.extension().is_some_and(|e| {
            ccoonverter_core::config::is_supported_extension(&e.to_string_lossy())
        });
        if !is_raw {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        // Real TIFF/CR3 structure at the start of the file.
        out.push(bytes[..bytes.len().min(96 * 1024)].to_vec());
        // Real camera JPEGs and their EXIF: smallest ones keep iterations fast.
        let mut found = embedded::find_jpegs(&bytes);
        found.sort_by_key(|c| c.len);
        for c in found.iter().take(2) {
            let j = c.bytes(&bytes);
            if let Some(t) = exif::jpeg_exif(j) {
                out.push(t);
            }
            if j.len() <= MAX_INPUT {
                out.push(j.to_vec());
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Mutations
// ---------------------------------------------------------------------------

const INTERESTING_U16: [u16; 8] = [0, 1, 2, 0x7FFF, 0x8000, 0xFFFE, 0xFFFF, 0x0112];
const INTERESTING_U32: [u32; 8] = [
    0,
    1,
    8,
    0x7FFF_FFFF,
    0x8000_0000,
    0xFFFF_FFF0,
    0xFFFF_FFFF,
    0x0100_0000,
];
const MARKERS: [&[u8]; 12] = [
    b"\xFF\xD8\xFF",
    b"\xFF\xC0\x00\x11\x08\xFF\xFF\xFF\xFF\x03\x01\x11\x00\x02\x11\x01\x03\x11\x01",
    b"\xFF\xC2\x00\x0B\x08\x00\x10\x00\x10\x01\x01\x11\x00",
    b"\xFF\xC3\x00\x0B\x08\x00\x10\x00\x10\x01\x01\x11\x00",
    b"\xFF\xDA\x00\x08\x01\x01\x00\x00\x3F\x00",
    b"\xFF\xD9",
    b"\xFF\xE1\xFF\xFFExif\0\0II*\0",
    b"\xFF\xE1\x00\x02",
    b"II*\0\x08\0\0\0",
    b"MM\0*\0\0\0\x08",
    b"xmp:Rating=\"",
    b"\xFF\xE2\x00\x10ICC_PROFILE\0\x01\x01",
];

fn rewrite_ifd_entry(rng: &mut Rng, v: &mut [u8]) {
    // TIFF-aware: pick the directory at the header offset and damage an entry.
    if v.len() < 16 || !(v[..2] == *b"II" || v[..2] == *b"MM") {
        return;
    }
    let le = v[0] == b'I';
    let rd32 = |b: &[u8]| {
        let a = [b[0], b[1], b[2], b[3]];
        if le {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        }
    };
    let ifd = rd32(&v[4..8]) as usize;
    if ifd + 2 >= v.len() {
        return;
    }
    let e = ifd + 2 + 12 * rng.below(32);
    if e + 12 > v.len() {
        return;
    }
    let field = 2 + rng.below(3) * 2 + rng.below(2) * 4; // type, count, value
    let w = INTERESTING_U32[rng.below(INTERESTING_U32.len())];
    let bytes = if le { w.to_le_bytes() } else { w.to_be_bytes() };
    for (i, b) in bytes.iter().enumerate() {
        if let Some(slot) = v.get_mut(e + field + i) {
            *slot = *b;
        }
    }
}

fn mutate(rng: &mut Rng, seeds: &[Vec<u8>], v: &mut Vec<u8>) {
    let rounds = 1 + rng.below(8);
    for _ in 0..rounds {
        if v.is_empty() {
            v.push(0);
        }
        let n = v.len();
        match rng.below(14) {
            0 => {
                let i = rng.below(n);
                v[i] ^= 1 << rng.below(8);
            }
            1 => {
                let i = rng.below(n);
                v[i] = [0x00, 0x01, 0x7F, 0x80, 0xFF, 0xD8, 0xD9, 0xDA][rng.below(8)];
            }
            2 => {
                let i = rng.below(n.saturating_sub(1));
                let w = INTERESTING_U16[rng.below(8)].to_be_bytes();
                v[i..(i + 2).min(n)].copy_from_slice(&w[..(n - i).min(2)]);
            }
            3 => {
                let i = rng.below(n.saturating_sub(3));
                let w = INTERESTING_U32[rng.below(8)];
                let b = if rng.below(2) == 0 {
                    w.to_le_bytes()
                } else {
                    w.to_be_bytes()
                };
                let end = (i + 4).min(n);
                v[i..end].copy_from_slice(&b[..end - i]);
            }
            4 => v.truncate(rng.below(n + 1)),
            5 => {
                let i = rng.below(n);
                let len = rng.below((n - i).min(4096) + 1);
                v.drain(i..i + len);
            }
            6 => {
                let i = rng.below(n + 1);
                let m = MARKERS[rng.below(MARKERS.len())];
                v.splice(i..i, m.iter().copied());
            }
            7 => {
                // Overwrite with a marker in place (keeps later offsets valid).
                let m = MARKERS[rng.below(MARKERS.len())];
                let i = rng.below(n);
                let end = (i + m.len()).min(n);
                v[i..end].copy_from_slice(&m[..end - i]);
            }
            8 => {
                // Splice from another seed.
                let other = &seeds[rng.below(seeds.len())];
                if !other.is_empty() {
                    let a = rng.below(other.len());
                    let len = rng.below((other.len() - a).min(64 * 1024) + 1);
                    let i = rng.below(n + 1);
                    v.splice(i..i, other[a..a + len].iter().copied());
                }
            }
            9 => {
                // Duplicate a region (nested streams, repeated markers).
                let a = rng.below(n);
                let len = rng.below((n - a).min(8192) + 1);
                let block = v[a..a + len].to_vec();
                let i = rng.below(n + 1);
                v.splice(i..i, block);
            }
            10 => {
                // Repeat a marker many times: stresses the scanner's work bound.
                let m = MARKERS[rng.below(MARKERS.len())];
                let times = 1 + rng.below(2000);
                let i = rng.below(n + 1);
                let block: Vec<u8> = m.iter().copied().cycle().take(m.len() * times).collect();
                v.splice(i..i, block);
            }
            11 => {
                let i = rng.below(n);
                let len = rng.below((n - i).min(512) + 1);
                let byte = [0x00, 0xFF][rng.below(2)];
                v[i..i + len].fill(byte);
            }
            _ => rewrite_ifd_entry(rng, v),
        }
        v.truncate(MAX_INPUT);
    }
}

// ---------------------------------------------------------------------------
// Targets
// ---------------------------------------------------------------------------

fn exercise_tiff(t: &[u8], info: &ExifInfo) {
    let _ = exif::lacks_fields(t, info);
    let _ = exif::tiff_orientation(t);
    let mut c = t.to_vec();
    let _ = exif::set_tiff_orientation(&mut c, 1);
    let _ = exif::set_tiff_color_space(&mut c, ColorSpace::AdobeRgb);
    let _ = exif::set_tiff_color_space(&mut c, ColorSpace::Srgb);
    let _ = exif::merge_exif(t, info, 1);
    let _ = encode::exif_fields_for_tiff(t);
    let tiny = RgbF32Image::from_pixel(2, 2, Rgb([0.5, 0.25, 0.75]));
    let meta = OutputMeta {
        exif: Some(t),
        ..Default::default()
    };
    let _ = encode::encode_tiff16(&tiny, &meta);
}

fn exercise_jpeg(j: &[u8], info: &ExifInfo) {
    let own = exif::jpeg_exif(j);
    if let Some(t) = &own {
        exercise_tiff(t, info);
    }
    let _ = exif::add_xmp_rating(j, 3);
    let _ = exif::replace_jpeg_exif(j, own.as_deref());
    let _ = exif::replace_jpeg_exif(j, None);
    let _ = image::load_from_memory_with_format(j, image::ImageFormat::Jpeg);
}

/// Our own code paths on untrusted bytes.
fn target_own(input: &[u8], info: &ExifInfo) {
    let _ = exif::find_xmp_rating(input);
    let candidates = embedded::find_jpegs(input);
    let _ = embedded::largest_jpeg_in(input);
    let _ = ccoonverter_core::thumbnail::thumbnail_from_bytes(input, 256);
    let _ = ccoonverter_core::sidecar::parse_rating(input);
    for rating in [None, Some(0u8), Some(4)] {
        if let Some(out) = ccoonverter_core::sidecar::with_rating(input, rating) {
            // A successful change must read back as what was asked.
            let wanted = rating.map(i32::from);
            assert_eq!(ccoonverter_core::sidecar::parse_rating(&out), Ok(wanted));
        }
    }
    for c in candidates.iter().take(4) {
        exercise_jpeg(c.bytes(input), info);
    }
    if input.starts_with(b"II") || input.starts_with(b"MM") {
        exercise_tiff(input, info);
    }
    if input.starts_with(&[0xFF, 0xD8]) {
        exercise_jpeg(input, info);
    }
    let _ = metering::preview_target_luma(input, 4000, 3000);
}

/// rawler's container parsing, as reached from `RawCtx`.
fn target_rawler(input: &[u8]) {
    use rawler::decoders::RawDecodeParams;
    use rawler::rawsource::RawSource;
    let source = RawSource::new_from_slice(input);
    if let Ok(d) = ccoonverter_core::rawguard::get_decoder(&source) {
        let _ = d.raw_metadata(&source, &RawDecodeParams::default());
        let _ = d.raw_image(&source, &RawDecodeParams::default(), true);
    }
}

thread_local! {
    static LAST_PANIC: RefCell<String> = const { RefCell::new(String::new()) };
}

const MAX_ITERATION: Duration = Duration::from_secs(3);

fn alloc_limit(input_len: usize) -> usize {
    // 300 MiB is the most a decoder may reasonably want for a 1 MiB input.
    (300 << 20) + input_len * 64
}

#[derive(Default)]
struct Report {
    iterations: AtomicU64,
    own_panics: Mutex<BTreeMap<String, (u64, u64)>>,
    rawler_panics: Mutex<BTreeMap<String, (u64, u64)>>,
    heavy: Mutex<Vec<(u64, usize, usize)>>,
    slow: Mutex<Vec<(u64, usize, u128)>>,
    peak_alloc: AtomicU64,
    slowest_ms: AtomicU64,
}

impl Report {
    fn note_panic(map: &Mutex<BTreeMap<String, (u64, u64)>>, key: String, n: u64) {
        let mut m = map.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(key).or_insert((0, n)).0 += 1;
    }
}

fn input_for(seed: u64, n: u64, seeds: &[Vec<u8>]) -> Vec<u8> {
    let mut rng = Rng::new(seed, n);
    let mut input = seeds[rng.below(seeds.len())].clone();
    mutate(&mut rng, seeds, &mut input);
    input
}

fn run_one(seed: u64, n: u64, seeds: &[Vec<u8>], info: &ExifInfo, report: &Report) {
    let input = input_for(seed, n, seeds);
    ITERATION.with(|i| i.set(n));
    LIVE.with(|l| l.set(0));
    PEAK.with(|p| p.set(0));
    let start = Instant::now();
    if panic::catch_unwind(AssertUnwindSafe(|| target_own(&input, info))).is_err() {
        let key = LAST_PANIC.with(|p| p.borrow().clone());
        Report::note_panic(&report.own_panics, key, n);
    }
    let elapsed = start.elapsed();
    let peak = PEAK.with(|p| p.get());
    report.peak_alloc.fetch_max(peak as u64, Ordering::Relaxed);
    report
        .slowest_ms
        .fetch_max(elapsed.as_millis() as u64, Ordering::Relaxed);
    if peak > alloc_limit(input.len()) {
        report
            .heavy
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((n, input.len(), peak));
    }
    if elapsed > MAX_ITERATION {
        report.slow.lock().unwrap_or_else(|e| e.into_inner()).push((
            n,
            input.len(),
            elapsed.as_millis(),
        ));
    }
    if include_rawler() && panic::catch_unwind(AssertUnwindSafe(|| target_rawler(&input))).is_err()
    {
        let key = LAST_PANIC.with(|p| p.borrow().clone());
        Report::note_panic(&report.rawler_panics, key, n);
    }
}

fn include_rawler() -> bool {
    std::env::var_os("CCOON_FUZZ_RAWLER").is_some_and(|v| v == "1")
}

fn run(duration: Option<Duration>, iterations: u64, seed: u64, threads: usize) -> Report {
    let seeds = seeds();
    let info = full_info();
    let report = Report::default();
    let next = AtomicU64::new(0);
    let stop = AtomicBool::new(false);
    let start = Instant::now();
    let previous_hook = panic::take_hook();
    panic::set_hook(Box::new(|p| {
        let loc = p
            .location()
            .map(|l| format!("{}:{}", l.file().replace('\\', "/"), l.line()))
            .unwrap_or_else(|| "unknown".into());
        let _ = LAST_PANIC.try_with(|s| *s.borrow_mut() = loc);
    }));
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let n = next.fetch_add(1, Ordering::Relaxed);
                if n >= iterations {
                    break;
                }
                run_one(seed, n, &seeds, &info, &report);
                report.iterations.fetch_add(1, Ordering::Relaxed);
                if let Some(limit) = duration {
                    if start.elapsed() >= limit {
                        stop.store(true, Ordering::Relaxed);
                    }
                }
            });
        }
    });
    panic::set_hook(previous_hook);
    report
}

fn print(report: &Report, elapsed: Duration, seed: u64) {
    let n = report.iterations.load(Ordering::Relaxed);
    println!(
        "fuzz: seed {seed:#x}, {n} iterations in {:.1} s ({:.0}/s), peak allocation {:.1} MiB, slowest iteration {} ms",
        elapsed.as_secs_f64(),
        n as f64 / elapsed.as_secs_f64().max(1e-3),
        report.peak_alloc.load(Ordering::Relaxed) as f64 / (1 << 20) as f64,
        report.slowest_ms.load(Ordering::Relaxed),
    );
    let show = |title: &str, map: &Mutex<BTreeMap<String, (u64, u64)>>| {
        let m = map.lock().unwrap_or_else(|e| e.into_inner());
        println!("fuzz: {title}: {} location(s)", m.len());
        for (loc, (count, first)) in m.iter() {
            println!("  {count:>8}x {loc} (first at iteration {first})");
        }
    };
    show("panics on our code paths", &report.own_panics);
    show(
        "panics inside rawler (contained per file)",
        &report.rawler_panics,
    );
    let heavy = report.heavy.lock().unwrap_or_else(|e| e.into_inner());
    println!("fuzz: over the allocation limit: {}", heavy.len());
    for (i, len, peak) in heavy.iter().take(10) {
        println!("  iteration {i}: input {len} bytes, peak {peak} bytes");
    }
    let slow = report.slow.lock().unwrap_or_else(|e| e.into_inner());
    println!("fuzz: over the time limit: {}", slow.len());
    for (i, len, ms) in slow.iter().take(10) {
        println!("  iteration {i}: input {len} bytes, {ms} ms");
    }
}

fn assert_clean(report: &Report) {
    let own = report.own_panics.lock().unwrap_or_else(|e| e.into_inner());
    assert!(own.is_empty(), "panics on our code paths: {own:?}");
    let heavy = report.heavy.lock().unwrap_or_else(|e| e.into_inner());
    assert!(heavy.is_empty(), "allocation limit exceeded: {heavy:?}");
    let slow = report.slow.lock().unwrap_or_else(|e| e.into_inner());
    assert!(slow.is_empty(), "time limit exceeded: {slow:?}");
}

/// Short run in the normal test suite.
#[test]
fn fuzz_smoke() {
    let start = Instant::now();
    let report = run(None, 800, 0xC0_0C, 2);
    print(&report, start.elapsed(), 0xC0_0C);
    assert_clean(&report);
}

/// Same iteration, same input: the harness is reproducible.
#[test]
fn inputs_are_deterministic() {
    let seeds = seeds();
    for n in [0u64, 1, 2, 1000, 999_999] {
        assert_eq!(input_for(7, n, &seeds), input_for(7, n, &seeds));
    }
    assert_ne!(input_for(7, 1, &seeds), input_for(8, 1, &seeds));
}

/// Time-boxed run: `CCOON_FUZZ_SECS` (default 60), `CCOON_FUZZ_SEED`,
/// `CCOON_FUZZ_THREADS`, or `CCOON_FUZZ_ONLY=<iteration>` to replay one input.
#[test]
#[ignore = "long-running; see the module documentation"]
fn fuzz_long() {
    let var = |n: &str| std::env::var(n).ok().and_then(|v| v.parse::<u64>().ok());
    let seed = var("CCOON_FUZZ_SEED").unwrap_or(0xCC00_0012);
    if let Some(n) = var("CCOON_FUZZ_ONLY") {
        let seeds = seeds();
        let input = input_for(seed, n, &seeds);
        println!("replaying iteration {n}: {} bytes", input.len());
        target_own(&input, &full_info());
        if include_rawler() {
            target_rawler(&input);
        }
        return;
    }
    let secs = var("CCOON_FUZZ_SECS").unwrap_or(60);
    let threads = var("CCOON_FUZZ_THREADS").unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get() as u64)
            .unwrap_or(4)
    }) as usize;
    let start = Instant::now();
    let report = run(Some(Duration::from_secs(secs)), u64::MAX, seed, threads);
    print(&report, start.elapsed(), seed);
    assert_clean(&report);
}
