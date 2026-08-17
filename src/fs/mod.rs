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
use crate::entry_preview;
use crate::highlight;
use crate::node_source::{Cancelled, NodeSource, Preview};
use crate::sanitize::SanitizedText;
use crate::toggle::Toggle;

pub mod docs;

const PREVIEW_READ_LIMIT: usize = 64 * 1024;

/// Name of the toggle, exposed via `available_toggles`, that controls
/// whether dotfile entries are included in listings. Filesystems are the
/// only kind of source where "hidden" is a meaningful concept at all, so
/// it's owned entirely here rather than threaded through `NodeSource` as a
/// parameter.
const HIDDEN_TOGGLE_NAME: &str = "hidden";

/// Name of the toggle, exposed via `available_toggles`, that controls
/// whether the preview shows the selected node's metadata (size,
/// permissions, timestamps, ...) instead of its normal contents (file text
/// or directory listing). Off by default.
const META_TOGGLE_NAME: &str = "meta";

/// Nerd Font glyph/color for a file whose type isn't recognized by
/// [`file_icon`]. Kept distinct from other icons so an unrecognized file
/// still reads as "a file" rather than blending into some other color.
const GENERIC_FILE_ICON: (char, Color) = ('\u{f15b}', Color::Gray);

/// Picks a Nerd Font glyph/color for a file based on its name, falling back
/// to [`GENERIC_FILE_ICON`] for anything not recognized. Codepoints are from
/// the "seti"/"devicons" glyph sets bundled with Nerd Fonts (all in the BMP
/// private-use area, so they render as a single terminal cell in a patched
/// font). Matching is deliberately scoped to the languages this crate
/// already syntax-highlights (see `highlight.rs`'s `Cargo.toml` deps) plus a
/// handful of ubiquitous extras (git, lock files, docs, images, archives).
/// Colors aren't chosen for any deep reason beyond giving different file
/// types a visually distinct look.
fn file_icon(name: &str) -> (char, Color) {
    let lower = name.to_lowercase();

    // Whole-filename matches take priority over extension matches, since
    // e.g. "Dockerfile" and ".gitignore" have no (or a misleading) extension.
    match lower.as_str() {
        "dockerfile" => return ('\u{f21f}', Color::Rgb(56, 150, 220)),
        "makefile" => return ('\u{f489}', Color::Gray),
        ".gitignore" | ".gitattributes" | ".gitmodules" => {
            return ('\u{f1d3}', Color::Rgb(230, 80, 60))
        }
        "cargo.lock" | "package-lock.json" | "yarn.lock" | "pnpm-lock.yaml" => {
            return ('\u{f023}', Color::DarkGray)
        }
        _ => {}
    }

    let ext = match lower.rsplit_once('.') {
        Some((_, ext)) => ext,
        None => return GENERIC_FILE_ICON,
    };

    match ext {
        "rs" => ('\u{e7a8}', Color::Rgb(222, 165, 132)),
        "toml" => ('\u{e6b2}', Color::Rgb(156, 107, 46)),
        "py" => ('\u{e73c}', Color::Yellow),
        "js" | "mjs" | "cjs" => ('\u{e74e}', Color::Yellow),
        "jsx" => ('\u{e7ba}', Color::Cyan),
        "ts" => ('\u{e628}', Color::Blue),
        "tsx" => ('\u{e7ba}', Color::Rgb(97, 175, 239)),
        "json" => ('\u{e60b}', Color::Rgb(203, 180, 30)),
        "yaml" | "yml" => ('\u{e615}', Color::Rgb(203, 75, 22)),
        "html" | "htm" => ('\u{e736}', Color::Rgb(227, 79, 38)),
        "css" => ('\u{e749}', Color::Rgb(86, 156, 214)),
        "c" | "h" => ('\u{e61e}', Color::Rgb(85, 116, 205)),
        "cpp" | "cc" | "cxx" | "hpp" => ('\u{e61d}', Color::Rgb(0, 89, 156)),
        "go" => ('\u{e627}', Color::Rgb(0, 173, 216)),
        "java" => ('\u{e738}', Color::Rgb(230, 80, 60)),
        "lua" => ('\u{e620}', Color::Rgb(0, 111, 184)),
        "rb" => ('\u{e739}', Color::Red),
        "sh" | "bash" | "zsh" => ('\u{e795}', Color::Green),
        "md" | "markdown" => highlight::MARKDOWN_ICON,
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "bmp" | "webp" => {
            ('\u{f1c5}', Color::Magenta)
        }
        "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" => ('\u{f1c6}', Color::Rgb(205, 133, 63)),
        "pdf" => ('\u{f1c1}', Color::Red),
        "lock" => ('\u{f023}', Color::DarkGray),
        _ => GENERIC_FILE_ICON,
    }
}

/// Picks the Nerd Font glyph/color for an entry, per `fs`'s policy:
/// files are looked up by name via [`file_icon`], falling back to
/// [`GENERIC_FILE_ICON`] for anything unrecognized. Folders always get
/// [`entry_preview::FOLDER_ICON`] but deliberately with no color opinion
/// (`None`): the UI already has a fixed color for directory names (see
/// `entry_preview::entry_label`), and falls back to that same color for an
/// icon with none of its own, so the folder icon tracks it automatically
/// instead of this module hardcoding a second, possibly-drifting choice.
fn entry_icon(name: &str, is_dir: bool) -> (char, Option<Color>) {
    if is_dir {
        (entry_preview::FOLDER_ICON, None)
    } else {
        let (icon, color) = file_icon(name);
        (icon, Some(color))
    }
}

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
    /// Whether directory previews (see `preview_tui_sync`) include Nerd
    /// Font icons. Set once at construction from the `--nerd-font` CLI
    /// flag — unlike the other fields here, it's not a user-facing toggle,
    /// just a static rendering preference threaded down from `main`, so a
    /// plain `bool` (rather than an `Arc<AtomicBool>`) is enough.
    nerd_font: bool,
}

impl FsSource {
    /// Resolves `root` (e.g. a CLI-supplied path) to an absolute, symlink-free
    /// directory the source will be scoped to. Errors carry `root` itself so
    /// callers can report e.g. `"some/bad/path: No such file or directory"`
    /// without needing to know this is backed by the filesystem.
    pub fn new(root: &str, nerd_font: bool) -> io::Result<Self> {
        let root =
            fs::canonicalize(root).map_err(|e| io::Error::new(e.kind(), format!("{root}: {e}")))?;
        Ok(FsSource {
            root,
            show_hidden: Arc::new(AtomicBool::new(false)),
            raw_markdown: Arc::new(AtomicBool::new(false)),
            show_meta: Arc::new(AtomicBool::new(false)),
            nerd_font,
        })
    }

    /// Resolves `id` to a real path under `root`, rejecting any segment
    /// that could step outside it or otherwise isn't a plain filename: `.`,
    /// `..`, a segment that smuggles a separator (e.g. `"a/../b"` as a
    /// single segment), or — on Windows — a drive/UNC prefix like `"C:"` or
    /// `"C:foo"`, which `PathBuf::push` treats as an instruction to replace
    /// the whole path rather than append to it, since none of those
    /// characters are literal path separators there. A segment is accepted
    /// only if `Path::new(segment)` parses as exactly one `Normal`
    /// component, which rules out all of the above uniformly on every
    /// platform instead of hand-picking characters to block.
    fn path_from_segments(&self, id: &[String]) -> io::Result<PathBuf> {
        let mut path = self.root.clone();
        for segment in id {
            let mut components = Path::new(segment).components();
            let is_plain_name = matches!(
                (components.next(), components.next()),
                (Some(std::path::Component::Normal(_)), None)
            );
            if !is_plain_name {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid path segment: {segment:?}"),
                ));
            }
            path.push(segment);
        }
        Ok(path)
    }

    /// `cancelled` is checked once per entry: cheap relative to the syscalls
    /// each iteration already does, and lets a scan of a huge directory bail
    /// out promptly once its result is known to be moot (see
    /// `preview_tui_sync`'s directory-preview branch, the only caller that
    /// passes a flag anyone actually sets — `read_dir` below always passes a
    /// fresh, never-cancelled one, since column loads have no supersession
    /// signal to wire up yet).
    fn read_dir_sync(&self, id: &[String], cancelled: &Cancelled) -> io::Result<Vec<Entry>> {
        let path = self.path_from_segments(id)?;
        let show_hidden = self.show_hidden.load(Ordering::SeqCst);
        let mut entries = Vec::new();
        // Shared by every entry from this listing rather than allocated per
        // entry, since FsSource currently exposes the same (empty) set of
        // commands on every node.
        let suggested_commands: Arc<[Command]> = Arc::from(Vec::new());

        for res in fs::read_dir(&path)? {
            if cancelled.is_cancelled() {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
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

            let (icon, icon_color) = entry_icon(&name, is_dir);

            let mut child_id = id.to_vec();
            child_id.push(name.clone());
            entries.push(Entry {
                name,
                id: child_id,
                is_dir,
                is_link,
                suggested_commands: suggested_commands.clone(),
                nerd_icon: Some(icon),
                nerd_icon_color: icon_color,
            });
        }

        entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });

        Ok(entries)
    }

    fn preview_tui_sync(&self, id: &[String], cancelled: &Cancelled) -> Preview {
        let path = match self.path_from_segments(id) {
            Ok(path) => path,
            Err(e) => return Preview::new(error_text(e.to_string())),
        };
        if self.show_meta.load(Ordering::SeqCst) {
            return Preview {
                text: preview_meta(&path),
                override_disable_line_numbers: true,
            };
        }
        match fs::metadata(&path) {
            Ok(meta) if meta.is_dir() => match self.read_dir_sync(id, cancelled) {
                Ok(entries) => Preview {
                    text: entry_preview::format_dir_preview(&entries, self.nerd_font),
                    override_disable_line_numbers: true,
                },
                Err(e) => Preview::new(error_text(e.to_string())),
            },
            Ok(_) => preview_file(&path, !self.raw_markdown.load(Ordering::SeqCst)),
            Err(e) => Preview::new(error_text(e.to_string())),
        }
    }
}

#[async_trait]
impl NodeSource for FsSource {
    async fn read_dir(&self, id: &[String]) -> io::Result<Vec<Entry>> {
        let source = self.clone();
        let id = id.to_vec();
        tokio::task::spawn_blocking(move || source.read_dir_sync(&id, &Cancelled::new()))
            .await
            .unwrap_or_else(|_| {
                Err(io::Error::other("panicked while reading directory"))
            })
    }

    async fn root_entry(&self) -> Entry {
        let name = self
            .root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.display().to_string());
        Entry {
            name,
            id: Vec::new(),
            is_dir: true,
            is_link: false,
            suggested_commands: Arc::from(Vec::new()),
            nerd_icon: Some(entry_preview::FOLDER_ICON),
            nerd_icon_color: None,
        }
    }

    async fn preview_tui(&self, id: &[String], cancelled: &Cancelled) -> Preview {
        let source = self.clone();
        let id = id.to_vec();
        let cancelled = cancelled.clone();
        tokio::task::spawn_blocking(move || source.preview_tui_sync(&id, &cancelled))
            .await
            .unwrap_or_else(|_| {
                Preview::new(error_text("panicked while loading preview".to_string()))
            })
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
                name: highlight::RAW_TOGGLE_NAME.to_string(),
                key: highlight::RAW_TOGGLE_KEY,
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
        } else if toggle.name == highlight::RAW_TOGGLE_NAME {
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
        } else if toggle.name == highlight::RAW_TOGGLE_NAME {
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

fn preview_file(path: &Path, hide_markers: bool) -> Preview {
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return Preview::new(error_text(e.to_string())),
    };

    let total_size = file.metadata().map(|m| m.len()).unwrap_or(0);

    let mut buf = vec![0u8; PREVIEW_READ_LIMIT];
    let n = match file.read(&mut buf) {
        Ok(n) => n,
        Err(e) => return Preview::new(error_text(e.to_string())),
    };
    buf.truncate(n);

    if buf.is_empty() {
        return Preview::new(SanitizedText::default());
    }
    if buf.contains(&0) {
        return Preview {
            text: dim_text(format!("binary file, {}", human_size(total_size))),
            override_disable_line_numbers: true,
        };
    }
    match String::from_utf8(buf) {
        Ok(text) => Preview::new(highlight::highlighted_text(path, &text, hide_markers)),
        Err(_) => Preview {
            text: dim_text(format!("binary file, {}", human_size(total_size))),
            override_disable_line_numbers: true,
        },
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
