//! Output file name templates (new in 0.12.0).
//!
//! Tokens: `{name}` source stem, `{date}` `YYYY-MM-DD`, `{time}` `HHMMSS`,
//! `{seq}` batch position from 1 (four digits, more when needed), `{camera}`
//! model, `{ext}` lower-case source extension. The output extension itself
//! always follows the output format.
//!
//! Names are fixed before any conversion starts, on the task order of the
//! batch: `{seq}` and the batch collision suffixes never depend on which
//! worker finishes first. Metadata tokens read each file once, up front, and
//! only when the template uses them; the default template reads nothing.
//!
//! The template is rejected when it holds a path separator or `..`, so a name
//! cannot leave the output folder. Values from the files are sanitised for
//! Windows and macOS: reserved characters, trailing dots and spaces, device
//! names and length.

use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use rawler::decoders::RawDecodeParams;
use rawler::rawsource::RawSource;
use rayon::prelude::*;

use crate::exif::{self as exifmod, ExifInfo};
use crate::pipeline::{disambiguate, FileTask};
use crate::settings::ConvertSettings;

pub const DEFAULT_TEMPLATE: &str = "{name}";

/// Placeholder when a file has neither an EXIF date nor a usable file date.
pub const MISSING_DATE: &str = "nodate";
pub const MISSING_TIME: &str = "notime";
pub const MISSING_CAMERA: &str = "unknown";

/// Long stems are cut to this many UTF-8 bytes, leaving room for the
/// conflict suffix and the extension inside the 255 limit of APFS and NTFS.
const MAX_STEM_BYTES: usize = 200;
const MAX_TEMPLATE_CHARS: usize = 200;

/// Why a template is not usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateError {
    Empty,
    TooLong,
    /// `/` or `\`: a name must not create or leave folders.
    PathSeparator,
    /// `..` in the literal text.
    Traversal,
    /// `{` without a matching `}`, or a stray `}`.
    UnbalancedBrace,
    UnknownToken(String),
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "the file name template is empty"),
            Self::TooLong => write!(
                f,
                "the file name template is longer than {MAX_TEMPLATE_CHARS} characters"
            ),
            Self::PathSeparator => {
                write!(f, "the file name template must not contain / or \\")
            }
            Self::Traversal => write!(f, "the file name template must not contain .."),
            Self::UnbalancedBrace => write!(f, "the file name template has an unmatched brace"),
            Self::UnknownToken(t) => write!(
                f,
                "unknown token {{{t}}} (use name, date, time, seq, camera, ext)"
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token {
    Name,
    Date,
    Time,
    Seq,
    Camera,
    Ext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Literal(String),
    Token(Token),
}

/// A parsed, valid template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameTemplate {
    parts: Vec<Part>,
}

/// The values a template can use for one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameFields {
    pub name: String,
    pub date: Option<String>,
    pub time: Option<String>,
    pub seq: usize,
    pub camera: Option<String>,
    pub ext: String,
}

impl NameTemplate {
    pub fn parse(text: &str) -> Result<Self, TemplateError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(TemplateError::Empty);
        }
        if text.chars().count() > MAX_TEMPLATE_CHARS {
            return Err(TemplateError::TooLong);
        }
        if text.contains(['/', '\\']) {
            return Err(TemplateError::PathSeparator);
        }
        if text.contains("..") {
            return Err(TemplateError::Traversal);
        }
        let mut parts = Vec::new();
        let mut rest = text;
        while let Some(open) = rest.find(['{', '}']) {
            if rest.as_bytes()[open] == b'}' {
                return Err(TemplateError::UnbalancedBrace);
            }
            let after = &rest[open + 1..];
            let close = after.find('}').ok_or(TemplateError::UnbalancedBrace)?;
            if open > 0 {
                parts.push(Part::Literal(rest[..open].to_string()));
            }
            let token = &after[..close];
            parts.push(Part::Token(match token {
                "name" => Token::Name,
                "date" => Token::Date,
                "time" => Token::Time,
                "seq" => Token::Seq,
                "camera" => Token::Camera,
                "ext" => Token::Ext,
                other => {
                    if other.contains('{') {
                        return Err(TemplateError::UnbalancedBrace);
                    }
                    return Err(TemplateError::UnknownToken(other.to_string()));
                }
            }));
            rest = &after[close + 1..];
        }
        if !rest.is_empty() {
            parts.push(Part::Literal(rest.to_string()));
        }
        Ok(Self { parts })
    }

    /// The default template: file names exactly as before 0.12.0.
    pub fn is_default(&self) -> bool {
        self.parts == [Part::Token(Token::Name)]
    }

    /// True when rendering needs the file's metadata.
    pub fn uses_metadata(&self) -> bool {
        self.parts
            .iter()
            .any(|p| matches!(p, Part::Token(Token::Date | Token::Time | Token::Camera)))
    }

    /// The sanitised output stem for `fields`.
    pub fn render(&self, fields: &NameFields) -> String {
        let mut out = String::new();
        for part in &self.parts {
            match part {
                Part::Literal(s) => out.push_str(s),
                Part::Token(t) => match t {
                    Token::Name => out.push_str(&fields.name),
                    Token::Date => out.push_str(fields.date.as_deref().unwrap_or(MISSING_DATE)),
                    Token::Time => out.push_str(fields.time.as_deref().unwrap_or(MISSING_TIME)),
                    Token::Seq => out.push_str(&format!("{:04}", fields.seq)),
                    Token::Camera => {
                        out.push_str(fields.camera.as_deref().unwrap_or(MISSING_CAMERA))
                    }
                    Token::Ext => out.push_str(if fields.ext.is_empty() {
                        "raw"
                    } else {
                        &fields.ext
                    }),
                },
            }
        }
        sanitize_stem(&out)
    }
}

const RESERVED_NAMES: [&str; 28] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "COM¹", "COM²", "COM³", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8",
    "LPT9", "LPT¹", "LPT²", "LPT³",
];

/// A file stem that is valid on Windows and macOS and stays in its folder.
pub fn sanitize_stem(stem: &str) -> String {
    let cleaned: String = stem
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trim = |s: &str| s.trim_matches([' ', '.']).to_string();
    let mut s = trim(&cleaned);
    if s.len() > MAX_STEM_BYTES {
        let mut end = MAX_STEM_BYTES;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s = trim(&s[..end]);
    }
    if s.is_empty() {
        return "image".to_string();
    }
    // `CON.anything` is reserved too, so only the part before the first dot counts.
    let device = s.split('.').next().unwrap_or("").trim_end();
    if RESERVED_NAMES
        .iter()
        .any(|r| r.eq_ignore_ascii_case(device))
    {
        s.insert(device.len(), '_');
    }
    s
}

/// `YYYY:MM:DD HH:MM:SS` (or with dashes) as (`YYYY-MM-DD`, `HHMMSS`).
/// Implausible values such as `0000:00:00 00:00:00` count as missing.
pub fn parse_exif_date(text: &str) -> Option<(String, String)> {
    let digits: Vec<u32> = text
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .map(|p| p.parse().ok())
        .collect::<Option<Vec<u32>>>()?;
    let [y, mo, d, h, mi, s, ..] = digits[..] else {
        return None;
    };
    let valid = (1..=9999).contains(&y)
        && (1..=12).contains(&mo)
        && (1..=31).contains(&d)
        && h < 24
        && mi < 60
        && s < 62;
    valid.then(|| {
        (
            format!("{y:04}-{mo:02}-{d:02}"),
            format!("{h:02}{mi:02}{s:02}"),
        )
    })
}

/// EXIF date and camera from rawler's metadata, else from the embedded JPEG
/// (which also works for cameras rawler cannot decode).
fn read_exif_fields(path: &Path) -> (Option<String>, Option<String>) {
    let Ok(source) = RawSource::new(path) else {
        return (None, None);
    };
    let from_rawler = crate::rawguard::get_decoder(&source).ok().and_then(|d| {
        d.raw_metadata(&source, &RawDecodeParams::default())
            .ok()
            .map(|m| ExifInfo::from_rawler(&m))
    });
    let (mut date, mut camera) = match from_rawler {
        Some(i) => (i.date_time_original, i.model),
        None => (None, None),
    };
    if date.is_none() || camera.is_none() {
        let buf = source.buf();
        if let Some(tiff) =
            crate::embedded::largest_jpeg_in(buf).and_then(|c| exifmod::jpeg_exif(c.bytes(buf)))
        {
            if let Ok(ex) = ::exif::Reader::new().read_raw(tiff) {
                let text = |tag| {
                    ex.get_field(tag, ::exif::In::PRIMARY).and_then(|f| {
                        match &f.value {
                            ::exif::Value::Ascii(v) => v
                                .first()
                                .map(|b| String::from_utf8_lossy(b).trim().to_string()),
                            _ => None,
                        }
                        .filter(|s| !s.is_empty())
                    })
                };
                date = date.or_else(|| text(::exif::Tag::DateTimeOriginal));
                camera = camera.or_else(|| text(::exif::Tag::Model));
            }
        }
    }
    (date, camera)
}

fn file_modified_date(path: &Path) -> Option<(String, String)> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let local: chrono::DateTime<chrono::Local> = modified.into();
    Some((
        local.format("%Y-%m-%d").to_string(),
        local.format("%H%M%S").to_string(),
    ))
}

/// Everything a template can use for `task`, except `seq`.
fn name_fields(task: &FileTask, need_metadata: bool) -> NameFields {
    let name = task
        .source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image".into());
    let mut fields = NameFields {
        name,
        ext: task.ext.clone(),
        ..Default::default()
    };
    if need_metadata {
        let (date, camera) = std::panic::catch_unwind(|| read_exif_fields(&task.source))
            .unwrap_or_else(|_| {
                log::warn!(
                    "{}: metadata reader panicked while naming the output",
                    task.source.display()
                );
                (None, None)
            });
        let parsed = date.as_deref().and_then(parse_exif_date).or_else(|| {
            log::info!(
                "{}: no EXIF capture date, using the file date for the name",
                task.source.display()
            );
            file_modified_date(&task.source)
        });
        if let Some((d, t)) = parsed {
            fields.date = Some(d);
            fields.time = Some(t);
        }
        fields.camera = camera;
    }
    fields
}

/// The batch's tasks with their final output stems and extension. Call inside
/// the batch's thread pool. Deterministic: `{seq}` is the position in `tasks`
/// (sorted when scanned), collisions are resolved in that same order.
pub fn plan_output_names(
    tasks: &[FileTask],
    settings: &ConvertSettings,
    cancel: &AtomicBool,
) -> Result<Vec<FileTask>, TemplateError> {
    let template = NameTemplate::parse(&settings.filename_template)?;
    let extension = settings.effective_format().extension();
    let mut out: Vec<FileTask> = tasks.to_vec();
    for t in &mut out {
        t.output_ext = extension;
    }
    if template.is_default() {
        return Ok(out);
    }
    let need = template.uses_metadata();
    let fields: Vec<NameFields> = tasks
        .par_iter()
        .map(|t| {
            let read = need && !cancel.load(Ordering::Relaxed);
            name_fields(t, read)
        })
        .collect();
    for (i, (task, mut f)) in out.iter_mut().zip(fields).enumerate() {
        f.seq = i + 1;
        task.stem = template.render(&f);
        task.batch_suffixed = false;
    }
    disambiguate(&mut out);
    Ok(out)
}

/// Stem and extension for a representative file, for the live example in the UI.
pub fn example_name(template: &str, extension: &str) -> Result<String, TemplateError> {
    let t = NameTemplate::parse(template)?;
    let stem = t.render(&NameFields {
        name: "IMG_1234".into(),
        date: Some("2026-11-09".into()),
        time: Some("143015".into()),
        seq: 1,
        camera: Some("ILCE-7M4".into()),
        ext: "arw".into(),
    });
    Ok(format!("{stem}.{extension}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::plan_tasks;
    use crate::settings::{ConflictPolicy, OutputFormat};
    use std::path::PathBuf;

    fn fields() -> NameFields {
        NameFields {
            name: "IMG_1".into(),
            date: Some("2026-01-02".into()),
            time: Some("030405".into()),
            seq: 7,
            camera: Some("Canon EOS R6".into()),
            ext: "cr3".into(),
        }
    }

    #[test]
    fn renders_every_token() {
        let t = NameTemplate::parse("{date}_{time}_{camera}_{seq}_{name}_{ext}").unwrap();
        assert_eq!(
            t.render(&fields()),
            "2026-01-02_030405_Canon EOS R6_0007_IMG_1_cr3"
        );
        assert!(t.uses_metadata());
        assert!(!NameTemplate::parse("{seq}-{name}.{ext}")
            .unwrap()
            .uses_metadata());
        assert!(NameTemplate::parse("{name}").unwrap().is_default());
        assert!(NameTemplate::parse(" {name} ").unwrap().is_default());
        assert!(!NameTemplate::parse("{name}_x").unwrap().is_default());
    }

    #[test]
    fn seq_grows_past_four_digits() {
        let t = NameTemplate::parse("{seq}").unwrap();
        let f = NameFields {
            seq: 12345,
            ..fields()
        };
        assert_eq!(t.render(&f), "12345");
    }

    #[test]
    fn rejects_bad_templates() {
        for (s, e) in [
            ("", TemplateError::Empty),
            ("   ", TemplateError::Empty),
            ("a/{name}", TemplateError::PathSeparator),
            ("..\\{name}", TemplateError::PathSeparator),
            ("../{name}", TemplateError::PathSeparator),
            ("{name}..x", TemplateError::Traversal),
            ("..", TemplateError::Traversal),
            ("{name", TemplateError::UnbalancedBrace),
            ("name}", TemplateError::UnbalancedBrace),
            ("{{name}}", TemplateError::UnbalancedBrace),
            ("{foo}", TemplateError::UnknownToken("foo".into())),
            ("{}", TemplateError::UnknownToken(String::new())),
        ] {
            assert_eq!(NameTemplate::parse(s), Err(e), "{s:?}");
        }
        assert_eq!(
            NameTemplate::parse(&"a".repeat(201)),
            Err(TemplateError::TooLong)
        );
    }

    #[test]
    fn sanitises_for_windows_and_macos() {
        assert_eq!(
            sanitize_stem("a<b>c:d\"e/f\\g|h?i*j"),
            "a_b_c_d_e_f_g_h_i_j"
        );
        assert_eq!(sanitize_stem("x\u{1}y\u{7f}z"), "x_y_z");
        assert_eq!(sanitize_stem("  name. . "), "name");
        assert_eq!(sanitize_stem("...hidden"), "hidden");
        assert_eq!(sanitize_stem(".."), "image");
        assert_eq!(sanitize_stem("   "), "image");
        assert_eq!(sanitize_stem("con"), "con_");
        assert_eq!(sanitize_stem("LPT1.backup"), "LPT1_.backup");
        assert_eq!(sanitize_stem("COM¹"), "COM¹_");
        assert_eq!(sanitize_stem("CONSOLE"), "CONSOLE");
        assert_eq!(sanitize_stem("사진 1"), "사진 1");
    }

    #[test]
    fn long_names_are_cut_on_a_character_boundary() {
        let s = sanitize_stem(&"한".repeat(100));
        assert!(s.len() <= MAX_STEM_BYTES, "{}", s.len());
        assert!(s.chars().all(|c| c == '한'));
    }

    #[test]
    fn hostile_metadata_cannot_escape_the_folder() {
        let t = NameTemplate::parse("{camera}").unwrap();
        let f = NameFields {
            camera: Some("../../Windows/evil".into()),
            ..fields()
        };
        let out = t.render(&f);
        assert!(!out.contains('/') && !out.contains('\\'), "{out}");
    }

    #[test]
    fn missing_values_have_placeholders() {
        let t = NameTemplate::parse("{date}_{time}_{camera}").unwrap();
        let f = NameFields {
            date: None,
            time: None,
            camera: None,
            ..fields()
        };
        assert_eq!(t.render(&f), "nodate_notime_unknown");
    }

    #[test]
    fn parses_exif_dates() {
        assert_eq!(
            parse_exif_date("2026:11:09 12:30:45"),
            Some(("2026-11-09".into(), "123045".into()))
        );
        assert_eq!(
            parse_exif_date("2026-11-09T12:30:45+09:00"),
            Some(("2026-11-09".into(), "123045".into()))
        );
        assert_eq!(parse_exif_date("0000:00:00 00:00:00"), None);
        assert_eq!(parse_exif_date("    :  :     :  :  "), None);
        assert_eq!(parse_exif_date("2026:13:09 12:30:45"), None);
        assert_eq!(parse_exif_date(""), None);
    }

    fn plan(files: &[&str], template: &str) -> Vec<FileTask> {
        let files: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
        let tasks = plan_tasks(&files, Path::new("/s"), Path::new("/o"));
        let s = ConvertSettings {
            filename_template: template.into(),
            output_format: OutputFormat::Png,
            ..Default::default()
        };
        plan_output_names(&tasks, &s, &AtomicBool::new(false)).unwrap()
    }

    #[test]
    fn seq_follows_task_order() {
        let t = plan(&["/s/a.ARW", "/s/b.ARW", "/s/c.ARW"], "x_{seq}");
        let names: Vec<PathBuf> = t.iter().map(|t| t.primary_output()).collect();
        assert_eq!(
            names,
            [
                PathBuf::from("/o/x_0001.png"),
                PathBuf::from("/o/x_0002.png"),
                PathBuf::from("/o/x_0003.png")
            ]
        );
    }

    #[test]
    fn identical_rendered_names_are_resolved_in_task_order() {
        let t = plan(&["/s/a.ARW", "/s/b.ARW", "/s/c.NEF"], "same");
        let names: Vec<String> = t
            .iter()
            .map(|t| {
                t.primary_output()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, ["same.png", "same_arw.png", "same_nef.png"]);
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), 3);
    }

    #[test]
    fn many_identical_names_stay_unique_and_fast() {
        let files: Vec<String> = (0..20_000).map(|i| format!("/s/f{i:05}.ARW")).collect();
        let refs: Vec<&str> = files.iter().map(String::as_str).collect();
        let start = std::time::Instant::now();
        let t = plan(&refs, "same");
        let unique: std::collections::HashSet<_> = t.iter().map(|t| t.primary_output()).collect();
        assert_eq!(unique.len(), 20_000);
        assert!(start.elapsed().as_secs() < 10, "{:?}", start.elapsed());
    }

    #[test]
    fn default_template_keeps_todays_names_but_follows_the_format() {
        let t = plan(&["/s/IMG_1.ARW", "/s/IMG_1.DNG"], "{name}");
        assert_eq!(t[0].primary_output(), PathBuf::from("/o/IMG_1.png"));
        assert_eq!(t[1].primary_output(), PathBuf::from("/o/IMG_1_dng.png"));
    }

    #[test]
    fn conflict_policy_works_with_a_template() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("s");
        let out = dir.path().join("o");
        std::fs::create_dir_all(&out).unwrap();
        let files = vec![src.join("A.CR3")];
        let tasks = plan_tasks(&files, &src, &out);
        let s = ConvertSettings {
            filename_template: "pic_{seq}".into(),
            ..Default::default()
        };
        let planned = plan_output_names(&tasks, &s, &AtomicBool::new(false)).unwrap();
        std::fs::write(out.join("pic_0001.jpg"), b"old").unwrap();
        let p = crate::pipeline::write_output(&planned[0], ConflictPolicy::Suffix, b"x")
            .unwrap()
            .unwrap();
        assert_eq!(p, out.join("pic_0001_cr3.jpg"));
        assert_eq!(
            crate::pipeline::write_output(&planned[0], ConflictPolicy::Skip, b"x").unwrap(),
            None
        );
    }

    #[test]
    fn example_shows_extension_and_errors() {
        assert_eq!(example_name("{name}", "jpg").unwrap(), "IMG_1234.jpg");
        assert_eq!(
            example_name("{date}_{seq}", "tif").unwrap(),
            "2026-11-09_0001.tif"
        );
        assert!(example_name("a/b", "jpg").is_err());
    }
}
