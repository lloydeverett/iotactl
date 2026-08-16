use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;

use crate::command::Command;
use crate::entry::Entry;
use crate::highlight;
use crate::node_source::NodeSource;
use crate::sanitize::SanitizedText;
use crate::toggle::Toggle;

const PREVIEW_READ_LIMIT: usize = 64 * 1024;

/// Name of the toggle, exposed via `available_toggles`, that controls
/// whether dotfile entries are included in listings. Filesystems are the
/// only kind of source where "hidden" is a meaningful concept at all, so
/// it's owned entirely here rather than threaded through `NodeSource` as a
/// parameter.
const HIDDEN_TOGGLE_NAME: &str = "hidden";

/// Name of the toggle, exposed via `available_toggles`, that controls
/// whether markdown formatting characters (`#`, ```` ``` ````, `*`, ...) are
/// shown as-is in previews (on) or hidden (off, the default). The underlying
/// syntax highlighting (e.g. headings/emphasis/code spans still being
/// colored) is unaffected either way — see `highlight::highlight`'s
/// `hide_markers` parameter, which is simply this toggle's negation.
const RAW_TOGGLE_NAME: &str = "raw";

/// Name of the toggle, exposed via `available_toggles`, that controls
/// whether the preview shows the selected node's metadata (size,
/// permissions, timestamps, ...) instead of its normal contents (file text
/// or directory listing). Off by default.
const META_TOGGLE_NAME: &str = "meta";

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
    /// Shared (not per-clone) so every `FsSource` handle backed by the same
    /// root sees the same toggle state, since `read_dir`/`preview_tui` clone
    /// `self` into a blocking task on every call.
    show_hidden: Arc<AtomicBool>,
    /// Same sharing rationale as `show_hidden`. Defaults to `false`: markdown
    /// markers are hidden (i.e. "rendered") by default.
    raw_markdown: Arc<AtomicBool>,
    /// Same sharing rationale as `show_hidden`. Defaults to `false`: the
    /// preview shows content, not metadata.
    show_meta: Arc<AtomicBool>,
}

impl FsSource {
    pub fn new(root: PathBuf) -> Self {
        FsSource {
            root,
            show_hidden: Arc::new(AtomicBool::new(false)),
            raw_markdown: Arc::new(AtomicBool::new(false)),
            show_meta: Arc::new(AtomicBool::new(false)),
        }
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

    fn read_dir_sync(&self, id: &[String]) -> io::Result<Vec<Entry>> {
        let path = self.path_from_segments(id)?;
        let show_hidden = self.show_hidden.load(Ordering::SeqCst);
        let mut entries = Vec::new();
        // Shared by every entry from this listing rather than allocated per
        // entry, since FsSource currently exposes the same (empty) set of
        // commands on every node.
        let suggested_commands: Arc<[Command]> = Arc::from(Vec::new());

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
                suggested_commands: suggested_commands.clone(),
            });
        }

        entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });

        Ok(entries)
    }

    fn preview_tui_sync(&self, id: &[String]) -> SanitizedText {
        let path = match self.path_from_segments(id) {
            Ok(path) => path,
            Err(e) => return error_text(e.to_string()),
        };
        if self.show_meta.load(Ordering::SeqCst) {
            return preview_meta(&path);
        }
        match fs::metadata(&path) {
            Ok(meta) if meta.is_dir() => match self.read_dir_sync(id) {
                Ok(entries) => preview_dir(&entries),
                Err(e) => error_text(e.to_string()),
            },
            Ok(_) => preview_file(&path, !self.raw_markdown.load(Ordering::SeqCst)),
            Err(e) => error_text(e.to_string()),
        }
    }
}

#[async_trait]
impl NodeSource for FsSource {
    async fn read_dir(&self, id: &[String]) -> io::Result<Vec<Entry>> {
        let source = self.clone();
        let id = id.to_vec();
        tokio::task::spawn_blocking(move || source.read_dir_sync(&id))
            .await
            .unwrap_or_else(|_| {
                Err(io::Error::other("panicked while reading directory"))
            })
    }

    async fn preview_tui(&self, id: &[String]) -> SanitizedText {
        let source = self.clone();
        let id = id.to_vec();
        tokio::task::spawn_blocking(move || source.preview_tui_sync(&id))
            .await
            .unwrap_or_else(|_| error_text("panicked while loading preview".to_string()))
    }

    fn available_commands(&self) -> Arc<[Command]> {
        Arc::from(Vec::new())
    }

    fn available_toggles(&self) -> Arc<[Toggle]> {
        Arc::from(vec![
            Toggle {
                name: HIDDEN_TOGGLE_NAME.to_string(),
                key: 'H',
            },
            Toggle {
                name: RAW_TOGGLE_NAME.to_string(),
                key: 'r',
            },
            Toggle {
                name: META_TOGGLE_NAME.to_string(),
                key: 'm',
            },
        ])
    }

    async fn set_toggle(&self, toggle: &Toggle, value: bool) -> io::Result<()> {
        if toggle.name == HIDDEN_TOGGLE_NAME {
            self.show_hidden.store(value, Ordering::SeqCst);
            Ok(())
        } else if toggle.name == RAW_TOGGLE_NAME {
            self.raw_markdown.store(value, Ordering::SeqCst);
            Ok(())
        } else if toggle.name == META_TOGGLE_NAME {
            self.show_meta.store(value, Ordering::SeqCst);
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("FsSource has no toggle named {:?}", toggle.name),
            ))
        }
    }

    async fn get_toggle(&self, toggle: &Toggle) -> io::Result<bool> {
        if toggle.name == HIDDEN_TOGGLE_NAME {
            Ok(self.show_hidden.load(Ordering::SeqCst))
        } else if toggle.name == RAW_TOGGLE_NAME {
            Ok(self.raw_markdown.load(Ordering::SeqCst))
        } else if toggle.name == META_TOGGLE_NAME {
            Ok(self.show_meta.load(Ordering::SeqCst))
        } else {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("FsSource has no toggle named {:?}", toggle.name),
            ))
        }
    }

    async fn execute_command(&self, command: &Command, _args: &[String]) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("FsSource has no command named {:?}", command.name),
        ))
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

fn preview_file(path: &Path, hide_markers: bool) -> SanitizedText {
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
            match highlight::highlight(path, &sanitized.plain(), hide_markers) {
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

/// Builds a "key: value" preview line. `key` is a fixed label owned by this
/// module (never untrusted), while `value` is routed through
/// `SanitizedText::from_label` so anything OS-provided (a symlink target, an
/// odd path) still gets escaped before it reaches the terminal.
fn meta_line(key: &str, value: String, value_style: Style) -> Line<'static> {
    let mut line = SanitizedText::from_label(&value, value_style);
    line.spans.insert(
        0,
        ratatui::text::Span::styled(
            format!("{key:<11} "),
            Style::default().fg(Color::DarkGray),
        ),
    );
    line
}

fn rwx_triplet(bits: u32) -> String {
    format!(
        "{}{}{}",
        if bits & 0b100 != 0 { 'r' } else { '-' },
        if bits & 0b010 != 0 { 'w' } else { '-' },
        if bits & 0b001 != 0 { 'x' } else { '-' },
    )
}

fn format_permissions(mode: u32) -> String {
    let perm = mode & 0o777;
    format!(
        "{perm:03o} ({}{}{})",
        rwx_triplet((perm >> 6) & 0o7),
        rwx_triplet((perm >> 3) & 0o7),
        rwx_triplet(perm & 0o7),
    )
}

/// Converts a day count since the Unix epoch (1970-01-01) to a
/// (year, month, day) civil date. Howard Hinnant's well-known algorithm
/// (<http://howardhinnant.github.io/date_algorithms.html>), used here to
/// format timestamps without pulling in a date/time crate for it.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

fn format_time(time: SystemTime) -> String {
    let secs = match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    };
    let days = secs.div_euclid(86400);
    let time_of_day = secs.rem_euclid(86400);
    let (year, month, day) = civil_from_days(days);
    let hour = time_of_day / 3600;
    let minute = (time_of_day % 3600) / 60;
    let second = time_of_day % 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC")
}

/// Builds the "meta" toggle's preview: a `key: value` listing of the
/// selected node's metadata rather than its normal contents. Applies
/// uniformly to files and directories, since both have this information.
fn preview_meta(path: &Path) -> SanitizedText {
    // `symlink_metadata` (lstat) rather than `metadata` (stat) so a symlink
    // is detected as one instead of transparently resolved.
    let link_meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) => return error_text(e.to_string()),
    };
    let is_link = link_meta.file_type().is_symlink();

    // For a symlink, prefer the resolved target's metadata (size, times,
    // permissions) like `stat` does, but fall back to the link's own
    // metadata for a broken link rather than erroring out entirely.
    let (meta, broken_link) = if is_link {
        match fs::metadata(path) {
            Ok(resolved) => (resolved, false),
            Err(_) => (link_meta.clone(), true),
        }
    } else {
        (link_meta.clone(), false)
    };

    let mut lines = Vec::new();

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "/".to_string());
    lines.push(meta_line("Name", name, Style::default()));
    lines.push(meta_line(
        "Path",
        path.to_string_lossy().to_string(),
        Style::default(),
    ));

    let type_desc = if is_link {
        match fs::read_link(path) {
            Ok(target) => {
                let target = target.to_string_lossy().to_string();
                if broken_link {
                    format!("symlink -> {target} (broken)")
                } else {
                    format!("symlink -> {target}")
                }
            }
            Err(e) => format!("symlink (unreadable target: {e})"),
        }
    } else if meta.is_dir() {
        "directory".to_string()
    } else if meta.is_file() {
        "regular file".to_string()
    } else {
        "other".to_string()
    };
    let type_style = if is_link {
        Style::default().fg(Color::Magenta)
    } else if meta.is_dir() {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    lines.push(meta_line("Type", type_desc, type_style));

    if meta.is_dir() {
        if let Ok(count) = fs::read_dir(path).map(|rd| rd.count()) {
            lines.push(meta_line("Entries", count.to_string(), Style::default()));
        }
    } else {
        lines.push(meta_line(
            "Size",
            format!("{} ({} bytes)", human_size(meta.len()), meta.len()),
            Style::default(),
        ));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        lines.push(meta_line(
            "Permissions",
            format_permissions(meta.permissions().mode()),
            Style::default(),
        ));
        lines.push(meta_line(
            "Owner",
            format!("uid={} gid={}", meta.uid(), meta.gid()),
            Style::default(),
        ));
    }

    if let Ok(modified) = meta.modified() {
        lines.push(meta_line("Modified", format_time(modified), Style::default()));
    }
    if let Ok(accessed) = meta.accessed() {
        lines.push(meta_line("Accessed", format_time(accessed), Style::default()));
    }
    if let Ok(created) = meta.created() {
        lines.push(meta_line("Created", format_time(created), Style::default()));
    }

    SanitizedText::assume_sanitized(lines)
}
