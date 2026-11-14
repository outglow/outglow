"""Firmware update protocol over BLE for AwoX/EGLO Connect.Z bulbs
(Telink Mesh + AwoX OTA channel), transport-independent.

Used by: flasher/ble_ota.py (real bulb via bleak) and
emulator/ble_ota_sim.py (original firmware in the emulator). Protocol source:
decompilation of the AwoX app (com.awox.home.control.ota.ZigBeeMeshOTAService)
and the GATT table of the original firmware (reference/awox-ble-ota.md).

Flow:
1. Telink Mesh login on ...1914: write 0x0c || rnd8 || AES(rnd, name^password)[:8],
   read ...1914 -> 0x0d || server rnd8 (0x0e = refused). The session key is not
   needed for OTA (OTA packets go in the clear).
2. Erase on ...1915: a (D+4)-byte packet, all zeros except [2] = len/4096 + 2.
   Then read ...1915 every 3 s until rsp[0] == 2.
3. Data on ...1915: idx_lo idx_hi || D bytes (the last one padded with 0xFF) ||
   CRC-16/MODBUS (LE) over everything before the CRC. Every 8 writes read ...1915.
4. End: idx_lo idx_hi || CRC (4 bytes), read ...1915: rsp[1] == 0 = success.

Block size D: classically MTU-7 (16 at the default ATT_MTU 23). The stock handler
takes D from the length of the erase packet (D = length - 4, at least 16) and then
accepts only data packets of exactly D+4 bytes, so the erase packet and the data
packets must agree (they do here: both follow `chunk`). Limits measured in the
emulator (docs/ble-first-flash.md): stock ATT_MTU 247, D up to 240 (its 8-slot
packet buffer holds up to 243); our C and Rust BLE images ATT_MTU 64, D up to 57.

Status read from ...1915 (two layouts, told apart by the length):
  stock (9 B): state, error, last written idx LE16 (0xFFFF = none), last received
     idx LE16, consecutive errors, erased sectors, 0. state 0 idle, 1 erasing,
     2 erased, 3 receiving, 4 done; error 0 ok, 1 CRC, 2 sequence, 3 idx beyond the
     buffer, 4 packet in the wrong state, 5 flash read-back differs. Errors 1 and 2
     are recoverable: the bulb keeps the blocks up to "last written" and takes the
     next one; after 3 consecutive errors it erases the staging and restarts.
  ours (20 B, firmware/ble/eglo_ble_ota.c, crates/ble/src/ota.rs): state, result
     (2 = failed, final), blocks received LE16 (twice), 0, percent, 0.

"unpaired" network password: for this model (product 0x20A7, firmware >= 3.0.0)
the app uses SHA-256(MAC6 || 56E850EDCFDD8B99 || 0000)[:16], for older ones "1234".
"""
import hashlib
import struct

from Crypto.Cipher import AES

TELINK_UUID = "00010203-0405-0607-0809-0a0b0c0d19{:02x}"
STATUS, COMMAND, OTA_OLD, PAIR, OTA = 0x11, 0x12, 0x13, 0x14, 0x15
AWOX_SECRET = bytes.fromhex("56E850EDCFDD8B99")

DEFAULT_MTU = 23
CHUNK_MIN = 16          # the stock takes an erase packet shorter than 20 B as no erase packet
# Largest block each target's OTA handler takes (see the module docstring). The negotiated
# ATT_MTU limits further (D <= MTU-7); an unknown target gets the classic 16.
CHUNK_CAP = {"stock": 240, "eglo": 57, "unknown": CHUNK_MIN}
STOCK_ERRORS = {0: "ok", 1: "CRC error", 2: "sequence error", 3: "index beyond the buffer",
                4: "packet in the wrong state", 5: "flash read-back differs"}


def uuid(short):
    return TELINK_UUID.format(short)


def crc16_modbus(data):
    crc = 0xFFFF
    for b in data:
        crc ^= b
        for _ in range(8):
            crc = (crc >> 1) ^ 0xA001 if crc & 1 else crc >> 1
    return crc


def telink_aes(key, data):
    """Telink aes_att_encryption: AES-128 on the reversed bytes of the key and data."""
    out = AES.new(bytes(reversed(key)), AES.MODE_ECB).encrypt(bytes(reversed(data)))
    return bytes(reversed(out))


def name_pass(name, password):
    n = name.encode() if isinstance(name, str) else name
    p = password.encode() if isinstance(password, str) else password
    return bytes(a ^ b for a, b in zip(n.ljust(16, b"\0"), p.ljust(16, b"\0")))


def login_packet(name, password, rnd8):
    enc = telink_aes(rnd8.ljust(16, b"\0"), name_pass(name, password))
    return b"\x0c" + rnd8 + enc[:8]


def session_key(name, password, rnd8, resp8):
    return telink_aes(name_pass(name, password), rnd8 + resp8)


def secure_password(mac6):
    """'unpaired' password in secure mode (product 0x20xx, fw >= 3.0.0); mac6 in
    display order (aa:bb:..:ff -> bytes aa..ff)."""
    return hashlib.sha256(mac6 + AWOX_SECRET + b"\0\0").digest()[:16]


def _block(mtu, chunk):
    return chunk if chunk else mtu - 7


def erase_packet(img_len, mtu=DEFAULT_MTU, chunk=None):
    b = bytearray(_block(mtu, chunk) + 4)
    b[2] = (img_len // 4096 + 2) & 0xFF
    return bytes(b)


def data_packet(img, i, d):
    p = struct.pack("<H", i) + img[i * d:(i + 1) * d].ljust(d, b"\xff")
    return p + struct.pack("<H", crc16_modbus(p))


def packet_count(img_len, mtu=DEFAULT_MTU, chunk=None):
    d = _block(mtu, chunk)
    return (img_len + d - 1) // d


def data_packets(img, mtu=DEFAULT_MTU, chunk=None):
    d = _block(mtu, chunk)
    for i in range(packet_count(len(img), mtu, chunk)):
        yield i, data_packet(img, i, d)


def end_packet(img_len, mtu=DEFAULT_MTU, chunk=None):
    p = struct.pack("<H", packet_count(img_len, mtu, chunk))
    return p + struct.pack("<H", crc16_modbus(p))


class OtaError(Exception):
    pass


class Status:
    """A status read of ...1915 in either layout (module docstring)."""

    def __init__(self, raw):
        self.raw = bytes(raw or b"")
        r = self.raw
        self.layout = "stock" if len(r) == 9 else "eglo" if len(r) >= 9 else "unknown"
        self.state = r[0] if r else None
        self.error = r[1] if len(r) > 1 else None
        self.last_ok = self.last_rx = self.errors = None
        if self.layout == "stock":
            self.last_ok, self.last_rx = struct.unpack_from("<HH", r, 2)
            self.errors = r[6]

    @property
    def resumable(self):
        """The stock rejected a block (CRC or sequence) and takes the next one after last_ok."""
        return self.layout == "stock" and self.state == 3 and self.error in (1, 2)

    @property
    def next_index(self):
        return (self.last_ok + 1) & 0xFFFF if self.last_ok is not None else None

    @property
    def failed(self):
        """Nothing more can come of this transfer: the bulb gave up or refuses."""
        if self.layout == "stock":
            return self.error in (3, 4, 5) or self.state == 0
        return self.error == 2

    def describe(self):
        if self.layout == "stock":
            last = "none" if self.last_ok == 0xFFFF else self.last_ok
            return (f"state {self.state}, {STOCK_ERRORS.get(self.error, f'error {self.error}')}, "
                    f"last written block {last}, last received {self.last_rx}, consecutive errors {self.errors}")
        rx = struct.unpack_from("<H", self.raw, 2)[0] if len(self.raw) >= 4 else None
        return f"state {self.state}, result {self.error}{' (failed)' if self.error == 2 else ''}, blocks received {rx}"


def detect_target(idle_status):
    """Target from the idle status read: the stock answers 9 bytes, our images 20."""
    n = len(idle_status or b"")
    return "stock" if n == 9 else "eglo" if n == 20 else "unknown"


def pick_chunk(mtu, target="unknown", chunk=None):
    """Block size for a negotiated ATT_MTU: MTU-7, at most what the target's handler takes.
    An explicit chunk is checked against the same limits. Returns (chunk, reason)."""
    room = mtu - 7
    cap = CHUNK_CAP.get(target, CHUNK_MIN)
    if chunk is not None:
        if chunk < CHUNK_MIN or chunk > room:
            raise OtaError(f"chunk {chunk} B outside {CHUNK_MIN}..{room} (ATT_MTU {mtu} - 7)")
        if chunk > cap:
            raise OtaError(f"chunk {chunk} B is more than the {target} OTA handler takes ({cap} B)")
        return chunk, "requested"
    d = max(CHUNK_MIN, min(room, cap))
    return d, (f"ATT_MTU {mtu} - 7" if d == room else f"{target} handler limit {cap} B")


def fault_every(n=0, kind="crc", at=()):
    """Test hook for run_ota(fault=...): every n-th first transmission of a data packet
    (0 = none), and the first transmission of each block in `at`, gets a damaged CRC
    ("crc") or is not sent at all ("drop"); resent packets go through intact. Returns
    the hook; hook.count says how many were hit."""
    seen = {"first": 0}
    at = set(at)

    def hook(i, attempt, pkt):
        if attempt:
            return pkt
        seen["first"] += 1
        if i not in at and (not n or seen["first"] % n):
            return pkt
        hook.count += 1
        return None if kind == "drop" else pkt[:-1] + bytes([pkt[-1] ^ 0x5A])

    hook.count = 0
    return hook


def login(t, name, password, rnd8, log=print):
    t.write(PAIR, login_packet(name, password, rnd8))
    rsp = t.read(PAIR)
    if not rsp or rsp[0] != 0x0D:
        raise OtaError(f"login refused: {rsp.hex() if rsp else rsp}")
    log(f"logged in (response {rsp[:9].hex()})")
    return session_key(name, password, rnd8, rsp[1:9])


def run_ota(t, img, log=print, mtu=DEFAULT_MTU, read_every=8, erase_timeout_s=90, chunk=None,
            max_resends=10, fault=None):
    """t: transport with write(char, data), read(char) -> bytes, sleep(ms).

    chunk: data block size (None = MTU-7, the classic 16 at MTU 23); the caller picks it
    with pick_chunk(). When the stock reports a CRC or sequence error, the blocks are sent
    again from the one after its last written block; more than max_resends in a row
    without getting further abort. fault: test hook (fault_every)."""
    d = _block(mtu, chunk)
    t.write(OTA, erase_packet(len(img), mtu, chunk))
    log(f"erasing: {len(img) // 4096 + 2} 4K sectors")
    if chunk is not None:
        log(f"block {d} B, packet {d + 4} B (ATT_MTU {mtu}), {packet_count(len(img), mtu, chunk)} packets")
    t.sleep(5000)
    waited = 5.0
    while True:
        rsp = t.read(OTA)
        if rsp and rsp[0] == 2:
            break
        if waited > erase_timeout_s:
            raise OtaError(f"erase did not finish: {rsp.hex() if rsp else rsp}")
        t.sleep(3000)
        waited += 3
    log(f"erase finished after ~{waited:.0f} s")
    total = packet_count(len(img), mtu, chunk)
    sent = {}               # index -> transmissions so far
    stats = {"resends": 0, "resent_packets": 0}
    stuck = {"from": -1, "n": 0}

    def resend_point(st, where):
        if not st.resumable:
            return None
        nxt = st.next_index
        if nxt <= stuck["from"]:
            stuck["n"] += 1
        else:
            stuck["from"], stuck["n"] = nxt, 1
        if stuck["n"] > max_resends:
            raise OtaError(f"{where}: {max_resends} resends from block {nxt} without progress ({st.describe()})")
        stats["resends"] += 1
        log(f"bulb reports {STOCK_ERRORS.get(st.error, st.error)} {where} ({st.describe()}): "
            f"resending from block {nxt} (resend {stats['resends']})")
        return nxt

    i = 0
    since_read = 0
    while True:
        while i < total:
            pkt = data_packet(img, i, d)
            attempt = sent.get(i, 0)
            sent[i] = attempt + 1
            if attempt:
                stats["resent_packets"] += 1
            if fault is not None:
                pkt = fault(i, attempt, pkt)
            if pkt is not None:
                t.write(OTA, pkt)
                since_read += 1
            if i % 2000 == 0 and not attempt:
                log(f"packet {i}/{total}")
            i += 1
            if since_read == read_every:
                since_read = 0
                st = Status(t.read(OTA))
                nxt = resend_point(st, f"after block {i - 1}")
                if nxt is not None:
                    i = nxt
                elif st.failed:
                    raise OtaError(f"bulb aborted OTA at packet {i - 1}: {st.raw.hex()} ({st.describe()})")
        t.write(OTA, end_packet(len(img), mtu, chunk))
        since_read = 0
        raw = t.read(OTA)
        st = Status(raw)
        nxt = resend_point(st, "at the end packet")
        if nxt is None:
            break
        i = nxt
    if len(raw) < 2 or raw[1] != 0:
        raise OtaError(f"OTA end: status {raw.hex() if raw else raw}")
    if stats["resends"]:
        log(f"resends: {stats['resends']}, packets sent again: {stats['resent_packets']}")
    log("OTA finished successfully (status 0)")
    return raw
