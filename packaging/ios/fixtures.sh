#!/bin/bash
# Test media for packaging/ios/scenarios.py, made with ffmpeg (a test tool only, never shipped) in
# target/fixtures: iPhone-like HEVC 4K 10-bit, the same tagged Dolby Vision (dvh1), H.264 1080p.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
F="$ROOT/target/fixtures"
mkdir -p "$F"
command -v ffmpeg >/dev/null || { echo "error: ffmpeg is needed (brew install ffmpeg)" >&2; exit 1; }
q=(-hide_banner -loglevel error -y)
[ -s "$F/iphone_hevc_4k.mov" ] || ffmpeg "${q[@]}" -f lavfi -i "testsrc2=size=3840x2160:rate=30,format=yuv420p10le" \
  -f lavfi -i "sine=frequency=440:sample_rate=48000" -t 20 -c:v libx265 -preset ultrafast \
  -x265-params "log-level=error:colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc" -tag:v hvc1 -c:a aac -shortest "$F/iphone_hevc_4k.mov"
[ -s "$F/h264_1080p.mp4" ] || ffmpeg "${q[@]}" -f lavfi -i "testsrc2=size=1920x1080:rate=30" \
  -f lavfi -i "sine=frequency=660:sample_rate=48000" -t 20 -c:v libx264 -preset veryfast -pix_fmt yuv420p -c:a aac -shortest "$F/h264_1080p.mp4"
# ffmpeg will not write a dvh1 entry without Dolby Vision metadata: retag the HEVC copy's stsd
/usr/bin/python3 -I - "$F/iphone_hevc_4k.mov" "$F/iphone_dvh1.mov" <<'PY'
import sys
d = bytearray(open(sys.argv[1], "rb").read())
i = d.find(b"stsd"); j = d.find(b"hvc1", i)
d[j:j + 4] = b"dvh1"
open(sys.argv[2], "wb").write(d)
PY
ls -la "$F"
