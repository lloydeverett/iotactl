#!/usr/bin/env bash
# Preview sanitization (src/sanitize.rs): tabs expand to spaces and other
# control characters render as printable escapes, instead of ever reaching
# the terminal raw. Regression coverage for the bug where an unescaped tab
# in .git/config desynced the terminal's cursor from ratatui's own column
# bookkeeping and corrupted the screen -- including previews shown *after*
# the offending file, since the corruption was a terminal-state artifact
# that outlived the frame that caused it.
set -uo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

FIXTURE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/iotactl-tui-test.XXXXXX")"
d="$FIXTURE_DIR"

# Tab-indented, like a real .git/config: exercises tab expansion (4-column
# stops) specifically, since that's what originally corrupted the terminal.
printf '[core]\n\tbare = false\n' >"$d/aaa_tabs.txt"

# A mix of C0 controls (SOH, ESC, DEL) and a C1 control (U+0080), each with
# plain text either side, to exercise both caret- and hex-notation
# escaping without disturbing the rest of the line.
printf 'plain\x01text\x1bmore\x7fend\xc2\x80done\n' >"$d/bbb_controls.txt"

# Sorts after both of the above: if sanitizing only fixed the file that
# has the corrupting bytes but left the terminal in a bad state
# afterwards, this file's own (clean) preview would show leftover
# fragments of the earlier ones instead -- the original bug's "and beyond"
# symptom.
printf 'clean content, no funny business\n' >"$d/ccc_after.txt"

start_app "$d"

# aaa_tabs.txt is selected on load (index 0, only file besides the two
# below). The tab should become 4 literal spaces, not a raw tab or
# misaligned/garbled text.
assert_contains '\[core\]'
assert_contains '    bare = false'

send_literal 'j'
assert_contains 'plain\^Atext\^\[more\^\?end<80>done'

send_literal 'j'
assert_contains 'clean content, no funny business'
# No leftover fragments from either earlier preview.
assert_not_contains '\[core\]'
assert_not_contains 'bare = false'
assert_not_contains 'plain\^Atext'

echo PASS
