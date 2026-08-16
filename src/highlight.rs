use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

/// Standard tree-sitter highlight-query capture names we look for. Anything
/// not in this list (including punctuation/operators, left deliberately
/// unstyled to avoid visual noise) renders as plain text.
///
/// The `text.*` and trailing `punctuation.special`/`string.escape` names are
/// specific to `tree-sitter-md`'s block-level highlights query; the rest are
/// the standard programming-language capture names shared by the other
/// grammars.
const RECOGNIZED_NAMES: &[&str] = &[
    "attribute",
    "boolean",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "escape",
    "function",
    "function.builtin",
    "function.macro",
    "keyword",
    "label",
    "module",
    "number",
    "property",
    "string",
    "string.special",
    "string.special.key",
    "tag",
    "type",
    "type.builtin",
    "variable.builtin",
    "variable.parameter",
    "text.title.1",
    "text.title.2",
    "text.title.3",
    "text.title.4",
    "text.title.5",
    "text.title.6",
    "text.literal",
    "text.uri",
    "text.reference",
    "text.emphasis",
    "text.strong",
    "punctuation.special",
    "punctuation.marker",
];

fn style_for(name: &str) -> Option<Style> {
    let color = match name {
        "keyword" => Color::Magenta,
        "string" | "string.special" | "escape" => Color::Green,
        "comment" | "punctuation.special" | "punctuation.marker" => Color::DarkGray,
        "number" | "boolean" | "attribute" => Color::Yellow,
        "type" | "type.builtin" | "constructor" | "tag" => Color::Cyan,
        "function" | "function.builtin" | "function.macro" | "module" => Color::Blue,
        "constant" | "constant.builtin" | "label" => Color::LightRed,
        "variable.builtin" | "variable.parameter" => Color::LightMagenta,
        "property" | "string.special.key" => Color::White,
        // Warm (top-level, most prominent) fading to cool (deepest nesting,
        // least prominent), rainbow-order: red, yellow, green, cyan, blue,
        // magenta.
        "text.title.1" => {
            return Some(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
        }
        "text.title.2" => {
            return Some(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
        }
        "text.title.3" => {
            return Some(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
        }
        "text.title.4" => {
            return Some(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        }
        "text.title.5" => {
            return Some(Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD))
        }
        "text.title.6" => {
            return Some(Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD))
        }
        "text.literal" => Color::Green,
        "text.uri" => Color::Blue,
        "text.reference" => Color::Cyan,
        "text.emphasis" => return Some(Style::default().add_modifier(Modifier::ITALIC)),
        "text.strong" => return Some(Style::default().add_modifier(Modifier::BOLD)),
        _ => return None,
    };
    Some(Style::default().fg(color))
}

/// `tree-sitter-md`'s bundled `INJECTION_QUERY_BLOCK` doesn't set
/// `injection.include-children` on any of its patterns. Without that flag,
/// `tree-sitter-highlight` excludes each captured node's own children from
/// the injected byte range by default — and the block grammar's scanner
/// gives leaf-looking nodes like `(inline)` and `(code_fence_content)`
/// several *anonymous* children (delimiter-run/line tokens used internally
/// for block-level disambiguation, invisible in `to_sexp()`/the named-node
/// tree) that span almost their entire text. Left at the default, the
/// range handed to the injected grammar gets fragmented into the small
/// gaps between those tokens — for `markdown_inline` that skips over
/// emphasis/strong markers entirely; for fenced code blocks it hands the
/// injected grammar a mangled, discontiguous snippet (e.g. HTML's strict
/// tag parser gets no matches at all, silently falling back to an
/// unhighlighted block; more error-tolerant grammars like Python still
/// partially misparse, e.g. `return` failing to highlight as a keyword).
/// So every pattern below sets `injection.include-children`; otherwise
/// identical to the upstream query.
const MARKDOWN_BLOCK_INJECTIONS_QUERY: &str = r#"
((fenced_code_block
  (info_string
    (language) @injection.language)
  (code_fence_content) @injection.content)
 (#set! injection.include-children))

((html_block) @injection.content
  (#set! injection.language "html")
  (#set! injection.include-children))

(document
  .
  (section
    .
    (thematic_break)
    (_) @injection.content
    (thematic_break))
  (#set! injection.language "yaml")
  (#set! injection.include-children))

((minus_metadata) @injection.content
  (#set! injection.language "yaml")
  (#set! injection.include-children))

((plus_metadata) @injection.content
  (#set! injection.language "toml")
  (#set! injection.include-children))

([
  (inline)
  (pipe_table_cell)
] @injection.content
 (#set! injection.language "markdown_inline")
 (#set! injection.include-children))
"#;

/// Same as upstream `tree_sitter_md::HIGHLIGHT_QUERY_BLOCK`, except
/// `(fenced_code_block)` (the container spanning the delimiters *and* the
/// content) is dropped entirely, replaced by `(fenced_code_block_delimiter)`
/// and `(info_string)` specifically — i.e. just the ` ```json ` / ` ``` `
/// lines, not `(code_fence_content)` itself (the now-pointless
/// `(code_fence_content) @none` that used to cancel the inherited tint over
/// the content is dropped too). Tried recognizing `@none` first — mapping it
/// to `Style::default()` to explicitly cancel the green tint over the
/// content — but `(code_fence_content)`'s span is *exactly* the injected
/// python/html/etc. layer's content range, and that exact-boundary overlap
/// between the outer "none" highlight and the injected layer corrupts
/// tree-sitter-highlight's cross-layer event ordering: a keyword's own
/// `HighlightEnd` (e.g. after `def`) got swapped with the far-later `@none`
/// region's end, so the highlight span leaked across unrelated tokens
/// several lines down. Simplest correct fix is to just never apply an outer
/// tint to `(code_fence_content)` in the first place — its delimiter/
/// info_string siblings don't overlap the injected range at all, so
/// tagging *them* is safe, and injected content renders with its own real
/// styling (or plain, if the fence's language isn't recognized) with no
/// ancestor style for anything to fight over.
///
/// The delimiter/info_string pair is tagged `@punctuation.special` (not
/// `@text.literal`) so `highlight()`'s `hide_markers` mode — see that
/// function's doc comment — hides the whole ` ```json `/` ``` ` line, same
/// as it hides `#` heading markers. List markers are tagged the *separate*
/// `@punctuation.marker` instead — dimmed the same way, but never hidden,
/// since unlike a `#` or a fence line, a list bullet is the only thing on
/// screen that says "this is a list item" and dropping it would erase
/// structure rather than decoration.
/// `(indented_code_block)` keeps `@text.literal` outright: it has no
/// language info, so it's never a target of an injection either, and it has
/// no delimiter of its own to hide.
const MARKDOWN_BLOCK_HIGHLIGHTS_QUERY: &str = r#"
(atx_heading
  (atx_h1_marker)
  heading_content: (inline) @text.title.1)

(atx_heading
  (atx_h2_marker)
  heading_content: (inline) @text.title.2)

(atx_heading
  (atx_h3_marker)
  heading_content: (inline) @text.title.3)

(atx_heading
  (atx_h4_marker)
  heading_content: (inline) @text.title.4)

(atx_heading
  (atx_h5_marker)
  heading_content: (inline) @text.title.5)

(atx_heading
  (atx_h6_marker)
  heading_content: (inline) @text.title.6)

(setext_heading
  heading_content: (paragraph) @text.title.1
  (setext_h1_underline))

(setext_heading
  heading_content: (paragraph) @text.title.2
  (setext_h2_underline))

[
  (atx_h1_marker)
  (atx_h2_marker)
  (atx_h3_marker)
  (atx_h4_marker)
  (atx_h5_marker)
  (atx_h6_marker)
  (setext_h1_underline)
  (setext_h2_underline)
] @punctuation.special

[
  (link_title)
  (indented_code_block)
] @text.literal

(link_destination) @text.uri

(link_label) @text.reference

[
  (list_marker_plus)
  (list_marker_minus)
  (list_marker_star)
  (list_marker_dot)
  (list_marker_parenthesis)
] @punctuation.marker

[
  (thematic_break)
  (fenced_code_block_delimiter)
  (info_string)
] @punctuation.special

[
  (block_continuation)
  (block_quote_marker)
] @punctuation.special

(backslash_escape) @string.escape
"#;

/// Same as upstream `tree_sitter_md::HIGHLIGHT_QUERY_INLINE`, except
/// `(emphasis_delimiter)` and `(code_span_delimiter)` — the `*`/`**` around
/// emphasis/strong text and the `` ` `` around a code span — are tagged
/// `@punctuation.special` instead of upstream's `@punctuation.delimiter`.
/// `punctuation.delimiter` isn't in `RECOGNIZED_NAMES`, so upstream's
/// delimiters render as plain, undecorated text; `punctuation.special` is
/// the same "markdown marker" bucket used by `MARKDOWN_BLOCK_HIGHLIGHTS_QUERY`
/// for heading/list/quote markers, which both styles them (dim, like every
/// other marker) and makes them eligible for `highlight()`'s `hide_markers`
/// mode.
const MARKDOWN_INLINE_HIGHLIGHTS_QUERY: &str = r#"
[
  (code_span)
  (link_title)
] @text.literal

[
  (emphasis_delimiter)
  (code_span_delimiter)
] @punctuation.special

(emphasis) @text.emphasis

(strong_emphasis) @text.strong

[
  (link_destination)
  (uri_autolink)
] @text.uri

[
  (link_label)
  (link_text)
  (image_description)
] @text.reference

[
  (backslash_escape)
  (hard_line_break)
] @string.escape

(image
  [
    "!"
    "["
    "]"
    "("
    ")"
  ] @punctuation.delimiter)

(inline_link
  [
    "["
    "]"
    "("
    ")"
  ] @punctuation.delimiter)

(shortcut_link
  [
    "["
    "]"
  ] @punctuation.delimiter)
"#;

/// Same as upstream `tree_sitter_json::HIGHLIGHTS_QUERY`, with `(pair key:
/// (_) @string.special.key)` moved *after* `(string) @string` instead of
/// before it. tree-sitter-highlight resolves multiple patterns matching
/// the same node by taking whichever pattern is declared *later* in the
/// query text (that's the documented convention: put more-specific
/// patterns after more-general ones). Upstream declares the specific key
/// capture first and the generic string capture second, so the generic
/// one silently wins and every JSON key renders with the same color as
/// string values. Swapping the order lets the key capture win instead.
const JSON_HIGHLIGHTS_QUERY: &str = r#"
(string) @string

(pair
  key: (_) @string.special.key)

(number) @number

[
  (null)
  (true)
  (false)
] @constant.builtin

(escape_sequence) @escape

(comment) @comment
"#;

fn build_config(
    lang: tree_sitter_language::LanguageFn,
    name: &'static str,
    highlights_query: &str,
    injections_query: &str,
) -> HighlightConfiguration {
    let mut config = HighlightConfiguration::new(
        lang.into(),
        name,
        highlights_query,
        injections_query,
        "",
    )
    .unwrap_or_else(|e| panic!("failed to build highlight query for {name}: {e}"));
    config.configure(RECOGNIZED_NAMES);
    config
}

fn registry() -> &'static HashMap<&'static str, HighlightConfiguration> {
    static REGISTRY: OnceLock<HashMap<&'static str, HighlightConfiguration>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut m = HashMap::new();
        m.insert(
            "rust",
            build_config(
                tree_sitter_rust::LANGUAGE,
                "rust",
                tree_sitter_rust::HIGHLIGHTS_QUERY,
                tree_sitter_rust::INJECTIONS_QUERY,
            ),
        );
        m.insert(
            "toml",
            build_config(
                tree_sitter_toml_ng::LANGUAGE,
                "toml",
                tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
                "",
            ),
        );
        m.insert(
            "python",
            build_config(
                tree_sitter_python::LANGUAGE,
                "python",
                tree_sitter_python::HIGHLIGHTS_QUERY,
                "",
            ),
        );
        m.insert(
            "javascript",
            build_config(
                tree_sitter_javascript::LANGUAGE,
                "javascript",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::INJECTIONS_QUERY,
            ),
        );
        m.insert(
            "typescript",
            build_config(
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
                "typescript",
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
                "",
            ),
        );
        m.insert(
            "tsx",
            build_config(
                tree_sitter_typescript::LANGUAGE_TSX,
                "tsx",
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
                "",
            ),
        );
        m.insert(
            "json",
            build_config(
                tree_sitter_json::LANGUAGE,
                "json",
                JSON_HIGHLIGHTS_QUERY,
                "",
            ),
        );
        m.insert(
            "yaml",
            build_config(
                tree_sitter_yaml::LANGUAGE,
                "yaml",
                tree_sitter_yaml::HIGHLIGHTS_QUERY,
                "",
            ),
        );
        m.insert(
            "c",
            build_config(
                tree_sitter_c::LANGUAGE,
                "c",
                tree_sitter_c::HIGHLIGHT_QUERY,
                "",
            ),
        );
        m.insert(
            "cpp",
            build_config(
                tree_sitter_cpp::LANGUAGE,
                "cpp",
                tree_sitter_cpp::HIGHLIGHT_QUERY,
                "",
            ),
        );
        m.insert(
            "go",
            build_config(
                tree_sitter_go::LANGUAGE,
                "go",
                tree_sitter_go::HIGHLIGHTS_QUERY,
                "",
            ),
        );
        m.insert(
            "html",
            build_config(
                tree_sitter_html::LANGUAGE,
                "html",
                tree_sitter_html::HIGHLIGHTS_QUERY,
                tree_sitter_html::INJECTIONS_QUERY,
            ),
        );
        m.insert(
            "css",
            build_config(
                tree_sitter_css::LANGUAGE,
                "css",
                tree_sitter_css::HIGHLIGHTS_QUERY,
                "",
            ),
        );
        m.insert(
            "bash",
            build_config(
                tree_sitter_bash::LANGUAGE,
                "bash",
                tree_sitter_bash::HIGHLIGHT_QUERY,
                "",
            ),
        );
        m.insert(
            "java",
            build_config(
                tree_sitter_java::LANGUAGE,
                "java",
                tree_sitter_java::HIGHLIGHTS_QUERY,
                "",
            ),
        );
        m.insert(
            "lua",
            build_config(
                tree_sitter_lua::LANGUAGE,
                "lua",
                tree_sitter_lua::HIGHLIGHTS_QUERY,
                tree_sitter_lua::INJECTIONS_QUERY,
            ),
        );
        m.insert(
            "ruby",
            build_config(
                tree_sitter_ruby::LANGUAGE,
                "ruby",
                tree_sitter_ruby::HIGHLIGHTS_QUERY,
                "",
            ),
        );
        // `tree-sitter-md` splits markdown into a block grammar (headings,
        // code fences, list markers, links) and a separate inline grammar
        // (bold, italic, inline code spans). Our injections query (see
        // `MARKDOWN_BLOCK_INJECTIONS_QUERY`) marks each `(inline)` node
        // with `injection.language = "markdown_inline"`, so registering
        // that name below and driving `highlight()` with a real
        // `injection_callback` (rather than `|_| None`) is enough to make
        // the standard tree-sitter-highlight injection mechanism delegate
        // into it — no separate parse pass needed. The same mechanism also
        // picks up fenced-code-block languages and HTML/YAML/TOML
        // injections declared in that query.
        m.insert(
            "markdown",
            build_config(
                tree_sitter_md::LANGUAGE,
                "markdown",
                MARKDOWN_BLOCK_HIGHLIGHTS_QUERY,
                MARKDOWN_BLOCK_INJECTIONS_QUERY,
            ),
        );
        m.insert(
            "markdown_inline",
            build_config(
                tree_sitter_md::INLINE_LANGUAGE,
                "markdown_inline",
                MARKDOWN_INLINE_HIGHLIGHTS_QUERY,
                tree_sitter_md::INJECTION_QUERY_INLINE,
            ),
        );
        m.insert(
            "dockerfile",
            build_config(
                arborium_dockerfile::language(),
                "dockerfile",
                arborium_dockerfile::HIGHLIGHTS_QUERY,
                arborium_dockerfile::INJECTIONS_QUERY,
            ),
        );
        m
    })
}

fn language_for_extension(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "rs" => "rust",
        "toml" => "toml",
        "py" | "pyw" => "python",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "go" => "go",
        "html" | "htm" => "html",
        "css" => "css",
        "sh" | "bash" | "zsh" | "ksh" => "bash",
        "java" => "java",
        "lua" => "lua",
        "rb" => "ruby",
        "md" | "markdown" => "markdown",
        "dockerfile" => "dockerfile",
        _ => return None,
    })
}

/// Maps well-known filenames (matched without regard to any extension logic)
/// to a language. Checked before the extension-based lookup so a name like
/// `Dockerfile` or `Dockerfile.dev` — which has no extension `language_for_extension`
/// would recognize, or one it would misread as `dev` — still resolves.
fn language_for_filename(name: &str) -> Option<&'static str> {
    let stem = name.split('.').next().unwrap_or(name);
    if stem.eq_ignore_ascii_case("dockerfile") || stem.eq_ignore_ascii_case("containerfile") {
        return Some("dockerfile");
    }
    Some(match name {
        "Cargo.lock" => "toml",
        "Gemfile" | "Rakefile" | "Vagrantfile" => "ruby",
        ".bashrc" | ".bash_profile" | ".bash_login" | ".profile" | ".zshrc" | ".zprofile"
        | ".zshenv" | ".zlogin" | ".zlogout" => "bash",
        _ => return None,
    })
}

/// Maps a shebang line's interpreter (e.g. `#!/usr/bin/env python3` or
/// `#!/bin/bash`) to a language, stripping any `env` wrapper and trailing
/// version digits (`python3.11` -> `python`).
fn language_for_shebang(text: &str) -> Option<&'static str> {
    let first_line = text.lines().next()?;
    let rest = first_line.strip_prefix("#!")?;
    let mut parts = rest.split_whitespace();
    let mut interpreter = parts.next()?.rsplit('/').next().unwrap_or("");
    if interpreter == "env" {
        interpreter = parts.next()?.rsplit('/').next().unwrap_or("");
    }
    let base = interpreter.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    Some(match base {
        "python" => "python",
        "sh" | "bash" | "dash" | "ksh" | "zsh" => "bash",
        "node" | "nodejs" => "javascript",
        "ruby" => "ruby",
        "lua" => "lua",
        _ => return None,
    })
}

/// Resolves `path`/`text` to a registered language name, trying (in order of
/// confidence) a well-known filename, then the extension, then — for
/// extensionless files — a `#!` shebang line.
fn language_for(path: &Path, text: &str) -> Option<&'static str> {
    if let Some(lang) = path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(language_for_filename)
    {
        return Some(lang);
    }
    if let Some(lang) = path
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(language_for_extension)
    {
        return Some(lang);
    }
    language_for_shebang(text)
}

/// Syntax-highlights `text` based on the language inferred from `path` (its
/// filename, extension, or — for extensionless files — a `#!` shebang line
/// in `text`). Returns `None` when nothing is recognized, so the caller can
/// fall back to rendering plain text.
///
/// `hide_markers` drives markdown "render" mode: when set, every span
/// captured as `@punctuation.special` — the markdown-only capture name used
/// for pure formatting decoration such as `#` heading markers, blockquote
/// markers, ` ``` `/info-string fence lines, and `*`/`**`/`` ` `` emphasis
/// and code-span delimiters (see `MARKDOWN_BLOCK_HIGHLIGHTS_QUERY` and
/// `MARKDOWN_INLINE_HIGHLIGHTS_QUERY`) — is dropped from the output instead
/// of rendered, while everything else (including syntax highlighting driven
/// by the presence of those characters, e.g. a heading's color or emphasis's
/// italics) is computed exactly as it would be with the markers shown.
/// List markers are deliberately exempt: they're tagged the separate
/// `@punctuation.marker` and always render, since — unlike the above — they
/// carry structure (there's nothing else on screen marking a line as a list
/// item) rather than pure decoration. Has no effect on non-markdown
/// languages, since none of them produce a `@punctuation.special` capture. A
/// plain, uncaptured whitespace run immediately following a hidden marker on
/// the same line — e.g. the space between an ATX `#` marker and its heading
/// text, which the grammar leaves outside of any node — is swallowed too, so
/// hiding a marker never leaves a dangling leading gap.
pub fn highlight(path: &Path, text: &str, hide_markers: bool) -> Option<Vec<Line<'static>>> {
    let lang_name = language_for(path, text)?;
    let config = registry().get(lang_name).expect("registered above");

    let mut highlighter = Highlighter::new();
    let events = highlighter
        .highlight(config, text.as_bytes(), None, |lang_name| {
            registry().get(lang_name)
        })
        .ok()?;

    let mut lines: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut name_stack: Vec<&str> = Vec::new();
    let mut swallow_leading_space = false;

    for event in events {
        match event.ok()? {
            HighlightEvent::HighlightStart(highlight) => {
                name_stack.push(RECOGNIZED_NAMES[highlight.0]);
            }
            HighlightEvent::HighlightEnd => {
                name_stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                let name = name_stack.last().copied();
                let hidden = hide_markers && name == Some("punctuation.special");

                let mut region = &text[start..end];
                if swallow_leading_space && name.is_none() {
                    region = region.trim_start_matches(' ');
                }
                // Only an ATX heading marker (`#` through `######`) leaves a
                // plain, uncaptured gap before the next node — a blockquote
                // marker folds its trailing space into the marker node
                // itself, and every other hideable marker type (fence
                // delimiters, emphasis/code-span delimiters, ...) sits
                // directly against its neighboring content. So swallowing
                // must key off the marker's own text, not just "was hidden",
                // or hiding e.g. a closing `*` would eat the real space that
                // follows it in the surrounding prose.
                swallow_leading_space =
                    hidden && !region.is_empty() && region.chars().all(|c| c == '#');

                if hidden {
                    // Walk newlines so line boundaries stay correct, but
                    // emit no spans for the hidden marker text itself.
                    for _ in 0..region.matches('\n').count() {
                        lines.push(Vec::new());
                    }
                    continue;
                }

                let style = name.and_then(style_for).unwrap_or_default();
                let mut region_lines = region.split('\n');
                if let Some(first) = region_lines.next() {
                    if !first.is_empty() {
                        lines
                            .last_mut()
                            .expect("lines is never empty")
                            .push(Span::styled(first.to_string(), style));
                    }
                }
                for line in region_lines {
                    lines.push(Vec::new());
                    if !line.is_empty() {
                        lines
                            .last_mut()
                            .expect("just pushed")
                            .push(Span::styled(line.to_string(), style));
                    }
                }
            }
        }
    }

    Some(lines.into_iter().map(Line::from).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    #[test]
    fn markdown_inline_emphasis_is_highlighted() {
        let path = Path::new("sample.md");
        let text = "This has *italic* and **bold** text.\n";
        let lines = highlight(path, text, false).unwrap();
        let spans = &lines[0].spans;
        let styled_texts: Vec<(&str, Style)> =
            spans.iter().map(|s| (s.content.as_ref(), s.style)).collect();

        // The `*`/`**` delimiters are their own `@punctuation.special` spans
        // (dim, split out from the emphasis/strong text) rather than part of
        // the italic/bold span, so `highlight()`'s `hide_markers` mode can
        // drop just the delimiters — see MARKDOWN_INLINE_HIGHLIGHTS_QUERY's
        // doc comment.
        assert!(
            styled_texts
                .iter()
                .any(|(t, s)| *t == "italic" && s.add_modifier.contains(Modifier::ITALIC)),
            "expected an italic span for italic, got {styled_texts:?}"
        );
        assert!(
            styled_texts
                .iter()
                .any(|(t, s)| *t == "bold" && s.add_modifier.contains(Modifier::BOLD)),
            "expected a bold span for bold, got {styled_texts:?}"
        );
        // `**` is scanned as two single-`*` `emphasis_delimiter` tokens
        // rather than one two-char token, so italic contributes 2 dim `*`
        // spans and bold contributes 4.
        assert_eq!(
            styled_texts
                .iter()
                .filter(|(t, s)| *t == "*" && s.fg == Some(Color::DarkGray))
                .count(),
            6,
            "expected all six `*` delimiter chars dim, got {styled_texts:?}"
        );
    }

    #[test]
    fn fenced_html_block_is_highlighted() {
        let path = Path::new("sample.md");
        let text = "```html\n<div class=\"foo\">bar</div>\n```\n";
        let lines = highlight(path, text, false).unwrap();
        let spans = &lines[1].spans;
        let styled_texts: Vec<(&str, Style)> =
            spans.iter().map(|s| (s.content.as_ref(), s.style)).collect();

        assert!(
            styled_texts
                .iter()
                .filter(|(t, s)| *t == "div" && s.fg == Some(Color::Cyan))
                .count()
                == 2,
            "expected both `div` tag names highlighted as cyan, got {styled_texts:?}"
        );
        assert!(
            styled_texts
                .iter()
                .any(|(t, s)| *t == "class" && s.fg == Some(Color::Yellow)),
            "expected `class` attribute highlighted as yellow, got {styled_texts:?}"
        );
    }

    #[test]
    fn fenced_json_block_is_highlighted() {
        let path = Path::new("sample.md");
        let text = "```json\n{\"foo\": \"value\", \"bar\": 42, \"baz\": true}\n```\n";
        let lines = highlight(path, text, false).unwrap();
        let spans = &lines[1].spans;
        let styled_texts: Vec<(&str, Style)> =
            spans.iter().map(|s| (s.content.as_ref(), s.style)).collect();

        assert!(
            styled_texts
                .iter()
                .any(|(t, s)| *t == "42" && s.fg == Some(Color::Yellow)),
            "expected `42` highlighted as a number, got {styled_texts:?}"
        );
        assert!(
            styled_texts
                .iter()
                .any(|(t, s)| *t == "true" && s.fg == Some(Color::LightRed)),
            "expected `true` highlighted as a builtin constant, got {styled_texts:?}"
        );
        assert!(
            styled_texts.iter().any(|(t, s)| *t == "{" && *s == Style::default()),
            "expected unstyled `{{` punctuation (no green code-block wash), got {styled_texts:?}"
        );
        assert!(
            styled_texts
                .iter()
                .any(|(t, s)| *t == "\"foo\"" && s.fg == Some(Color::White)),
            "expected key `\"foo\"` highlighted distinctly (white), got {styled_texts:?}"
        );
        assert!(
            styled_texts
                .iter()
                .any(|(t, s)| *t == "\"value\"" && s.fg == Some(Color::Green)),
            "expected string value `\"value\"` highlighted as a string (green), got {styled_texts:?}"
        );

        // The fence delimiter and info string are `@punctuation.special`
        // (dim, like every other markdown marker) rather than tinted by the
        // fence's own language, so they don't fight visually with the
        // fenced content's real highlighting and so `highlight()`'s
        // `hide_markers` mode can hide them — see
        // MARKDOWN_BLOCK_HIGHLIGHTS_QUERY's doc comment.
        let opening_fence: Vec<(&str, Style)> = lines[0]
            .spans
            .iter()
            .map(|s| (s.content.as_ref(), s.style))
            .collect();
        assert!(
            opening_fence
                .iter()
                .all(|(_, s)| s.fg == Some(Color::DarkGray)),
            "expected the whole opening fence line (```json) to be dim, got {opening_fence:?}"
        );
        let closing_fence: Vec<(&str, Style)> = lines[2]
            .spans
            .iter()
            .map(|s| (s.content.as_ref(), s.style))
            .collect();
        assert!(
            closing_fence
                .iter()
                .all(|(_, s)| s.fg == Some(Color::DarkGray)),
            "expected the closing fence line (```) to be dim, got {closing_fence:?}"
        );
    }

    #[test]
    fn fenced_python_keyword_does_not_leak_across_lines() {
        // Regression test: the `def` keyword's highlight span used to leak
        // all the way to the end of the fenced block (covering `():`, the
        // indent, and the space after `return`) instead of ending right
        // after `def`. See MARKDOWN_BLOCK_HIGHLIGHTS_QUERY's doc comment.
        let path = Path::new("sample.md");
        let text = "```python\ndef foo():\n    return 42\n```\n";
        let lines = highlight(path, text, false).unwrap();
        let spans = &lines[1].spans;
        let styled_texts: Vec<(&str, Style)> =
            spans.iter().map(|s| (s.content.as_ref(), s.style)).collect();

        assert!(
            styled_texts
                .iter()
                .any(|(t, s)| *t == "def" && s.fg == Some(Color::Magenta)),
            "expected `def` highlighted as a keyword, got {styled_texts:?}"
        );
        assert!(
            !styled_texts
                .iter()
                .any(|(t, s)| t.contains("():") && s.fg == Some(Color::Magenta)),
            "keyword highlight leaked past `def` onto `():`, got {styled_texts:?}"
        );
        assert!(
            styled_texts
                .iter()
                .any(|(t, s)| *t == "foo" && s.fg == Some(Color::Blue)),
            "expected `foo` highlighted as a function name, got {styled_texts:?}"
        );
    }

    #[test]
    fn plain_rust_file_is_highlighted() {
        // Every other test drives highlighting through markdown's fenced-code
        // injection. This one exercises the plain, non-markdown path
        // (opening a `.rs` file directly) so a break in the base per-language
        // registry/query wiring can't hide behind markdown-only coverage.
        let path = Path::new("sample.rs");
        let text = "fn main() {\n    let x = 42;\n}\n";
        let lines = highlight(path, text, false).unwrap();
        let styled_texts: Vec<(&str, Style)> = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| (s.content.as_ref(), s.style))
            .collect();

        assert!(
            styled_texts
                .iter()
                .any(|(t, s)| *t == "fn" && s.fg == Some(Color::Magenta)),
            "expected `fn` highlighted as a keyword, got {styled_texts:?}"
        );
        assert!(
            styled_texts
                .iter()
                .any(|(t, s)| *t == "main" && s.fg == Some(Color::Blue)),
            "expected `main` highlighted as a function name, got {styled_texts:?}"
        );
        assert!(
            // tree-sitter-rust captures integer literals as
            // `@constant.builtin` rather than `@number`.
            styled_texts
                .iter()
                .any(|(t, s)| *t == "42" && s.fg == Some(Color::LightRed)),
            "expected `42` highlighted as a builtin constant, got {styled_texts:?}"
        );
    }

    #[test]
    fn unrecognized_extension_returns_none() {
        let path = Path::new("sample.this-extension-does-not-exist");
        assert!(highlight(path, "hello\n", false).is_none());
    }

    #[test]
    fn dockerfile_is_recognized_by_bare_filename() {
        let path = Path::new("Dockerfile");
        let text = "FROM ubuntu:22.04\nRUN echo hi\n";
        let lines = highlight(path, text, false).unwrap();
        let styled_texts: Vec<(&str, Style)> = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| (s.content.as_ref(), s.style))
            .collect();

        assert!(
            styled_texts
                .iter()
                .any(|(t, s)| *t == "FROM" && s.fg == Some(Color::Magenta)),
            "expected `FROM` highlighted as a keyword, got {styled_texts:?}"
        );
        assert!(
            styled_texts
                .iter()
                .any(|(t, s)| *t == "RUN" && s.fg == Some(Color::Magenta)),
            "expected `RUN` highlighted as a keyword, got {styled_texts:?}"
        );
    }

    #[test]
    fn dockerfile_suffixed_variant_is_recognized() {
        let path = Path::new("Dockerfile.dev");
        let text = "FROM scratch\n";
        assert!(highlight(path, text, false).is_some());
    }

    #[test]
    fn cargo_lock_is_highlighted_as_toml() {
        let path = Path::new("Cargo.lock");
        let text = "# This file is automatically generated by Cargo.\nversion = 4\n\n[[package]]\nname = \"iotactl\"\n";
        let lines = highlight(path, text, false).unwrap();
        let styled_texts: Vec<(&str, Style)> = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| (s.content.as_ref(), s.style))
            .collect();

        assert!(
            styled_texts
                .iter()
                .any(|(t, s)| *t == "\"iotactl\"" && s.fg == Some(Color::Green)),
            "expected string value highlighted, got {styled_texts:?}"
        );
    }

    #[test]
    fn shebang_detects_python_for_extensionless_file() {
        let path = Path::new("myscript");
        let text = "#!/usr/bin/env python3\ndef foo():\n    return 1\n";
        let lines = highlight(path, text, false).unwrap();
        let styled_texts: Vec<(&str, Style)> = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| (s.content.as_ref(), s.style))
            .collect();

        assert!(
            styled_texts
                .iter()
                .any(|(t, s)| *t == "def" && s.fg == Some(Color::Magenta)),
            "expected `def` highlighted as a keyword, got {styled_texts:?}"
        );
    }

    #[test]
    fn shebang_detects_bash_without_env_wrapper() {
        let path = Path::new("myscript");
        let text = "#!/bin/bash\necho hi\n";
        assert!(highlight(path, text, false).is_some());
    }

    #[test]
    fn extension_takes_priority_over_shebang() {
        // A `.py` file whose shebang points at a shell should still be
        // highlighted as Python, since the extension is the stronger signal.
        let path = Path::new("script.py");
        let text = "#!/bin/sh\ndef foo():\n    return 1\n";
        let lines = highlight(path, text, false).unwrap();
        let styled_texts: Vec<(&str, Style)> = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| (s.content.as_ref(), s.style))
            .collect();

        assert!(
            styled_texts
                .iter()
                .any(|(t, s)| *t == "def" && s.fg == Some(Color::Magenta)),
            "expected `def` highlighted as a keyword (Python won over shebang's `sh`), got {styled_texts:?}"
        );
    }

    #[test]
    fn no_shebang_and_no_extension_returns_none() {
        let path = Path::new("myscript");
        assert!(highlight(path, "just some text\n", false).is_none());
    }

    #[test]
    fn fenced_block_with_unknown_language_falls_back_to_plain_text() {
        // The fence's `injection.language` capture is whatever text follows
        // the opening ``` — for a language we don't have a grammar for, the
        // injection callback returns None and the content must render as
        // plain, unstyled text rather than panicking or losing content.
        let path = Path::new("sample.md");
        let text = "```not-a-real-language\nsome content here\n```\n";
        let lines = highlight(path, text, false).unwrap();
        let content_line: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(content_line, "some content here");
        assert!(
            lines[1].spans.iter().all(|s| s.style == Style::default()),
            "expected unrecognized fenced language to render as fully plain text, got {:?}",
            lines[1].spans
        );
    }

    #[test]
    fn yaml_frontmatter_is_highlighted() {
        // Regression coverage for MARKDOWN_BLOCK_INJECTIONS_QUERY's
        // `(minus_metadata) @injection.content` pattern, which was
        // hand-written (to add `injection.include-children`) but never
        // actually exercised by a test.
        let path = Path::new("sample.md");
        let text = "---\nkey: value\n---\n";
        let lines = highlight(path, text, false).unwrap();
        let styled_texts: Vec<(&str, Style)> = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| (s.content.as_ref(), s.style))
            .collect();

        assert!(
            styled_texts
                .iter()
                .any(|(t, s)| *t == "key" && s.fg == Some(Color::White)),
            "expected frontmatter key highlighted distinctly (white), got {styled_texts:?}"
        );
        assert!(
            styled_texts
                .iter()
                .any(|(t, s)| *t == "value" && s.fg == Some(Color::Green)),
            "expected frontmatter value highlighted as a string (green), got {styled_texts:?}"
        );
    }

    #[test]
    fn heading_levels_get_distinct_colors() {
        let path = Path::new("sample.md");
        let text = "# One\n## Two\n### Three\n#### Four\n##### Five\n###### Six\n";
        let lines = highlight(path, text, false).unwrap();
        let heading_style = |line: usize, text_content: &str| {
            lines[line]
                .spans
                .iter()
                .find(|s| s.content.as_ref() == text_content)
                .unwrap_or_else(|| panic!("no span {text_content:?} on line {line}: {:?}", lines[line].spans))
                .style
        };

        let colors: Vec<Option<Color>> = vec![
            heading_style(0, "One").fg,
            heading_style(1, "Two").fg,
            heading_style(2, "Three").fg,
            heading_style(3, "Four").fg,
            heading_style(4, "Five").fg,
            heading_style(5, "Six").fg,
        ];

        for c in &colors {
            assert!(c.is_some(), "expected every heading level to have a color, got {colors:?}");
        }
        let unique: std::collections::HashSet<_> = colors.iter().collect();
        assert_eq!(
            unique.len(),
            colors.len(),
            "expected all six heading levels to have distinct colors, got {colors:?}"
        );
    }

    #[test]
    fn setext_headings_get_distinct_colors() {
        let path = Path::new("sample.md");
        let text = "Title One\n=========\n\nTitle Two\n---------\n";
        let lines = highlight(path, text, false).unwrap();

        let one_color = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "Title One")
            .expect("Title One span")
            .style
            .fg;
        // Blank line then underline separates the two setext headings.
        let two_line = lines
            .iter()
            .position(|l| l.spans.iter().any(|s| s.content.as_ref() == "Title Two"))
            .expect("Title Two line");
        let two_color = lines[two_line]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "Title Two")
            .expect("Title Two span")
            .style
            .fg;

        assert!(one_color.is_some() && two_color.is_some());
        assert_ne!(
            one_color, two_color,
            "expected setext h1 and h2 to have distinct colors"
        );
    }

    #[test]
    fn hide_markers_strips_atx_heading_marker_and_gap() {
        let path = Path::new("sample.md");
        let text = "# Header\n";
        let lines = highlight(path, text, true).unwrap();
        let rendered: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(
            rendered, "Header",
            "expected the `#` marker and the gap after it both dropped, got {:?}",
            lines[0].spans
        );
        let heading_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "Header")
            .expect("Header span");
        assert!(
            heading_span.style.fg == Some(Color::Red) && heading_span.style.add_modifier.contains(Modifier::BOLD),
            "expected heading styling preserved even with markers hidden, got {:?}",
            heading_span.style
        );
    }

    #[test]
    fn hide_markers_blanks_fenced_code_delimiter_lines() {
        let path = Path::new("sample.md");
        let text = "```rust\nfn f() {}\n```\n";
        let lines = highlight(path, text, true).unwrap();
        assert!(
            lines[0].spans.is_empty(),
            "expected the opening ``` fence line to be blank, got {:?}",
            lines[0].spans
        );
        assert!(
            lines[2].spans.is_empty(),
            "expected the closing ``` fence line to be blank, got {:?}",
            lines[2].spans
        );
        let content: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(content, "fn f() {}");
        assert!(
            lines[1]
                .spans
                .iter()
                .any(|s| s.content.as_ref() == "fn" && s.style.fg == Some(Color::Magenta)),
            "expected fenced content to still be syntax-highlighted with markers hidden, got {:?}",
            lines[1].spans
        );
    }

    #[test]
    fn hide_markers_strips_emphasis_and_code_span_delimiters() {
        let path = Path::new("sample.md");
        let text = "This is *italic* and `code`.\n";
        let lines = highlight(path, text, true).unwrap();
        let rendered: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(rendered, "This is italic and code.");
        let italic_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "italic")
            .expect("italic span");
        assert!(
            italic_span.style.add_modifier.contains(Modifier::ITALIC),
            "expected emphasis styling preserved even with the `*` delimiters hidden, got {:?}",
            italic_span.style
        );
        let code_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "code")
            .expect("code span");
        assert_eq!(
            code_span.style.fg,
            Some(Color::Green),
            "expected code-span styling preserved even with the ` delimiters hidden, got {:?}",
            code_span.style
        );
    }

    #[test]
    fn hide_markers_strips_blockquote_marker_but_keeps_list_marker() {
        // List bullets carry structure (nothing else on screen says "this
        // is a list item"), so hide_markers exempts them — unlike a
        // blockquote marker, which is pure decoration and still gets hidden.
        let path = Path::new("sample.md");
        let text = "- item one\n> quoted\n";
        let lines = highlight(path, text, true).unwrap();
        let line0: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        let line1: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(line0, "- item one");
        assert_eq!(line1, "quoted");
        let marker_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "- ")
            .expect("list marker span");
        assert_eq!(
            marker_span.style.fg,
            Some(Color::DarkGray),
            "expected the list marker to still be dimmed, got {:?}",
            marker_span.style
        );
    }

    #[test]
    fn hide_markers_is_noop_for_non_markdown() {
        let path = Path::new("sample.rs");
        let text = "fn main() {\n    let x = 42;\n}\n";
        let shown = highlight(path, text, false).unwrap();
        let hidden = highlight(path, text, true).unwrap();
        let render = |lines: &[Line<'static>]| -> String {
            lines
                .iter()
                .flat_map(|l| l.spans.iter())
                .map(|s| s.content.as_ref())
                .collect()
        };
        assert_eq!(
            render(&shown),
            render(&hidden),
            "hide_markers should have no effect on a language with no @punctuation.special capture"
        );
    }
}
