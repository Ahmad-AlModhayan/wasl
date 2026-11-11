#!/bin/bash
# Signs a packaged Wasl.app for files on demand (docs/ON_DEMAND.md §2.6), inside out: the
# extension with its sandbox and app group, then wasld, the CLI, the helper and the app with the
# app group (wasld opens its File Provider socket in the group's container). Run after
# `npm run package`, with the same team as build.sh:
#
#   WASL_TEAM_ID=ABCDE12345 WASL_SIGN_IDENTITY="Developer ID Application: …" \
#     ./sign.sh ../src-tauri/target/release/bundle/macos/Wasl.app
#
# A Developer ID build is then notarized as usual (xcrun notarytool submit, xcrun stapler staple)
# and the DMG made again from the signed app.

set -euo pipefail
cd "$(dirname "$0")"

APP="${1:?usage: sign.sh path/to/Wasl.app}"
IDENTITY="${WASL_SIGN_IDENTITY:?set WASL_SIGN_IDENTITY (security find-identity -v -p codesigning)}"
: "${WASL_TEAM_ID:?set WASL_TEAM_ID}"
[ -f build/App.entitlements ] || { echo "run build.sh first" >&2; exit 1; }
grep -q "$WASL_TEAM_ID.com.kahflabs.wasl" build/App.entitlements ||
  { echo "build/ was built for another team: run build.sh with WASL_TEAM_ID=$WASL_TEAM_ID" >&2; exit 1; }

sign() { codesign --force --timestamp --options runtime --sign "$IDENTITY" "$@"; }
sign --entitlements build/Extension.entitlements "$APP/Contents/PlugIns/WaslFileProvider.appex"
for exe in wasld wasl wasl-fileprovider; do
  sign --entitlements build/App.entitlements "$APP/Contents/MacOS/$exe"
done
sign --entitlements build/App.entitlements "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"
