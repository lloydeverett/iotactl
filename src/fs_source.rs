use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use ratatui::style::{Color, Modifier, Style};

use crate::entry::Entry;
use crate::highlight;
use crate::node_source::NodeSource;
use crate::sanitize::SanitizedText;

const PREVIEW_READ_LIMIT: usize = 64 * 1024;

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "K", "M", "G", "T", "P"];
    if bytes == 0 {
        return "0B".to_string();
    }
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes}{}", UNITS[unit])
    } else {
        format!("{size:.1}{}", UNITS[unit])
    }
}

/// A `NodeSource` backed by the local filesystem, rooted at a fixed
/// directory. Node ids are segments relative to that root, so the root
/// itself is addressed as `id == []` and callers can never resolve a path
/// outside it: access is scoped to wherever the source was constructed.
#[derive(Clone)]
pub struct FsSource {
    root: PathBuf,
}

impl FsSource {
    pub fn new(root: PathBuf) -> Self {
        FsSource { root }
    }

    /// Resolves `id` to a real path under `root`, rejecting any segment
    /// that could step outside it (`.`, `..`) or that smuggles a separator
    /// (e.g. `"a/../b"` as a single segment).
    fn path_from_segments(&self, id: &[String]) -> io::Result<PathBuf> {
        let mut path = self.root.clone();
        for segment in id {
            if segment.is_empty()
                || segment == "."
                || segment == ".."
                || segment.contains(std::path::is_separator)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid path segment: {segment:?}"),
                ));
            }
            path.push(segment);
        }
        Ok(path)
    }

    fn read_dir_sync(&self, id: &[String], show_hidden: bool) -> io::Result<Vec<Entry>> {
        let path = self.path_from_segments(id)?;
        let mut entries = Vec::new();

        for res in fs::read_dir(&path)? {
            let dir_entry = res?;
            let name = dir_entry.file_name().to_string_lossy().to_string();
            if !show_hidden && name.starts_with('.') {
                continue;
            }

            // DirEntry::metadata does not follow symlinks, so we can detect them
            // and then resolve the target separately to know if it's a directory.
            let link_metadata = dir_entry.metadata()?;
            let is_link = link_metadata.file_type().is_symlink();
            let is_dir = if is_link {
                fs::metadata(dir_entry.path())
                    .map(|target_meta| target_meta.is_dir())
                    .unwrap_or(false)
            } else {
                link_metadata.is_dir()
            };

            let mut child_id = id.to_vec();
            child_id.push(name.clone());
            entries.push(Entry {
                name,
                id: child_id,
                is_dir,
                is_link,
            });
        }

        entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });

        Ok(entries)
    }

    fn preview_tui_sync(&self, id: &[String], show_hidden: bool) -> SanitizedText {
        let path = match self.path_from_segments(id) {
            Ok(path) => path,
            Err(e) => return error_text(e.to_string()),
        };
        match fs::metadata(&path) {
            Ok(meta) if meta.is_dir() => match self.read_dir_sync(id, show_hidden) {
                Ok(entries) => preview_dir(&entries),
                Err(e) => error_text(e.to_string()),
            },
            Ok(_) => preview_file(&path),
            Err(e) => error_text(e.to_string()),
        }
    }
}

#[async_trait]
impl NodeSource for FsSource {
    async fn read_dir(&self, id: &[String], show_hidden: bool) -> io::Result<Vec<Entry>> {
        let source = self.clone();
        let id = id.to_vec();
        tokio::task::spawn_blocking(move || source.read_dir_sync(&id, show_hidden))
            .await
            .unwrap_or_else(|_| {
                Err(io::Error::other("panicked while reading directory"))
            })
    }

    async fn preview_tui(&self, id: &[String], show_hidden: bool) -> SanitizedText {
        let source = self.clone();
        let id = id.to_vec();
        tokio::task::spawn_blocking(move || source.preview_tui_sync(&id, show_hidden))
            .await
            .unwrap_or_else(|_| error_text("panicked while loading preview".to_string()))
    }
}

fn error_text(msg: String) -> SanitizedText {
    SanitizedText::from_text(&msg, Style::default().fg(Color::Red))
}

fn dim_text(msg: String) -> SanitizedText {
    SanitizedText::from_text(&msg, Style::default().fg(Color::DarkGray))
}

fn preview_dir(entries: &[Entry]) -> SanitizedText {
    if entries.is_empty() {
        return dim_text("empty directory".to_string());
    }
    // Each label is built via `SanitizedText::from_label`, so the collected
    // lines are already free of raw control characters — entry names come
    // straight from the filesystem and, unlike `/`, aren't restricted from
    // containing them.
    let lines = entries
        .iter()
        .map(|entry| {
            let (label, style) = if entry.is_dir {
                (
                    format!("{}/", entry.name),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
            } else if entry.is_link {
                (
                    format!("{}@", entry.name),
                    Style::default().fg(Color::Magenta),
                )
            } else {
                (entry.name.clone(), Style::default())
            };
            SanitizedText::from_label(&label, style)
        })
        .collect();
    SanitizedText::assume_sanitized(lines)
}

fn preview_file(path: &Path) -> SanitizedText {
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return error_text(e.to_string()),
    };

    let total_size = file.metadata().map(|m| m.len()).unwrap_or(0);

    let mut buf = vec![0u8; PREVIEW_READ_LIMIT];
    let n = match file.read(&mut buf) {
        Ok(n) => n,
        Err(e) => return error_text(e.to_string()),
    };
    buf.truncate(n);

    if buf.is_empty() {
        return SanitizedText::default();
    }
    if buf.contains(&0) {
        return dim_text(format!("binary file, {}", human_size(total_size)));
    }
    match String::from_utf8(buf) {
        Ok(text) => {
            let sanitized = SanitizedText::from_text(&text, Style::default());
            match highlight::highlight(path, &sanitized.plain()) {
                // Safe: the highlighter only re-slices and re-styles the
                // already-sanitized plain text handed to it above, so it
                // can't introduce a raw control character of its own.
                Some(lines) => SanitizedText::assume_sanitized(lines),
                None => sanitized,
            }
        }
        Err(_) => dim_text(format!("binary file, {}", human_size(total_size))),
    }
}
