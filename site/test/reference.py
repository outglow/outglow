#!/usr/bin/env python3
"""Reference values for test/awox-ota.test.ts, from flasher/awox_ota.py and
ota-packer/telink_image.py (the command-line flasher the browser one must
match). Prints JSON on stdout; writes nothing.

  python3 site/test/reference.py [image.bin]

Without an image a deterministic synthetic one is built (Outglow board tag,
finalized with the Telink header and CRC)."""
import base64
import json
import os
import random
import sys

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")
sys.path.insert(0, os.path.join(ROOT, "flasher"))
sys.path.insert(0, os.path.join(ROOT, "ota-packer"))
import telink_image  # noqa: E402

try:
    import awox_ota  # needs pycryptodome (mesh login)
except ImportError as e:
    raise SystemExit(f"reference.py: {e} (pip install pycryptodome)")


def synthetic():
    rnd = random.Random(1)
    body = bytearray(rnd.getrandbits(8) for _ in range(5000))
    body[8:12] = b"KNLT"
    tag = b"open-eglo-board:ESMLFzm_w6_TW-4.63\0"
    body[0x100:0x100 + len(tag)] = tag
    return telink_image.finalize(bytes(body))


def main():
    img = open(sys.argv[1], "rb").read() if len(sys.argv) > 1 else synthetic()
    n = awox_ota.packet_count(len(img))
    idx = sorted({0, 1, 2, n // 2, n - 2, n - 1})
    mac6 = bytes.fromhex("a4c138123456")
    rnd8 = bytes(range(1, 9))
    pw = awox_ota.secure_password(mac6)
    out = {
        "image": base64.b64encode(img).decode(),
        "verify": telink_image.verify(img),
        "count": n,
        "erase": awox_ota.erase_packet(len(img)).hex(),
        "data": {str(i): awox_ota.data_packet(img, i, 16).hex() for i in idx},
        "end": awox_ota.end_packet(len(img)).hex(),
        "crc16": awox_ota.crc16_modbus(b"123456789"),
        "secure_password": pw.hex(),
        "login": awox_ota.login_packet("unpaired", pw, rnd8).hex(),
        "login_1234": awox_ota.login_packet("telink_mesh1", "123", rnd8).hex(),
        "chunk40": {
            "erase": awox_ota.erase_packet(len(img), chunk=40).hex(),
            "data": {str(i): awox_ota.data_packet(img, i, 40).hex()
                     for i in (0, awox_ota.packet_count(len(img), chunk=40) - 1)},
            "end": awox_ota.end_packet(len(img), chunk=40).hex(),
        },
    }
    json.dump(out, sys.stdout)


if __name__ == "__main__":
    main()
