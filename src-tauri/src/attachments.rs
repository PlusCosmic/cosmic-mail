//! Filesystem helpers for saving attachments: the downloads directory, filename
//! sanitization, and non-overwriting collision naming.
//!
//! Attachment extraction from parsed messages lives in [`crate::sync::imap`];
//! this module is only the write-to-disk side and is deliberately pure and
//! unit-tested (no IMAP, no DB).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Resolve the user's downloads directory, creating it if necessary.
///
/// Prefers the XDG `download_dir()`, falling back to `$HOME/Downloads`.
pub fn downloads_dir() -> Result<PathBuf> {
    let dir = dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        .context("could not determine a downloads directory")?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating downloads directory {}", dir.display()))?;
    Ok(dir)
}

/// Produce a safe base filename for an attachment.
///
/// Strips any path components and control characters, refuses leading dots
/// (so a name can't become a dotfile or `..`), and falls back to
/// `attachment-<id>` — with an extension guessed from the MIME type when
/// reasonable — for empty or fully-stripped names.
pub fn safe_filename(raw: &str, mime_type: &str, attachment_id: i64) -> String {
    // Keep only the final path segment; reject separators outright.
    let base = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .replace('\0', "");
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .trim_start_matches('.')
        .trim()
        .to_string();

    if cleaned.is_empty() {
        let mut name = format!("attachment-{attachment_id}");
        if let Some(ext) = extension_for_mime(mime_type) {
            name.push('.');
            name.push_str(ext);
        }
        return name;
    }
    cleaned
}

/// Guess a file extension from a MIME type for the common cases only.
fn extension_for_mime(mime_type: &str) -> Option<&'static str> {
    match mime_type.trim().to_ascii_lowercase().as_str() {
        "application/pdf" => Some("pdf"),
        "application/zip" => Some("zip"),
        "application/json" => Some("json"),
        "application/msword" => Some("doc"),
        "text/plain" => Some("txt"),
        "text/html" => Some("html"),
        "text/csv" => Some("csv"),
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/svg+xml" => Some("svg"),
        _ => None,
    }
}

/// Return a path in `dir` for `name` that does not overwrite an existing file.
///
/// On collision, suffixes the stem: `name (1).ext`, `name (2).ext`, … There is
/// an inherent check-then-write race, but that is acceptable for a desktop save.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = split_name(name);
    for n in 1.. {
        let alt = match ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        let path = dir.join(alt);
        if !path.exists() {
            return path;
        }
    }
    unreachable!("collision counter is unbounded")
}

/// Split a filename into (stem, extension) preserving the original casing.
/// A leading dot is treated as part of the stem, not an extension separator.
fn split_name(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => (stem, Some(ext)),
        _ => (name, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_path_separators_and_control_chars() {
        assert_eq!(
            safe_filename("../../etc/passwd", "application/octet-stream", 1),
            "passwd"
        );
        assert_eq!(
            safe_filename("a\\b\\report.pdf", "application/pdf", 1),
            "report.pdf"
        );
        assert_eq!(safe_filename("na\u{7}me.txt", "text/plain", 1), "name.txt");
    }

    #[test]
    fn rejects_leading_dots_and_empty_names() {
        // A dotfile / traversal attempt collapses to the id fallback.
        assert_eq!(
            safe_filename("..", "application/pdf", 7),
            "attachment-7.pdf"
        );
        assert_eq!(safe_filename(".hidden", "", 7), "hidden");
        assert_eq!(
            safe_filename("   ", "application/octet-stream", 9),
            "attachment-9"
        );
        assert_eq!(safe_filename("", "image/png", 3), "attachment-3.png");
    }

    #[test]
    fn keeps_ordinary_names_verbatim() {
        assert_eq!(
            safe_filename("Quarterly Report.pdf", "application/pdf", 1),
            "Quarterly Report.pdf"
        );
    }

    #[test]
    fn unique_path_suffixes_on_collision() {
        let dir = tempfile::tempdir().expect("temp dir");
        let p1 = unique_path(dir.path(), "file.pdf");
        assert_eq!(p1, dir.path().join("file.pdf"));
        std::fs::write(&p1, b"a").expect("write first");

        let p2 = unique_path(dir.path(), "file.pdf");
        assert_eq!(p2, dir.path().join("file (1).pdf"));
        std::fs::write(&p2, b"b").expect("write second");

        let p3 = unique_path(dir.path(), "file.pdf");
        assert_eq!(p3, dir.path().join("file (2).pdf"));
    }

    #[test]
    fn unique_path_suffixes_extensionless_names() {
        let dir = tempfile::tempdir().expect("temp dir");
        let p1 = unique_path(dir.path(), "attachment-5");
        std::fs::write(&p1, b"a").expect("write");
        let p2 = unique_path(dir.path(), "attachment-5");
        assert_eq!(p2, dir.path().join("attachment-5 (1)"));
    }
}
