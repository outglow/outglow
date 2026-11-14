#!/usr/bin/env python3
"""Builds and parses a Zigbee OTA Upgrade Image file (.ota).

Format per the ZCL OTA Upgrade cluster (0x0019) specification, matching what the
Zigbee2MQTT / zigbee-herdsman coordinator expects when serving OTA.

Header (little-endian), without optional fields = 56 bytes:

    offset size  field
    0      4     otaUpgradeFileIdentifier   = 0x0BEEF11E
    4      2     otaHeaderVersion           = 0x0100
    6      2     otaHeaderLength            = 56 (when no optional fields)
    8      2     otaHeaderFieldControl      bit0 secCred, bit1 devSpecific, bit2 hwVer
    10     2     manufacturerCode
    12     2     imageType
    14     4     fileVersion
    18     2     zigbeeStackVersion         = 0x0002 (Zigbee PRO)
    20     32    otaHeaderString            (UTF-8, zero-padded)
    52     4     totalImageSize             (header + all subelements)

Optional fields (when the fieldControl bits are set) follow the base header and
grow otaHeaderLength:
    secCred bit0:      securityCredentialVersion  u8
    devSpecific bit1:  upgradeFileDestination     8B (EUI64)
    hwVer bit2:        minimumHardwareVersion u16, maximumHardwareVersion u16

After the header: subelements (Tag-Length-Value):
    2  tagID    (0x0000 = Upgrade Image, i.e. the actual firmware)
    4  length
    N  data

Sources: ZCL spec, OTA Upgrade chapter; Koenkk/zigbee-OTA; zigbee-herdsman
ota parser. Verified against the public .ota format.
"""
from __future__ import annotations

import argparse
import struct
from dataclasses import dataclass, field

OTA_FILE_IDENTIFIER = 0x0BEEF11E
OTA_HEADER_VERSION = 0x0100
ZIGBEE_STACK_PRO = 0x0002
TAG_UPGRADE_IMAGE = 0x0000
BASE_HEADER_LEN = 56

FC_SECURITY_CREDENTIAL = 0x01
FC_DEVICE_SPECIFIC = 0x02
FC_HARDWARE_VERSIONS = 0x04


@dataclass
class OtaHeader:
    manufacturer_code: int
    image_type: int
    file_version: int
    header_string: str = "open-eglo firmware"
    zigbee_stack_version: int = ZIGBEE_STACK_PRO
    # optional
    security_credential_version: int | None = None
    upgrade_file_destination: bytes | None = None  # 8B EUI64
    minimum_hardware_version: int | None = None
    maximum_hardware_version: int | None = None

    def field_control(self) -> int:
        fc = 0
        if self.security_credential_version is not None:
            fc |= FC_SECURITY_CREDENTIAL
        if self.upgrade_file_destination is not None:
            fc |= FC_DEVICE_SPECIFIC
        if self.minimum_hardware_version is not None or self.maximum_hardware_version is not None:
            fc |= FC_HARDWARE_VERSIONS
        return fc

    def header_length(self) -> int:
        n = BASE_HEADER_LEN
        if self.security_credential_version is not None:
            n += 1
        if self.upgrade_file_destination is not None:
            n += 8
        if self.minimum_hardware_version is not None or self.maximum_hardware_version is not None:
            n += 4
        return n


@dataclass
class SubElement:
    tag_id: int
    data: bytes

    def pack(self) -> bytes:
        return struct.pack("<HI", self.tag_id, len(self.data)) + self.data


@dataclass
class OtaFile:
    header: OtaHeader
    sub_elements: list[SubElement] = field(default_factory=list)

    def pack(self) -> bytes:
        h = self.header
        header_len = h.header_length()
        body = b"".join(se.pack() for se in self.sub_elements)
        total = header_len + len(body)

        hs = h.header_string.encode("utf-8")[:32].ljust(32, b"\x00")
        out = struct.pack(
            "<IHHHHHIH32sI",
            OTA_FILE_IDENTIFIER,
            OTA_HEADER_VERSION,
            header_len,
            h.field_control(),
            h.manufacturer_code,
            h.image_type,
            h.file_version,
            h.zigbee_stack_version,
            hs,
            total,
        )
        assert len(out) == BASE_HEADER_LEN, len(out)
        if h.security_credential_version is not None:
            out += struct.pack("<B", h.security_credential_version)
        if h.upgrade_file_destination is not None:
            dest = h.upgrade_file_destination
            if len(dest) != 8:
                raise ValueError("upgrade_file_destination must be 8 bytes (EUI64)")
            out += dest
        if h.minimum_hardware_version is not None or h.maximum_hardware_version is not None:
            out += struct.pack(
                "<HH",
                h.minimum_hardware_version or 0,
                h.maximum_hardware_version or 0,
            )
        assert len(out) == header_len, (len(out), header_len)
        return out + body


def parse(data: bytes) -> OtaFile:
    """Parses .ota back into a structure (for round-trip tests and validation)."""
    (identifier, hver, hlen, fc, mfg, itype, fver, zstack, hs, total) = struct.unpack(
        "<IHHHHHIH32sI", data[:BASE_HEADER_LEN]
    )
    if identifier != OTA_FILE_IDENTIFIER:
        raise ValueError(f"bad OTA identifier: 0x{identifier:08X}")
    off = BASE_HEADER_LEN
    sec = dest = minhw = maxhw = None
    if fc & FC_SECURITY_CREDENTIAL:
        (sec,) = struct.unpack("<B", data[off:off + 1]); off += 1
    if fc & FC_DEVICE_SPECIFIC:
        dest = data[off:off + 8]; off += 8
    if fc & FC_HARDWARE_VERSIONS:
        minhw, maxhw = struct.unpack("<HH", data[off:off + 4]); off += 4
    if off != hlen:
        raise ValueError(f"otaHeaderLength ({hlen}) != actual ({off})")
    if total != len(data):
        raise ValueError(f"totalImageSize ({total}) != file size ({len(data)})")

    header = OtaHeader(
        manufacturer_code=mfg, image_type=itype, file_version=fver,
        header_string=hs.rstrip(b"\x00").decode("utf-8", "replace"),
        zigbee_stack_version=zstack,
        security_credential_version=sec, upgrade_file_destination=dest,
        minimum_hardware_version=minhw, maximum_hardware_version=maxhw,
    )
    subs = []
    while off < len(data):
        tag, length = struct.unpack("<HI", data[off:off + 6]); off += 6
        subs.append(SubElement(tag, data[off:off + length])); off += length
    return OtaFile(header, subs)


def build_ota(firmware: bytes, *, manufacturer_code: int, image_type: int,
              file_version: int, header_string: str = "open-eglo firmware",
              min_hw: int | None = None, max_hw: int | None = None) -> bytes:
    """Build .ota bytes from a raw firmware image (single subelement 0x0000)."""
    header = OtaHeader(
        manufacturer_code=manufacturer_code, image_type=image_type,
        file_version=file_version, header_string=header_string,
        minimum_hardware_version=min_hw, maximum_hardware_version=max_hw,
    )
    ota = OtaFile(header, [SubElement(TAG_UPGRADE_IMAGE, firmware)])
    return ota.pack()


def _main() -> int:
    ap = argparse.ArgumentParser(description="Pack a firmware .bin into a Zigbee .ota file")
    ap.add_argument("firmware", help="raw firmware image (.bin)")
    ap.add_argument("-o", "--output", required=True, help="output .ota file")
    ap.add_argument("--manufacturer-code", required=True, type=lambda x: int(x, 0),
                    help="manufacturer code (e.g. 0x1021 for AwoX)")
    ap.add_argument("--image-type", required=True, type=lambda x: int(x, 0))
    ap.add_argument("--file-version", required=True, type=lambda x: int(x, 0))
    ap.add_argument("--header-string", default="open-eglo firmware")
    ap.add_argument("--min-hw", type=lambda x: int(x, 0), default=None)
    ap.add_argument("--max-hw", type=lambda x: int(x, 0), default=None)
    args = ap.parse_args()

    with open(args.firmware, "rb") as f:
        fw = f.read()
    blob = build_ota(
        fw, manufacturer_code=args.manufacturer_code, image_type=args.image_type,
        file_version=args.file_version, header_string=args.header_string,
        min_hw=args.min_hw, max_hw=args.max_hw,
    )
    with open(args.output, "wb") as f:
        f.write(blob)
    parsed = parse(blob)  # self-check
    print(f"OK: {args.output} ({len(blob)} B), firmware {len(fw)} B, "
          f"mfg=0x{parsed.header.manufacturer_code:04X} "
          f"imageType=0x{parsed.header.image_type:04X} "
          f"fileVersion=0x{parsed.header.file_version:08X}")
    return 0


if __name__ == "__main__":
    raise SystemExit(_main())
