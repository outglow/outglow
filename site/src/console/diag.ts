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
  /** Context for interpreting the reading; not a diagnosis from a single value. */
  healthy?: string;
}

/** Zigbee Diagnostics cluster 0x0B05 (ZCL 8, 3.15) and Outglow's own attributes from 0x5000. */
export const COUNTERS: Record<number, CounterInfo> = {
  0x0000: {"name": "Restarts", "what": "Starts caused by power-on, a software restart or the watchdog, which restarts the chip if the software stops responding.", "healthy": "Compare unexpected increases with the last restart reason; an update or manual restart also increases this count."},
  0x0001: {"name": "Settings saved", "what": "Records written to permanent storage, including network settings, light state and power-on history.", "healthy": "Repeated increases while nothing changes are worth investigating; this count alone does not identify the cause."},
  0x0100: {"name": "Radio broadcasts received", "what": "Radio messages addressed to many devices, including announcements that help devices find a network."},
  0x0101: {"name": "Radio broadcasts sent", "what": "Radio messages sent without asking each receiver to confirm receipt."},
  0x0102: {"name": "Direct radio messages received", "what": "Radio messages addressed specifically to this bulb."},
  0x0103: {"name": "Direct radio messages sent", "what": "Messages to one neighbour that were transmitted at least once. A transmission does not prove delivery."},
  0x0104: {"name": "Radio retries", "what": "Extra attempts after a neighbour did not confirm receipt.", "healthy": "Watch for a rise relative to traffic. Interference, weak reception and a missing neighbour can all cause retries."},
  0x0105: {"name": "Radio deliveries failed", "what": "Messages that exhausted their retries without confirmation, or could not be sent because the channel stayed busy.", "healthy": "Look for a rising count when a device becomes unreachable, then check nearby routers and radio activity."},
  0x0106: {"name": "Group or network commands received", "what": "Complete commands or reports addressed to a group or to the whole network."},
  0x0107: {"name": "Group or network commands sent", "what": "Complete messages this bulb sent to a group or to the whole network."},
  0x0108: {"name": "Direct commands received", "what": "Complete messages addressed to this bulb, counted before repeated copies are removed."},
  0x0109: {"name": "Own messages handed on", "what": "Messages from this bulb passed to the first device on their route. This is not confirmation of arrival at the final destination."},
  0x010a: {"name": "Long-message parts retried", "what": "Parts of a split message sent again.", "healthy": "A rise suggests that a transfer needed extra delivery attempts."},
  0x010b: {"name": "Own messages not sent", "what": "Messages from this bulb that could not be sent because no route or storage was available, or a transfer was abandoned.", "healthy": "Check route and queue readings when this rises."},
  0x010c: {"name": "Route searches started", "what": "Searches initiated by this bulb to find a path through the network.", "healthy": "Frequent new searches can accompany disappearing routers or changing connections."},
  0x010d: {"name": "Nearby devices added", "what": "New entries added to the bulb’s list of nearby devices.", "healthy": "An increase is expected after startup or when devices join or move nearby."},
  0x010e: {"name": "Nearby devices removed", "what": "Entries removed or replaced in the bulb’s list of nearby devices.", "healthy": "Compare changes with devices being powered off, removed or changing their network role."},
  0x010f: {"name": "Routers gone silent", "what": "Nearby routers whose regular connection reports have been missing beyond the allowed waiting period.", "healthy": "Repeated increases can point to intermittent power or reception."},
  0x0110: {"name": "Joins accepted", "what": "Devices that joined or returned to the network through this bulb."},
  0x0111: {"name": "End-device connections removed", "what": "Devices removed from the list of those using this bulb as their parent router."},
  0x0112: {"name": "Old network messages rejected", "what": "Protected network messages rejected because their message number was not newer than the last one accepted.", "healthy": "Repeated radio traffic can increase this count; it is not by itself evidence of an attack."},
  0x0113: {"name": "Old private messages rejected", "what": "Messages protected by a device-to-device key rejected because their message number was not newer than the last one accepted.", "healthy": "Repeated messages or a disagreement about saved state may need investigation."},
  0x0114: {"name": "Messages with no shared key", "what": "Encrypted messages from a device for which the bulb has no matching private key.", "healthy": "Check pairing and key setup. This counter alone does not establish who sent the message or why."},
  0x0115: {"name": "Network decryption failures", "what": "Network messages that could not be decrypted or verified.", "healthy": "Check for recent key changes and whether other devices are also losing contact."},
  0x0116: {"name": "Private-message verification failures", "what": "Messages protected by a device-to-device key that failed their integrity check.", "healthy": "Check key agreement and device state if this keeps increasing."},
  0x0117: {"name": "Messages lost for lack of storage", "what": "Messages that found no space while waiting for a route, a retry or a sleeping battery device.", "healthy": "A rising count shows that waiting traffic exceeded the bulb’s storage capacity."},
  0x0118: {"name": "Messages forwarded for others", "what": "Messages this bulb passed along on behalf of other devices.", "healthy": "This shows how much forwarding work the bulb is doing, not how many messages reached their final destination."},
  0x0119: {"name": "Incoming messages lost: queue full", "what": "Messages lost because the bulb’s incoming-message queue had no free space.", "healthy": "A rising count means messages arrived faster than the bulb could process them."},
  0x011a: {"name": "Malformed radio messages", "what": "Messages whose opening fields could not be interpreted as a valid packet.", "healthy": "A rising count calls for closer inspection of the traffic; it does not identify the cause on its own."},
  0x011b: {"name": "Average retries per own message", "what": "Average number of radio retries for messages originating from this bulb.", "healthy": "Compare the trend over time; more retries mean more airtime used for the same traffic."},
  0x011c: {"name": "Last reception quality", "what": "The chip’s reception-quality score for the latest recorded incoming Zigbee message, on a scale from zero to 255. A direct Zigbee read refreshes this; reading over Bluetooth does not.", "healthy": "A larger score means better reception of that one message, not a measurement of the whole route."},
  0x011d: {"name": "Last signal strength", "what": "Signal strength of the latest recorded incoming Zigbee message, in dBm. A more negative reading means a weaker signal.", "healthy": "Compare readings over time; signal strength alone does not establish successful delivery."},
  0x5000: {"name": "Time since restart", "what": "Seconds since the bulb last started."},
  0x5001: {"name": "Last restart reason", "what": "The recorded cause: zero means power-on, one means the watchdog restarted unresponsive software, and two means a software-requested restart.", "healthy": "A software restart may follow an update, a manual command or error recovery. Check the event log for context."},
  0x5002: {"name": "Watchdog restarts", "what": "Restarts triggered because the software stopped responding, counted since the diagnostics were last cleared or factory-reset.", "healthy": "Repeated increases deserve investigation even if the bulb reconnects afterwards."},
  0x5003: {"name": "Nearby routers", "what": "Routers heard recently enough to remain usable neighbours.", "healthy": "Few neighbours can mean fewer alternative paths, but the number alone does not establish a fault."},
  0x5004: {"name": "Connected end devices", "what": "Devices using this bulb as their parent router. They are often battery-powered sensors or remotes; some stay awake, while sleeping devices need the router to hold messages for them."},
  0x5005: {"name": "Forwarding losses: no storage", "what": "Other devices’ messages dropped because there was no room to wait for a route or retry.", "healthy": "A rise means waiting traffic exceeded the available storage."},
  0x5006: {"name": "Forwarding losses: no route", "what": "Other devices’ messages dropped because the bulb could not find a route.", "healthy": "Compare increases with routers or whole rooms being powered off."},
  0x5007: {"name": "Forwarding losses: no receipt", "what": "Other devices’ messages that the next device on the route never confirmed receiving.", "healthy": "Check whether that neighbour has gone away or become hard to reach."},
  0x5008: {"name": "Repeated forwarded messages dropped", "what": "Copies recognised as messages already seen. Discarding these prevents needless forwarding.", "healthy": "An increase is not itself a delivery loss; repeated copies are expected in some network exchanges."},
  0x5009: {"name": "Delivery failures reported", "what": "Failure reports sent to tell another device that a route or connection could not deliver its message.", "healthy": "These reports help the sender recover. Check what caused them rather than treating the reports themselves as a fault."},
  0x500a: {"name": "Broadcasts lost: tracking full", "what": "Network-wide messages dropped because no space remained to track them.", "healthy": "A rise can accompany a large burst of network-wide traffic."},
  0x500b: {"name": "Broadcasts without retries", "what": "Network-wide messages sent once but not queued for further attempts because the retry queue was full.", "healthy": "A neighbour that missed the first transmission may not get another chance to receive it."},
  0x500c: {"name": "Radio channel found busy", "what": "Checks that found the radio channel occupied before transmission.", "healthy": "Other Zigbee devices, Wi-Fi or other radio activity can contribute; this reading does not identify the source."},
  0x500d: {"name": "Sends abandoned: channel busy", "what": "Messages that could not be sent after all attempts to find a free channel.", "healthy": "A rise means the bulb was unable to obtain airtime, not necessarily that the destination was absent."},
  0x500e: {"name": "Largest incoming queue", "what": "The highest number of messages waiting to be processed at once.", "healthy": "Read alongside incoming-message losses to see whether bursts are filling the queue."},
  0x500f: {"name": "Largest sleeping-device queue", "what": "The highest number of messages held at once for battery devices that were asleep."},
  0x5010: {"name": "Sleeping-device messages lost", "what": "Messages for sleeping battery devices dropped because storage was unavailable.", "healthy": "A growing count can explain commands that never reach a battery device when it wakes."},
  0x5011: {"name": "Sleeping-device messages expired", "what": "Messages removed because a sleeping battery device did not collect them before the waiting period ended.", "healthy": "Check how often that device wakes and whether it is still connected through this bulb."},
  0x5012: {"name": "Bluetooth radio sessions", "what": "Times the shared radio switched from Zigbee to Bluetooth."},
  0x5013: {"name": "Time spent on Bluetooth", "what": "Milliseconds the shared radio spent using Bluetooth instead of listening for Zigbee.", "healthy": "A Bluetooth connection or update increases this. Compare it with total running time when investigating missed Zigbee traffic."},
  0x5014: {"name": "Settings-storage erases", "what": "Permanent-storage regions erased while saving settings; firmware-update storage is not counted.", "healthy": "Frequent erases without corresponding setting changes are worth investigating."},
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
