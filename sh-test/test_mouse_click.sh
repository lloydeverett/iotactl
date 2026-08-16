#!/usr/bin/env bash
# Mouse click behavior: click selects a row; clicking an already-selected
# row opens it; clicking a row in an earlier, already-open column jumps
# back to it; clicking a non-focused column's border/title (or blank space
# past its last row) also jumps focus there without changing its selection.
set -uo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

make_fixture >/dev/null
start_app "$FIXTURE_DIR"

# Entry rows in the root column, top-to-bottom (dirs first, alphabetical):
# bbb_dir/(0) empty_dir/(1) many_dir/(2) aaa_file.txt(3) long_file.txt(4) ...
# Column boxes always start at (x=0, y=0); the top border/title is row 0, so
# entry index k sits at terminal row k+1. Any x well inside the column
# width is fine since the whole row is clickable.
CLICK_X=5
row_y() { echo $(($1 + 1)); }

# Selection starts on bbb_dir/, already selected -- clicking it should open
# it immediately, same as pressing 'l'/Enter on it (see test_column_stack.sh).
send_mouse_click "$CLICK_X" "$(row_y 0)"
assert_contains ' bbb_dir '
assert_contains 'nested_file\.txt'
assert_contains 'nested content'

# Back to the root column, still with bbb_dir selected.
send_literal 'h'
assert_contains 'nested_file\.txt'
assert_not_contains 'nested content'

# empty_dir is not currently selected: a single click should only move the
# cursor there (preview switches to the "empty directory" placeholder), not
# open it -- opening it would show the "Directory is empty" toast instead.
send_mouse_click "$CLICK_X" "$(row_y 1)"
assert_contains 'empty directory'
assert_not_contains 'Directory is empty:'

# Clicking the same (now-selected) row again opens it, which fails right
# back out with the toast, same as pressing 'l' would (see test_empty_dir.sh).
send_mouse_click "$CLICK_X" "$(row_y 1)"
assert_contains 'Directory is empty: /empty_dir'

# many_dir is not selected (empty_dir still is): click it once to select it
# only -- its directory-listing preview should show file_01.txt among its
# 60 entries, without opening it as a column.
send_mouse_click "$CLICK_X" "$(row_y 2)"
assert_contains 'file_01\.txt'

# Clicking a row in an earlier (already-open) column jumps straight to it,
# truncating the columns to the right of it. many_dir is already selected
# in root, so this same click again opens it, giving two columns: root
# (now non-focused, narrower) selected on many_dir, and many_dir (focused)
# selected on file_01.txt by default -- whose *content* now shows up too.
send_mouse_click "$CLICK_X" "$(row_y 2)"
assert_contains 'line 01 of file 01'

# Clicking root's title/border row (y=0) misses every item's hitbox, but
# root isn't the focused column -- it should still jump focus back to root,
# closing the many_dir column, without touching root's existing selection
# (many_dir stays selected, so its directory listing still shows in the
# preview -- just not file_01.txt's own content anymore).
send_mouse_click "$CLICK_X" 0
assert_not_contains 'line 01 of file 01'
assert_contains 'file_01\.txt'

# We're back to a single root column with many_dir selected (see above).
# Clicking the preview pane itself -- which is showing many_dir's directory
# listing, since it's a directory and hasn't been entered -- should open it
# too, the same as clicking its already-selected row would. The preview
# pane starts right after the focused root column's 40-wide box plus its
# border.
send_mouse_click 41 1
assert_contains 'line 01 of file 01'

echo PASS
