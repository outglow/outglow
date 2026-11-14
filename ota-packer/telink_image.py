#!/usr/bin/env python3
"""Finalize a Telink TLSR825x firmware image — bootloader header + CRC.

Reconstructed from `telink_zigbee_sdk` (ota.c / mcu_boot.h) and from the pvvx
finalizer `tl_check_fw.py` (this variant really boots on devices). Verified
against an image built with our toolchain (KNLT @0x08, size @0x18).

Image layout (little-endian):
    0x00  4  startup/branch TC32
    0x06  2  marker "firmware has CRC" = 0x025D (on disk: 5D 02)
    0x08  4  boot flag "KNLT" = 0x544C4E4B (byte 0x08 = 0x4B = active bank)
    0x18  4  firmware size in bytes (including the 4-byte CRC at the end)
    ...      firmware body
    len-4 4  image CRC-32: binascii.crc32(image_without_crc) XOR 0xFFFFFFFF, LE

The bootloader (ota_newImageValid) reads the size from 0x18 and compares the CRC with
the 4 bytes at the end. Dual-bank: the new image goes to the second bank (0x40000 for 512 KB),
after CRC validation the bootloader writes 0x4B to new_bank+0x08 and 0x00 to old_bank+0x08.
"""
from __future__ import annotations

import binascii
import struct

BOOT_MARK_OFF = 0x08          # byte 0x4B = "KNLT" LE, active bank
BOOT_MARK_VALUE = 0x4B
FW_SIZE_OFF = 0x18            # u32 LE: size with CRC
CRC_MARKER_OFF = 0x06        # u16: 0x025D when the image already has a CRC
CRC_MARKER = b"\x5d\x02"
TL_START_UP_FLAG = b"KNLT"   # 0x544C4E4B LE

# Boot banks by flash size (multi_boot_addr_e)
OTA_BANK_ADDR = {0x80000: 0x40000, 0x100000: 0x80000}  # 512KB -> 0x40000


def is_finalized(image: bytes) -> bool:
    return image[CRC_MARKER_OFF:CRC_MARKER_OFF + 2] == CRC_MARKER


def finalize(image: bytes) -> bytes:
    """Add the marker, fix the size @0x18 and append the CRC. Idempotent."""
    fw = bytearray(image)
    if is_finalized(fw):
        return bytes(fw)
    # pad to a multiple of 16
    pad = (16 - len(fw) % 16) % 16
    fw += b"\xFF" * pad
    # size = length + 4 (CRC)
    fw[FW_SIZE_OFF:FW_SIZE_OFF + 4] = struct.pack("<I", len(fw) + 4)
    # "has CRC" marker
    fw[CRC_MARKER_OFF:CRC_MARKER_OFF + 2] = CRC_MARKER
    # CRC
    crc = binascii.crc32(bytes(fw)) ^ 0xFFFFFFFF
    fw += struct.pack("<I", crc)
    return bytes(fw)


def verify(image: bytes) -> bool:
    """Check like the bootloader: marker, size @0x18 and CRC at the end."""
    if image[BOOT_MARK_OFF:BOOT_MARK_OFF + 4] != TL_START_UP_FLAG:
        return False
    if not is_finalized(image):
        return False
    size = struct.unpack("<I", image[FW_SIZE_OFF:FW_SIZE_OFF + 4])[0]
    if size != len(image):
        return False
    body, stored = image[:-4], struct.unpack("<I", image[-4:])[0]
    return (binascii.crc32(body) ^ 0xFFFFFFFF) == stored


def boot_bank(flash_size: int = 0x80000) -> int:
    return OTA_BANK_ADDR.get(flash_size, 0x40000)


if __name__ == "__main__":
    import sys
    if len(sys.argv) != 2:
        print("usage: telink_image.py firmware.bin  (finalizes in place)")
        raise SystemExit(2)
    with open(sys.argv[1], "rb") as f:
        data = f.read()
    if is_finalized(data):
        print("Image already finalized.")
    else:
        out = finalize(data)
        with open(sys.argv[1], "wb") as f:
            f.write(out)
        print(f"Finalized: {len(data)} -> {len(out)} B, CRC ok={verify(out)}")
