#!/usr/bin/env python3
"""BLE OTA onto a REAL AwoX/EGLO Connect.Z bulb (stock) from a laptop, via bleak.

The protocol (erase, blocks + CRC16, resend, end) lives in flasher/awox_ota.py —
the same module emulator/ble_ota_sim.py runs through the original firmware.
This file is only the GATT transport and the safeguards against flashing something
to the wrong place.

  python3 flasher/ble_ota.py scan [--seconds 10]
      BLE advertisements of AwoX devices (vendor 0x0160): address, mesh name, product, MAC.
      Connects to nothing, writes nothing.

  python3 flasher/ble_ota.py probe <address|MAC>
      Connects, prints the Telink Mesh characteristics, reads Device Information
      and OTA channel status. READS ONLY (no writes to the bulb).

  python3 flasher/ble_ota.py push <address|MAC> <image.bin> --confirm-write [--mtu 247]
      Sends the image over the ...1915 channel. It finds the board the image is
      for (PRODUCTS: ESMLFzm_w6_Dimm -> product 0x20A7, ESMLFzm_w6_TW -> 0x20A6, ...)
      and refuses if the bulb advertises a different product. By default it accepts
      ONLY the stock image of the given variant (canary: stock/stock-dimm.bin /
      stock/stock-tw.bin — nothing changes beyond proving that the AwoX bootloader
      installs what we send).
      --allow-nonstock and --product lift those locks deliberately.
      Boards (docs/stock-profiles.md): the same product in another hardware
      revision has its LEDs on other pins. The image's revision (the Rust
      firmware's tag `open-eglo-board:<model>-<rev>`, or the stock image's own
      "4.6x") must equal the bulb's Device Information hardware revision, read
      before anything is written; --any-revision lifts that lock deliberately.
      --mtu N (> 23) uses a larger ATT_MTU: the block size follows the negotiated
      MTU (MTU-7), capped at what the bulb's OTA handler takes — told apart by the
      length of the idle status read: stock (9 B) up to 240 B, our C/Rust BLE
      images (20 B) up to 57 B, anything else the classic 16 B. Without --mtu the
      transfer is the classic one (16 B blocks), as verified first in the emulator.
      When the stock reports a CRC or sequence error, the blocks are sent again from
      the one after its last written block (--max-resends in a row without progress).

Findings from the emulator (stock actually executed, emulator/ble_ota_sim.py):
  * ...1915 has props 0x06 (Read + Write Without Response), ...1914 0x0A (Read+Write).
  * OTA does NOT require a mesh login (--no-login in ble_ota_sim.py): the erase and the
    whole transfer go through without a packet on 1914. We don't need the AwoX app password.
    --login stays as a fallback option (secure = SHA-256 from MAC, or a custom one).
  * Advertisement: manufacturer data 0x0160 = product LE16 || 4 low bytes of MAC (LE);
    the two high ones (a4:c1, Telink prefix) are not advertised — hence the MAC on macOS,
    where CoreBluetooth hides addresses. (Verified on the stock ADV_IND in the emulator.)
"""
import argparse
import asyncio
import hashlib
import os
import re
import struct
import sys
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, "..", "ota-packer"))
import awox_ota  # noqa: E402

AWOX_VENDOR = 0x0160
PRODUCT_DIMM = 0x20A7
PRODUCT_TW = 0x20A6
# The boards of the Rust firmware (crates/bulb-tc32/boards.tsv) and of the stock
# images of the app: modelId -> BLE product. Longest names first: EGLO_ZM_TW is
# the start of EGLO_ZM_TW_CLP.
PRODUCTS = {b"EGLO_ZM_Dimmable": 0x2098, b"EGLO_ZM_TW_CLP": 0x20A4, b"ESMLzm_TW_GU10": 0x203E,
            b"ESMLFzm_w6_Dimm": PRODUCT_DIMM, b"ESMLFzm_w6_TW": PRODUCT_TW, b"EGLO_ZM_TW": 0x2097}
BOARD_TAG = re.compile(rb"open-eglo-board:([A-Za-z0-9_]+)-(\d\.\d+)")
TELINK_PREFIX = bytes.fromhex("a4c1")  # the advertisement carries only the 4 low MAC bytes
STOCK_DIR = os.path.join(HERE, "..", "stock")
DEVINFO = {"2a24": "model", "2a26": "firmware", "2a27": "hardware", "2a29": "manufacturer"}


def parse_awox_adv(adv):
    """-> (product, mac6) or None, from manufacturer data 0x0160."""
    md = adv.manufacturer_data.get(AWOX_VENDOR)
    if not md or len(md) < 6:
        return None
    product = struct.unpack_from("<H", md, 0)[0]
    mac6 = TELINK_PREFIX + bytes(reversed(md[2:6]))
    return product, mac6


def fmt_mac(mac6):
    return ":".join(f"{b:02x}" for b in mac6)


async def scan(seconds):
    from bleak import BleakScanner
    seen = {}

    def cb(d, a):
        seen[d.address] = (d, a)

    s = BleakScanner(cb)
    await s.start()
    await asyncio.sleep(seconds)
    await s.stop()
    rows = []
    for addr, (d, a) in seen.items():
        p = parse_awox_adv(a)
        if p:
            rows.append((a.rssi, addr, a.local_name, p[0], fmt_mac(p[1])))
    rows.sort(reverse=True)
    print(f"AwoX devices (vendor {AWOX_VENDOR:#06x}): {len(rows)} of {len(seen)} visible")
    for rssi, addr, name, product, mac in rows:
        tag = {PRODUCT_DIMM: "ESMLFzm_w6_Dimm", PRODUCT_TW: "ESMLFzm_w6_TW"}.get(product, "other product")
        print(f"  {addr}  rssi {rssi:>4}  mesh '{name}'  product {product:#06x} ({tag})  MAC {mac}")
    return rows


async def find_device(target, seconds):
    """target = bleak address (UUID on macOS / MAC on Linux) or MAC from the advertisement."""
    from bleak import BleakScanner
    t = target.lower()
    found = {}

    def cb(d, a):
        p = parse_awox_adv(a)
        if d.address.lower() == t or (p and fmt_mac(p[1]) == t):
            found["dev"] = (d, a, p)

    s = BleakScanner(cb)
    await s.start()
    end = time.time() + seconds
    while time.time() < end and "dev" not in found:
        await asyncio.sleep(0.2)
    await s.stop()
    if "dev" not in found:
        raise SystemExit(f"device {target} not seen in BLE advertisements ({seconds} s)")
    return found["dev"]


class BleakTransport:
    """Synchronous transport for awox_ota (write/read/sleep) over asynchronous bleak.

    awox_ota.run_ota runs in a worker thread; every write/read goes to the asyncio
    loop via run_coroutine_threadsafe. We write ...1915 without a response
    (props 0x06), the rest with a response."""

    def __init__(self, client, loop, chars, delay_ms=0.0, log=print):
        self.client, self.loop, self.chars, self.log = client, loop, chars, log
        self.delay = delay_ms / 1000.0
        self.writes = self.reads = 0

    def _run(self, coro):
        return asyncio.run_coroutine_threadsafe(coro, self.loop).result(timeout=30)

    def write(self, char, data):
        ch = self.chars[char]
        no_rsp = char == awox_ota.OTA and "write-without-response" in ch.properties
        self._run(self.client.write_gatt_char(ch, bytes(data), response=not no_rsp))
        self.writes += 1
        if self.delay:
            time.sleep(self.delay)

    def read(self, char):
        self.reads += 1
        return bytes(self._run(self.client.read_gatt_char(self.chars[char])))

    def sleep(self, ms):
        time.sleep(ms / 1000.0)


def telink_chars(client):
    """{short code 0x11..0x15: BleakGATTCharacteristic} by Telink UUID."""
    out = {}
    for svc in client.services:
        for ch in svc.characteristics:
            u = ch.uuid.lower()
            for code in (awox_ota.STATUS, awox_ota.COMMAND, awox_ota.OTA_OLD, awox_ota.PAIR, awox_ota.OTA):
                if u == awox_ota.uuid(code):
                    out[code] = ch
    return out


async def connect(target, seconds, log=print):
    from bleak import BleakClient
    d, a, p = await find_device(target, seconds)
    product, mac6 = p if p else (None, None)
    log(f"device: {d.address} mesh '{a.local_name}' product {product:#06x} MAC {fmt_mac(mac6)}"
        if p else f"device: {d.address} (no AwoX data in the advertisement)")
    client = BleakClient(d, timeout=20.0)
    await client.connect()
    log(f"connected, MTU {client.mtu_size}")
    chars = telink_chars(client)
    for code, ch in sorted(chars.items()):
        log(f"  ...19{code:02x} handle {ch.handle:#06x} props {','.join(ch.properties)}")
    return client, chars, product, mac6


async def probe(a):
    client, chars, product, mac6 = await connect(a.target, a.seconds)
    try:
        for svc in client.services:
            for ch in svc.characteristics:
                short = ch.uuid[4:8]
                if short in DEVINFO and "read" in ch.properties:
                    v = await client.read_gatt_char(ch)
                    print(f"  {DEVINFO[short]:10s} {v.decode(errors='replace')!r}")
        missing = [f"...19{c:02x}" for c in (awox_ota.PAIR, awox_ota.OTA) if c not in chars]
        if missing:
            print(f"WARNING: missing characteristics {missing} — this does not look like stock Telink Mesh")
        else:
            st = await client.read_gatt_char(chars[awox_ota.OTA])
            target = awox_ota.detect_target(st)
            print(f"  OTA status (...1915): {st.hex()}  (0000.. = idle; {len(st)} B = {target} layout)")
            print(f"  ATT_MTU {client.mtu_size}: push --mtu {client.mtu_size} would send "
                  f"{awox_ota.pick_chunk(client.mtu_size, target)[0]} B blocks")
        print("probe: reads only, nothing written")
    finally:
        await client.disconnect()


def check_image(path, allow_nonstock):
    import telink_image
    img = open(path, "rb").read()
    if img[8:12] != b"KNLT":
        raise SystemExit("image has no KNLT marker @0x08 — it is not finalized (make ota)")
    size = struct.unpack_from("<I", img, 0x18)[0]
    if size != len(img):
        raise SystemExit(f"size in header {size:#x} != file length {len(img):#x}")
    if not telink_image.verify(img):
        raise SystemExit("image CRC mismatch — the bootloader would reject it, not sending")
    if len(img) > 0x64000:
        raise SystemExit(f"image {len(img)} B does not fit in the stock OTA buffer (0x6000..0x69fff)")
    model, revision = image_board(img)
    product = PRODUCTS[model]
    # the filament bulbs' stock images may be in stock/
    stock_name = {b"ESMLFzm_w6_Dimm": "stock-dimm.bin", b"ESMLFzm_w6_TW": "stock-tw.bin"}.get(model)
    variant = model.decode()
    stock_path = os.path.join(STOCK_DIR, stock_name) if stock_name else None
    is_stock = bool(stock_path) and os.path.exists(stock_path) and hashlib.sha256(img).digest() == hashlib.sha256(
        open(stock_path, "rb").read()).digest()
    if not is_stock and not allow_nonstock:
        canary = f"stock/{stock_name}" if stock_name else "the board's stock image (stock/apk/)"
        raise SystemExit(f"this is NOT the {variant} stock image. Canary = {canary} first; "
                         "our image requires --allow-nonstock (after flashing, BLE is gone, return only via Zigbee OTA)")
    print(f"image: {path} {len(img)} B, {model.decode()} hardware revision {revision} (product {product:#06x}), "
          f"KNLT+CRC ok, {'STOCK (canary)' if is_stock else 'NON-stock'}")
    return img, product, revision


def image_board(img):
    """-> (modelId, hardware revision) the image is for: the Rust firmware's
    board tag, or a stock image's model name and its single revision string."""
    tag = BOARD_TAG.search(img)
    if tag:
        if tag.group(1) not in PRODUCTS:
            raise SystemExit(f"board {tag.group(1).decode()} is not known to this flasher")
        return tag.group(1), tag.group(2).decode()
    model = next((m for m in PRODUCTS if m + b"\x00" in img), None)
    revs = sorted({m.decode() for m in re.findall(rb"(?<![\d.])(4\.\d\d)(?![\d.])", img)})
    if model is None or len(revs) != 1:
        raise SystemExit(f"cannot recognize the board of the image: model {model}, revisions {revs}")
    return model, revs[0]


async def read_revision(client):
    """The bulb's Device Information hardware revision (0x2A27), or None."""
    for svc in client.services:
        for ch in svc.characteristics:
            if ch.uuid[4:8] == "2a27" and "read" in ch.properties:
                return bytes(await client.read_gatt_char(ch)).decode(errors="replace").strip("\x00 ")
    return None


async def negotiate(client, chars, a, log=print):
    """-> (ATT_MTU, block size or None for the classic 16 B). With --mtu above 23 (or
    --chunk): reads the idle OTA status (which layout = which handler), takes the MTU the
    OS negotiated (capped by --mtu and by what a Write Without Response may carry) and
    the block size that both allow."""
    if a.mtu <= awox_ota.DEFAULT_MTU and a.chunk is None:
        log(f"classic transfer: ATT_MTU {awox_ota.DEFAULT_MTU}, 16 B blocks")
        return awox_ota.DEFAULT_MTU, None
    ch = chars[awox_ota.OTA]
    idle = bytes(await client.read_gatt_char(ch))
    target = awox_ota.detect_target(idle)
    log(f"OTA status before the transfer: {idle.hex()} ({len(idle)} B -> {target} handler, "
        f"block limit {awox_ota.CHUNK_CAP[target]} B)")
    if idle and idle[0] not in (0, 4):
        log(f"WARNING: OTA state {idle[0]} before the transfer (0 = idle)")
    backend = getattr(client, "_backend", None)
    if hasattr(backend, "_acquire_mtu"):        # BlueZ: the MTU is known only after this
        try:
            await backend._acquire_mtu()
        except Exception as e:  # noqa: BLE001
            log(f"MTU exchange through BlueZ failed ({e}); staying at what bleak reports")
    mtu = min(a.mtu, client.mtu_size)
    room = getattr(ch, "max_write_without_response_size", None)
    if room and room + 3 < mtu:
        log(f"the OS lets a Write Without Response carry {room} B: ATT_MTU taken as {room + 3}")
        mtu = room + 3
    try:
        chunk, why = awox_ota.pick_chunk(mtu, target, a.chunk)
    except awox_ota.OtaError as e:
        raise SystemExit(str(e))
    log(f"ATT_MTU {client.mtu_size} negotiated, {mtu} used; block {chunk} B ({why})")
    return mtu, chunk


async def push(a):
    img, img_product, img_revision = check_image(a.image, a.allow_nonstock)
    expected = a.product if a.product is not None else img_product
    client, chars, product, mac6 = await connect(a.target, a.seconds)
    try:
        if product != expected:
            raise SystemExit(f"device product {product:#06x} != {expected:#06x} required by the image "
                             f"(dimm=0x20a7, tw=0x20a6). --product only deliberately.")
        revision = await read_revision(client)
        print(f"bulb hardware revision {revision!r}, image for {img_revision!r}")
        if revision != img_revision and not a.any_revision:
            raise SystemExit(f"the bulb is hardware revision {revision!r}, the image is for {img_revision!r}: "
                             "its LEDs may be on other pins. --any-revision only deliberately.")
        for c in (awox_ota.OTA,):
            if c not in chars:
                raise SystemExit("no OTA characteristic ...1915")
        if not a.confirm_write:
            raise SystemExit("refused: no --confirm-write (this writes the bulb flash and restarts it)")
        mtu, chunk = await negotiate(client, chars, a)
        loop = asyncio.get_running_loop()
        tr = BleakTransport(client, loop, chars, delay_ms=a.delay_ms)
        if a.login:
            pw = bytes.fromhex(a.mesh_password) if a.mesh_password else awox_ota.secure_password(mac6)
            await loop.run_in_executor(None, lambda: awox_ota.login(tr, a.mesh_name, pw, os.urandom(8)))
        t0 = time.time()
        st = await loop.run_in_executor(None, lambda: awox_ota.run_ota(tr, img, mtu=mtu, chunk=chunk,
                                                                       max_resends=a.max_resends))
        print(f"final status {st.hex()}; {tr.writes} writes, {tr.reads} reads, {time.time() - t0:.0f} s")
        print("the bulb should now reset itself (the AwoX bootloader installs the image).")
    finally:
        try:
            await client.disconnect()
        except Exception:
            pass


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("scan")
    s.add_argument("--seconds", type=float, default=10.0)
    p = sub.add_parser("probe")
    p.add_argument("target")
    p.add_argument("--seconds", type=float, default=15.0)
    u = sub.add_parser("push")
    u.add_argument("target")
    u.add_argument("image")
    u.add_argument("--confirm-write", action="store_true", help="without this nothing is written")
    u.add_argument("--allow-nonstock", action="store_true", help="allow sending an image other than stock")
    u.add_argument("--any-revision", action="store_true",
                   help="send although the bulb's hardware revision differs from the image's")
    u.add_argument("--product", type=lambda x: int(x, 0), default=None,
                   help="expected product in the advertisement; default from the image variant (dimm 0x20a7, tw 0x20a6)")
    u.add_argument("--mtu", type=int, default=awox_ota.DEFAULT_MTU,
                   help="requested ATT_MTU; 23 (default) = classic 16 B blocks. Larger: the block follows the "
                        "negotiated MTU, capped at what the bulb's handler takes (stock 240, ours 57)")
    u.add_argument("--chunk", type=int, default=None,
                   help="block size in bytes (16..MTU-7, within the handler's limit); default from the MTU")
    u.add_argument("--max-resends", type=int, default=10,
                   help="resends after a CRC/sequence error reported by the stock, in a row without progress")
    u.add_argument("--delay-ms", type=float, default=0.0, help="pause after each write")
    u.add_argument("--login", action="store_true", help="mesh login before OTA (unnecessary in the emulator)")
    u.add_argument("--mesh-name", default="unpaired")
    u.add_argument("--mesh-password", help="hex 16 B; default secure = SHA-256(MAC||secret)")
    u.add_argument("--seconds", type=float, default=15.0)
    a = ap.parse_args()
    if a.cmd == "scan":
        asyncio.run(scan(a.seconds))
    elif a.cmd == "probe":
        asyncio.run(probe(a))
    else:
        asyncio.run(push(a))


if __name__ == "__main__":
    main()
