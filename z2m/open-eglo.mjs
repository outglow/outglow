// Zigbee2MQTT external converter for EGLO / AwoX bulbs running the open
// Rust firmware (crates/bulb-tc32: Basic ModelIdentifier "open-eglo-dimm",
// "open-eglo-tw", "open-eglo-rgbtw", "open-eglo-rgbw" or "open-eglo-ebf",
// ManufacturerName "EGLO"). Optional: without it Zigbee2MQTT generates a
// definition from what the bulb shows (light, identify, the diagnostic values
// of endpoints 2-6, the settings of endpoints 7-8). This converter gives the
// same entities under the same names, the start-up brightness, and the
// router diagnostics of the Diagnostics cluster (ZCL 0x0B05, standard and
// manufacturer-specific under code 0x1141), laid out as:
//
// - health: a dozen plain entities to graph and alert on in Home Assistant
//   (uptime, why it last started, restarts, noise floor, neighbours, the
//   way to the coordinator, MAC retry and failure rates, the longest main
//   loop round, and `link_health`, a verdict from the thresholds of
//   docs/diagnostics.md);
// - groups (composites; one card each in the frontend): network, radio,
//   routing, aps_security, queues, bluetooth, system;
// - tables (lists): `neighbor_table` and `event_log`;
// - `diagnostics_refresh` (read everything now) and `diagnostics_clear`;
// - the black box (crates/zigbee/src/stack/blackbox.rs), not exposed and
//   never polled: setting `black_box_read` to "all" (or a page number) reads
//   its pages, each published as `black_box_page` {page, hex};
//   tools/bulb-diag.py --dump does that and decodes them.
//
// Everything is read every `diagnostics_poll_interval` seconds (default 600;
// 0: only on demand): 13 + 10 Read Attributes frames, nothing reported.
// The counters only grow and wrap (ZCL 3.15.2.2.2) and start from 0 at a
// power-on; the rates are over the time since then.
//
// Install: copy to <zigbee2mqtt data>/external_converters/ and restart.
import * as exposes from "zigbee-herdsman-converters/lib/exposes";
import * as m from "zigbee-herdsman-converters/lib/modernExtend";

const e = exposes.presets;
const ea = exposes.access;

const MFR = 0x1141;
const mfr = {manufacturerCode: MFR};
// ZCL data types
const UINT8 = 0x20;
const UINT16 = 0x21;
const UINT32 = 0x23;
const INT8 = 0x28;
const ENUM8 = 0x30;
const OCTETS = 0x41;

const hex4 = (v) => `0x${v.toString(16).padStart(4, "0")}`;
const i8 = (b) => (b > 127 ? b - 256 : b);
const RESET_CAUSES = ["power_on", "watchdog", "software"];
// Network Status codes (Zigbee R22 Table 3-50)
const NWK_STATUS = [
    "no route", "tree link failure", "non-tree link failure", "low battery", "no routing capacity",
    "no indirect capacity", "indirect expiry", "target unavailable", "target unallocated", "parent link failure",
    "validate route", "source route failure", "many-to-one failure", "address conflict", "verify addresses",
    "PAN id update", "address update", "bad frame counter", "bad key sequence", "unknown command",
];

// --- the attributes ------------------------------------------------------------------------------------
// [zigbee-herdsman name, id (manufacturer-specific only), type, group, key, description, unit, shown as]
// Group "health" values are plain entities; the others are fields of their group's composite.
const STANDARD = [
    ["numberOfResets", , , "health", "number_of_resets", "Restarts (power-on, watchdog, software)"],
    ["persistentMemoryWrites", , , "system", "persistent_memory_writes", "Records written to flash"],
    ["macRxBcast", , , "radio", "mac_rx_bcast", "MAC broadcasts received"],
    ["macTxBcast", , , "radio", "mac_tx_bcast", "MAC broadcasts sent"],
    ["macRxUcast", , , "radio", "mac_rx_ucast", "MAC unicasts received"],
    ["macTxUcast", , , "radio", "mac_tx_ucast", "MAC unicasts sent"],
    ["macTxUcastRetry", , , "radio", "mac_tx_ucast_retry", "MAC retransmissions of unicasts"],
    ["macTxUcastFail", , , "radio", "mac_tx_ucast_fail", "MAC unicasts not acknowledged after all retries"],
    ["aPSRxBcast", , , "aps_security", "aps_rx_bcast", "APS broadcasts received"],
    ["aPSTxBcast", , , "aps_security", "aps_tx_bcast", "APS broadcasts sent"],
    ["aPSRxUcast", , , "aps_security", "aps_rx_ucast", "APS unicasts received"],
    ["aPSTxUcastSuccess", , , "aps_security", "aps_tx_ucast_success", "APS unicasts sent"],
    ["aPSTxUcastRetry", , , "aps_security", "aps_tx_ucast_retry", "APS retries"],
    ["aPSTxUcastFail", , , "aps_security", "aps_tx_ucast_fail", "APS unicasts that failed"],
    ["routeDiscInitiated", , , "routing", "route_disc_initiated", "Route discoveries started"],
    ["neighborAdded", , , "routing", "neighbor_added", "Neighbour table entries added"],
    ["neighborRemoved", , , "routing", "neighbor_removed", "Neighbour table entries removed"],
    ["neighborStale", , , "routing", "neighbor_stale", "Neighbours gone silent"],
    ["joinIndication", , , "routing", "join_indication", "Devices joined or rejoined through this bulb"],
    ["childMoved", , , "routing", "child_moved", "Children removed from the child table"],
    ["nwkFcFailure", , , "aps_security", "nwk_fc_failure", "NWK frames dropped: frame counter not new"],
    ["apsFcFailure", , , "aps_security", "aps_fc_failure", "APS frames dropped: frame counter not new"],
    ["apsUnauthorizedKey", , , "aps_security", "aps_unauthorized_key", "APS frames dropped: key not authorised"],
    ["nwkDecryptFailures", , , "aps_security", "nwk_decrypt_failures", "NWK frames that did not decrypt"],
    ["apsDecryptFailures", , , "aps_security", "aps_decrypt_failures", "APS frames that did not decrypt"],
    ["packetBufferAllocateFailures", , , "queues", "packet_buffer_allocate_failures", "Frames that found no buffer"],
    ["relayedUcast", , , "routing", "relayed_ucast", "Unicasts relayed for others"],
    ["phyToMacQueueLimitReached", , , "queues", "phy_to_mac_queue_limit_reached", "Frames lost: radio receive queue full"],
    ["packetValidateDropCount", , , "queues", "packet_validate_drop_count", "Malformed frames dropped"],
    ["averageMacRetryPerApsMessageSent", , , "aps_security", "average_mac_retry_per_aps_message", "Average MAC retries per APS message (rounded)"],
    ["lastMessageLqi", , , "radio", "last_message_lqi", "Link quality of the frame that asked (the read itself)"],
    ["lastMessageRssi", , , "radio", "last_message_rssi", "Signal strength of the frame that asked", "dBm"],
];
const MANUFACTURER = [
    ["openEgloUptime", 0x5000, UINT32, "health", "uptime", "Time since the last start", "s"],
    ["openEgloResetCause", 0x5001, ENUM8, "health", "reset_cause", "Why the bulb last started", undefined, "cause"],
    ["openEgloWatchdogResets", 0x5002, UINT16, "health", "watchdog_resets", "Restarts by the watchdog (a hang)"],
    ["openEgloNeighbors", 0x5003, UINT8, "health", "neighbors", "Router neighbours heard lately"],
    ["openEgloChildren", 0x5004, UINT8, "routing", "children", "End devices joined through this bulb"],
    ["openEgloRelayDropQueueFull", 0x5005, UINT16, "routing", "relay_drop_queue_full", "Relayed frames lost: no room to keep them"],
    ["openEgloRelayDropNoRoute", 0x5006, UINT16, "routing", "relay_drop_no_route", "Relayed frames lost: no route"],
    ["openEgloRelayDropMacFail", 0x5007, UINT16, "routing", "relay_drop_mac_fail", "Relayed frames lost: next hop did not acknowledge"],
    ["openEgloRelayDropDuplicate", 0x5008, UINT16, "routing", "relay_drop_duplicate", "Copies of relayed frames dropped (replays)"],
    ["openEgloNwkStatusSent", 0x5009, UINT16, "routing", "nwk_status_sent", "Network Status reports sent"],
    ["openEgloBroadcastDropTableFull", 0x500a, UINT16, "queues", "broadcast_drop_table_full", "Broadcasts dropped: transaction table full"],
    ["openEgloBroadcastQueueFull", 0x500b, UINT16, "queues", "broadcast_queue_full", "Broadcasts sent once only: queue full"],
    ["openEgloCcaBusy", 0x500c, UINT32, "radio", "cca_busy", "Channel assessments that found the channel busy"],
    ["openEgloChannelAccessFailures", 0x500d, UINT16, "radio", "channel_access_failures", "Transmissions given up: channel busy"],
    ["openEgloRxQueuePeak", 0x500e, UINT8, "queues", "rx_queue_peak", "Most frames waiting in the radio queue (of 16)"],
    ["openEgloIndirectPeak", 0x500f, UINT8, "queues", "indirect_peak", "Most frames held for sleeping children (of 24)"],
    ["openEgloIndirectDropped", 0x5010, UINT16, "queues", "indirect_dropped", "Frames for sleeping children dropped (no room)"],
    ["openEgloIndirectExpired", 0x5011, UINT16, "queues", "indirect_expired", "Frames for sleeping children not collected in time"],
    ["openEgloBleLends", 0x5012, UINT32, "bluetooth", "ble_lends", "Times the radio was lent to Bluetooth"],
    ["openEgloBleMs", 0x5013, UINT32, "bluetooth", "ble_ms", "Time the radio was lent to Bluetooth", "ms"],
    ["openEgloFlashErases", 0x5014, UINT16, "system", "flash_erases", "Flash sector erases of the settings"],
    ["openEgloRssiBelow90", 0x5015, UINT32, "radio", "rssi_below_90", "Frames received below -90 dBm"],
    ["openEgloRssi90to80", 0x5016, UINT32, "radio", "rssi_90_to_80", "Frames received at -90..-80 dBm"],
    ["openEgloRssi80to70", 0x5017, UINT32, "radio", "rssi_80_to_70", "Frames received at -80..-70 dBm"],
    ["openEgloRssi70to60", 0x5018, UINT32, "radio", "rssi_70_to_60", "Frames received at -70..-60 dBm"],
    ["openEgloRssiAbove60", 0x5019, UINT32, "radio", "rssi_above_60", "Frames received at -60 dBm and above"],
    ["openEgloNoiseNow", 0x501a, INT8, "health", "noise_floor", "Noise floor of the channel (measured each minute, passively)", "dBm", "dbm"],
    ["openEgloNoiseMax", 0x501b, INT8, "radio", "noise_floor_max", "Highest noise floor since the start", "dBm", "dbm"],
    ["openEgloCoordNextHop", 0x5020, UINT16, "routing", "coordinator_next_hop", "First hop to the coordinator", undefined, "address"],
    ["openEgloCoordCost", 0x5021, UINT8, "routing", "coordinator_link_cost", "Link cost to that hop (1 best, 7 worst)"],
    ["openEgloCoordManyToOne", 0x5022, UINT8, "routing", "coordinator_many_to_one", "That way came from the coordinator's many-to-one request", undefined, "bool"],
    ["openEgloRoutesUsed", 0x5023, UINT8, "routing", "routes_used", "Entries in the routing table"],
    ["openEgloNwkStatusRx", 0x5024, UINT16, "routing", "nwk_status_received", "Network Status reports received"],
    ["openEgloLastStatusRx", 0x5025, UINT8, "routing", "last_nwk_status_received", "The last Network Status received", undefined, "status"],
    ["openEgloLastStatusTx", 0x5026, UINT8, "routing", "last_nwk_status_sent", "The last Network Status sent", undefined, "status"],
    ["openEgloChannel", 0x5030, UINT8, "network", "nwk_channel", "Zigbee channel"],
    ["openEgloPan", 0x5031, UINT16, "network", "nwk_pan_id", "PAN identifier", undefined, "address"],
    ["openEgloExtPan", 0x5032, OCTETS, "network", "nwk_ext_pan_id", "Extended PAN identifier", undefined, "eui64"],
    ["openEgloUpdateId", 0x5033, UINT8, "network", "nwk_update_id", "Network update identifier"],
    ["openEgloKeySeq", 0x5034, UINT8, "network", "nwk_key_seq", "Sequence number of the network key in use"],
    ["openEgloFrameCounter", 0x5035, UINT32, "network", "nwk_frame_counter", "Outgoing NWK frame counter"],
    ["openEgloDepth", 0x5036, UINT8, "network", "nwk_depth", "Depth in the network tree"],
    ["openEgloParent", 0x5037, UINT16, "network", "nwk_parent", "The device it joined through", undefined, "address"],
    ["openEgloLoopMax", 0x5040, UINT32, "system", "loop_max", "Longest main loop round (the watchdog bites at 2 s)", "µs"],
    ["openEgloBleLendMax", 0x5041, UINT32, "bluetooth", "ble_lend_max", "Longest single loan of the radio to Bluetooth", "µs"],
    ["openEgloFlashBusyMax", 0x5042, UINT32, "system", "flash_busy_max", "Longest flash sector erase (interrupts off)", "µs"],
    ["openEgloStackFree", 0x5043, UINT32, "system", "stack_free", "Main stack (and the free RAM below it) never used since the start", "B"],
    ["openEgloIrqStackFree", 0x5044, UINT16, "system", "irq_stack_free", "Interrupt stack never used since the start", "B"],
    ["openEgloBleConnections", 0x5048, UINT32, "bluetooth", "ble_connections", "Connections since the start"],
    ["openEgloBleDisconnects", 0x5049, UINT32, "bluetooth", "ble_disconnects", "Disconnections since the start"],
    ["openEgloBleLinksLost", 0x504a, UINT32, "bluetooth", "ble_links_lost", "Links lost (the central went silent)"],
    ["openEgloBleEvents", 0x504b, UINT32, "bluetooth", "ble_events", "Connection events run"],
    ["openEgloBleEventsSkipped", 0x504c, UINT32, "bluetooth", "ble_events_skipped", "Connection events missed (radio or loop busy)"],
    ["openEgloNeighbors0", 0x5050, OCTETS, "table"],
    ["openEgloNeighbors1", 0x5051, OCTETS, "table"],
    ["openEgloNeighbors2", 0x5052, OCTETS, "table"],
    ["openEgloNeighbors3", 0x5053, OCTETS, "table"],
    ["openEgloEvents0", 0x5058, OCTETS, "table"],
    ["openEgloEvents1", 0x5059, OCTETS, "table"],
    ["openEgloEvents2", 0x505a, OCTETS, "table"],
    ["openEgloEvents3", 0x505b, OCTETS, "table"],
    ["openEgloEvents4", 0x505c, OCTETS, "table"],
    ["openEgloEvents5", 0x505d, OCTETS, "table"],
];
const NEIGHBOR_PAGES = MANUFACTURER.filter((a) => a[0].startsWith("openEgloNeighbors") && a[0] !== "openEgloNeighbors").map((a) => a[0]);
const EVENT_PAGES = MANUFACTURER.filter((a) => a[0].startsWith("openEgloEvents")).map((a) => a[0]);
const BY_NAME = Object.fromEntries([...STANDARD, ...MANUFACTURER].map((a) => [a[0], a]));
const GROUPS = {
    network: "Network: the parameters to compare with the coordinator's",
    radio: "Radio and MAC: frames, retries, channel, signal strength of what is heard",
    routing: "Routing: the way to the coordinator, routes, Network Status, neighbours, relaying",
    aps_security: "APS and security: application frames, frame counters, decryption",
    queues: "Queues and buffers: what overflowed",
    bluetooth: "Bluetooth: connections and the radio time it takes",
    system: "System: flash writes and the worst-case timings",
};

// Values as the attribute gives them, shown the way that reads best.
function shown(a, v) {
    switch (a[7]) {
        case "cause":
            return RESET_CAUSES[v] ?? v;
        case "address":
            return v === 0xffff ? "none" : hex4(v);
        case "bool":
            return !!v;
        case "status":
            return NWK_STATUS[v] ?? `0x${v.toString(16)}`;
        case "eui64":
            return `0x${Buffer.from(v).reverse().toString("hex")}`;
        default:
            return v;
    }
}

// --- the tables (octet strings, crates/zigbee/src/stack/diag.rs `diag_bytes`) ---------------------------
const EVENT_TEXT = {
    1: (a, b) => `started (${RESET_CAUSES[a] ?? a}), restart #${b}`,
    2: (a, b) => `on the network as ${hex4(a)}, joined through ${hex4(b)}`,
    3: (a) => `rejoin started${a ? " (secured)" : " (unsecured)"}`,
    4: () => "left the network",
    5: (a, b) => `neighbour ${hex4(a)} added (LQI ${b})`,
    6: (a, b) => `neighbour ${hex4(a)} not heard for 3 link status periods (LQI ${b})`,
    7: (a, b) => `neighbour ${hex4(a)} removed (LQI ${b})`,
    8: (a) => `route discovery for ${hex4(a)}`,
    9: (a, b) => `Network Status received: ${NWK_STATUS[a] ?? a} about ${hex4(b)}`,
    10: (a, b) => `Network Status sent: ${NWK_STATUS[a] ?? a} about ${hex4(b)}`,
    11: (a) => `network key switched to #${a}`,
    12: (a) => `channel changed to ${a}`,
    13: (a) => `PAN id changed to ${hex4(a)}`,
    14: (a) => `address conflict on ${hex4(a)}`,
    15: (a) => `Bluetooth connected (#${a})`,
    16: (a) => `Bluetooth disconnected (#${a})`,
    17: (a) => `Bluetooth link lost (#${a})`,
    18: (a, b) => `transmission to ${hex4(a)} failed (${["ok", "no ack", "channel busy"][b] ?? b})`,
};
const EVENT_KINDS = {
    1: "start", 2: "up", 3: "rejoin", 4: "left", 5: "nb_added", 6: "nb_stale", 7: "nb_removed", 8: "route_disc",
    9: "status_rx", 10: "status_tx", 11: "key_switch", 12: "channel", 13: "pan", 14: "conflict",
    15: "ble_connect", 16: "ble_disconnect", 17: "ble_lost", 18: "tx_fail",
};

/// A page of the neighbor table: 4 entries of 15 bytes.
function neighbors(buf) {
    const out = [];
    for (let o = 0; o + 15 <= buf.length; o += 15) {
        const flags = buf[o + 5];
        if (!(flags & 1)) continue;
        out.push({
            address: hex4(buf.readUInt16LE(o)),
            lqi: buf[o + 2],
            out_cost: buf[o + 3],
            age: buf[o + 4],
            relays: !!(flags & 2),
            in_link_status: !!(flags & 4),
            child: !!(flags & 8),
            stale: buf[o + 6],
            rx: buf.readUInt16LE(o + 7),
            rssi_avg: i8(buf[o + 9]),
            rssi_min: i8(buf[o + 10]),
            tx: buf[o + 11],
            retries: buf[o + 12],
            fails: buf[o + 13],
            // R22 3.6.3.1: the cost routing uses, measured from our frames and their
            // acknowledgements once there are enough, else from the average LQI
            link_cost: (flags >> 4) & 7,
            cost_measured: !!(flags & 0x80),
            delivery: flags & 0x80 ? Math.round((100 * buf[o + 14]) / 255) : null,
        });
    }
    return out;
}

/// A page of the event log, newest first: 5 entries of 12 bytes (number, start, kind, seconds, a, b).
function events(buf) {
    const out = [];
    for (let o = 0; o + 12 <= buf.length; o += 12) {
        const start = buf[o + 2];
        const kind = buf[o + 3];
        const t = buf.readUInt32LE(o + 4);
        const a = buf.readUInt16LE(o + 8);
        const b = buf.readUInt16LE(o + 10);
        out.push({
            seq: buf.readUInt16LE(o),
            start,
            t,
            kind: EVENT_KINDS[kind] ?? kind,
            a,
            b,
            text: `#${start} +${t}s ${EVENT_TEXT[kind] ? EVENT_TEXT[kind](a, b) : `event ${kind} ${a} ${b}`}`,
        });
    }
    return out;
}

/// The log as known so far and newly read entries: each (start, number) once, newest first, the last 30.
function mergeEvents(known, fresh) {
    const key = (x) => `${x.start}/${x.seq}`;
    const all = new Map();
    for (const x of [...(known ?? []), ...fresh]) {
        if (x.seq !== undefined) all.set(key(x), x);
    }
    // newer starts first (the start number wraps at 256: within 128 of the newest counts as newer)
    const top = fresh.length ? fresh[0].start : 0;
    const age = (x) => (top - x.start + 256) % 256;
    return [...all.values()].sort((x, y) => age(x) - age(y) || y.seq - x.seq).slice(0, 30);
}

// --- health: what the counters say, the way an alert wants it ---------------------------------------------
const rate = (part, whole) => (whole ? Math.round((1000 * part) / whole) / 10 : 0);
function health(g) {
    const out = {};
    const r = g.radio ?? {};
    const route = g.routing ?? {};
    if (r.mac_tx_ucast !== undefined) {
        out.mac_retry_rate = rate(r.mac_tx_ucast_retry ?? 0, r.mac_tx_ucast);
        out.mac_fail_rate = rate(r.mac_tx_ucast_fail ?? 0, r.mac_tx_ucast);
    }
    const b = g.bluetooth ?? {};
    if (b.ble_events !== undefined) {
        out.ble_skip_rate = rate(b.ble_events_skipped ?? 0, (b.ble_events ?? 0) + (b.ble_events_skipped ?? 0));
    }
    if (route.coordinator_next_hop !== undefined) {
        const hop = route.coordinator_next_hop;
        out.coordinator_route =
            hop === "none" ? "no route" : hop === "0x0000" ? `direct, cost ${route.coordinator_link_cost}` : `via ${hop}, cost ${route.coordinator_link_cost}`;
    }
    if (g.system?.loop_max !== undefined) {
        out.loop_max_ms = Math.round(g.system.loop_max / 100) / 10;
    }
    // docs/diagnostics.md: retries < 10 % good, 10..30 % crowded, > 30 % a bad link;
    // failures < 1 %; link cost 1..3 good, up to 5 fair
    if (out.mac_retry_rate !== undefined && route.coordinator_link_cost !== undefined) {
        const cost = route.coordinator_link_cost;
        const bad = out.mac_retry_rate > 30 || out.mac_fail_rate > 5 || route.coordinator_next_hop === "none" || cost > 5;
        const fair = out.mac_retry_rate > 10 || out.mac_fail_rate > 1 || cost > 3;
        out.link_health = bad ? "poor" : fair ? "fair" : "good";
    }
    return out;
}

// the black box: pages of 64 bytes from 0x6000 (blackbox.rs BOX_FIRST, BOX_PAGES)
const BOX_FIRST = 0x6000;
const BOX_PAGES = 93;
const BOX = Array.from({length: BOX_PAGES}, (_, i) => [`openEgloBox${i}`, BOX_FIRST + i, OCTETS]);

const diagnosticCluster = m.deviceAddCustomCluster("haDiagnostic", {
    name: "haDiagnostic",
    ID: 0x0b05,
    attributes: Object.fromEntries([...MANUFACTURER, ...BOX].map(([name, ID, type]) => [name, {name, ID, type, manufacturerCode: MFR}])),
    commands: {
        openEgloClearCounters: {name: "openEgloClearCounters", ID: 0x00, parameters: [], manufacturerCode: MFR},
    },
    commandsResponse: {},
});

const chunks = (list, n) => list.reduce((a, x, i) => (i % n ? a[a.length - 1].push(x) : a.push([x]), a), []);

/// Reads every counter (in one-frame portions: 8 attributes a read) and every table page (one a read).
async function readAll(device) {
    const ep = device.getEndpoint(1);
    for (const part of chunks(STANDARD.map((a) => a[0]), 8)) {
        await ep.read("haDiagnostic", part);
    }
    for (const part of chunks(MANUFACTURER.filter((a) => a[2] !== OCTETS).map((a) => a[0]), 8)) {
        await ep.read("haDiagnostic", part, mfr);
    }
    for (const a of MANUFACTURER.filter((a) => a[2] === OCTETS)) {
        await ep.read("haDiagnostic", [a[0]], mfr);
    }
}

/// Reads the attributes behind one exposed key.
async function readKey(entity, key) {
    const list = [...STANDARD, ...MANUFACTURER].filter((a) => a[3] === key || a[4] === key);
    const std = list.filter((a) => a[1] === undefined).map((a) => a[0]);
    const man = list.filter((a) => a[1] !== undefined);
    for (const part of chunks(std, 8)) await entity.read("haDiagnostic", part);
    for (const part of chunks(man.filter((a) => a[2] !== OCTETS).map((a) => a[0]), 8)) await entity.read("haDiagnostic", part, mfr);
    for (const a of man.filter((a) => a[2] === OCTETS)) await entity.read("haDiagnostic", [a[0]], mfr);
}

const diagnostics = {
    fromZigbee: [
        {
            cluster: "haDiagnostic",
            type: ["attributeReport", "readResponse"],
            convert: (model, msg, publish, options, meta) => {
                const state = meta.state ?? {};
                const result = {};
                const groups = {};
                const group = (name) => {
                    groups[name] ??= {...(state[name] ?? {})};
                    return groups[name];
                };
                for (const [name, value] of Object.entries(msg.data)) {
                    if (name.startsWith("openEgloBox")) {
                        result.black_box_page = {page: Number(name.slice(11)), hex: Buffer.from(value).toString("hex")};
                        continue;
                    }
                    const a = BY_NAME[name];
                    if (!a || a[3] === "table") continue;
                    const v = shown(a, value);
                    if (a[3] === "health") result[a[4]] = v;
                    else group(a[3])[a[4]] = v;
                }
                NEIGHBOR_PAGES.forEach((name, page) => {
                    if (msg.data[name] === undefined) return;
                    const kept = (result.neighbor_table ?? state.neighbor_table ?? []).filter((x) => x.page !== page);
                    const list = neighbors(Buffer.from(msg.data[name])).map((x) => ({...x, page}));
                    result.neighbor_table = [...kept, ...list].sort((x, y) => x.page - y.page);
                });
                EVENT_PAGES.forEach((name) => {
                    if (msg.data[name] === undefined) return;
                    result.event_log = mergeEvents(result.event_log ?? state.event_log, events(Buffer.from(msg.data[name])));
                });
                // the last status is 0 ("no route") also when none came: the counter tells
                const r = groups.routing;
                if (r) {
                    if (r.nwk_status_received === 0) r.last_nwk_status_received = "none";
                    if (r.nwk_status_sent === 0) r.last_nwk_status_sent = "none";
                }
                Object.assign(result, groups);
                // health from the groups as they now are
                const all = Object.fromEntries(Object.keys(GROUPS).map((g) => [g, groups[g] ?? state[g]]));
                return {...result, ...health(all)};
            },
        },
    ],
    toZigbee: [
        {
            key: [
                ...[...STANDARD, ...MANUFACTURER].filter((a) => a[3] === "health").map((a) => a[4]),
                ...Object.keys(GROUPS),
                "neighbor_table",
                "event_log",
            ],
            convertGet: async (entity, key, meta) => {
                if (key === "neighbor_table") return await readPages(entity, NEIGHBOR_PAGES);
                if (key === "event_log") return await readPages(entity, EVENT_PAGES);
                await readKey(entity, key);
            },
        },
        {
            key: ["diagnostics_clear"],
            convertSet: async (entity, key, value, meta) => {
                await entity.command("haDiagnostic", "openEgloClearCounters", {}, {...mfr, disableDefaultResponse: false});
                await readAll(meta.device);
            },
        },
        {
            // the black box, page by page (page 0 first: it holds the records still until the last)
            key: ["black_box_read"],
            convertSet: async (entity, key, value, meta) => {
                const pages = value === "all" ? BOX.map((_, i) => i) : [Number(value)];
                for (const p of pages) {
                    await entity.read("haDiagnostic", [BOX[p][0]], mfr);
                }
            },
        },
        {
            key: ["diagnostics_refresh"],
            convertSet: async (entity, key, value, meta) => {
                await readAll(meta.device);
            },
        },
    ],
    exposes: [
        // health
        e.numeric("uptime", ea.STATE_GET).withUnit("s").withDescription("Time since the last start").withCategory("diagnostic"),
        e.enum("reset_cause", ea.STATE_GET, RESET_CAUSES).withDescription("Why the bulb last started").withCategory("diagnostic"),
        e.numeric("number_of_resets", ea.STATE_GET).withDescription("Restarts (power-on, watchdog, software)").withCategory("diagnostic"),
        e.numeric("watchdog_resets", ea.STATE_GET).withDescription("Restarts by the watchdog: hangs (0 is right)").withCategory("diagnostic"),
        e.enum("link_health", ea.STATE, ["good", "fair", "poor"])
            .withDescription("Verdict from the MAC retry and failure rates and the link to the coordinator (docs/diagnostics.md)")
            .withCategory("diagnostic"),
        e.numeric("mac_retry_rate", ea.STATE).withUnit("%").withDescription("MAC retransmissions per unicast sent (< 10 % good, > 30 % a bad link)").withCategory("diagnostic"),
        e.numeric("mac_fail_rate", ea.STATE).withUnit("%").withDescription("Unicasts not acknowledged after all retries (< 1 % good)").withCategory("diagnostic"),
        e.text("coordinator_route", ea.STATE).withDescription("The way to the coordinator and the cost of its first link").withCategory("diagnostic"),
        e.numeric("neighbors", ea.STATE_GET).withDescription("Router neighbours heard lately").withCategory("diagnostic"),
        e.numeric("noise_floor", ea.STATE_GET).withUnit("dBm").withDescription("Noise floor of the channel (each minute, passively)").withCategory("diagnostic"),
        e.numeric("loop_max_ms", ea.STATE).withUnit("ms").withDescription("Longest main loop round (the watchdog bites at 2000)").withCategory("diagnostic"),
        e.numeric("ble_skip_rate", ea.STATE).withUnit("%").withDescription("Bluetooth connection events missed").withCategory("diagnostic"),
        // groups
        ...Object.entries(GROUPS).map(([g, description]) => {
            let c = e.composite(g, g, ea.STATE_GET).withDescription(description).withCategory("diagnostic");
            for (const a of [...STANDARD, ...MANUFACTURER].filter((x) => x[3] === g)) {
                const f = ["address", "status", "eui64", "cause"].includes(a[7])
                    ? e.text(a[4], ea.STATE)
                    : a[7] === "bool"
                      ? e.binary(a[4], ea.STATE, true, false)
                      : e.numeric(a[4], ea.STATE);
                if (a[6] && f.withUnit) f.withUnit(a[6]);
                c = c.withFeature(f.withDescription(a[5]));
            }
            return c;
        }),
        // tables
        e.list(
            "neighbor_table",
            ea.STATE_GET,
            e
                .composite("neighbor", "neighbor", ea.STATE)
                .withFeature(e.text("address", ea.STATE).withDescription("Network address"))
                .withFeature(e.numeric("lqi", ea.STATE).withDescription("Link quality of its frames (running average)"))
                .withFeature(e.numeric("out_cost", ea.STATE).withDescription("Cost it reports for our frames (1 best, 7 worst; 0 unknown)"))
                .withFeature(e.numeric("age", ea.STATE).withDescription("Link status periods since it was heard"))
                .withFeature(e.numeric("stale", ea.STATE).withDescription("Times it went silent for 3 periods"))
                .withFeature(e.numeric("rx", ea.STATE).withDescription("Frames heard from it"))
                .withFeature(e.numeric("rssi_avg", ea.STATE).withUnit("dBm").withDescription("Signal strength, average"))
                .withFeature(e.numeric("rssi_min", ea.STATE).withUnit("dBm").withDescription("Signal strength, lowest"))
                .withFeature(e.numeric("tx", ea.STATE).withDescription("Our transmissions to it"))
                .withFeature(e.numeric("retries", ea.STATE).withDescription("Their MAC retries"))
                .withFeature(e.numeric("fails", ea.STATE).withDescription("Those not acknowledged at all"))
                .withFeature(e.binary("relays", ea.STATE, true, false).withDescription("Heard relaying broadcasts"))
                .withFeature(e.binary("in_link_status", ea.STATE, true, false).withDescription("In our last Link Status"))
                .withFeature(e.binary("child", ea.STATE, true, false).withDescription("Also our child"))
                .withFeature(e.numeric("link_cost", ea.STATE).withDescription("Link cost for routing, 1 best .. 7 (R22 3.6.3.1)"))
                .withFeature(e.binary("cost_measured", ea.STATE, true, false).withDescription("The cost is measured (else estimated from the LQI)"))
                .withFeature(e.numeric("delivery", ea.STATE).withUnit("%").withDescription("Measured share of transmission attempts acknowledged")),
        )
            .withDescription("Neighbour table: who this bulb hears and how the links go")
            .withCategory("diagnostic"),
        e.list("event_log", ea.STATE_GET, e.composite("event", "event", ea.STATE).withFeature(e.text("text", ea.STATE)))
            .withDescription("The last 30 events, newest first (#start, seconds since that start)")
            .withCategory("diagnostic"),
        e.enum("diagnostics_refresh", ea.SET, ["refresh"]).withDescription("Read every counter and table now").withCategory("config"),
        e.enum("diagnostics_clear", ea.SET, ["clear"]).withDescription("Set every counter back to 0").withCategory("config"),
    ],
    isModernExtend: true,
};

async function readPages(entity, pages) {
    for (const p of pages) await entity.read("haDiagnostic", [p], mfr);
}

const poll = m.poll({
    key: "diagnostics",
    defaultIntervalSeconds: 600,
    poll: readAll,
    option: e
        .numeric("diagnostics_poll_interval", ea.SET)
        .withValueMin(0)
        .withDescription("How often the diagnostics are read, in seconds (default 600; 0: only on demand)"),
});

// the settings cluster 0xFC5F (docs/rust-ble.md): when Bluetooth runs, with "off" that endpoint 8 cannot say
const settingsCluster = m.deviceAddCustomCluster("openEgloSettings", {
    name: "openEgloSettings",
    ID: 0xfc5f,
    manufacturerCode: MFR,
    attributes: {
        // ZCL types: 0x30 enum8, 0x21 uint16, 0x10 boolean
        bleMode: {name: "bleMode", ID: 0x0000, type: 0x30, write: true, max: 0xff},
        bleWindow: {name: "bleWindow", ID: 0x0001, type: 0x21, write: true, max: 0xffff},
        bleActive: {name: "bleActive", ID: 0x0002, type: 0x10},
    },
    commands: {},
    commandsResponse: {},
});

const bleMode = m.enumLookup({
    name: "ble_mode",
    cluster: "openEgloSettings",
    attribute: "bleMode",
    lookup: {always: 0, window_after_power_on: 1, off: 2},
    description:
        "When Bluetooth runs: always, for the window after power-on (endpoint 8), or never. " +
        "Leaving the network or a factory reset by power cycling turns it back to always.",
    zigbeeCommandOptions: mfr,
    access: "ALL",
    reporting: false,
    entityCategory: "config",
});
bleMode.configure = [];

const bleActive = m.binary({
    name: "ble_active",
    cluster: "openEgloSettings",
    attribute: "bleActive",
    valueOn: [true, 1],
    valueOff: [false, 0],
    description: "Bluetooth is running now",
    zigbeeCommandOptions: mfr,
    access: "STATE_GET",
    reporting: false,
    entityCategory: "diagnostic",
});
bleActive.configure = [];

// the entities Zigbee2MQTT generates from the bulb (tools/z2m-generated-definition.mjs), under the same names
const objects = [
    m.numeric({
        name: ["analog_in_count", "analog_input", "analog_in_percentage", "analog_in_count", "analog_in_count"],
        label: ["Relayed frames lost", "MAC retries per APS message", "Radio time lent to BLE", "Router neighbours", "End-device children"],
        valueMin: [0, 0, 0, 0, 0],
        valueMax: [16777216, 12, 100, 16, 32],
        valueStep: [1, 0.01, 0.1, 1, 1],
        cluster: "genAnalogInput",
        attribute: "presentValue",
        reporting: {min: "MIN", max: "MAX", change: 1},
        description: [
            "Relayed unicasts lost (no room, no route, next hop silent)",
            "Average MAC retries per APS message sent",
            "Share of the time since the start the radio was lent to Bluetooth",
            "Router neighbours heard lately",
            "End devices joined through this bulb",
        ],
        access: "STATE_GET",
        endpointNames: ["2", "3", "4", "5", "6"],
        unit: [undefined, undefined, "%", undefined, undefined],
        precision: [0, 2, 1, 0, 0],
        entityCategory: "diagnostic",
    }),
    m.binary({
        name: "binary_output",
        label: ["Allow end devices to join"],
        cluster: "genBinaryOutput",
        attribute: "presentValue",
        reporting: {min: "MIN", max: "MAX", change: 1},
        valueOn: ["ON", 1],
        valueOff: ["OFF", 0],
        description: ["End devices (sensors, buttons) may join through this bulb; off: none new, those there stay"],
        access: "ALL",
        endpointName: ["7"],
        entityCategory: "config",
    }),
    m.numeric({
        name: ["analog_output"],
        label: ["BLE window after power-on"],
        valueMin: [0],
        valueMax: [1440],
        valueStep: [1],
        cluster: "genAnalogOutput",
        attribute: "presentValue",
        reporting: {min: "MIN", max: "MAX", change: 1},
        description: ["Minutes after power-on Bluetooth stays on (0: always; with ble_mode off it is off and this endpoint is out of service); a power cycle opens it again"],
        access: "ALL",
        endpointNames: ["8"],
        unit: ["min"],
        precision: [0],
        entityCategory: "config",
    }),
];

const endpoints = m.deviceEndpoints({
    endpoints: {1: 1, 2: 2, 3: 3, 4: 4, 5: 5, 6: 6, 7: 7, 8: 8},
    multiEndpointSkip: ["state", "on", "off", "brightness", "level_config", "power_on_behavior", "color_temp", "color_temp_startup", "color_mode", "color_options", "color"],
});

function definition(model, description, light) {
    return {
        zigbeeModel: [model],
        model,
        vendor: "EGLO",
        description,
        extend: [endpoints, diagnosticCluster, settingsCluster, m.identify(), m.light(light), ...objects, bleMode, bleActive, diagnostics, poll],
        ota: true,
    };
}

export default [
    definition("open-eglo-dimm", "Dimmable filament bulb (AwoX TLSR8258) with the open firmware", {
        levelConfig: {features: ["current_level_startup"]},
    }),
    // the boards of this kind span 153..370 or 153..454 mireds; the bulb reports its own (ColorTempPhysicalMin/Max)
    definition("open-eglo-tw", "Tunable white bulb (AwoX TLSR8258) with the open firmware", {
        colorTemp: {range: [153, 454]},
        levelConfig: {features: ["current_level_startup"]},
    }),
    // colour bulbs (docs/stock-colour.md): hue and saturation, CIE xy, the colour temperature on the white LEDs
    definition("open-eglo-rgbtw", "Colour and tunable white bulb (AwoX TLSR8258) with the open firmware", {
        colorTemp: {range: [153, 370]},
        color: {modes: ["xy", "hs"], enhancedHue: false},
        levelConfig: {features: ["current_level_startup"]},
    }),
    // one white channel of 3000 K: a colour temperature command of 333 mireds switches to it
    definition("open-eglo-rgbw", "Colour and white bulb (AwoX TLSR8258) with the open firmware", {
        colorTemp: {range: [333, 333]},
        color: {modes: ["xy", "hs"], enhancedHue: false},
        levelConfig: {features: ["current_level_startup"]},
    }),
    // a tunable white ring and a colour strip, both lit: the strip at full value whatever the brightness
    definition("open-eglo-ebf", "Tunable white light with a colour strip (AwoX TLSR8258) with the open firmware", {
        colorTemp: {range: [153, 370]},
        color: {modes: ["xy", "hs"], enhancedHue: false},
        levelConfig: {features: ["current_level_startup"]},
    }),
];
