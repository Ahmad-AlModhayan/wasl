#!/usr/bin/env bash
# One run as Miqyas makes it (docs/testing/MIQYAS.md): two test devices on this machine, 100 files
# made on A, the time until B holds them, then teardown. Prints wait's JSON line.
#
# Usage: tools/miqyas/sync-100.sh [BIN_DIR]   (default: engine/target/miqyas/release, see build.sh)
set -euo pipefail

repo="$(cd "$(dirname "$0")/../.." && pwd)"
bin="${1:-$repo/engine/target/miqyas/release}"
harness="$bin/wasl-miqyas"
# Short on purpose: the daemon's socket path must stay under 104 bytes on macOS.
root="$(mktemp -d "${TMPDIR:-/tmp}/wasl-miqyas.XXXXXX")"
trap '"$harness" down --root "$root" >/dev/null 2>&1 || true' EXIT

"$harness" up --root "$root" >/dev/null
a="$root/a/folder"
for i in $(seq -w 1 100); do
  printf 'file %s\n' "$i" >"$a/file-$i.txt"
done
"$harness" wait --root "$root" --timeout 120
"$harness" down --root "$root" >/dev/null
trap - EXIT
