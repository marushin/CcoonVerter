//! Recursive RAW file discovery (port of `RawConverter.scan_folder` / `utils.is_raw_file`).

use std::fs;
use std::path::{Path, PathBuf};

use crate::config;

/// True if the path has a supported RAW extension (case-insensitive).
pub fn is_raw_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(config::is_supported_extension)
        .unwrap_or(false)
}

/// Recursively collect supported RAW files below `root`, sorted by path.
///
/// Symbolic links to directories are not followed (like Python's `os.walk`),
/// which also rules out loops. Unreadable sub-directories are logged and
/// skipped; an unreadable `root` is an error.
pub fn scan_folder(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let meta = fs::metadata(root)?;
    if !meta.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            format!("not a directory: {}", root.display()),
        ));
    }
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut first = true;
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if first => return Err(e),
            Err(e) => {
                log::warn!("Skipping unreadable folder {}: {e}", dir.display());
                continue;
            }
        };
        first = false;
        for entry in entries.flatten() {
            let path = entry.path();
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() && is_raw_file(&path) {
                out.push(path);
            } else if ft.is_symlink() && is_raw_file(&path) && path.is_file() {
                // A symlink to a RAW file is fine; symlinked dirs are not followed.
                out.push(path);
            }
        }
    }
    out.sort();
    log::info!("Scanned {}: found {} RAW files", root.display(), out.len());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_raw_files_recursively_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("b/sub")).unwrap();
        fs::create_dir_all(root.join("한글 폴더")).unwrap();
        for f in [
            "z.ARW",
            "a.cr3",
            "b/sub/c.Nef",
            "b/d.dng",
            "한글 폴더/사진.CR2",
            "x.jpg",
            "y.RAF",
            "one.orf",
            "two.Rw2",
            "three.PEF",
            "four.srw",
            "skip.X3F",
            "noext",
        ] {
            fs::write(root.join(f), b"x").unwrap();
        }
        let files = scan_folder(root).unwrap();
        let names: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(
            names,
            vec![
                "a.cr3",
                "b/d.dng",
                "b/sub/c.Nef",
                "four.srw",
                "one.orf",
                "three.PEF",
                "two.Rw2",
                "y.RAF",
                "z.ARW",
                "한글 폴더/사진.CR2"
            ]
        );
    }

    #[test]
    fn missing_root_is_error() {
        assert!(scan_folder(Path::new("definitely/not/here")).is_err());
    }
}
