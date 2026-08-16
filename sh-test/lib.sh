#!/usr/bin/env bash
# Shared helpers for tmux-driven TUI tests. Each test_*.sh sources this,
# calls make_fixture + start_app, drives the app with send_keys/send_literal,
# and asserts on tmux's rendered pane via assert_contains/assert_not_contains.
#
# Run a single test directly (bash test_foo.sh) or all of them via run_all.sh.

set -uo pipefail

TUI_TEST_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$TUI_TEST_DIR/.." && pwd)"
BIN="$REPO_ROOT/target/debug/iotactl"

COLS=100
ROWS=30
SESSION="iotactl-tui-test-$$"

FIXTURE_DIR=""
_cleanup_ran=0
cleanup() {
    [ "$_cleanup_ran" = 1 ] && return
    _cleanup_ran=1
    tmux kill-session -t "$SESSION" >/dev/null 2>&1 || true
    if [ -n "$FIXTURE_DIR" ]; then
        rm -rf "$FIXTURE_DIR"
    fi
}
trap cleanup EXIT INT TERM

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

# Builds a deterministic directory tree under a fresh temp dir and prints its
# path (also stashed in $FIXTURE_DIR for cleanup). Layout, sorted the way
# fs_source.rs sorts it (directories first, then files, case-insensitive):
#
#   bbb_dir/nested_file.txt
#   empty_dir/                (empty -> triggers the "directory is empty" toast)
#   many_dir/file_01.txt .. file_60.txt
#   aaa_file.txt
#   long_file.txt             (200 lines, for scroll tests)
#   zzz_link.txt@ -> aaa_file.txt
#   .hidden_dir/inner.txt
#   .hidden_file
make_fixture() {
    FIXTURE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/iotactl-tui-test.XXXXXX")"
    local d="$FIXTURE_DIR"

    mkdir -p "$d/bbb_dir" "$d/empty_dir" "$d/many_dir" "$d/.hidden_dir"

    printf 'hello world\nline two\nline three\n' >"$d/aaa_file.txt"
    printf 'nested content\n' >"$d/bbb_dir/nested_file.txt"
    printf 'i am hidden\n' >"$d/.hidden_file"
    printf 'hidden nested\n' >"$d/.hidden_dir/inner.txt"
    ln -s aaa_file.txt "$d/zzz_link.txt"

    local i
    for i in $(seq -w 1 60); do
        printf 'line %s of file %s\n' "$i" "$i" >"$d/many_dir/file_$i.txt"
    done

    : >"$d/long_file.txt"
    for i in $(seq -w 1 200); do
        printf 'L%s\n' "$i" >>"$d/long_file.txt"
    done

    echo "$d"
}

# start_app <dir> — launches iotactl in a detached tmux session against <dir>.
# Args are passed straight to execvp (no shell re-parsing), so <dir> is safe
# even if it contains spaces.
start_app() {
    local dir="$1"
    [ -x "$BIN" ] || fail "binary not found at $BIN -- run 'cargo build' first"
    tmux kill-session -t "$SESSION" >/dev/null 2>&1 || true
    tmux new-session -d -s "$SESSION" -x "$COLS" -y "$ROWS" -- "$BIN" "$dir"
    wait_for '.' 5 || fail "app never rendered anything"
}

capture() {
    tmux capture-pane -t "$SESSION" -p
}

# send_keys <tmux key name...> — e.g. send_keys Enter / send_keys C-d
send_keys() {
    tmux send-keys -t "$SESSION" "$@"
}

# send_literal <string> — sends characters as-is (no key-name interpretation),
# for things like "j", "gg", "H", "w", "G".
send_literal() {
    tmux send-keys -t "$SESSION" -l "$1"
}

# wait_for <extended-regex> [timeout_seconds] — polls the pane until it
# matches, so tests don't race the app's async column/preview fetches.
wait_for() {
    local pattern="$1" timeout="${2:-5}"
    local iterations=$((timeout * 10))
    local i
    for ((i = 0; i < iterations; i++)); do
        capture | grep -Eq "$pattern" && return 0
        sleep 0.1
    done
    return 1
}

wait_for_not() {
    local pattern="$1" timeout="${2:-5}"
    local iterations=$((timeout * 10))
    local i
    for ((i = 0; i < iterations; i++)); do
        capture | grep -Eq "$pattern" || return 0
        sleep 0.1
    done
    return 1
}

wait_for_session_gone() {
    local timeout="${1:-5}"
    local iterations=$((timeout * 10))
    local i
    for ((i = 0; i < iterations; i++)); do
        tmux has-session -t "$SESSION" 2>/dev/null || return 0
        sleep 0.1
    done
    return 1
}

_dump_pane_and_fail() {
    echo "----- pane dump -----" >&2
    capture >&2
    echo "----------------------" >&2
    fail "$1"
}

assert_contains() {
    local pattern="$1" timeout="${2:-5}"
    wait_for "$pattern" "$timeout" || _dump_pane_and_fail "expected pane to contain /$pattern/"
}

assert_not_contains() {
    local pattern="$1" timeout="${2:-5}"
    wait_for_not "$pattern" "$timeout" || _dump_pane_and_fail "expected pane to NOT contain /$pattern/"
}

# line_no <extended-regex> — first matching line number in the current pane
# capture, or empty if no match. Used to assert relative ordering of entries.
line_no() {
    capture | grep -nE "$1" | head -1 | cut -d: -f1
}
