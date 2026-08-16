#!/usr/bin/env bash
# Runs every test_*.sh in this directory against a fresh cargo build and
# prints a pass/fail summary. Usage: sh-test/run_all.sh
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

command -v tmux >/dev/null 2>&1 || {
    echo "tmux is required to run these tests" >&2
    exit 1
}

echo "Building iotactl..."
if ! (cd .. && cargo build --quiet); then
    echo "cargo build failed" >&2
    exit 1
fi

pass=0
fail=0
failed_names=()

for t in test_*.sh; do
    printf '%-42s' "$t"
    if out=$(bash "$t" 2>&1); then
        echo "PASS"
        pass=$((pass + 1))
    else
        echo "FAIL"
        fail=$((fail + 1))
        failed_names+=("$t")
        echo "$out" | sed 's/^/    /'
    fi
done

echo
echo "$pass passed, $fail failed"
if [ "$fail" -gt 0 ]; then
    echo "Failed: ${failed_names[*]}"
    exit 1
fi
