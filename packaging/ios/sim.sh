#!/bin/bash
# Build FilmCraft (Debug) for the iPad simulator, install it, launch it with the control channel
# open on 127.0.0.1:9876, and wait until it answers. No iPad needed:
#
#   packaging/ios/sim.sh              build, install, launch
#   packaging/ios/sim.sh --no-build   launch the app already installed
#
# The token for the control channel is written to target/ios-sim/token (scenarios.py reads it).
# FILMCRAFT_SIM=<name or udid> picks the simulator (default: "iPad (A16)", the 2360 × 1640 screen
# of an iPad 10th generation).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
OUT="$ROOT/target/ios-sim"
mkdir -p "$OUT"
export PATH="$HOME/.cargo/bin:/opt/homebrew/opt/rustup/bin:/usr/local/opt/rustup/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

NAME="${FILMCRAFT_SIM:-iPad (A16)}"
LIST="$(xcrun simctl list devices available)"
# a name ("iPad (A16)") or already an identifier (no match: used as it is)
SIM="$(printf '%s\n' "$LIST" | grep -F "$NAME (" | sed -n '1s/.*(\([0-9A-F-]\{36\}\)).*/\1/p' || true)"
if [ -z "$SIM" ]; then SIM="$NAME"; fi
xcrun simctl boot "$SIM" 2>/dev/null || true
xcrun simctl bootstatus "$SIM" -b >/dev/null

APP="$OUT/DerivedData/Build/Products/Debug-iphonesimulator/FilmCraft.app"
if [ "${1:-}" != "--no-build" ]; then
  INSTALLED="$(rustup target list --installed 2>/dev/null || true)"
  grep -qx 'aarch64-apple-ios-sim' <<<"$INSTALLED" || rustup target add aarch64-apple-ios-sim
  [ -f "$HERE/Generated/Signing.xcconfig" ] || echo "// simulator builds are not signed" > "$HERE/Generated/Signing.xcconfig"
  # the linker flags (Rust.xcconfig) must exist before Xcode reads the project
  PLATFORM_NAME=iphonesimulator CONFIGURATION=Debug "$HERE/build-rust.sh"
  (cd "$HERE" && xcodegen generate --quiet)
  xcodebuild -project "$HERE/FilmCraft.xcodeproj" -scheme FilmCraft -configuration Debug \
    -destination "id=$SIM" -derivedDataPath "$OUT/DerivedData" build > "$OUT/xcodebuild.log" 2>&1 || true
  if ! grep -q "BUILD SUCCEEDED" "$OUT/xcodebuild.log"; then
    grep -E "error:" "$OUT/xcodebuild.log" | sort -u | head -20 >&2
    echo "error: the simulator build failed, see $OUT/xcodebuild.log" >&2
    exit 1
  fi
  xcrun simctl install "$SIM" "$APP"
fi

BUNDLE="$(/usr/libexec/PlistBuddy -c 'Print CFBundleIdentifier' "$APP/Info.plist")"
# another simulator than the one built for: install the app there too
xcrun simctl get_app_container "$SIM" "$BUNDLE" >/dev/null 2>&1 || xcrun simctl install "$SIM" "$APP"
# FilmCraft running in another simulator would hold the control port: only this one answers
for other in $(xcrun simctl list devices booted | sed -n 's/.*(\([0-9A-F-]\{36\}\)).*/\1/p'); do
  [ "$other" = "$SIM" ] || xcrun simctl terminate "$other" "$BUNDLE" >/dev/null 2>&1 || true
done
TOKEN="$(/usr/bin/python3 -c 'import secrets; print(secrets.token_hex(32))')"
printf '%s\n' "$TOKEN" > "$OUT/token"
chmod 600 "$OUT/token"
SIMCTL_CHILD_FILMCRAFT_CONTROL_TOKEN="$TOKEN" SIMCTL_CHILD_FILMCRAFT_CONTROL_PORT=9876 \
  xcrun simctl launch --terminate-running-process "$SIM" "$BUNDLE" >/dev/null
for _ in $(seq 1 90); do
  if /usr/bin/python3 -c 'import socket; socket.create_connection(("127.0.0.1", 9876), 1).close()' 2>/dev/null; then
    echo "FilmCraft runs in the simulator $SIM (control channel on 127.0.0.1:9876)"
    echo "$SIM" > "$OUT/sim"
    exit 0
  fi
  sleep 1
done
echo "error: FilmCraft did not open its control channel in 90 s" >&2
exit 1
