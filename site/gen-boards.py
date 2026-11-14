#!/usr/bin/env python3
"""Writes site/public/boards.json from crates/bulb-tc32/src/boards.rs: the boards the
site lists and the flasher checks images and bulbs against.

  python3 site/gen-boards.py
"""
import json
import os
import re

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.join(HERE, "..", "crates", "bulb-tc32", "src", "boards.rs")

FIELDS = {
    "name": r'name: "([^"]+)"',
    "model": r'model: "([^"]+)"',
    "ble_product": r"ble_product: (0x[0-9a-fA-F]+)",
    "hw_revision": r'hw_revision: "([^"]+)"',
    "kind": r'kind: "([^"]+)"',
    "image_type": r"image_type: (0x[0-9a-fA-F]+)",
}


def main():
    text = open(SRC).read()
    boards = []
    for block in re.findall(r"Board \{(.*?)\n    \}", text, re.S):
        b = {}
        for key, rx in FIELDS.items():
            m = re.search(rx, block)
            if not m:
                raise SystemExit(f"{key} missing in a board of {SRC}")
            v = m.group(1)
            b[key] = int(v, 16) if v.startswith("0x") else v
        boards.append(b)
    out = os.path.join(HERE, "public", "boards.json")
    with open(out, "w") as f:
        json.dump(boards, f, indent=1)
        f.write("\n")
    print(f"{out}: {len(boards)} boards")


if __name__ == "__main__":
    main()
