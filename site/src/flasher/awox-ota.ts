// Firmware update over Bluetooth LE for bulbs with the AwoX/Telink mesh
// profile: the original firmware and Outglow. Transport-independent: the same
// steps as crates/outglow/src/flash/protocol.rs (packets, CRC, erase wait, status polling and
// resends), so the browser flasher and the command-line one send the same
// bytes (test/awox-ota.test.ts compares them).
//
// Flow on the OTA characteristic (...1915):
//   1. erase: a (D+4)-byte packet, all zeros but [2] = len/4096 + 2; then the
//      status is read every 3 s until state 2 (erased)
//   2. data: idx LE16 || D bytes (last one padded with 0xFF) || CRC-16/MODBUS
//      LE over what comes before; the status is read after every 8 writes
//   3. end: count LE16 || CRC-16/MODBUS LE; status[1] == 0 is success
// D is 16 by default (the classic transfer at ATT_MTU 23, the one verified on
// hardware). An optional mesh login on ...1914 comes before all of it.

export const TELINK_SERVICE = "00010203-0405-0607-0809-0a0b0c0d1910";
export const PAIR = 0x14;
export const OTA = 0x15;
export type Char = typeof PAIR | typeof OTA;
export const AWOX_VENDOR = 0x0160;
export const BLOCK = 16;
/** Largest block each firmware's OTA handler takes (crates/outglow/src/flash/protocol.rs). */
export const CHUNK_CAP = { stock: 240, outglow: 57, unknown: BLOCK } as const;
export const STAGING_MAX = 0x64000;
export const STOCK_ERRORS: Record<number, string> = {
  0: "ok", 1: "CRC error", 2: "sequence error", 3: "index beyond the buffer",
  4: "packet in the wrong state", 5: "flash read-back differs",
};

export function telinkUuid(short: number): string {
  return `00010203-0405-0607-0809-0a0b0c0d19${short.toString(16).padStart(2, "0")}`;
}

export function hex(s: string): Uint8Array {
  const out = new Uint8Array(s.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(s.substr(i * 2, 2), 16);
  return out;
}

export function toHex(b: Uint8Array): string {
  return Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
}

const AWOX_SECRET = hex("56E850EDCFDD8B99");

export function crc16Modbus(data: Uint8Array): number {
  let crc = 0xffff;
  for (const b of data) {
    crc ^= b;
    for (let i = 0; i < 8; i++) crc = crc & 1 ? (crc >>> 1) ^ 0xa001 : crc >>> 1;
  }
  return crc;
}

const CRC32_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

/** The standard CRC-32 (zlib, binascii.crc32). */
export function crc32(data: Uint8Array): number {
  let c = 0xffffffff;
  for (const b of data) c = CRC32_TABLE[(c ^ b) & 0xff]! ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function u32(b: Uint8Array, o: number): number {
  return (b[o]! | (b[o + 1]! << 8) | (b[o + 2]! << 16) | (b[o + 3]! << 24)) >>> 0;
}

export function erasePacket(imgLen: number, d = BLOCK): Uint8Array {
  const b = new Uint8Array(d + 4);
  b[2] = (Math.floor(imgLen / 4096) + 2) & 0xff;
  return b;
}

export function packetCount(imgLen: number, d = BLOCK): number {
  return Math.ceil(imgLen / d);
}

export function dataPacket(img: Uint8Array, i: number, d = BLOCK): Uint8Array {
  const p = new Uint8Array(d + 4);
  p[0] = i & 0xff;
  p[1] = (i >> 8) & 0xff;
  p.fill(0xff, 2, 2 + d);
  p.set(img.subarray(i * d, Math.min((i + 1) * d, img.length)), 2);
  const crc = crc16Modbus(p.subarray(0, d + 2));
  p[d + 2] = crc & 0xff;
  p[d + 3] = crc >> 8;
  return p;
}

export function endPacket(imgLen: number, d = BLOCK): Uint8Array {
  const n = packetCount(imgLen, d);
  const p = new Uint8Array([n & 0xff, (n >> 8) & 0xff, 0, 0]);
  const crc = crc16Modbus(p.subarray(0, 2));
  p[2] = crc & 0xff;
  p[3] = crc >> 8;
  return p;
}

export type Layout = keyof typeof CHUNK_CAP;

/** A status read of ...1915: the original firmware answers 9 bytes, Outglow 20. */
export class Status {
  readonly raw: Uint8Array;
  readonly layout: Layout;
  readonly state: number | null;
  readonly error: number | null;
  readonly lastOk: number | null = null;
  readonly lastRx: number | null = null;
  readonly errors: number | null = null;

  constructor(raw: Uint8Array | null | undefined) {
    const r = (this.raw = raw ?? new Uint8Array());
    this.layout = r.length === 9 ? "stock" : r.length >= 9 ? "outglow" : "unknown";
    this.state = r.length ? r[0]! : null;
    this.error = r.length > 1 ? r[1]! : null;
    if (this.layout === "stock") {
      this.lastOk = r[2]! | (r[3]! << 8);
      this.lastRx = r[4]! | (r[5]! << 8);
      this.errors = r[6]!;
    }
  }
  /** The original firmware rejected a block (CRC or sequence) and takes the next one after lastOk. */
  get resumable(): boolean {
    return this.layout === "stock" && this.state === 3 && (this.error === 1 || this.error === 2);
  }
  get nextIndex(): number | null {
    return this.lastOk === null ? null : (this.lastOk + 1) & 0xffff;
  }
  /** Nothing more can come of this transfer. */
  get failed(): boolean {
    if (this.layout === "stock") return [3, 4, 5].includes(this.error ?? -1) || this.state === 0;
    return this.error === 2;
  }
  describe(): string {
    if (this.layout === "stock") {
      const last = this.lastOk === 0xffff ? "none" : this.lastOk;
      return `state ${this.state}, ${STOCK_ERRORS[this.error ?? -1] ?? `error ${this.error}`}, ` +
        `last written block ${last}, last received ${this.lastRx}, consecutive errors ${this.errors}`;
    }
    const rx = this.raw.length >= 4 ? this.raw[2]! | (this.raw[3]! << 8) : null;
    return `state ${this.state}, result ${this.error}${this.error === 2 ? " (failed)" : ""}, blocks received ${rx}`;
  }
}

export class OtaError extends Error {}

/** Block size for a firmware's handler: requested, else 16; checked against its cap and the ATT payload room. */
export function pickChunk(layout: Layout, requested?: number, mtu = 23): number {
  if (requested === undefined) return BLOCK;
  const cap = Math.min(CHUNK_CAP[layout], mtu - 7);
  if (!Number.isInteger(requested) || requested < BLOCK || requested > cap) {
    throw new OtaError(`block ${requested} B outside ${BLOCK}..${cap} B (${layout} handler, ATT_MTU ${mtu})`);
  }
  return requested;
}

// ---- boards, images, bulbs ---------------------------------------------------

export interface Board {
  name: string;
  model: string;
  ble_product: number;
  hw_revision: string;
  kind: string;
  image_type: number;
}

export interface ImageInfo {
  model: string;
  revision: string;
  product: number;
  outglow: boolean;
  size: number;
  board: Board | undefined;
}

const BOARD_TAG = /open-eglo-board:([A-Za-z0-9_]+)-(\d\.\d+)/;

function latin1(b: Uint8Array): string {
  let s = "";
  for (let i = 0; i < b.length; i += 0x8000) s += String.fromCharCode(...b.subarray(i, i + 0x8000));
  return s;
}

/**
 * Checks an image the way the bootloader and `outglow ble push` do and says
 * which board it is for. Throws OtaError.
 */
export function checkImage(img: Uint8Array, boards: Board[]): ImageInfo {
  if (img.length < 0x20 || latin1(img.subarray(8, 12)) !== "KNLT") {
    throw new OtaError("no KNLT marker at 0x08: not a finished Telink image " +
      "(a Zigbee .ota file is not what goes over Bluetooth: take the -ota.bin)");
  }
  const size = u32(img, 0x18);
  if (size !== img.length) throw new OtaError(`size in the header ${size} != file length ${img.length}`);
  if (((crc32(img.subarray(0, size - 4)) ^ 0xffffffff) >>> 0) !== u32(img, size - 4)) {
    throw new OtaError("image CRC mismatch: the bootloader would reject it");
  }
  if (img.length > STAGING_MAX) throw new OtaError(`image of ${img.length} B does not fit the OTA buffer (${STAGING_MAX} B)`);
  const text = latin1(img);
  const tag = BOARD_TAG.exec(text);
  let model: string | undefined, revision: string, outglow: boolean;
  if (tag) {
    model = tag[1]!;
    revision = tag[2]!;
    outglow = true;
  } else {
    const models = [...new Set(boards.map((b) => b.model))].sort((a, b) => b.length - a.length);
    model = models.find((m) => text.includes(m + "\0"));
    const revs = [...new Set([...text.matchAll(/(?<![\d.])(4\.\d\d)(?![\d.])/g)].map((m) => m[1]!))];
    if (!model || revs.length !== 1) {
      throw new OtaError(`cannot recognise the board of the image (model ${model ?? "?"}, revisions ${revs.join(", ") || "none"})`);
    }
    revision = revs[0]!;
    outglow = false;
  }
  const any = boards.find((b) => b.model === model);
  if (!any) throw new OtaError(`board ${model} is not known to this flasher`);
  const board = boards.find((b) => b.model === model && b.hw_revision === revision);
  return { model, revision, product: any.ble_product, outglow, size, board };
}

/** What the flasher learnt about the bulb. */
export interface Bulb {
  product: number | null;
  mac6: Uint8Array | null;
  model: string | null;
  revision: string | null;
  firmware: string | null;
  /** from the idle OTA status: the original firmware answers 9 bytes, Outglow 20 */
  running: Layout;
}

/** The board of a bulb: model and hardware revision, else product and revision. */
export function identify(bulb: Bulb, boards: Board[]): Board | undefined {
  return boards.find((b) => b.model === bulb.model && b.hw_revision === bulb.revision) ??
    boards.find((b) => bulb.product !== null && b.ble_product === bulb.product && b.hw_revision === bulb.revision);
}

/** The bulb against the image. Returns the problems; empty = they match. */
export function matchBulb(image: ImageInfo, bulb: Bulb): string[] {
  const out: string[] = [];
  if (bulb.product !== null && bulb.product !== image.product) {
    out.push(`the bulb is product 0x${bulb.product.toString(16)}, the image is for 0x${image.product.toString(16)} (${image.model})`);
  }
  if (bulb.model && bulb.model !== image.model) out.push(`the bulb says it is ${bulb.model}, the image is for ${image.model}`);
  if (bulb.revision === null) out.push("the bulb did not tell its hardware revision");
  else if (bulb.revision !== image.revision) {
    out.push(`the bulb is hardware revision ${bulb.revision}, the image is for ${image.revision}: its LEDs may be on other pins`);
  }
  return out;
}

/** Product and MAC from manufacturer data 0x0160 (product LE16 || the 4 low MAC bytes reversed). */
export function parseAdvertisement(data: Uint8Array): { product: number; mac6: Uint8Array } | null {
  if (data.length < 6) return null;
  const product = data[0]! | (data[1]! << 8);
  const mac6 = new Uint8Array([0xa4, 0xc1, data[5]!, data[4]!, data[3]!, data[2]!]);
  return { product, mac6 };
}

// ---- releases ------------------------------------------------------------------

/** One image in firmware.json. */
export interface Release {
  board: string;
  version: string;
  /** Zigbee OTA file version, "0x00020004" */
  fileVersion: string;
  /** the Bluetooth image (…-ota.bin), relative to the site root or absolute */
  url: string;
  size: number;
  sha256: string;
  date?: string;
  notes?: string;
}

export interface Manifest {
  schema: 1;
  images: Release[];
}

/** The newest release for a board (by file version). */
export function latestFor(manifest: Manifest, board: string): Release | undefined {
  return manifest.images
    .filter((r) => r.board === board)
    .sort((a, b) => parseInt(b.fileVersion, 16) - parseInt(a.fileVersion, 16))[0];
}

// ---- mesh login ------------------------------------------------------------------

function rev(b: Uint8Array): Uint8Array {
  return Uint8Array.from(b).reverse();
}

/** AES-128 of one block (ECB, through CBC with a zero IV: the first block is the same). */
export async function aesBlock(key: Uint8Array, block: Uint8Array, subtle: SubtleCrypto = globalThis.crypto.subtle): Promise<Uint8Array> {
  const k = await subtle.importKey("raw", key as BufferSource, { name: "AES-CBC" }, false, ["encrypt"]);
  const out = await subtle.encrypt({ name: "AES-CBC", iv: new Uint8Array(16) }, k, block as BufferSource);
  return new Uint8Array(out).subarray(0, 16);
}

/** Telink aes_att_encryption: AES-128 on the reversed bytes of key and data. */
export async function telinkAes(key: Uint8Array, data: Uint8Array, subtle?: SubtleCrypto): Promise<Uint8Array> {
  return rev(await aesBlock(rev(key), rev(data), subtle));
}

function pad16(b: Uint8Array): Uint8Array {
  const out = new Uint8Array(16);
  out.set(b.subarray(0, 16));
  return out;
}

export function namePass(name: string, password: string | Uint8Array): Uint8Array {
  const n = pad16(new TextEncoder().encode(name));
  const p = pad16(typeof password === "string" ? new TextEncoder().encode(password) : password);
  return n.map((x, i) => x ^ p[i]!);
}

export async function loginPacket(name: string, password: string | Uint8Array, rnd8: Uint8Array, subtle?: SubtleCrypto): Promise<Uint8Array> {
  const enc = await telinkAes(pad16(rnd8), namePass(name, password), subtle);
  const out = new Uint8Array(17);
  out[0] = 0x0c;
  out.set(rnd8, 1);
  out.set(enc.subarray(0, 8), 9);
  return out;
}

/** The "unpaired" password of current products: SHA-256(MAC6 || secret || 00 00)[:16]. */
export async function securePassword(mac6: Uint8Array, subtle: SubtleCrypto = globalThis.crypto.subtle): Promise<Uint8Array> {
  const m = new Uint8Array(16);
  m.set(mac6, 0);
  m.set(AWOX_SECRET, 6);
  return new Uint8Array(await subtle.digest("SHA-256", m)).subarray(0, 16);
}

// ---- the transfer ------------------------------------------------------------------

export interface Transport {
  write(char: Char, data: Uint8Array): Promise<void>;
  read(char: Char): Promise<Uint8Array>;
  sleep(ms: number): Promise<void>;
}

export type Phase = "erasing" | "writing" | "verifying";

export interface OtaOptions {
  log?: (line: string, verbose?: boolean) => void;
  phase?: (p: Phase) => void;
  progress?: (done: number, total: number) => void;
  chunk?: number;
  readEvery?: number;
  eraseTimeoutS?: number;
  maxResends?: number;
  /** pause after each data write */
  delayMs?: number;
  signal?: AbortSignal;
}

export async function runOta(t: Transport, img: Uint8Array, opts: OtaOptions = {}): Promise<Uint8Array> {
  const log = opts.log ?? (() => {});
  const progress = opts.progress ?? (() => {});
  const readEvery = opts.readEvery ?? 8;
  const eraseTimeoutS = opts.eraseTimeoutS ?? 90;
  const maxResends = opts.maxResends ?? 10;
  const d = opts.chunk ?? BLOCK;
  const check = () => {
    if (opts.signal?.aborted) throw new OtaError("stopped");
  };
  opts.phase?.("erasing");
  await t.write(OTA, erasePacket(img.length, d));
  log(`erasing ${Math.floor(img.length / 4096) + 2} sectors of 4 KB; block ${d} B`);
  await t.sleep(5000);
  let waited = 5;
  for (;;) {
    check();
    const rsp = await t.read(OTA);
    log(`erase status ${toHex(rsp)}`, true);
    if (rsp.length && rsp[0] === 2) break;
    if (waited > eraseTimeoutS) throw new OtaError(`erase did not finish: ${toHex(rsp)}`);
    await t.sleep(3000);
    waited += 3;
  }
  log(`erase finished after about ${waited} s`);
  opts.phase?.("writing");
  const total = packetCount(img.length, d);
  const stuck = { from: -1, n: 0 };
  let resends = 0;

  const resendPoint = (st: Status, where: string): number | null => {
    if (!st.resumable) return null;
    const nxt = st.nextIndex!;
    if (nxt <= stuck.from) stuck.n++;
    else Object.assign(stuck, { from: nxt, n: 1 });
    if (stuck.n > maxResends) throw new OtaError(`${where}: ${maxResends} resends from block ${nxt} without progress (${st.describe()})`);
    resends++;
    log(`the bulb reports ${STOCK_ERRORS[st.error!]} ${where} (${st.describe()}): resending from block ${nxt}`);
    return nxt;
  };

  let i = 0;
  let sinceRead = 0;
  let raw: Uint8Array;
  for (;;) {
    while (i < total) {
      check();
      await t.write(OTA, dataPacket(img, i, d));
      if (opts.delayMs) await t.sleep(opts.delayMs);
      sinceRead++;
      i++;
      if (i % 16 === 0 || i === total) progress(i, total);
      if (sinceRead === readEvery) {
        sinceRead = 0;
        const st = new Status(await t.read(OTA));
        if (i % 512 === 0) log(`block ${i}/${total}: ${toHex(st.raw)}`, true);
        const nxt = resendPoint(st, `after block ${i - 1}`);
        if (nxt !== null) i = nxt;
        else if (st.failed) throw new OtaError(`the bulb aborted the update at block ${i - 1}: ${toHex(st.raw)} (${st.describe()})`);
      }
    }
    opts.phase?.("verifying");
    await t.write(OTA, endPacket(img.length, d));
    sinceRead = 0;
    raw = await t.read(OTA);
    log(`end status ${toHex(raw)}`, true);
    const nxt = resendPoint(new Status(raw), "at the end packet");
    if (nxt === null) break;
    opts.phase?.("writing");
    i = nxt;
  }
  if (raw.length < 2 || raw[1] !== 0) throw new OtaError(`end of the update: status ${toHex(raw)}`);
  if (resends) log(`resends: ${resends}`);
  log("the bulb accepted the image (status 0)");
  return raw;
}
