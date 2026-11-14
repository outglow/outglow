#!/usr/bin/env bash
# Safe TLSR825x firmware flash over SWS, using pvvx/TlsrComProg825x.
# Enforces the order: STOCK DUMP -> write -> verify. A partial write (we 0)
# erases only the image sectors. The bulb flash is 1 MB (AwoX stock: bootloader at
# 0x0, application at 0x6a000..0xae274 - see docs/emulation.md). The chip factory
# data (MAC address 0xFF000, calibration 0xFE000) sit at the end and are NOT touched:
# our full image (bootloader + application) ends below 0x50000.
#
# Usage:
#   ./flash.sh -c            -f ../firmware/build/open-eglo-full.bin   # preflight (no hardware)
#   ./flash.sh -p /dev/ttyUSB0 -f ../firmware/build/open-eglo-full.bin [-t 600]
#
# Requires: python3, pyserial, and the TlsrComProg825x directory (cloned if missing).
set -euo pipefail

PORT=""; FW=""; TACT=600; CHECK_ONLY=0
PROG_DIR="${TLSRCOMPROG_DIR:-$HOME/oss/TlsrComProg825x}"
FLASH_SIZE=0x100000      # 1 MB (TLSR8258F1)
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

usage() { echo "usage: $0 -p PORT -f FIRMWARE.bin [-t ACT_MS] | $0 -c -f FIRMWARE.bin"; exit 1; }
while getopts "p:f:t:d:ch" o; do case "$o" in
  p) PORT=$OPTARG;; f) FW=$OPTARG;; t) TACT=$OPTARG;; d) PROG_DIR=$OPTARG;; c) CHECK_ONLY=1;; *) usage;;
esac; done
[ -n "$FW" ] || usage
[ -f "$FW" ] || { echo "no firmware file: $FW" >&2; exit 1; }

# --- preflight: validates tools and image BEFORE we touch the flash ---
preflight() {
  local ok=1
  python3 -c 'import serial' 2>/dev/null || { echo "  [x] no pyserial (pip install pyserial)"; ok=0; }
  if [ -d "$PROG_DIR" ] || command -v git >/dev/null; then echo "  [v] TlsrComProg: $PROG_DIR (cloned if missing)";
  else echo "  [x] no $PROG_DIR and no git to clone it"; ok=0; fi
  # the image must be bootable: byte 8 == 0x4b ('K', KNLT marker checked by boot ROM)
  local b8; b8=$(od -An -tx1 -j8 -N1 "$FW" | tr -d ' ')
  if [ "$b8" = "4b" ]; then echo "  [v] bootable image (byte 8 = 0x4b)";
  else echo "  [x] byte 8 = 0x$b8 (expected 0x4b) — this is not a bootable Telink image"; ok=0; fi
  # if ota-packer is present, check finalization/CRC (inline, WITHOUT mutating the file)
  local pk="$HERE/../ota-packer"
  if [ -f "$pk/telink_image.py" ]; then
    FW="$FW" PK="$pk" python3 - <<'PY' || true
import os, sys
sys.path.insert(0, os.environ["PK"])
import telink_image as ti
data = open(os.environ["FW"], "rb").read()
if len(data) > 0x10008 and data[0x10008] == 0x4B:      # full image: check the application
    data = data[0x10000:]
    print("  [i] full image: checking the application from 0x10000")
if ti.is_finalized(data):
    print("  [v] image finalized, CRC ok=" + str(ti.verify(data)))
else:
    print("  [!] image NOT finalized (no size@0x18/CRC). It still boots over SWS")
    print("      (the ROM only checks byte 8), but a finalized open-eglo-ota.bin is safer.")
PY
  fi
  # the image must not reach the factory data (calibration 0xFE000, MAC 0xFF000)
  local sz; sz=$(wc -c < "$FW")
  if [ "$sz" -le $((0xE0000)) ]; then echo "  [v] size $sz B — does not reach NV or factory data";
  else echo "  [x] image is $sz B and would overwrite NV / factory data (>= 0xE0000)"; ok=0; fi
  if [ "$(od -An -tx1 -j$((0x10008)) -N1 "$FW" 2>/dev/null | tr -d ' ')" = "4b" ]; then
    echo "  [v] full image: bootloader @0x0 + application @0x10000 (Zigbee OTA will install later versions)"
  else
    echo "  [!] image without bootloader — the firmware will run, but Zigbee OTA updates will not install."
    echo "      For the first flash use firmware/build/open-eglo-full.bin."
  fi
  [ "$ok" = 1 ] || { echo ">> preflight FAILED — fix the above before flashing." >&2; return 1; }
  echo ">> preflight OK."
}

echo ">> preflight ($FW)"
preflight
if [ "$CHECK_ONLY" = 1 ]; then exit 0; fi
[ -n "$PORT" ] || { echo "pass -p PORT to flash (or -c for preflight only)"; usage; }

if [ ! -d "$PROG_DIR" ]; then
  echo ">> cloning TlsrComProg825x into $PROG_DIR"
  git clone https://github.com/pvvx/TlsrComProg825x "$PROG_DIR"
fi
PROG="python3 $PROG_DIR/TlsrComProg.py -p $PORT -t $TACT"

STAMP=$(date +%Y%m%d-%H%M%S)
BACKUP="backup-stock-$STAMP.bin"

echo ">> [1/3] Full stock dump into $BACKUP (do NOT skip — RF calibration!)"
$PROG rf 0 $FLASH_SIZE "$BACKUP"
BSIZE=$(wc -c < "$BACKUP")
echo "   dumped $BSIZE B"
[ "$BSIZE" -ge $((FLASH_SIZE)) ] || { echo "dump is $BSIZE B, expected $((FLASH_SIZE)) — abort and check wiring" >&2; exit 1; }

echo ">> [2/3] Writing $FW from address 0 (we = with erase of image sectors)"
$PROG we 0 "$FW"

echo ">> [3/3] Verification: read back and compare the first $(wc -c < "$FW") B"
VERIFY="verify-$STAMP.bin"
FWLEN=$(wc -c < "$FW")
$PROG rf 0 "$FWLEN" "$VERIFY"
if cmp -s "$FW" "$VERIFY"; then
  echo "   OK — write matches."
else
  echo "   MISMATCH! Restore stock:  $PROG we 0 $BACKUP" >&2
  exit 1
fi
echo ">> Done. Stock backup: $BACKUP (keep it — restore: $PROG we 0 $BACKUP)"
