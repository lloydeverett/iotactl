#!/usr/bin/env bash
# Initial listing content/order, and basic j/k/l/h movement between entries.
set -uo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

make_fixture >/dev/null
start_app "$FIXTURE_DIR"

# Directories sorted before files, both case-insensitive alphabetical:
# bbb_dir/, empty_dir/, many_dir/, aaa_file.txt, long_file.txt, zzz_link.txt@
assert_contains 'bbb_dir/'
assert_contains 'empty_dir/'
assert_contains 'many_dir/'
assert_contains 'aaa_file\.txt'
assert_contains 'long_file\.txt'
assert_contains 'zzz_link\.txt@'

n_bbb=$(line_no 'bbb_dir/')
n_empty=$(line_no 'empty_dir/')
n_many=$(line_no 'many_dir/')
n_aaa=$(line_no 'aaa_file\.txt')
n_zzz=$(line_no 'zzz_link\.txt@')
[ -n "$n_bbb" ] && [ -n "$n_empty" ] && [ -n "$n_many" ] && [ -n "$n_aaa" ] && [ -n "$n_zzz" ] ||
    fail "could not locate all entries in the listing"
[ "$n_bbb" -lt "$n_empty" ] || fail "bbb_dir/ should sort before empty_dir/"
[ "$n_empty" -lt "$n_many" ] || fail "empty_dir/ should sort before many_dir/"
[ "$n_many" -lt "$n_aaa" ] || fail "directories should sort before files"
[ "$n_aaa" -lt "$n_zzz" ] || fail "aaa_file.txt should sort before zzz_link.txt"

# Selection starts on the first entry (bbb_dir), so the preview column shows
# its title and contents.
assert_contains 'nested_file\.txt'

# Move down to aaa_file.txt (3rd entry) and check the preview follows.
send_literal 'j'
send_literal 'j'
send_literal 'j'
assert_contains 'hello world'

# Move back up onto bbb_dir and confirm the preview reverts.
send_literal 'k'
send_literal 'k'
send_literal 'k'
assert_contains 'nested_file\.txt'

# h at the root is a no-op (can't go above the start dir): listing stays put.
send_literal 'h'
assert_contains 'bbb_dir/'
assert_contains '6 items'

echo PASS
