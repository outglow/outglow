#!/usr/bin/env python3
"""Zigbee OTA packer tests — no dependencies, run: python3 test_zigbee_ota.py"""
import struct
import sys

from zigbee_ota import (
    BASE_HEADER_LEN, OTA_FILE_IDENTIFIER, TAG_UPGRADE_IMAGE,
    build_ota, parse,
)

FW = bytes(range(256)) * 4  # 1024 B of sample firmware


def test_magic_and_size():
    blob = build_ota(FW, manufacturer_code=0x1021, image_type=0x0101, file_version=0x03020611)
    ident, = struct.unpack("<I", blob[:4])
    assert ident == OTA_FILE_IDENTIFIER, hex(ident)
    total, = struct.unpack("<I", blob[52:56])
    assert total == len(blob), (total, len(blob))
    # header without optional fields = 56
    hlen, = struct.unpack("<H", blob[6:8])
    assert hlen == BASE_HEADER_LEN, hlen
    # total = 56 + (2+4+len(fw))
    assert total == BASE_HEADER_LEN + 6 + len(FW)


def test_round_trip():
    blob = build_ota(FW, manufacturer_code=0x1021, image_type=0x0101, file_version=0x03020611,
                     header_string="open-eglo 1")
    ota = parse(blob)
    assert ota.header.manufacturer_code == 0x1021
    assert ota.header.image_type == 0x0101
    assert ota.header.file_version == 0x03020611
    assert ota.header.header_string == "open-eglo 1"
    assert len(ota.sub_elements) == 1
    assert ota.sub_elements[0].tag_id == TAG_UPGRADE_IMAGE
    assert ota.sub_elements[0].data == FW


def test_optional_hw_fields():
    blob = build_ota(FW, manufacturer_code=1, image_type=2, file_version=3,
                     min_hw=0x0001, max_hw=0x00FF)
    hlen, = struct.unpack("<H", blob[6:8])
    assert hlen == BASE_HEADER_LEN + 4, hlen  # +4 for min/max hw
    ota = parse(blob)
    assert ota.header.minimum_hardware_version == 0x0001
    assert ota.header.maximum_hardware_version == 0x00FF
    total, = struct.unpack("<I", blob[52:56])
    assert total == len(blob)


def test_header_string_truncated_to_32():
    blob = build_ota(FW, manufacturer_code=1, image_type=2, file_version=3,
                     header_string="x" * 100)
    ota = parse(blob)
    assert len(ota.header.header_string) == 32


def test_empty_firmware():
    blob = build_ota(b"", manufacturer_code=1, image_type=2, file_version=3)
    ota = parse(blob)
    assert ota.sub_elements[0].data == b""
    assert len(blob) == BASE_HEADER_LEN + 6


def _run():
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
    failed = 0
    for t in tests:
        try:
            t()
            print(f"  ok   {t.__name__}")
        except Exception as e:  # noqa: BLE001
            failed += 1
            print(f"  FAIL {t.__name__}: {e}")
    print(f"\n{len(tests) - failed}/{len(tests)} passed")
    return 1 if failed else 0


# --- Telink image finalizer tests ---
import telink_image as ti  # noqa: E402


def _fake_image(n=2048):
    img = bytearray(b"\x00" * n)
    img[0x08:0x0c] = b"KNLT"      # boot marker (as from the linker)
    return bytes(img)


def test_telink_finalize_verify():
    img = _fake_image()
    out = ti.finalize(img)
    assert ti.verify(out), "after finalize the image must pass CRC validation"
    # size @0x18 == file length
    size = int.from_bytes(out[0x18:0x1c], "little")
    assert size == len(out) == len(img) + 4  # 2048 divisible by 16 => no padding


def test_telink_finalize_idempotent():
    img = _fake_image()
    once = ti.finalize(img)
    twice = ti.finalize(once)
    assert once == twice, "finalize must be idempotent (marker 0x025D)"


def test_telink_padding_to_16():
    img = _fake_image(2000 + 7)  # not divisible by 16
    out = ti.finalize(img)
    assert (len(out) - 4) % 16 == 0, "the body must be aligned to 16 B"
    assert ti.verify(out)


def test_telink_crc_detects_corruption():
    out = bytearray(ti.finalize(_fake_image()))
    out[100] ^= 0xFF  # corrupt a byte
    assert not ti.verify(bytes(out))


def test_telink_bank_512k():
    assert ti.boot_bank(0x80000) == 0x40000


if __name__ == "__main__":
    sys.exit(_run())
