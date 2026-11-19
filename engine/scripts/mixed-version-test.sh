#!/usr/bin/env bash
# One local command for the mixed-version test (wasl#27): builds the previous release (the ref in
# engine/PREVIOUS_RELEASE) and runs it next to the daemon of this tree. Run it from anywhere.
# Extra arguments go to the test binary, e.g. `-- --nocapture`.
set -euo pipefail
engine="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
paths="$("$engine/scripts/build-previous-release.sh")"
while IFS='=' read -r name value; do
  case "$name" in WASL_PREVIOUS_*) export "$name=$value" ;; esac
done <<< "$paths"
cd "$engine"
exec cargo test -p wasld --features miqyas --test mixed_version "$@"
