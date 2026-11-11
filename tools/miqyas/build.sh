#!/usr/bin/env bash
# Builds the Miqyas test build of Wasl (docs/testing/MIQYAS.md) from the current commit, release
# profile: wasld and wasl-miqyas with the `miqyas` feature, and the wasl CLI as released. Prints the
# commit and the versions.
#
# Usage: tools/miqyas/build.sh [--allow-dirty]
# Output: $WASL_MIQYAS_TARGET/release (default engine/target/miqyas/release)
set -euo pipefail

repo="$(cd "$(dirname "$0")/../.." && pwd)"
engine="$repo/engine"
# Its own target folder, so a test build never lands where release packaging looks
# (engine/target/release).
target="${WASL_MIQYAS_TARGET:-$engine/target/miqyas}"

if [[ "${1:-}" != "--allow-dirty" && -n "$(git -C "$repo" status --porcelain)" ]]; then
  echo "build.sh: uncommitted changes; commit them or pass --allow-dirty" >&2
  exit 1
fi
commit="$(git -C "$repo" rev-parse HEAD)"
describe="$(git -C "$repo" describe --tags --always --dirty)"

cargo build --manifest-path "$engine/Cargo.toml" --release --target-dir "$target" \
  -p wasld --features wasld/miqyas --bins
cargo build --manifest-path "$engine/Cargo.toml" --release --target-dir "$target" -p wasl-cli

bin="$target/release"
echo "commit:   $commit ($describe)"
echo "wasld:    $("$bin/wasld" --version)"
echo "wasl:     $("$bin/wasl" --version)"
echo "binaries: $bin"
