#!/usr/bin/env bash
# scripts/check-dep-dag.sh
#
# Enforces the workspace dependency rules from the plan:
#   1. medasr-types is a leaf — no medasr-* deps.
#   2. medasr-state never depends on platform crates (audio, asr, inject,
#      focus, permissions, hotkey).
#   3. Library crates have at most 3 medasr-* peer deps. Library exception:
#      medasr-lifecycle is the orchestrator and may depend on most peers.
#   4. Binaries (medasr-cli, src-tauri) are exempt from the peer-dep cap.
#   5. Only medasr-model may depend on reqwest/hyper/ureq.
#
# Run from the workspace root.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

fail=0

# Helper: list medasr-* peer deps for a crate's Cargo.toml. Returns 0 even
# when there are no matches (grep -E -> 1 would otherwise abort under `set -e`).
peer_deps() {
    { grep -E '^medasr-[a-z-]+[[:space:]]*=|^medasr-[a-z-]+[[:space:]]*\.' "$1" 2>/dev/null || true; } \
        | sed -E 's/[[:space:]]*=.*$//' \
        | sed -E 's/^([a-z-]+).*/\1/' \
        | sort -u
}

# Allowed >3 peer deps for these names (library exception + binaries).
# medasr-lifecycle is the orchestrator (intentional fan-in); medasr-cli,
# medasr-app, and medasr-gui are binaries, exempt from the library cap.
unlimited_peers='medasr-lifecycle medasr-cli medasr-app medasr-gui'

check_crate() {
    local toml="$1"
    local name
    name="$(grep -E '^name\s*=' "$toml" | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"
    local count
    count="$(peer_deps "$toml" | wc -l | tr -d ' ')"

    # Rule 1: medasr-types must have zero medasr-* deps.
    if [[ "$name" == "medasr-types" ]] && [[ "$count" -ne 0 ]]; then
        echo "FAIL: medasr-types is a leaf but has $count peer deps"
        fail=1
    fi

    # Rule 2: medasr-state must not depend on platform crates.
    if [[ "$name" == "medasr-state" ]]; then
        for forbidden in medasr-audio medasr-asr medasr-inject medasr-focus medasr-permissions medasr-hotkey; do
            if peer_deps "$toml" | grep -qx "$forbidden"; then
                echo "FAIL: medasr-state depends on platform crate $forbidden"
                fail=1
            fi
        done
    fi

    # Rule 3+4: peer-dep cap, with exceptions.
    if ! echo "$unlimited_peers" | grep -qw "$name"; then
        if [[ "$count" -gt 3 ]]; then
            echo "FAIL: $name has $count medasr-* peer deps (max 3 for library crates)"
            fail=1
        fi
    fi

    # Rule 5: only medasr-model may use HTTP client deps.
    if [[ "$name" != "medasr-model" ]]; then
        if grep -qE '^(reqwest|hyper|ureq|http|isahc)[[:space:]]*=' "$toml" 2>/dev/null; then
            echo "FAIL: $name uses an HTTP client crate (only medasr-model may)"
            fail=1
        fi
    fi
    return 0
}

for toml in crates/*/Cargo.toml; do
    check_crate "$toml"
done

if [[ "$fail" -eq 0 ]]; then
    echo "dep-dag: OK"
fi
exit "$fail"
