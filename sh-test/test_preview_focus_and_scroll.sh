#!/usr/bin/env bash
# Focusing the preview pane on a file, scrolling it (j/k, gg/G, Ctrl-D/U,
# PageUp/PageDown), and returning focus to the column.
set -uo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

make_fixture >/dev/null
start_app "$FIXTURE_DIR"

# Order: bbb_dir/, empty_dir/, many_dir/, aaa_file.txt, long_file.txt, zzz_link.txt@
# Move onto long_file.txt (index 4) and focus its preview.
for _ in 1 2 3 4; do send_literal 'j'; done
assert_contains 'L001'

send_literal 'l'
assert_contains 'j/k scroll'
assert_contains 'L001'

# Half-page down (Ctrl-D, 5 lines): L001 should scroll out of view, L006 in.
send_keys C-d
assert_contains 'L006'
assert_not_contains 'L001'

# Half-page up (Ctrl-U) returns to the top.
send_keys C-u
assert_contains 'L001'

# Full page down/up (10 lines).
send_keys PageDown
assert_contains 'L011'
assert_not_contains 'L001'
send_keys PageUp
assert_contains 'L001'

# gg / G jump to top/bottom.
send_literal 'G'
assert_contains 'L200'
send_literal 'g'
send_literal 'g'
assert_contains 'L001'

# h returns focus to the column stack; footer hint switches back.
send_literal 'h'
assert_contains 'h/j/k/l move'
assert_contains '6 items'

echo PASS
