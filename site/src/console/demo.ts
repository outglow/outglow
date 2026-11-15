// A made-up bulb behind the same characteristics as a real one (?demo), so the
// console can be seen without hardware. Plausible values, clearly sample data.
import { COUNTERS } from "./diag";

export interface Char {
  readValue(): Promise<DataView>;
  writeValue(v: BufferSource): Promise<void>;
}
export type Chars = Record<"info" | "state" | "log" | "cmd" | "frames" | "config" | "pair", Char> & { counters: Char | null };

const dv = (n: number) => new DataView(new ArrayBuffer(n));
const ro = (fn: () => DataView): Char => ({ readValue: async () => fn(), writeValue: async () => {} });

export function demoChars(): Chars {
  const t0 = Date.now();
  const m = { on: 1, level: 178, ct: 370, su: 255, suLevel: 0xff, lvOpt: 0, ctOpt: 1, suCt: 0xffff, logged: 0, res: 0, cnt: 0 };
  const up = () => 18_425 + Math.floor((Date.now() - t0) / 1000);
  const bytes = (v: BufferSource) => new Uint8Array(ArrayBuffer.isView(v) ? v.buffer.slice(v.byteOffset, v.byteOffset + v.byteLength) : v);
  return {
    info: ro(() => {
      const model = "ESMLFzm_w6_TW", fw = "outglow 0.4.0", v = dv(48);
      v.setUint8(0, 1); v.setUint8(1, 1); v.setUint8(2, 1); v.setUint8(3, 4);
      // a documentation-style address, not a real bulb's
      [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef].forEach((b, i) => v.setUint8(4 + i, b));
      v.setUint8(12, model.length);
      [...model].forEach((ch, i) => v.setUint8(13 + i, ch.charCodeAt(0)));
      v.setUint8(13 + model.length, fw.length);
      [...fw].forEach((ch, i) => v.setUint8(14 + model.length + i, ch.charCodeAt(0)));
      return v;
    }),
    state: ro(() => {
      const v = dv(40), u = up();
      v.setUint8(0, 1); v.setUint32(1, u, true); v.setUint16(10, 42, true);
      v.setUint8(12, 1); v.setUint16(13, 0x7a21, true); v.setUint16(15, 0x1a62, true); v.setUint8(17, 15);
      v.setUint8(18, 4); v.setUint8(19, 4); v.setUint8(20, 1); v.setUint8(21, 168); v.setInt8(22, -52);
      v.setUint32(23, 48_240 + u, true); v.setUint32(27, 19_310 + u, true); v.setUint16(31, 812, true); v.setUint16(33, 9, true);
      v.setUint8(35, m.on); v.setUint8(36, m.level); v.setUint16(37, m.ct, true);
      v.setUint8(39, m.res | ((m.cnt & 7) << 4) | (m.logged ? 0x80 : 0));
      return v;
    }),
    config: ro(() => {
      const v = dv(20);
      v.setUint8(0, 1); v.setUint8(1, m.su); v.setUint8(2, m.suLevel); v.setUint8(3, m.lvOpt); v.setUint8(4, m.ctOpt);
      v.setUint16(5, m.suCt, true); v.setUint16(7, 153, true); v.setUint16(9, 454, true); v.setUint8(11, 1);
      v.setUint8(18, 1); v.setUint8(19, 254);
      return v;
    }),
    frames: ro(() => {
      const fr = [[18_410, 0x0000, 0x0008, 0x04, 0x11, 168, -52], [18_402, 0x0000, 0x0006, 0x01, 0x11, 171, -51],
        [17_200, 0x0000, 0x0b05, 0x00, 0x10, 160, -55], [12_011, 0x4d2e, 0x0006, 0x02, 0x11, 140, -63], [12, 0x0000, 0x0000, 0x00, 0x10, 175, -50]];
      const v = dv(1 + fr.length * 12);
      v.setUint8(0, fr.length * 12);
      fr.forEach((f, i) => {
        const o = 1 + i * 12;
        v.setUint32(o, f[0]!, true); v.setUint16(o + 4, f[1]!, true); v.setUint16(o + 6, f[2]!, true);
        v.setUint8(o + 8, f[3]!); v.setUint8(o + 9, f[4]!); v.setUint8(o + 10, f[5]!); v.setInt8(o + 11, f[6]!);
      });
      return v;
    }),
    log: ro(() => {
      const ev = [[18_300, 8, 0], [9_870, 38, 92], [9_910, 37, 101], [312, 40, 0], [9, 5, 15], [0, 1, 0]];
      const v = dv(1 + ev.length * 6);
      v.setUint8(0, ev.length);
      ev.forEach((e, i) => { v.setUint32(1 + i * 6, e[0]!, true); v.setUint8(5 + i * 6, e[1]!); v.setUint8(6 + i * 6, e[2]!); });
      return v;
    }),
    counters: ro(() => {
      const u = up();
      const values: Record<number, number> = {
        0x0000: 42, 0x0001: 1_204, 0x0100: 31_002 + u, 0x0101: 8_812, 0x0102: 48_240 + u, 0x0103: 19_310 + u,
        0x0104: 812, 0x0105: 9, 0x0106: 2_210, 0x0107: 14, 0x0108: 1_830, 0x0109: 1_652, 0x010c: 18, 0x010d: 9,
        0x010e: 5, 0x010f: 5, 0x0110: 2, 0x0111: 1, 0x0112: 61, 0x0118: 6_405, 0x011b: 0, 0x011c: 168, 0x011d: -52,
        0x5000: u, 0x5003: 4, 0x5004: 1, 0x5006: 3, 0x5007: 2, 0x5008: 61, 0x5009: 1, 0x500c: 2_410, 0x500e: 5,
        0x500f: 2, 0x5012: 41_100, 0x5013: 129_000, 0x5014: 37,
      };
      const ids = Object.keys(COUNTERS).map(Number).sort((a, b) => a - b);
      const v = dv(1 + ids.length * 7);
      v.setUint8(0, 1);
      let o = 1;
      for (const id of ids) {
        const val = values[id] ?? 0;
        const wide = (id >= 0x0100 && id <= 0x0103) || id === 0x5000 || id === 0x500c || id === 0x5012 || id === 0x5013;
        const type = id === 0x011d ? 0x28 : id === 0x011c || id === 0x5003 || id === 0x5004 || id === 0x500e || id === 0x500f ? 0x20
          : id === 0x5001 ? 0x30 : wide ? 0x23 : 0x21;
        v.setUint16(o, id, true); v.setUint8(o + 2, type);
        if (type === 0x23) { v.setUint32(o + 3, val, true); o += 7; }
        else if (type === 0x21) { v.setUint16(o + 3, val, true); o += 5; }
        else if (type === 0x28) { v.setInt8(o + 3, val); o += 4; }
        else { v.setUint8(o + 3, val); o += 4; }
      }
      return new DataView(v.buffer, 0, o);
    }),
    pair: { readValue: async () => new DataView(Uint8Array.of(0x0d).buffer), writeValue: async () => { m.logged = 1; } },
    cmd: {
      readValue: async () => dv(1),
      writeValue: async (raw) => {
        const b = bytes(raw);
        m.cnt++; m.res = 0;
        if (b[0] !== 0x01 && !m.logged) { m.res = 1; return; }
        const p1 = b[1] ?? 0, p2 = b[2] ?? 0;
        if (b[0] === 0x10) m.on = p1 === 2 ? 1 - m.on : p1;
        else if (b[0] === 0x11) { m.level = p1; m.on = 1; }
        else if (b[0] === 0x12) m.ct = p1 | (p2 << 8);
        else if (b[0] === 0x20) m.su = p1;
        else if (b[0] === 0x21) m.suLevel = p1;
        else if (b[0] === 0x22) m.lvOpt = p1;
        else if (b[0] === 0x23) m.ctOpt = p1;
        else if (b[0] === 0x24) m.suCt = p1 | (p2 << 8);
      },
    },
  };
}
