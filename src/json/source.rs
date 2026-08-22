use std::io;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use ratatui::style::{Color, Style};
use serde_json::Value;

use crate::command::Command;
use crate::entry::Entry;
use crate::entry_preview;
use crate::highlight;
use crate::node_source::{Cancelled, NodeSource, NodeSourceType, Preview};
use crate::sanitize::SanitizedText;
use crate::streams::{ByteStream, SeekableByteStream};

/// Bound on how much of a leaf value's pretty-printed text
/// [`preview_value`] will syntax-highlight and show. The whole document is
/// already fully parsed into memory by the time any preview runs (see
/// `NODE_SOURCE_TYPE`'s `construct_fn`), so this isn't about avoiding a big
/// read the way `fs::PREVIEW_READ_LIMIT` is — it's just to keep a
/// pathologically large embedded string (or a huge subtree opened as a
/// single value) from costing a real tree-sitter highlight pass and a
/// giant `Text` no preview pane could usefully show anyway.
const PREVIEW_TEXT_LIMIT: usize = 64 * 1024;

/// A synthetic filename handed to [`highlight::highlighted_text`] purely so
/// it infers "json" as the language. No file by this name is ever actually
/// opened.
const SYNTHETIC_VALUE_PATH: &str = "value.json";

/// This type's contribution to [`crate::registry::NODE_SOURCE_TYPES`].
pub static NODE_SOURCE_TYPE: NodeSourceType = NodeSourceType {
    schemes: &["json://"],
    manual_page: Some(&super::manual::MANUAL_PAGE),
    commands: &[],
    toggles: &[],
    construct_fn: |_scheme, rest, pipe| {
        let rest = rest.to_string();
        Box::pin(async move {
            let Some(pipe) = pipe else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "json:// has nothing to read on its own — pipe another node \
                     source's bytes into it, e.g. \"file://data.json | json://\"",
                ));
            };
            let mut stream = pipe.open(&[]).await?;
            let mut bytes = Vec::new();
            tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut bytes).await?;
            // Parsing is real CPU work for a large document — see
            // `node_source::NodeSource`'s trait docs on keeping that off the
            // render thread.
            let value = tokio::task::spawn_blocking(move || serde_json::from_slice::<Value>(&bytes))
                .await
                .map_err(|_| io::Error::other("panicked while parsing JSON"))?
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("invalid JSON: {e}")))?;
            Ok(Arc::new(JsonSource::new(value, &rest)?) as Arc<dyn NodeSource>)
        })
    },
    set_toggle_fn: |toggle, _value| {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("the JSON source has no toggle named {:?}", toggle.name),
        ))
    },
    get_toggle_fn: |toggle| {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("the JSON source has no toggle named {:?}", toggle.name),
        ))
    },
};

fn is_container(value: &Value) -> bool {
    matches!(value, Value::Object(_) | Value::Array(_))
}

/// Nerd Font glyph reused, unmodified, from `fs::GENERIC_FILE_ICON` for every
/// scalar leaf: a JSON value has no filename of its own to pick an icon from
/// the way a real file does, so every leaf gets the same glyph and is told
/// apart only by color (see [`value_icon`]) — string, number, and
/// bool/null colored the same way [`highlight::highlight`]'s JSON query
/// colors them.
const LEAF_ICON: char = '\u{f15b}';

fn value_icon(value: &Value) -> (char, Option<Color>) {
    match value {
        Value::Object(_) | Value::Array(_) => (entry_preview::FOLDER_ICON, None),
        Value::String(_) => (LEAF_ICON, Some(Color::Green)),
        Value::Number(_) => (LEAF_ICON, Some(Color::Yellow)),
        Value::Bool(_) | Value::Null => (LEAF_ICON, Some(Color::LightRed)),
    }
}

/// Walks `id` down from `root`, one segment per level: an object segment
/// matches a key exactly, an array segment parses as a position. Returns
/// `None` as soon as a segment doesn't resolve — a bad key, an
/// out-of-range or non-numeric position, or one more segment than a scalar
/// has children for.
fn resolve<'v>(root: &'v Value, id: &[String]) -> Option<&'v Value> {
    let mut current = root;
    for segment in id {
        current = match current {
            Value::Object(map) => map.get(segment)?,
            Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

fn no_such_node(id: &[String]) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("no such node in the JSON tree: {}", id.join("/")),
    )
}

fn error_text(msg: String) -> SanitizedText {
    SanitizedText::from_text(&msg, Style::default().fg(Color::Red))
}

/// Splits the CLI path's rest after `json://` into id segments — the same
/// shape `NodeSource::read_dir` takes. Ignores empty segments (a leading,
/// trailing, or doubled `/`), matching `manual::split_id`. A key that
/// itself contains a literal `/` can't be addressed this way; see the
/// manual page for the workaround (open at the root, then navigate in-app).
fn split_id(rest: &str) -> Vec<String> {
    rest.split('/').filter(|s| !s.is_empty()).map(str::to_string).collect()
}

fn child_entry(id: &[String], name: String, value: &Value) -> Entry {
    let mut child_id = id.to_vec();
    child_id.push(name.clone());
    let (icon, icon_color) = value_icon(value);
    Entry {
        name,
        id: child_id,
        is_dir: is_container(value),
        is_link: false,
        suggested_commands: Arc::from(Vec::new()),
        nerd_icon: Some(icon),
        nerd_icon_color: icon_color,
    }
}

/// Pretty-prints and syntax-highlights `value` (a whole subtree, or a
/// scalar) as a node's preview.
fn preview_value(value: &Value) -> SanitizedText {
    let pretty = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    let truncated = truncate_to_char_boundary(&pretty, PREVIEW_TEXT_LIMIT);
    highlight::highlighted_text(Path::new(SYNTHETIC_VALUE_PATH), truncated, false)
}

fn truncate_to_char_boundary(s: &str, limit: usize) -> &str {
    if s.len() <= limit {
        return s;
    }
    let mut end = limit;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// A `NodeSource` over a JSON document, fully parsed into memory once at
/// construction (see [`NODE_SOURCE_TYPE`]'s `construct_fn`) and scoped to
/// `root` — same idea as `manual::ManualSource` scoping itself into its
/// fixed page tree, just with a document parsed from piped-in bytes instead
/// of one baked into the binary.
#[derive(Clone)]
pub struct JsonSource {
    /// The whole parsed document. `Arc` so cloning a `JsonSource` (done
    /// once per `spawn_blocking` call — see e.g. `read_dir`) never
    /// re-copies it.
    root_value: Arc<Value>,
    /// Absolute id (within `root_value`) this source is scoped to. `[]` for
    /// plain `json://` (the whole document); e.g. `["some", "key"]` for
    /// `json://some/key`.
    root: Vec<String>,
}

impl JsonSource {
    /// `root` (the CLI path's rest after `json://`) is split into id
    /// segments and scopes this source the way `manual::ManualSource::new`'s
    /// `root` parameter scopes a manual source: every id given to this
    /// source afterward is resolved relative to it. Rejected eagerly if it
    /// doesn't resolve within `value`, mirroring `ManualSource::new`
    /// rejecting a nonexistent page the same way.
    fn new(value: Value, root: &str) -> io::Result<Self> {
        let root = split_id(root);
        resolve(&value, &root).ok_or_else(|| no_such_node(&root))?;
        Ok(JsonSource {
            root_value: Arc::new(value),
            root,
        })
    }

    /// Resolves a relative `id` (as given to any `NodeSource` method) to the
    /// absolute id [`resolve`] expects.
    fn absolute(&self, id: &[String]) -> Vec<String> {
        self.root.iter().chain(id).cloned().collect()
    }

    fn read_dir_sync(&self, id: &[String]) -> io::Result<Vec<Entry>> {
        let absolute = self.absolute(id);
        let value = resolve(&self.root_value, &absolute).ok_or_else(|| no_such_node(&absolute))?;
        match value {
            Value::Object(map) => Ok(map.iter().map(|(k, v)| child_entry(id, k.clone(), v)).collect()),
            Value::Array(items) => Ok(items
                .iter()
                .enumerate()
                .map(|(i, v)| child_entry(id, i.to_string(), v))
                .collect()),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is a leaf value, not a container", absolute.join("/")),
            )),
        }
    }

    fn preview_sync(&self, id: &[String]) -> Preview {
        let absolute = self.absolute(id);
        let Some(value) = resolve(&self.root_value, &absolute) else {
            return Preview::new(error_text(no_such_node(&absolute).to_string()));
        };
        // Both a container and a leaf preview the same way here: the node's
        // full pretty-printed JSON, so an object or array can be read at a
        // glance instead of needing to drill into each child just to see
        // what it holds. Line numbers stay on (unlike `fs`/`manual`'s
        // directory-listing previews, which turn them off) since this is
        // real, indented multi-line text they correspond to.
        Preview::new(preview_value(value))
    }

    fn serialize_sync(&self, id: &[String]) -> io::Result<Vec<u8>> {
        let absolute = self.absolute(id);
        let value = resolve(&self.root_value, &absolute).ok_or_else(|| no_such_node(&absolute))?;
        serde_json::to_vec(value)
            .map_err(|e| io::Error::other(format!("failed to serialize JSON: {e}")))
    }
}

#[async_trait]
impl NodeSource for JsonSource {
    async fn read_dir(&self, id: &[String]) -> io::Result<Vec<Entry>> {
        let source = self.clone();
        let id = id.to_vec();
        tokio::task::spawn_blocking(move || source.read_dir_sync(&id))
            .await
            .unwrap_or_else(|_| Err(io::Error::other("panicked while reading JSON node")))
    }

    async fn root_entry(&self) -> Entry {
        // Always succeeds: `self.root` was already validated by
        // `JsonSource::new`, and never changes afterward.
        let value = resolve(&self.root_value, &self.root).expect("JsonSource::new validated `root`");
        // "$" is the conventional JSONPath/jq spelling for "the document's
        // own root", used here as the display name only when `root` is
        // empty (bare `json://`) and there's no key/position of its own to
        // show instead.
        let name = self.root.last().cloned().unwrap_or_else(|| "$".to_string());
        let (icon, icon_color) = value_icon(value);
        Entry {
            name,
            id: Vec::new(),
            is_dir: is_container(value),
            is_link: false,
            suggested_commands: Arc::from(Vec::new()),
            nerd_icon: Some(icon),
            nerd_icon_color: icon_color,
        }
    }

    async fn preview_tui(&self, id: &[String], _cancelled: &Cancelled) -> Preview {
        let source = self.clone();
        let id = id.to_vec();
        tokio::task::spawn_blocking(move || source.preview_sync(&id))
            .await
            .unwrap_or_else(|_| Preview::new(error_text("panicked while loading preview".to_string())))
    }

    async fn open(&self, id: &[String]) -> io::Result<ByteStream> {
        let source = self.clone();
        let id = id.to_vec();
        let bytes = tokio::task::spawn_blocking(move || source.serialize_sync(&id))
            .await
            .unwrap_or_else(|_| Err(io::Error::other("panicked while serializing JSON")))?;
        Ok(Box::pin(std::io::Cursor::new(bytes)))
    }

    async fn open_seekable(&self, id: &[String]) -> io::Result<SeekableByteStream> {
        let source = self.clone();
        let id = id.to_vec();
        let bytes = tokio::task::spawn_blocking(move || source.serialize_sync(&id))
            .await
            .unwrap_or_else(|_| Err(io::Error::other("panicked while serializing JSON")))?;
        // `std::io::Cursor` implements `AsyncSeek` too, so this can always
        // satisfy the guarantee `open_seekable` makes — the whole value is
        // already in memory, so there's no natural "not random access"
        // case here the way there is for e.g. a streamed zip entry.
        Ok(Box::pin(std::io::Cursor::new(bytes)))
    }

    async fn execute_command(&self, command: &Command, _args: &[String]) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("the JSON source has no command named {:?}", command.name),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(json: &str, rest: &str) -> io::Result<JsonSource> {
        let value: Value = serde_json::from_str(json).unwrap();
        JsonSource::new(value, rest)
    }

    #[tokio::test]
    async fn construct_rejects_a_bare_json_scheme_with_no_pipe() {
        let Err(err) = NODE_SOURCE_TYPE.construct("json://", "", None).await else {
            panic!("expected json:// with no pipe to fail");
        };
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[tokio::test]
    async fn read_dir_lists_object_keys_in_document_order() {
        let source = build(r#"{"z": 1, "a": 2, "m": 3}"#, "").unwrap();
        let entries = source.read_dir(&[]).await.unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["z", "a", "m"], "object keys should keep document order, not be re-sorted");
    }

    #[tokio::test]
    async fn read_dir_lists_array_positions_in_index_order() {
        let source = build(r#"[10, 20, 30]"#, "").unwrap();
        let entries = source.read_dir(&[]).await.unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["0", "1", "2"]);
        assert!(entries.iter().all(|e| !e.is_dir), "array of scalars should have no container children");
    }

    #[tokio::test]
    async fn read_dir_marks_nested_containers_as_dirs() {
        let source = build(r#"{"obj": {"a": 1}, "arr": [1, 2], "leaf": "x"}"#, "").unwrap();
        let entries = source.read_dir(&[]).await.unwrap();
        let is_dir = |name: &str| entries.iter().find(|e| e.name == name).unwrap().is_dir;
        assert!(is_dir("obj"));
        assert!(is_dir("arr"));
        assert!(!is_dir("leaf"));
    }

    #[tokio::test]
    async fn read_dir_on_a_leaf_fails() {
        let source = build(r#"{"leaf": "x"}"#, "").unwrap();
        let Err(err) = source.read_dir(&["leaf".to_string()]).await else {
            panic!("expected read_dir on a scalar leaf to fail");
        };
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[tokio::test]
    async fn read_dir_on_a_missing_key_fails_not_found() {
        let source = build(r#"{"a": 1}"#, "").unwrap();
        let Err(err) = source.read_dir(&["nope".to_string()]).await else {
            panic!("expected read_dir on a missing key to fail");
        };
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[tokio::test]
    async fn construct_rejects_a_root_that_does_not_resolve() {
        let value: Value = serde_json::from_str(r#"{"a": 1}"#).unwrap();
        let Err(err) = JsonSource::new(value, "nope") else {
            panic!("expected construction with a bad root path to fail");
        };
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[tokio::test]
    async fn construction_scopes_every_id_relative_to_root() {
        let source = build(r#"{"outer": {"inner": {"x": 1, "y": 2}}}"#, "outer/inner").unwrap();
        let entries = source.read_dir(&[]).await.unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["x", "y"]);
    }

    #[tokio::test]
    async fn root_entry_uses_dollar_sign_for_an_unscoped_root() {
        let source = build(r#"{"a": 1}"#, "").unwrap();
        assert_eq!(source.root_entry().await.name, "$");
    }

    #[tokio::test]
    async fn root_entry_uses_the_last_scoping_segment_otherwise() {
        let source = build(r#"{"outer": {"inner": 1}}"#, "outer").unwrap();
        assert_eq!(source.root_entry().await.name, "outer");
    }

    #[tokio::test]
    async fn open_returns_compact_json_bytes_for_a_subtree() {
        let source = build(r#"{"a": {"b": 1, "c": [1, 2]}}"#, "").unwrap();
        let mut stream = source.open(&["a".to_string()]).await.unwrap();
        let mut bytes = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut bytes).await.unwrap();
        let parsed: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed, serde_json::json!({"b": 1, "c": [1, 2]}));
    }

    #[tokio::test]
    async fn open_seekable_supports_seeking_back_to_the_start() {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let source = build(r#""hello""#, "").unwrap();
        let mut stream = source.open_seekable(&[]).await.unwrap();
        let mut first = [0u8; 1];
        stream.read_exact(&mut first).await.unwrap();
        stream.seek(std::io::SeekFrom::Start(0)).await.unwrap();
        let mut all = Vec::new();
        stream.read_to_end(&mut all).await.unwrap();
        assert_eq!(all, b"\"hello\"");
    }

    #[tokio::test]
    async fn preview_of_a_container_shows_its_full_pretty_printed_subtree() {
        let source = build(r#"{"a": 1, "b": {"c": 2}}"#, "").unwrap();
        let preview = source.preview_tui(&[], &Cancelled::new()).await;
        assert!(!preview.override_disable_line_numbers);
        assert_eq!(preview.text.plain(), "{\n  \"a\": 1,\n  \"b\": {\n    \"c\": 2\n  }\n}");
    }

    #[tokio::test]
    async fn preview_of_a_leaf_shows_its_value() {
        let source = build(r#"{"a": 42}"#, "").unwrap();
        let preview = source.preview_tui(&["a".to_string()], &Cancelled::new()).await;
        assert!(!preview.override_disable_line_numbers);
        assert_eq!(preview.text.plain(), "42");
    }

    #[test]
    fn split_id_ignores_empty_segments() {
        assert_eq!(split_id("a/b"), vec!["a", "b"]);
        assert_eq!(split_id("/a/b/"), vec!["a", "b"]);
        assert_eq!(split_id(""), Vec::<String>::new());
    }
}
