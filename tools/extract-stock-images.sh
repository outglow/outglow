#!/bin/sh
# Extracts the stock TLSR8258 firmware images of the AwoX app (the
# assets/smartlightz/*.bin of the APK inside the XAPK kept in stock/apk/) to
# stock/apk/smartlightz/, under the short names the emulator tools use:
# "<name>_jing_xun_<hw>_<rest>.bin" -> "<name>__<hw>.bin", e.g.
# EGLO_ZM_RGB_TW__4.60.bin (docs/stock-profiles.md). stock/apk/ is not committed.
#
#   tools/extract-stock-images.sh [app.xapk [out-dir]]
#
# Default app: the newest stock/apk/*.xapk.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
xapk=${1:-$(ls "$root"/stock/apk/*.xapk 2>/dev/null | sort | tail -n 1)}
out=${2:-$root/stock/apk/smartlightz}
[ -n "$xapk" ] && [ -f "$xapk" ] || { echo "no app (XAPK) in stock/apk/" >&2; exit 1; }
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
# the app itself, not one of the config.*.apk splits
inner=$(unzip -Z1 "$xapk" | grep '\.apk$' | grep -v '^config\.' | head -n 1)
[ -n "$inner" ] || { echo "$xapk: no APK inside" >&2; exit 1; }
unzip -p "$xapk" "$inner" > "$tmp/app.apk"
mkdir -p "$out"
n=0
for name in $(unzip -Z1 "$tmp/app.apk" | grep '^assets/smartlightz/.*\.bin$'); do
    base=$(basename "$name" .bin)
    short=$(printf '%s' "$base" | sed -E 's/_jing_xun_([0-9]\.[0-9]+)_.*/__\1/')
    unzip -p "$tmp/app.apk" "$name" > "$out/$short.bin"
    n=$((n + 1))
done
echo "$n images -> $out"
