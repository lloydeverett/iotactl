#!/usr/bin/env bash
# Entering an empty directory doesn't push a column: it pops right back and
# shows a toast instead.
set -uo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

make_fixture >/dev/null
start_app "$FIXTURE_DIR"

# Order: bbb_dir/(0), empty_dir/(1), ...
send_literal 'j'
assert_contains 'empty directory'  # preview pane already shows this for empty_dir

send_literal 'l'
assert_contains 'Directory is empty: /empty_dir'

# We should still be sitting in the root column on empty_dir, not have
# descended into it.
assert_contains 'bbb_dir/'
assert_contains 'empty_dir/'

# The preview pane should still show the "empty directory" placeholder for
# the (still-selected) empty_dir, not be left blank by the failed enter().
assert_contains 'empty directory'

echo PASS
