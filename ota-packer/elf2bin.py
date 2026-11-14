#!/usr/bin/env python3
"""The flash image of a linked TC32 firmware ELF.

    elf2bin.py <firmware.elf> <firmware.bin>

Every PT_LOAD segment with file bytes goes to its physical (load) address,
counted from the start of the image; what lies between segments is 0xFF, as
in erased flash. The code is TC32 already (clang -mcpu=tc32, ld.lld), so
nothing is re-encoded.

An ELF whose ARM build attributes do not name the tc32 core is refused: it
holds code in the ARM Thumb encoding, which the chip would run as garbage.

SPDX-License-Identifier: GPL-3.0-or-later
"""
import struct
import sys

PT_LOAD = 1
SHT_ARM_ATTRIBUTES = 0x70000003
TAG_FILE = 1
TAG_CPU_NAME = 5


class Refused(Exception):
    pass


def uleb(b, i):
    """(value, next index) of an unsigned LEB128 at b[i]."""
    v = shift = 0
    while True:
        byte = b[i]
        i += 1
        v |= (byte & 0x7F) << shift
        shift += 7
        if not byte & 0x80:
            return v, i


def cstr(b, i):
    """(text, next index) of a NUL-terminated string at b[i]."""
    end = b.index(0, i)
    return b[i:end].decode("ascii", "replace"), end + 1


def sections(elf):
    """(type, file offset, size) of every section."""
    shoff, = struct.unpack_from("<I", elf, 0x20)
    shentsize, shnum = struct.unpack_from("<HH", elf, 0x2E)
    for n in range(shnum):
        _name, typ, _flags, _addr, off, size = struct.unpack_from("<IIIIII", elf, shoff + n * shentsize)
        yield typ, off, size


def cpu_name(elf):
    """Tag_CPU_name of the file-wide "aeabi" build attributes, or None."""
    for typ, off, size in sections(elf):
        if typ != SHT_ARM_ATTRIBUTES:
            continue
        a = elf[off:off + size]
        if not a or a[0] != ord("A"):
            return None
        i = 1
        while i + 4 <= len(a):
            length, = struct.unpack_from("<I", a, i)
            if length < 5:
                break
            vendor, j = cstr(a, i + 4)
            end = i + length
            while vendor == "aeabi" and j < end:
                tag, k = uleb(a, j)
                sub_size, = struct.unpack_from("<I", a, k)
                sub_end, k = j + sub_size, k + 4
                while tag == TAG_FILE and k < sub_end:
                    t, k = uleb(a, k)
                    if t == TAG_CPU_NAME:
                        return cstr(a, k)[0]
                    if t == 32:                     # Tag_compatibility: a number, then a string
                        k = cstr(a, uleb(a, k)[1])[1]
                    elif t in (4, 5, 65, 67) or (t > 32 and t & 1):
                        k = cstr(a, k)[1]           # string-valued tags
                    else:
                        k = uleb(a, k)[1]           # number-valued tags
                j = sub_end
            i = end
        return None
    return None


def image_of(elf):
    """(image bytes, number of segments placed)."""
    if elf[:4] != b"\x7fELF" or elf[4] != 1 or elf[5] != 1:
        raise Refused("not a 32-bit little-endian ELF")
    cpu = cpu_name(elf)
    if (cpu or "").lower() != "tc32":
        raise Refused(f"the build attributes name the core {cpu!r}, not 'tc32' (build with -mcpu=tc32)")
    phoff, = struct.unpack_from("<I", elf, 0x1C)
    phentsize, phnum = struct.unpack_from("<HH", elf, 0x2A)
    image = bytearray()
    placed = 0
    for n in range(phnum):
        typ, off, _vaddr, paddr, filesz = struct.unpack_from("<IIIII", elf, phoff + n * phentsize)
        if typ != PT_LOAD or not filesz:
            continue
        end = paddr + filesz
        if len(image) < end:
            image += b"\xff" * (end - len(image))
        image[paddr:end] = elf[off:off + filesz]
        placed += 1
    if not placed:
        raise Refused("no loadable segment with bytes in the file")
    return bytes(image), placed


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    try:
        image, placed = image_of(open(sys.argv[1], "rb").read())
    except Refused as e:
        sys.exit(f"elf2bin: {sys.argv[1]}: {e}")
    with open(sys.argv[2], "wb") as f:
        f.write(image)
    print(f"{sys.argv[2]}: {len(image)} bytes from {placed} segment(s)")


if __name__ == "__main__":
    main()
