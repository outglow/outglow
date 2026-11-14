#!/usr/bin/env python3
"""Builds the Zigbee2MQTT OTA index of Outglow (and the release list of the
web flasher) from built images.

  python3 ota/make-index.py --version 0.2.4 --tag v0.2.4 target/tc32*/bulb-tc32.ota \
      [--index ota/index.json] [--site site/public/firmware.json] [--assets dist-assets/]

For each Zigbee OTA file (`bulb-tc32.ota`, crates/bulb-tc32/build.sh) it reads
the header (manufacturer code, image type, file version, header string), finds
the board from the image's `open-eglo-board:` tag, and writes an entry of the
format of github.com/Koenkk/zigbee-OTA (index.json), with the URL where the
release puts the file:

  https://github.com/outglow/outglow/releases/download/<tag>/outglow-<board>-<version>.ota

The Bluetooth image next to it (`bulb-tc32-ota.bin`) goes into the flasher's
release list (`--site`), under the URL the site serves it from:
firmware/<tag>/outglow-<board>-<version>-ota.bin (the site's build copies the
release assets there: the browser cannot download GitHub release assets
directly, they carry no CORS headers).

--assets DIR copies both files under those release names into DIR, ready to
upload as release assets. Entries for other versions already in the index
are kept (an image of the same board and file version is replaced), so the
index can list the previous release for downgrades.
"""
import argparse
import datetime
import hashlib
import json
import os
import re
import shutil
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "ota-packer"))
import zigbee_ota  # noqa: E402

REPO = "https://github.com/outglow/outglow"
BOARD_TAG = re.compile(rb"open-eglo-board:([A-Za-z0-9_]+-\d\.\d+)")


def board_of(image):
    m = BOARD_TAG.search(image)
    if not m:
        raise SystemExit("the image has no open-eglo-board: tag (is it an Outglow image?)")
    return m.group(1).decode()


def kind_of(image):
    """The Zigbee model id the image reports (open-eglo-dimm, -tw, ...)."""
    m = re.search(rb"open-eglo-(?:rgbtw|rgbw|dimm|tw|ebf)", image)
    return m.group(0).decode() if m else None


def load(path, default):
    if path and os.path.exists(path):
        with open(path) as f:
            return json.load(f)
    return default


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("ota", nargs="+", help="bulb-tc32.ota files (one per board)")
    ap.add_argument("--version", required=True, help="release version, e.g. 0.2.4")
    ap.add_argument("--tag", help="release tag (default v<version>)")
    ap.add_argument("--index", default=os.path.join(HERE, "index.json"))
    ap.add_argument("--site", help="the web flasher's release list to update (site/public/firmware.json)")
    ap.add_argument("--assets", help="directory to copy the release assets into")
    ap.add_argument("--notes", default="", help="release notes for Zigbee2MQTT and the flasher")
    ap.add_argument("--base-url", default=REPO + "/releases/download", help="where release assets are downloaded from")
    a = ap.parse_args()
    tag = a.tag or f"v{a.version}"

    index = load(a.index, [])
    site = load(a.site, {"schema": 1, "images": []}) if a.site else None
    if a.assets:
        os.makedirs(a.assets, exist_ok=True)

    for path in a.ota:
        data = open(path, "rb").read()
        f = zigbee_ota.parse(data)
        h = f.header
        image = next((s.data for s in f.sub_elements if s.tag_id == zigbee_ota.TAG_UPGRADE_IMAGE), None)
        if image is None:
            raise SystemExit(f"{path}: no upgrade image sub-element")
        board = board_of(image)
        name = f"outglow-{board}-{a.version}"
        entry = {
            "fileName": f"{name}.ota",
            "fileVersion": h.file_version,
            "fileSize": len(data),
            "url": f"{a.base_url}/{tag}/{name}.ota",
            "imageType": h.image_type,
            "manufacturerCode": h.manufacturer_code,
            "sha512": hashlib.sha512(data).hexdigest(),
            "otaHeaderString": h.header_string,
        }
        kind = kind_of(image)
        if kind:
            entry["modelId"] = kind
        if a.notes:
            entry["releaseNotes"] = a.notes
        index = [e for e in index if not (e["imageType"] == h.image_type and e["manufacturerCode"] == h.manufacturer_code
                                          and e["fileVersion"] == h.file_version)]
        index.append(entry)
        print(f"{path}: {board}, image type {h.image_type:#06x}, file version {h.file_version:#010x}, {len(data)} B")

        ble = os.path.join(os.path.dirname(path), "bulb-tc32-ota.bin")
        if a.assets:
            shutil.copyfile(path, os.path.join(a.assets, f"{name}.ota"))
            if os.path.exists(ble):
                shutil.copyfile(ble, os.path.join(a.assets, f"{name}-ota.bin"))
        if site is not None:
            if not os.path.exists(ble):
                raise SystemExit(f"{ble}: no Bluetooth image next to {path}")
            b = open(ble, "rb").read()
            if board_of(b) != board:
                raise SystemExit(f"{ble} is for {board_of(b)}, {path} for {board}")
            site["images"] = [r for r in site["images"] if not (r["board"] == board and r["version"] == a.version)]
            site["images"].append({
                "board": board,
                "version": a.version,
                "fileVersion": f"{h.file_version:#010x}",
                "url": f"firmware/{tag}/{name}-ota.bin",
                "size": len(b),
                "sha256": hashlib.sha256(b).hexdigest(),
                "date": datetime.date.today().isoformat(),
                **({"notes": a.notes} if a.notes else {}),
            })

    index.sort(key=lambda e: (e["imageType"], e["fileVersion"]))
    with open(a.index, "w") as f:
        json.dump(index, f, indent=2)
        f.write("\n")
    print(f"{a.index}: {len(index)} images")
    if site is not None:
        site["images"].sort(key=lambda r: (r["board"], int(r["fileVersion"], 16)))
        with open(a.site, "w") as f:
            json.dump(site, f, indent=2)
            f.write("\n")
        print(f"{a.site}: {len(site['images'])} images")


if __name__ == "__main__":
    main()
