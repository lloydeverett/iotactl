#!/usr/bin/env bash
# H toggles dotfile visibility, and reloads every open column with it.
set -uo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

make_fixture >/dev/null
start_app "$FIXTURE_DIR"

assert_not_contains '\.hidden_file'
assert_not_contains '\.hidden_dir'
assert_contains '6 items'

send_literal 'H'
assert_contains '\.hidden_file'
assert_contains '\.hidden_dir/'
assert_contains '8 items'

send_literal 'H'
assert_not_contains '\.hidden_file'
assert_not_contains '\.hidden_dir'
assert_contains '6 items'

echo PASS
