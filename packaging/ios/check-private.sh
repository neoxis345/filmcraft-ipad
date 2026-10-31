#!/bin/bash
# Refuse an app whose files reveal this Mac: its user name, home folder or the repository's path.
#
#   packaging/ios/check-private.sh path/to/FilmCraft.app
set -euo pipefail
APP="${1:?usage: check-private.sh path/to/FilmCraft.app}"
[ -d "$APP" ] || { echo "error: $APP not found" >&2; exit 1; }
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
needles=("$HOME" "$ROOT" "/Users/")
[ -n "${USER:-}" ] && needles+=("$USER")
GIT_EMAIL="$(git -C "$ROOT" config --global user.email 2>/dev/null || true)"
[ -n "$GIT_EMAIL" ] && needles+=("$GIT_EMAIL")
found=0
while IFS= read -r -d '' f; do
  dump="$(strings -a "$f" 2>/dev/null || true)"
  for n in "${needles[@]}"; do
    if grep -qF -- "$n" <<<"$dump"; then
      echo "private text in ${f#"$APP"/}: $(grep -cF -- "$n" <<<"$dump") line(s) match one of the Mac's own names" >&2
      found=1
    fi
  done
done < <(find "$APP" -type f -not -name '*.car' -print0)
[ "$found" = 0 ] || { echo "error: this build reveals private information" >&2; exit 1; }
echo "Nothing private in $(basename "$APP")"
