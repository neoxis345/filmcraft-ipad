#!/bin/bash
# Every iPad size and generation that still gets updates, one simulator after the other: install
# FilmCraft, run packaging/ios/scenarios.py ROUNDS times on each, keep a screenshot per model in
# target/ios-sim/shots. The simulators are created if missing (iPadOS 27 runtime).
#
#   packaging/ios/matrix.sh [rounds] [scenario…]
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROUNDS="${1:-2}"; shift || true
P=com.apple.CoreSimulator.SimDeviceType
RT="$(xcrun simctl list runtimes | sed -n 's/.*\(com.apple.CoreSimulator.SimRuntime.iOS-27-[0-9]*\).*/\1/p' | head -1)"
MODELS=(
  "FC iPad 9:iPad-9th-generation"                       # A13, 3 GB, 10.2" 810 x 1080 pt: the oldest
  "FC iPad mini 6:iPad-mini-6th-generation"             # A15, 4 GB, 8.3" 744 x 1133 pt: the smallest
  "FC iPad 10:iPad-10th-generation"                     # A14, 4 GB, 10.9" 820 x 1180 pt
  "FC iPad Pro 11 2e:iPad-Pro--11-inch---2nd-generation-"  # A12Z, 6 GB, 834 x 1194 pt
  "FC iPad Pro 12.9 4e:iPad-Pro--12-9-inch---4th-generation-"  # A12Z, 6 GB, 1024 x 1366 pt
  "FC iPad Pro 13 M5:iPad-Pro-13-inch-M5-12GB"           # M5, 12 GB, 1032 x 1376 pt: the biggest
)
fail=0
"$HERE/sim.sh" >/dev/null || { echo "build failed"; exit 1; }   # one build for every simulator
for m in "${MODELS[@]}"; do
  name="${m%%:*}"; type="${m##*:}"
  xcrun simctl list devices | grep -qF "$name (" || xcrun simctl create "$name" "$P.$type" "$RT" >/dev/null
  echo "== $name"
  if ! FILMCRAFT_SIM="$name" "$HERE/sim.sh" --no-build >/dev/null; then echo "   FAIL launch"; fail=1; continue; fi
  # a simulator's first boots index apps and draw widgets for minutes: measuring playback then
  # measures the Mac's load, not FilmCraft
  for _ in $(seq 1 30); do
    [ "$(sysctl -n vm.loadavg | awk '{print int($2)}')" -lt 12 ] && break
    sleep 10
  done
  tag="$(echo "$name" | tr ' .' '__')"
  FC_TAG="$tag" "$HERE/scenarios.py" --loop "$ROUNDS" "$@" 2>&1 | sed 's/^/   /' | grep -E "FAIL|failure|OK"
  [ "${PIPESTATUS[0]}" = 0 ] || fail=1
  sim="$(cat "$HERE/../../target/ios-sim/sim")"
  xcrun simctl io "$sim" screenshot "$HERE/../../target/ios-sim/shots/$tag-portrait.png" >/dev/null 2>&1
  xcrun simctl shutdown "$sim" >/dev/null 2>&1
done
exit $fail
