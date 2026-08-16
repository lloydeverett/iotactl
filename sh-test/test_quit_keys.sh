#!/usr/bin/env bash
# q, Esc, and Ctrl-C all quit the app.
set -uo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

make_fixture >/dev/null

start_app "$FIXTURE_DIR"
send_literal 'q'
wait_for_session_gone 5 || fail "'q' did not quit the app"

start_app "$FIXTURE_DIR"
send_keys Escape
wait_for_session_gone 5 || fail "Esc did not quit the app"

start_app "$FIXTURE_DIR"
send_keys C-c
wait_for_session_gone 5 || fail "Ctrl-C did not quit the app"

echo PASS
