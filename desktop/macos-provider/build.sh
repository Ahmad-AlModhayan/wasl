#!/bin/bash
# Builds Wasl's File Provider extension and its helper (docs/ON_DEMAND.md §2.6) with the Swift
# compiler alone (Command Line Tools are enough; Xcode is not needed):
#
#   build/WaslFileProvider.appex   goes to Wasl.app/Contents/PlugIns
#   build/wasl-fileprovider        goes to Wasl.app/Contents/MacOS, next to wasld
#   build/App.entitlements         for Wasl.app, wasld and the helper (sign.sh)
#   build/Extension.entitlements   for the extension
#
# Signing needs an Apple team: the app group is "<team id>.com.kahflabs.wasl", and the system
# runs a File Provider extension only from an app signed by that team (Apple Development for
# testing, Developer ID for release).
#
#   WASL_TEAM_ID=ABCDE12345 WASL_SIGN_IDENTITY="Developer ID Application: …" ./build.sh
#
# Without them it builds ad-hoc signed, with a placeholder team: the bundle is complete and
# checkable, but macOS won't run the extension.

set -euo pipefail
cd "$(dirname "$0")"

TEAM="${WASL_TEAM_ID:-LOCALDEV00}"
IDENTITY="${WASL_SIGN_IDENTITY:--}"
GROUP="$TEAM.com.kahflabs.wasl"
VERSION="$(sed -n 's/^  "version": "\(.*\)",$/\1/p' ../src-tauri/tauri.conf.json)"
ARCH="$(uname -m)"
TARGET="$ARCH-apple-macos13.0"
OUT=build
APPEX="$OUT/WaslFileProvider.appex"

rm -rf "$OUT"
mkdir -p "$APPEX/Contents/MacOS"
fill() { sed -e "s/@GROUP@/$GROUP/g" -e "s/@VERSION@/$VERSION/g" "$1" > "$2"; }
fill Extension-Info.plist "$APPEX/Contents/Info.plist"
fill Extension.entitlements "$OUT/Extension.entitlements"
fill App.entitlements "$OUT/App.entitlements"

SWIFTC=(xcrun swiftc -O -swift-version 5 -target "$TARGET" -module-cache-path "$OUT/cache")

# An app extension: Foundation's NSExtensionMain is the entry point.
"${SWIFTC[@]}" -module-name WaslFileProvider -parse-as-library -application-extension \
  Sources/Shared/*.swift Sources/Extension/*.swift \
  -framework FileProvider -Xlinker -e -Xlinker _NSExtensionMain \
  -o "$APPEX/Contents/MacOS/WaslFileProvider"

"${SWIFTC[@]}" -module-name WaslFileProviderCtl \
  Sources/Shared/*.swift Sources/Control/main.swift \
  -framework FileProvider \
  -o "$OUT/wasl-fileprovider"
rm -rf "$OUT/cache"

codesign --force --sign "$IDENTITY" --options runtime --entitlements "$OUT/Extension.entitlements" "$APPEX"
codesign --force --sign "$IDENTITY" --options runtime --entitlements "$OUT/App.entitlements" "$OUT/wasl-fileprovider"
echo "built $APPEX and $OUT/wasl-fileprovider (team $TEAM, group $GROUP, signed: $IDENTITY)"
