// The browser flasher's protocol module against the command-line flasher
// (`outglow ble`, crates/outglow/src/flash): the same packets, CRCs and login
// bytes. The reference values come from `outglow ble vectors [image]`.
//
//   npm test
//   OUTGLOW=../target/debug/outglow OUTGLOW_IMAGE=../target/tc32/bulb-tc32-ota.bin npm test
//
// By default the tool runs through `cargo run -q -p outglow` from the
// repository root; OUTGLOW names a built binary instead. With OUTGLOW_IMAGE
// the packets of that image are compared as well.

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, test } from "vitest";
import * as ota from "../src/flasher/awox-ota.ts";

const here = dirname(fileURLToPath(import.meta.url));
const boards = JSON.parse(readFileSync(join(here, "..", "public", "boards.json"), "utf8")) as ota.Board[];
const root = join(here, "..", "..");

interface Reference {
  image: string;
  verify: boolean;
  count: number;
  erase: string;
  data: Record<string, string>;
  end: string;
  crc16: number;
  secure_password: string;
  login: string;
  login_1234: string;
  chunk40: { erase: string; data: Record<string, string>; end: string };
}

function reference(image?: string): Reference {
  const args = ["ble", "vectors"];
  if (image) args.push(resolve(image));
  const [cmd, all] = process.env.OUTGLOW
    ? [resolve(process.env.OUTGLOW), args]
    : ["cargo", ["run", "-q", "-p", "outglow", "--", ...args]];
  return JSON.parse(execFileSync(cmd, all, { cwd: root, maxBuffer: 1 << 26 }).toString()) as Reference;
}

const bytes = (b64: string) => new Uint8Array(Buffer.from(b64, "base64"));

function compare(ref: Reference): Uint8Array {
  const img = bytes(ref.image);
  expect(ota.packetCount(img.length)).toBe(ref.count);
  expect(ota.toHex(ota.erasePacket(img.length))).toBe(ref.erase);
  for (const [i, pkt] of Object.entries(ref.data)) expect(ota.toHex(ota.dataPacket(img, Number(i))), `packet ${i}`).toBe(pkt);
  expect(ota.toHex(ota.endPacket(img.length))).toBe(ref.end);
  return img;
}

const ref = reference();

describe("against outglow ble", () => {
  test("packets of a synthetic image", () => {
    const img = compare(ref);
    expect(ref.verify).toBe(true);
    const info = ota.checkImage(img, boards);
    expect([info.model, info.revision, info.product, info.outglow, info.board?.name])
      .toEqual(["ESMLFzm_w6_TW", "4.63", 0x20a6, true, "ESMLFzm_w6_TW-4.63"]);
  });

  test("packets with 40-byte blocks", () => {
    const img = bytes(ref.image);
    expect(ota.toHex(ota.erasePacket(img.length, 40))).toBe(ref.chunk40.erase);
    for (const [i, pkt] of Object.entries(ref.chunk40.data)) expect(ota.toHex(ota.dataPacket(img, Number(i), 40))).toBe(pkt);
    expect(ota.toHex(ota.endPacket(img.length, 40))).toBe(ref.chunk40.end);
  });

  test("CRC-16/MODBUS check value", () => {
    const s = new TextEncoder().encode("123456789");
    expect(ota.crc16Modbus(s)).toBe(ref.crc16);
    expect(ota.crc16Modbus(s)).toBe(0x4b37);
  });

  test("mesh login packet and the secure password", async () => {
    const rnd8 = Uint8Array.from([1, 2, 3, 4, 5, 6, 7, 8]);
    const pw = await ota.securePassword(ota.hex("a4c138123456"));
    expect(ota.toHex(pw)).toBe(ref.secure_password);
    expect(ota.toHex(await ota.loginPacket("unpaired", pw, rnd8))).toBe(ref.login);
    expect(ota.toHex(await ota.loginPacket("telink_mesh1", "123", rnd8))).toBe(ref.login_1234);
  });

  test.skipIf(!process.env.OUTGLOW_IMAGE)("the built Outglow image (OUTGLOW_IMAGE)", () => {
    const img = compare(reference(process.env.OUTGLOW_IMAGE));
    expect(ota.checkImage(img, boards).outglow).toBe(true);
  });
});

describe("checks", () => {
  test("a damaged image is refused", () => {
    const img = bytes(ref.image);
    img[300]! ^= 1;
    expect(() => ota.checkImage(img, boards)).toThrow(/CRC/);
    const noMarker = img.slice();
    noMarker[8] = 0;
    expect(() => ota.checkImage(noMarker, boards)).toThrow(/KNLT/);
  });

  const tw: ota.ImageInfo = { model: "ESMLFzm_w6_TW", revision: "4.63", product: 0x20a6, outglow: true, size: 0, board: undefined };
  const bulb = (o: Partial<ota.Bulb>): ota.Bulb =>
    ({ product: null, mac6: null, model: null, revision: null, firmware: null, running: "stock", ...o });

  test("the bulb must match the image", () => {
    expect(ota.matchBulb(tw, bulb({ product: 0x20a6, revision: "4.63", model: "ESMLFzm_w6_TW" }))).toEqual([]);
    expect(ota.matchBulb(tw, bulb({ product: 0x20a7, revision: "4.63" }))).toHaveLength(1);
    expect(ota.matchBulb(tw, bulb({ revision: "4.62" }))).toHaveLength(1);
    expect(ota.matchBulb(tw, bulb({}))).toHaveLength(1);
  });

  test("a bulb is identified by model and revision, or by product and revision", () => {
    expect(ota.identify(bulb({ model: "EGLO_ZM_TW", revision: "4.62" }), boards)?.name).toBe("EGLO_ZM_TW-4.62");
    expect(ota.identify(bulb({ product: 0x2097, revision: "4.69" }), boards)?.name).toBe("EGLO_ZM_TW-4.69");
    expect(ota.identify(bulb({ product: 0x2097, revision: "4.99" }), boards)).toBeUndefined();
  });

  test("the newest release of a board is proposed", () => {
    const r = (board: string, fileVersion: string): ota.Release => ({ board, version: fileVersion, fileVersion, url: "", size: 0, sha256: "" });
    const m: ota.Manifest = { schema: 1, images: [r("A", "0x00020003"), r("A", "0x00020010"), r("B", "0x00030000")] };
    expect(ota.latestFor(m, "A")?.fileVersion).toBe("0x00020010");
    expect(ota.latestFor(m, "C")).toBeUndefined();
  });

  test("block size limits", () => {
    expect(ota.pickChunk("stock")).toBe(16);
    expect(ota.pickChunk("outglow", 57, 247)).toBe(57);
    expect(() => ota.pickChunk("outglow", 58, 247)).toThrow();
    expect(() => ota.pickChunk("stock", 40)).toThrow(); // ATT_MTU 23 leaves 16
  });

  test("advertisement: product and MAC", () => {
    const a = ota.parseAdvertisement(Uint8Array.from([0xa7, 0x20, 0x56, 0x34, 0x12, 0x00]))!;
    expect(a.product).toBe(0x20a7);
    expect(ota.toHex(a.mac6)).toBe("a4c100123456");
  });
});

test("a transfer through a model of the original firmware succeeds and resends after a CRC error", async () => {
  const img = bytes(ref.image);
  const total = ota.packetCount(img.length);
  // status layout of the original firmware: state, error, last written LE16, last received LE16, errors, sectors, 0
  let state = 0, lastOk = 0xffff, error = 0, rejectOnce = 37;
  const written: number[] = [];
  const t: ota.Transport = {
    async write(ch, p) {
      expect(ch).toBe(ota.OTA);
      if (state === 0) { state = 2; return; }
      if (p.length === 4) { state = 4; return; }
      const i = p[0]! | (p[1]! << 8);
      state = 3;
      if (i === rejectOnce) { rejectOnce = -1; error = 1; return; }
      if (error) return; // what follows a rejected block is dropped until the resend
      expect(i).toBe((lastOk + 1) & 0xffff);
      lastOk = i;
      written.push(i);
    },
    async read() {
      if (state === 4) return Uint8Array.from([4, 0, 0, 0, 0, 0, 0, 100, 0]);
      const r = Uint8Array.from([state, error, lastOk & 0xff, lastOk >> 8, 0, 0, 0, 0, 0]);
      error = 0;
      return r;
    },
    async sleep() {},
  };
  const logs: string[] = [];
  const phases: string[] = [];
  await ota.runOta(t, img, { log: (s) => logs.push(s), phase: (p) => phases.push(p) });
  expect(written).toHaveLength(total);
  expect(logs.some((l) => l.includes("resending from block 37"))).toBe(true);
  expect(phases).toEqual(["erasing", "writing", "verifying"]);
});
