#!/usr/bin/env bash
# Pre-release dependency gate (docs/SECURITY.md S5-2). Run from anywhere; exits non-zero if any
# check fails. Needs network access (advisory databases, the npm registry).
#
#   cargo install cargo-audit cargo-deny --locked
#   tools/security-check.sh
#
# 1. cargo audit: RustSec advisories against both Rust lock files (vulnerabilities fail; unmaintained
#    and yanked crates are printed as warnings).
# 2. cargo deny: advisories, licenses, bans and sources for both Rust workspaces, with the shared
#    policy in engine/deny.toml.
# 3. npm audit: the desktop app's runtime JavaScript dependencies (dev tooling is not shipped).
#
# Lock files are never rewritten (--locked): a stale lock file fails the gate and is fixed by hand.

set -u

root="$(cd "$(dirname "$0")/.." && pwd)"
deny_config="$root/engine/deny.toml"
failed=()

missing=0
for tool in "cargo audit:cargo install cargo-audit --locked" "cargo deny:cargo install cargo-deny --locked" "npm:install Node.js"; do
    cmd="${tool%%:*}"
    if ! $cmd --version >/dev/null 2>&1; then
        echo "missing: $cmd (${tool#*:})" >&2
        missing=1
    fi
done
[ "$missing" -eq 0 ] || exit 2

step() {
    local name="$1"
    shift
    echo
    echo "==> $name"
    if ! "$@"; then
        failed+=("$name")
    fi
}

npm_audit() {
    (cd "$root/desktop" && npm audit --omit=dev)
}

step "cargo audit (engine)" cargo audit --file "$root/engine/Cargo.lock"
step "cargo audit (desktop)" cargo audit --file "$root/desktop/src-tauri/Cargo.lock"
step "cargo deny (engine)" cargo deny --locked --manifest-path "$root/engine/Cargo.toml" \
    --config "$deny_config" check --hide-inclusion-graph
step "cargo deny (desktop)" cargo deny --locked --manifest-path "$root/desktop/src-tauri/Cargo.toml" \
    --config "$deny_config" check --hide-inclusion-graph
step "npm audit (desktop, runtime dependencies)" npm_audit

echo
if [ "${#failed[@]}" -eq 0 ]; then
    echo "security check passed"
else
    printf 'security check FAILED: %s\n' "${failed[@]}"
    exit 1
fi
