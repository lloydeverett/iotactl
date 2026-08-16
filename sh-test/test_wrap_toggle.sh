#!/usr/bin/env bash
# w toggles preview line-wrapping. Without wrap, a line longer than the pane
# is clipped at the pane edge; with wrap on, the overflow reflows onto the
# next screen row and becomes visible.
set -uo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

make_fixture >/dev/null

d="$FIXTURE_DIR"
# ~140 columns of "filler " before the marker, well past the preview pane's
# width (~55-60 cols at 100x30), so unwrapped it's clipped off-screen.
python3 - "$d/wide_file.txt" <<'PY'
import sys
path = sys.argv[1]
with open(path, "w") as f:
    f.write(("filler " * 20) + "ZZMARKZZ" + (" filler" * 5) + "\n")
PY

start_app "$FIXTURE_DIR"

send_literal 'j'
send_literal 'j'
send_literal 'j'
send_literal 'j'
send_literal 'j'
assert_contains 'filler'
assert_not_contains 'ZZMARKZZ'

send_literal 'w'
assert_contains 'ZZMARKZZ'

send_literal 'w'
assert_not_contains 'ZZMARKZZ'

echo PASS
