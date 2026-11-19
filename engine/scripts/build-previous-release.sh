#!/usr/bin/env bash
# Builds `wasld` and `wasl` of the previous release (the ref named in engine/PREVIOUS_RELEASE) from
# a separate checkout of that ref, for the mixed-version test (wasl#27). Needs no network beyond a
# `git fetch` of the ref when this clone does not have it. Never downloads a binary.
#
# Prints the two paths as `WASL_PREVIOUS_WASLD=...` and `WASL_PREVIOUS_WASL=...` lines, the
# environment the test reads. Safe to run again: a build already there for the same ref is reused.
#
# Where things go (all under engine/target unless set):
#   WASL_PREVIOUS_DIR   work folder: src/ (the checkout), target/ (cargo), bin/ (the results)
#   WASL_PREVIOUS_REF   build this ref instead of the one in engine/PREVIOUS_RELEASE
set -euo pipefail

engine="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
root="$(git -C "$engine" rev-parse --show-toplevel)"
work="${WASL_PREVIOUS_DIR:-$engine/target/previous-release}"
ref="${WASL_PREVIOUS_REF:-$(grep -v '^[[:space:]]*#' "$engine/PREVIOUS_RELEASE" | grep -v '^[[:space:]]*$' | head -n 1 | tr -d '[:space:]')}"
[ -n "$ref" ] || { echo "engine/PREVIOUS_RELEASE names no ref" >&2; exit 1; }

exe=""
case "$(uname -s)" in MINGW* | MSYS* | CYGWIN*) exe=".exe" ;; esac

fail() { echo "previous release $ref: $*" >&2; exit 1; }

# The commit of the ref. A full commit hash is its own answer, so a build that is already there
# (the CI cache) needs no git at all; anything else must resolve, and is fetched once if missing.
resolve() { git -C "$root" rev-parse --verify --quiet "$ref^{commit}"; }
if [[ "$ref" =~ ^[0-9a-f]{40}$ ]]; then
  commit="$ref"
elif ! commit="$(resolve)"; then
  echo "fetching $ref" >&2
  git -C "$root" fetch --quiet --depth 1 origin "refs/tags/$ref:refs/tags/$ref" 2>/dev/null \
    || fail "not a tag or commit of this repository, and origin does not have it"
  commit="$(resolve)" || fail "not a commit after fetching"
fi

# The test build of a release may need a small fix to compile (engine/PREVIOUS_RELEASE.patch, when
# that file exists); what was built is named by the commit and the fix together.
patch="$engine/PREVIOUS_RELEASE.patch"
built="$commit"
[ -f "$patch" ] && built="$commit+$(git hash-object "$patch")"

bin="$work/bin"
stamp="$bin/built"
if [ -f "$stamp" ] && [ "$(cat "$stamp")" = "$built" ] && [ -x "$bin/wasld$exe" ] && [ -x "$bin/wasl$exe" ]; then
  echo "reusing the build of $built" >&2
else
  src="$work/src"
  mkdir -p "$work"
  # A commit this clone lacks (a shallow CI checkout) is fetched on its own.
  git -C "$root" cat-file -e "$commit^{commit}" 2>/dev/null \
    || git -C "$root" fetch --quiet --depth 1 origin "$commit" \
    || fail "$commit is not in this clone and origin does not have it"
  # A checkout left by an earlier run is reused only while git still knows it (a cache restore
  # brings back the folder, not the worktree entry in .git).
  if [ -e "$src" ] && ! git -C "$src" rev-parse --git-dir >/dev/null 2>&1; then
    rm -rf "$src"
    git -C "$root" worktree prune
  fi
  if [ -e "$src/.git" ]; then
    git -C "$src" checkout --quiet --force --detach "$commit" || fail "cannot check out $commit in $src"
    git -C "$src" clean --quiet -fd -e target
  else
    git -C "$root" worktree add --quiet --detach "$src" "$commit" || fail "cannot check out $commit into $src"
  fi
  if [ -f "$patch" ]; then
    git -C "$src" apply "$patch" || fail "engine/PREVIOUS_RELEASE.patch does not apply to $commit"
  fi
  echo "building wasld and wasl of $built (the miqyas test build, loopback only)" >&2
  # `cargo` picks the toolchain the old tree names; the old checkout is never built in place of
  # the current one, so its target folder is its own.
  (cd "$src/engine" && CARGO_TARGET_DIR="$work/target" cargo build --locked -p wasld -p wasl-cli --features wasld/miqyas >&2) \
    || fail "the build failed (see the cargo output above)"
  mkdir -p "$bin"
  rm -f "$stamp"
  for name in wasld wasl; do
    cp "$work/target/debug/$name$exe" "$bin/$name$exe" || fail "the build did not produce $name$exe"
  done
  echo "$built" > "$stamp"
fi

# Native Windows tools (cargo, the test) need C:/... paths, not Git Bash's /c/... ones.
out="$bin"
command -v cygpath >/dev/null 2>&1 && out="$(cygpath -m "$bin")"
echo "WASL_PREVIOUS_WASLD=$out/wasld$exe"
echo "WASL_PREVIOUS_WASL=$out/wasl$exe"
