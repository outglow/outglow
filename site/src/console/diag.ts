// The bulb's diagnostic service over Bluetooth (crates/ble/src/diag.rs): UUIDs,
// commands and decoders for every characteristic. Byte layouts are fixed: the
// lab's C firmware has the same service, so both decode with this file.

export const UUID = {
  service: "e6100000-5a4b-4c3d-8e2f-1a2b3c4d5e6f",
  info: "e6100001-5a4b-4c3d-8e2f-1a2b3c4d5e6f",
  state: "e6100002-5a4b-4c3d-8e2f-1a2b3c4d5e6f",
  log: "e6100003-5a4b-4c3d-8e2f-1a2b3c4d5e6f",
  cmd: "e6100004-5a4b-4c3d-8e2f-1a2b3c4d5e6f",
  frames: "e6100005-5a4b-4c3d-8e2f-1a2b3c4d5e6f",
  config: "e6100006-5a4b-4c3d-8e2f-1a2b3c4d5e6f",
  counters: "e6100007-5a4b-4c3d-8e2f-1a2b3c4d5e6f",
} as const;

type Bytes = DataView | ArrayBufferView | ArrayBuffer;

function view(v: Bytes): DataView {
  if (v instanceof DataView) return v;
  if (ArrayBuffer.isView(v)) return new DataView(v.buffer, v.byteOffset, v.byteLength);
  return new DataView(v);
}

const hex4 = (v: number) => "0x" + v.toString(16).padStart(4, "0");
const hex2 = (v: number) => "0x" + v.toString(16).padStart(2, "0");

const u16 = (v: number) => [v & 0xff, (v >> 8) & 0xff];

/** Writes to the cmd characteristic. All but identify need a mesh login on the connection. */
export const commands = {
  identify: () => Uint8Array.of(0x01),
  onOff: (mode: 0 | 1 | 2) => Uint8Array.of(0x10, mode),
  level: (level: number, tenths = 5) => Uint8Array.of(0x11, level, ...u16(tenths)),
  colorTemp: (mireds: number, tenths = 5) => Uint8Array.of(0x12, ...u16(mireds), ...u16(tenths)),
  startUpOnOff: (v: number) => Uint8Array.of(0x20, v),
  startUpLevel: (v: number) => Uint8Array.of(0x21, v),
  levelOptions: (v: number) => Uint8Array.of(0x22, v),
  colorOptions: (v: number) => Uint8Array.of(0x23, v),
  startUpColorTemp: (mireds: number) => Uint8Array.of(0x24, ...u16(mireds)),
  reboot: () => Uint8Array.of(0x30),
  factoryReset: () => Uint8Array.of(0x31, 0x52, 0x53, 0x54),
};

export const RESET_CAUSE = ["power-on", "watchdog", "software reset"];
const CMD_RESULT = ["done", "requires login", "invalid parameters"];

export interface Info {
  variant: "dimm" | "tw";
  watchdog: boolean;
  sense: boolean;
  build: number;
  ieee: string;
  model: string;
  firmware: string;
}

function ascii(dv: DataView, off: number, len: number): string {
  let s = "";
  for (let i = 0; i < len && off + i < dv.byteLength; i++) s += String.fromCharCode(dv.getUint8(off + i));
  return s;
}

export function decodeInfo(value: Bytes): Info {
  const dv = view(value);
  if (dv.byteLength < 14 || dv.getUint8(0) !== 1) throw new Error("unknown info format");
  const ieee: string[] = [];
  for (let i = 11; i >= 4; i--) ieee.push(dv.getUint8(i).toString(16).padStart(2, "0"));
  const mlen = dv.getUint8(12);
  const flen = dv.getUint8(13 + mlen);
  return {
    variant: dv.getUint8(1) === 1 ? "tw" : "dimm",
    watchdog: !!(dv.getUint8(2) & 0x01),
    sense: !!(dv.getUint8(2) & 0x02),
    build: dv.getUint8(3),
    ieee: ieee.join(":"),
    model: ascii(dv, 13, mlen),
    firmware: ascii(dv, 14 + mlen, flen),
  };
}

export interface State {
  uptime: number;
  resetCause: number;
  resetCauseText: string;
  crashCount: number;
  watchdogResets: number;
  safeMode: boolean;
  resetStep: number;
  bootCount: number;
  zigbee: {
    joined: boolean;
    address: number | null;
    pan: number | null;
    channel: number;
    neighbors: number;
    routers: number;
    children: number;
    lqi: number;
    rssi: number;
  };
  mac: { rx: number; tx: number; retry: number; fail: number; retryRate: number; failRate: number };
  loggedIn: boolean;
  cmdResult: number;
  cmdResultText: string;
  cmdCount: number;
  light: { on: boolean; level: number; percent: number; mireds: number | null; kelvin: number | null };
}

export function decodeState(value: Bytes): State {
  const dv = view(value);
  if (dv.byteLength < 39 || dv.getUint8(0) !== 1) throw new Error("unknown state format");
  const joined = dv.getUint8(12) === 1;
  const ct = dv.getUint16(37, true);
  const rx = dv.getUint32(23, true), tx = dv.getUint32(27, true);
  const retry = dv.getUint16(31, true), fail = dv.getUint16(33, true);
  const last = dv.byteLength > 39 ? dv.getUint8(39) : 0;
  return {
    uptime: dv.getUint32(1, true),
    resetCause: dv.getUint8(5),
    resetCauseText: RESET_CAUSE[dv.getUint8(5)] ?? "unknown",
    crashCount: dv.getUint8(6),
    watchdogResets: dv.getUint8(7),
    safeMode: dv.getUint8(8) === 1,
    resetStep: dv.getUint8(9),
    bootCount: dv.getUint16(10, true),
    zigbee: {
      joined,
      address: joined ? dv.getUint16(13, true) : null,
      pan: joined ? dv.getUint16(15, true) : null,
      channel: dv.getUint8(17),
      neighbors: dv.getUint8(18),
      routers: dv.getUint8(19),
      children: dv.getUint8(20),
      lqi: dv.getUint8(21),
      rssi: dv.getInt8(22),
    },
    mac: { rx, tx, retry, fail, retryRate: tx ? retry / tx : 0, failRate: tx ? fail / tx : 0 },
    loggedIn: !!(last & 0x80),
    cmdResult: last & 0x0f,
    cmdResultText: CMD_RESULT[last & 0x0f] ?? "unknown",
    cmdCount: (last >> 4) & 0x07,
    light: {
      on: dv.getUint8(35) === 1,
      level: dv.getUint8(36),
      percent: Math.round((dv.getUint8(36) * 100) / 254),
      mireds: ct === 0xffff ? null : ct,
      kelvin: ct === 0xffff || ct === 0 ? null : Math.round(1e6 / ct / 10) * 10,
    },
  };
}

export interface Config {
  startUpOnOff: number;
  startUpLevel: number;
  executeIfOff: boolean;
  coupleColorTemp: boolean;
  hasColor: boolean;
  colorExecuteIfOff: boolean;
  startUpColorTemp: number | null;
  ctMin: number;
  ctMax: number;
}

export function decodeConfig(value: Bytes): Config {
  const dv = view(value);
  if (dv.byteLength < 20 || dv.getUint8(0) !== 1) throw new Error("unknown config format");
  const hasColor = dv.getUint8(4) !== 0xff;
  const sct = dv.getUint16(5, true);
  return {
    startUpOnOff: dv.getUint8(1),
    startUpLevel: dv.getUint8(2),
    executeIfOff: !!(dv.getUint8(3) & 0x01),
    coupleColorTemp: !!(dv.getUint8(3) & 0x02),
    hasColor,
    colorExecuteIfOff: hasColor && !!(dv.getUint8(4) & 0x01),
    startUpColorTemp: !hasColor || sct === 0xffff ? null : sct,
    ctMin: dv.getUint16(7, true),
    ctMax: dv.getUint16(9, true),
  };
}

// ---- event log ---------------------------------------------------------------------

const STAGES: Record<number, string> = {
  1: "event and timer handling", 2: "the Zigbee stack", 3: "the BLE + Zigbee loop", 4: "mesh transmission", 5: "the light application",
};
function faultText(v: number): string {
  if (!v) return "no fault";
  if (v >= 64) return `stack exception, code ${v - 64}`;
  return "hang in " + (STAGES[v] ?? `stage ${v}`);
}
const NWK_STATUS: Record<number, string> = {
  0x00: "no route", 0x01: "tree link failure", 0x02: "non-tree link failure", 0x03: "low battery",
  0x04: "no routing capacity", 0x05: "no indirect capacity", 0x06: "indirect expiry", 0x07: "target unavailable",
  0x08: "target address unallocated", 0x09: "parent link failure", 0x0a: "validate route", 0x0b: "source route failure",
  0x0c: "many-to-one route failure", 0x0d: "address conflict", 0x0e: "verify addresses", 0x0f: "PAN identifier update",
  0x10: "network address update", 0x11: "bad frame counter", 0x12: "bad key sequence number", 0x13: "unknown command",
};
const nwkStatus = (v: number) => NWK_STATUS[v] ?? `code ${hex2(v)}`;

const EVENTS: Record<number, (a: number) => string> = {
  1: (a) => `start: ${RESET_CAUSE[a] ?? "cause " + a}`,
  2: (a) => `safe mode (resets in a row: ${a})`,
  3: () => "factory reset",
  4: (a) => `factory reset sequence: step ${a} of 7`,
  5: (a) => `joined the Zigbee network (channel ${a})`,
  6: () => "left the Zigbee network",
  7: (a) => `stable operation, reset counter cleared (was ${a})`,
  8: () => "Bluetooth connection",
  9: () => "Bluetooth disconnection",
  10: () => "identify on request",
  11: (a) => "previous start: " + faultText(a),
  35: (a) => `rejoin started${a ? " (secured)" : " (unsecured)"}`,
  37: (a) => `neighbour added (LQI ${a})`,
  38: (a) => `neighbour went silent (LQI ${a})`,
  39: (a) => `neighbour removed (LQI ${a})`,
  40: () => "route discovery started",
  41: (a) => `Network Status received (${nwkStatus(a)})`,
  42: (a) => `Network Status sent (${nwkStatus(a)})`,
  43: (a) => `network key switched (sequence ${a})`,
  44: (a) => `channel changed to ${a}`,
  45: () => "PAN identifier changed",
  46: () => "address conflict",
  49: () => "Bluetooth link lost (the phone or computer went silent)",
  50: (a) => `transmission to a neighbour failed (MAC status ${hex2(a)})`,
  51: () => "identify on request",
};

export interface LogEntry { time: number; previous: boolean; code: number; arg: number; text: string }

export function decodeLog(value: Bytes): LogEntry[] {
  const dv = view(value);
  const n = Math.min(dv.getUint8(0), Math.floor((dv.byteLength - 1) / 6));
  const out: LogEntry[] = [];
  for (let i = 0; i < n; i++) {
    const o = 1 + i * 6;
    const time = dv.getUint32(o, true), code = dv.getUint8(o + 4), arg = dv.getUint8(o + 5);
    const text = EVENTS[code]?.(arg) ?? `event ${code} (${arg})`;
    out.push({ time, previous: time === 0xffffffff, code, arg, text });
  }
  return out;
}

// ---- recent ZCL frames ---------------------------------------------------------------

const CLUSTERS: Record<number, string> = {
  0x0000: "Basic", 0x0003: "Identify", 0x0004: "Groups", 0x0005: "Scenes", 0x0006: "On/Off", 0x0008: "Level",
  0x000c: "Analog Input", 0x000d: "Analog Output", 0x0010: "Binary Output", 0x0019: "OTA", 0x0300: "Color",
  0x0406: "Occupancy", 0x0b05: "Diagnostics", 0x1000: "Touchlink", 0xfc5f: "Bluetooth switch",
};
const GENERAL: Record<number, string> = {
  0x00: "Read Attributes", 0x01: "Read Attributes Response", 0x02: "Write Attributes", 0x03: "Write Attributes Undivided",
  0x04: "Write Attributes Response", 0x05: "Write Attributes No Response", 0x06: "Configure Reporting",
  0x07: "Configure Reporting Response", 0x08: "Read Reporting Configuration", 0x0a: "Report Attributes",
  0x0b: "Default Response", 0x0c: "Discover Attributes", 0x11: "Discover Commands Received",
};
const SPECIFIC: Record<number, Record<number, string>> = {
  0x0003: { 0x00: "Identify", 0x01: "Identify Query", 0x40: "Trigger Effect" },
  0x0004: { 0x00: "Add Group", 0x01: "View Group", 0x02: "Get Membership", 0x03: "Remove Group", 0x04: "Remove All Groups" },
  0x0005: { 0x00: "Add Scene", 0x01: "View Scene", 0x02: "Remove Scene", 0x04: "Store Scene", 0x05: "Recall Scene", 0x06: "Get Scene Membership" },
  0x0006: { 0x00: "Off", 0x01: "On", 0x02: "Toggle", 0x40: "Off With Effect", 0x41: "On With Recall Global Scene", 0x42: "On With Timed Off" },
  0x0008: { 0x00: "Move To Level", 0x01: "Move", 0x02: "Step", 0x03: "Stop", 0x04: "Move To Level (On/Off)",
    0x05: "Move (On/Off)", 0x06: "Step (On/Off)", 0x07: "Stop (On/Off)" },
  0x0300: { 0x00: "Move To Hue", 0x06: "Move To Hue And Saturation", 0x07: "Move To Color", 0x0a: "Move To Color Temperature",
    0x41: "Enhanced Move Hue", 0x47: "Stop Move Step", 0x4b: "Move Color Temperature", 0x4c: "Step Color Temperature" },
  0x0019: { 0x00: "Image Notify", 0x02: "Query Next Image Response", 0x05: "Image Block Response", 0x07: "Upgrade End Response" },
};

export interface Frame {
  time: number; src: string; cluster: string; command: string; manufacturer: boolean; lqi: number; rssi: number;
}

export function decodeFrames(value: Bytes): Frame[] {
  const dv = view(value);
  const n = Math.min(Math.floor(dv.getUint8(0) / 12), Math.floor((dv.byteLength - 1) / 12));
  const out: Frame[] = [];
  for (let i = 0; i < n; i++) {
    const o = 1 + i * 12;
    const cluster = dv.getUint16(o + 6, true), cmd = dv.getUint8(o + 8), fc = dv.getUint8(o + 9);
    const name = (fc & 0x03) === 1 ? SPECIFIC[cluster]?.[cmd] : GENERAL[cmd];
    out.push({
      time: dv.getUint32(o, true),
      src: hex4(dv.getUint16(o + 4, true)),
      cluster: CLUSTERS[cluster] ?? hex4(cluster),
      command: name ?? `command ${hex2(cmd)}`,
      manufacturer: !!(fc & 0x04),
      lqi: dv.getUint8(o + 10),
      rssi: dv.getInt8(o + 11),
    });
  }
  return out;
}

// ---- the Diagnostics cluster's counters ---------------------------------------------------

export interface CounterInfo {
  name: string;
  /** What it counts, in plain words. */
  what: string;
  /** What a healthy bulb shows, if there is a rule of thumb. */
  healthy?: string;
}

/** Zigbee Diagnostics cluster 0x0B05 (ZCL 8, 3.15) and Outglow's own attributes from 0x5000. */
export const COUNTERS: Record<number, CounterInfo> = {
  0x0000: { name: "Restarts", what: "Every start: power-on, watchdog or software restart.", healthy: "Grows only when the power is switched on. Jumps without that mean the bulb hangs." },
  0x0001: { name: "Settings writes", what: "Records written to flash: network settings, the light's state, the power-on counter.", healthy: "Tens a day. Hundreds an hour mean something writes in a loop." },
  0x0100: { name: "MAC broadcasts received", what: "Radio frames received for everyone, including beacons." },
  0x0101: { name: "MAC broadcasts sent", what: "Radio frames sent without asking for an acknowledgement." },
  0x0102: { name: "MAC unicasts received", what: "Radio frames addressed to this bulb." },
  0x0103: { name: "MAC unicasts sent", what: "Frames to one neighbour that went on the air at least once." },
  0x0104: { name: "MAC retries", what: "Each repeat of a frame the neighbour did not acknowledge.", healthy: "Under 10 % of unicasts sent is good, 15–30 % a busy channel, over 30 % a poor link." },
  0x0105: { name: "MAC failures", what: "Frames never acknowledged after all retries, or the channel stayed busy.", healthy: "Under 1 % of unicasts sent. More: a neighbour comes and goes, or a link on the edge." },
  0x0106: { name: "APS broadcasts received", what: "Application messages to everyone or to a group." },
  0x0107: { name: "APS broadcasts sent", what: "Application messages this bulb sent to everyone or a group." },
  0x0108: { name: "APS unicasts received", what: "Application messages to this bulb, before duplicates are dropped." },
  0x0109: { name: "APS unicasts delivered", what: "This bulb's own messages handed to the first hop." },
  0x010a: { name: "APS retries", what: "Blocks of a fragmented message sent again.", healthy: "Usually 0." },
  0x010b: { name: "APS failures", what: "This bulb's own messages that could not go out: no route, no room, a transfer abandoned.", healthy: "Usually 0." },
  0x010c: { name: "Route discoveries", what: "Route requests this bulb started.", healthy: "A few an hour. Tens a minute: routes keep breaking." },
  0x010d: { name: "Neighbours added", what: "New entries in the neighbour table.", healthy: "As many as neighbours after a start, then rarely." },
  0x010e: { name: "Neighbours removed", what: "Neighbours forgotten, replaced, left, or that became children.", healthy: "Grows when rooms are switched off at the wall." },
  0x010f: { name: "Neighbours gone silent", what: "Routers not heard for three Link Status periods.", healthy: "Growing steadily: unstable links." },
  0x0110: { name: "Joins accepted", what: "Devices that joined or rejoined through this bulb." },
  0x0111: { name: "Children moved", what: "End devices removed from this bulb's child table." },
  0x0112: { name: "NWK replays dropped", what: "Network frames with an old frame counter: repeats and copies.", healthy: "A few % of frames under heavy traffic, 0 when quiet." },
  0x0113: { name: "APS replays dropped", what: "The same for messages encrypted with a link key.", healthy: "0." },
  0x0114: { name: "APS unauthorised key", what: "Encrypted messages from a sender this bulb shares no key with.", healthy: "0. More: a stranger on the network." },
  0x0115: { name: "NWK decrypt failures", what: "Network frames that could not be decrypted.", healthy: "0. Briefly more after the network key changes." },
  0x0116: { name: "APS decrypt failures", what: "Link-key messages with a bad check value.", healthy: "0." },
  0x0117: { name: "Out of buffers", what: "Frames that found no room to wait for a route, a retry or a sleeping child.", healthy: "0. More: overloaded." },
  0x0118: { name: "Unicasts relayed", what: "Other devices' frames this bulb passed on.", healthy: "How much routing the bulb does for the network." },
  0x0119: { name: "Receive queue full", what: "Frames lost because the radio's receive queue (16) was full.", healthy: "0. More: the main loop can't keep up." },
  0x011a: { name: "Malformed frames", what: "Frames whose headers don't parse.", healthy: "0." },
  0x011b: { name: "MAC retries per message", what: "Average radio retries for this bulb's own messages.", healthy: "0 is good, 1 or more is bad." },
  0x011c: { name: "Last LQI", what: "Link quality of the frame that asked for these values." },
  0x011d: { name: "Last RSSI", what: "Signal strength of that frame, in dBm." },
  0x5000: { name: "Uptime", what: "Seconds since the last start." },
  0x5001: { name: "Restart reason", what: "0 power-on, 1 watchdog, 2 software restart (update, crash).", healthy: "1 means the bulb hung." },
  0x5002: { name: "Watchdog restarts", what: "Watchdog restarts since the last factory reset.", healthy: "0." },
  0x5003: { name: "Router neighbours", what: "Routers heard in the last three Link Status periods.", healthy: "2–6 in a typical home. 0–1: the bulb is at the edge of the network." },
  0x5004: { name: "Children", what: "End devices joined through this bulb." },
  0x5005: { name: "Relays lost: no room", what: "Relayed frames dropped for lack of room to retry or wait for a route.", healthy: "0. More: overloaded." },
  0x5006: { name: "Relays lost: no route", what: "Relayed frames dropped because no route was found.", healthy: "Grows when devices or rooms go away." },
  0x5007: { name: "Relays lost: next hop silent", what: "Relayed frames the next hop never acknowledged.", healthy: "Grows when a neighbour disappears." },
  0x5008: { name: "Relay copies dropped", what: "Copies of relayed frames recognised as repeats. Not losses." },
  0x5009: { name: "Route errors sent", what: "Network Status messages this bulb sent about broken routes or links.", healthy: "0 in a healthy network." },
  0x500a: { name: "Broadcasts dropped: table full", what: "Broadcasts dropped because the broadcast table (80) was full.", healthy: "0. More: a broadcast storm." },
  0x500b: { name: "Broadcasts not repeated", what: "Broadcasts sent once only because the queue (8) was full.", healthy: "0." },
  0x500c: { name: "Channel busy", what: "Channel checks (−85 dBm threshold) that found the channel in use.", healthy: "Grows with Wi-Fi on the same frequencies." },
  0x500d: { name: "Channel never free", what: "Sends abandoned because the channel was busy in all 5 attempts.", healthy: "0. More: strong interference." },
  0x500e: { name: "Receive queue peak", what: "Most frames ever waiting in the receive queue (of 16).", healthy: "Under 8." },
  0x500f: { name: "Sleeping-child queue peak", what: "Most frames ever held for sleeping children (of 24)." },
  0x5010: { name: "Sleeping-child frames lost", what: "Frames for sleeping children that lost or found no place.", healthy: "0." },
  0x5011: { name: "Sleeping-child frames expired", what: "Frames a child didn't collect within 7.68 s.", healthy: "A child that polls rarely." },
  0x5012: { name: "Bluetooth turns", what: "Times the radio went over to Bluetooth." },
  0x5013: { name: "Bluetooth time", what: "Milliseconds the radio spent on Bluetooth.", healthy: "Under 1 % of the uptime with only advertising." },
  0x5014: { name: "Settings erases", what: "Flash sectors erased for settings (updates not counted).", healthy: "A few a day." },
};

// ZCL type -> [bytes, signed]
const ZCL_TYPES: Record<number, [number, boolean]> = {
  0x08: [1, false], 0x10: [1, false], 0x18: [1, false], 0x20: [1, false], 0x21: [2, false], 0x22: [3, false],
  0x23: [4, false], 0x28: [1, true], 0x29: [2, true], 0x2b: [4, true], 0x30: [1, false], 0x31: [2, false],
};

export interface Counter extends CounterInfo { id: number; idText: string; manufacturer: boolean; value: number }
export interface Counters { list: Counter[]; byId: Map<number, number>; macRetryRate: number; macFailRate: number }

/** [0] format 1, then per attribute: id u16, ZCL type, value as wide as the type. */
export function decodeCounters(value: Bytes): Counters {
  const dv = view(value);
  if (dv.byteLength < 1 || dv.getUint8(0) !== 1) throw new Error("unknown counters format");
  const list: Counter[] = [];
  const byId = new Map<number, number>();
  let o = 1;
  while (o + 3 <= dv.byteLength) {
    const id = dv.getUint16(o, true), type = dv.getUint8(o + 2);
    const t = ZCL_TYPES[type];
    if (!t || o + 3 + t[0] > dv.byteLength) break;
    let v = 0;
    for (let i = t[0] - 1; i >= 0; i--) v = v * 256 + dv.getUint8(o + 3 + i);
    if (t[1] && v >= 2 ** (8 * t[0] - 1)) v -= 2 ** (8 * t[0]);
    const info = COUNTERS[id] ?? { name: hex4(id), what: "" };
    list.push({ ...info, id, idText: hex4(id), manufacturer: id >= 0x5000, value: v });
    byId.set(id, v);
    o += 3 + t[0];
  }
  const tx = byId.get(0x0103) ?? 0;
  return {
    list,
    byId,
    macRetryRate: tx ? (byId.get(0x0104) ?? 0) / tx : 0,
    macFailRate: tx ? (byId.get(0x0105) ?? 0) / tx : 0,
  };
}

export function formatDuration(s: number): string {
  const d = Math.floor(s / 86400), h = Math.floor((s % 86400) / 3600), m = Math.floor((s % 3600) / 60);
  if (d) return `${d} d ${h} h`;
  if (h) return `${h} h ${m} min`;
  if (m) return `${m} min ${s % 60} s`;
  return `${s} s`;
}
