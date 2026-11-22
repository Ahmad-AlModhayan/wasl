#!/usr/bin/env bash
# Own CI script for Wasl. Called by kahf-ci (and optionally GitHub Actions) with the leg or extra name.
set -euo pipefail

leg="${1:?usage: ci/run.sh <leg|extra>}"
echo "wasl ci: ${leg}"

cd engine

# Use a writable, cacheable Cargo home inside the workspace when the image's default CARGO_HOME is read-only (e.g. uid 1000).
if [ "${leg}" != "nat-lab" ]; then
  export CARGO_HOME="$PWD/.cargo"
fi

if [ "${leg}" = "nat-lab" ]; then
  echo "::step:: nat-lab tools"
  if ! command -v ip >/dev/null 2>&1 || ! command -v iptables >/dev/null 2>&1; then
    apt-get update -qq && apt-get install -y --no-install-recommends iproute2 iptables
  fi

  echo "::step:: build nat-lab"
  cargo test -p wasl-net --test nat_lab --no-run

  echo "::step:: run nat-lab"
  bin=$(cargo test -p wasl-net --test nat_lab --no-run --message-format=json \
    | jq -r 'select(.profile.test == true and .target.name == "nat_lab") | .executable')
  WASL_NAT_LAB=1 "$bin" nat_lab --exact --nocapture --test-threads=1
  exit 0
fi

echo "::step:: rustfmt"
cargo fmt --all --check

echo "::step:: license boundaries"
if command -v python3 >/dev/null 2>&1; then
  python3 scripts/check-license-boundaries.py
else
  python scripts/check-license-boundaries.py
fi

echo "::step:: clippy"
cargo clippy --workspace --all-targets -- -D warnings

echo "::step:: clippy no-unwrap"
cargo clippy --workspace --lib --bins --examples -- -D warnings -D clippy::unwrap_used

echo "::step:: test workspace"
if [ "${leg}" = "linux" ] && [ -r /dev/fuse ] && [ -w /dev/fuse ]; then
  WASL_FUSE_TESTS=require cargo test --workspace
else
  cargo test --workspace
fi

echo "::step:: two-daemon tests"
cargo test -p wasld --features miqyas --test two_daemons

if [ "${leg}" = "linux" ]; then
  echo "::step:: kotlin bindings"
  cargo build -p wasl-ffi
  out_dir="${TMPDIR:-/tmp}/wasl-kotlin-bindings"
  rm -rf "$out_dir"
  cargo run -p wasl-ffi --features bindgen --bin uniffi-bindgen -- generate \
    --library target/debug/libwasl_mobile.so --language kotlin --no-format --out-dir "$out_dir"
  for name in \
    "interface StatusSink" \
    "data class FolderStatus" \
    "data class DeviceStatus" \
    "enum class DevicePresence" \
    "fun `watchStatus`" \
    "fun `deviceStatuses`" \
    "class PairRuleException" \
    "fun `createPairTyped`" \
    "fun `updatePairTyped`" \
    "fun `deletePairTyped`"; do
    grep -rqF -- "$name" "$out_dir" || { echo "missing in the Kotlin bindings: $name"; exit 1; }
  done
fi
