#!/usr/bin/env bash
# Entering a directory pushes a Miller-column; going back up pops it.
set -uo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

make_fixture >/dev/null
start_app "$FIXTURE_DIR"

# Selection starts on bbb_dir/. Enter it: a new focused column titled
# "bbb_dir" appears, listing nested_file.txt, and the preview shows that
# file's contents.
send_literal 'l'
assert_contains ' bbb_dir '
assert_contains 'nested_file\.txt'
assert_contains 'nested content'

# The root column ("iotactl-tui-test.XXXX...") should still be visible to the
# left, just no longer focused/highlighted -- we can at least confirm the
# original root entries are still on screen.
assert_contains 'bbb_dir/'
assert_contains 'empty_dir/'

# Go back up: the bbb_dir column closes and focus returns to the root column,
# back on bbb_dir with its own preview.
send_literal 'h'
assert_contains 'nested_file\.txt'
assert_not_contains 'nested content'

echo PASS
