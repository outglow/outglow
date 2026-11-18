// Loads z2m/outglow.mjs against the zigbee-herdsman-converters in ZHC (as
// Zigbee2MQTT does with an external converter), checks that a bulb with the
// device model the emulator dumped (examples/device_model.rs) is matched to
// it and not to a generated or built-in definition, lists its exposes, and
// feeds its fromZigbee converters a read response of the Diagnostics cluster
// (standard and manufacturer-specific attributes) and an Analog Input
// report, as zigbee-herdsman decodes them; and, on a bulb with Occupancy
// Sensing, the motion sensing entities, their decoding and the switch.
//
//   node tools/z2m-converter-check.mjs model.json [z2m/outglow.mjs]

import {createRequire} from "node:module";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import {fileURLToPath} from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const ZHC = process.env.ZHC ?? path.join(os.homedir(), "oss", "zigbee-herdsman-converters");
const require = createRequire(path.join(ZHC, "package.json"));
const zhc = require(path.join(ZHC, "dist", "index.js"));
const {Zcl} = require("zigbee-herdsman");

const model = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const file = process.argv[3] ?? path.join(here, "..", "z2m", "outglow.mjs");

// the converter imports zigbee-herdsman-converters by package name: resolve it to ZHC
const text = fs.readFileSync(file, "utf8").replace(/"zigbee-herdsman-converters\/lib\/(\w+)"/g, (_, mod) => `"file://${path.join(ZHC, "dist", "lib", mod)}.js"`);
const tmp = path.join(os.tmpdir(), `z2m-check-${process.pid}.mjs`);
fs.writeFileSync(tmp, text);
const loaded = (await import(`file://${tmp}`)).default;
fs.unlinkSync(tmp);
const defs = Array.isArray(loaded) ? loaded : [loaded];
for (const d of defs) zhc.addExternalDefinition(d);

let failed = 0;
const check = (what, ok, detail = "") => {
    console.log(`${ok ? "PASS" : "FAIL"}  ${what}${detail ? `  ${detail}` : ""}`);
    if (!ok) failed++;
};

// a minimal zigbee-herdsman device with the dumped endpoints
const clusterOf = (key, device) => {
    try {
        return Zcl.Utils.getCluster(key, undefined, device?.customClusters ?? {});
    } catch {
        return undefined;
    }
};
const device = {
    ieeeAddr: model.ieeeAddr,
    networkAddress: model.networkAddress,
    type: model.type,
    manufacturerID: model.manufacturerID,
    manufacturerName: model.manufacturerName,
    modelID: model.modelID,
    softwareBuildID: model.softwareBuildID,
    powerSource: "Mains (single phase)",
    interviewCompleted: true,
    meta: {},
    customClusters: {},
    endpoints: [],
    getEndpoint(id) {
        return this.endpoints.find((e) => e.ID === id);
    },
    addCustomCluster(name, def) {
        this.customClusters[name] = def;
    },
};
device.endpoints = model.endpoints.map((e) => ({
    ID: e.ID,
    inputClusters: e.inputClusters,
    outputClusters: e.outputClusters,
    getDevice: () => device,
    getInputClusters: () => e.inputClusters.map((c) => clusterOf(c, device)).filter(Boolean),
    getOutputClusters: () => e.outputClusters.map((c) => clusterOf(c, device)).filter(Boolean),
    supportsInputCluster: (k) => e.inputClusters.includes(clusterOf(k, device)?.ID),
    supportsOutputCluster: (k) => e.outputClusters.includes(clusterOf(k, device)?.ID),
    getClusterAttributeValue: () => undefined,
}));

const definition = await zhc.findByDevice(device, true);
check("the bulb is matched to the external converter", !definition.generated && definition.model === model.modelID,
      `${definition.vendor} ${definition.model}`);
const exposesWith = (options) => (typeof definition.exposes === "function" ? definition.exposes(device, options) : definition.exposes);
const propsOf = (list) => {
    const out = [];
    const walk = (x) => {
        if (x.property) out.push(x.property);
        for (const f of x.features ?? []) walk(f);
    };
    for (const x of list) walk(x);
    return out;
};
// the level of diagnostics: summary by default, nothing at "off", everything at "full"
const summary = exposesWith({}).map((x) => x.property);
const summaryDiag = ["link_health", "coordinator_route", "neighbors", "uptime"];
check("by default the diagnostics are a summary of four", summaryDiag.every((n) => summary.includes(n)) &&
      !summary.includes("radio") && !summary.includes("neighbor_table") && !summary.includes("analog_in_count_2"),
      summary.join(" "));
const offList = exposesWith({diagnostics: "off"});
const off = [...offList.map((x) => x.property), ...propsOf(offList)];
check("diagnostics off: none of them shown", !summaryDiag.some((n) => off.includes(n)) && off.includes("state"), off.join(" "));
// the rest of the checks: at "full"
const ex = exposesWith({diagnostics: "full"});
const props = propsOf(ex);
console.log(`exposes at full (${props.length}): ${props.join(" ")}`);
const generatedNames = ["analog_in_count_2", "analog_input_3", "analog_in_percentage_4", "analog_in_count_5", "analog_in_count_6",
                        "binary_output_7", "state", "brightness", "power_on_behavior", "effect", "identify"];
const missing = generatedNames.filter((n) => !props.includes(n));
// (endpoint 8, the Bluetooth window, is the device option bluetooth_window here)
check("every entity of the generated definition, under the same name", missing.length === 0, missing.join(" "));
check("the start-up brightness", props.includes("current_level_startup"));
// health as plain entities, the rest in groups and tables
const health = ["uptime", "reset_cause", "number_of_resets", "watchdog_resets", "link_health", "mac_retry_rate", "mac_fail_rate",
                "coordinator_route", "neighbors", "noise_floor", "loop_max_ms", "ble_skip_rate"];
const top = ex.map((x) => x.property);
check("the health entities", health.every((n) => top.includes(n)), health.filter((n) => !top.includes(n)).join(" "));
const groups = ["network", "radio", "routing", "aps_security", "queues", "bluetooth", "system"];
const comp = ex.filter((x) => x.type === "composite" && groups.includes(x.property));
check("the groups, each a composite with its counters", comp.length === groups.length &&
      comp.find((x) => x.property === "radio").features.some((f) => f.property === "mac_tx_ucast_retry") &&
      comp.find((x) => x.property === "routing").features.some((f) => f.property === "relay_drop_mac_fail"),
      comp.map((x) => `${x.property}:${x.features.length}`).join(" "));
check("the tables, as lists", ex.some((x) => x.type === "list" && x.property === "neighbor_table") && ex.some((x) => x.type === "list" && x.property === "event_log"));
const diag = ex.filter((x) => [...health, ...groups, "neighbor_table", "event_log"].includes(x.property));
check("all diagnostic, none settable", diag.every((x) => x.category === "diagnostic" && (x.access & 2) === 0),
      diag.filter((x) => x.category !== "diagnostic" || x.access & 2).map((x) => x.property).join(" "));
const every = [...ex.flatMap((x) => [x.property, ...(x.features ?? []).map((f) => f.property)])];
check("every counter somewhere", ["mac_tx_ucast_retry", "relayed_ucast", "relay_drop_mac_fail", "nwk_channel", "ble_links_lost", "loop_max",
                                   "rssi_below_90", "nwk_decrypt_failures", "rx_queue_peak"].every((n) => every.includes(n)));

// the custom cluster, as the converter's start/configure step adds it
const handlers = typeof definition.onEvent === "function" ? [definition.onEvent] : (definition.onEvent ?? []);
for (const fn of handlers) {
    try {
        await fn({type: "start", data: {device, options: {diagnostics_poll_interval: 0}}});
    } catch {
        /* other handlers want more of a real device */
    }
}
const c = Zcl.Utils.getCluster("haDiagnostic", MFR(), device.customClusters);
function MFR() {
    return 0x1141;
}
check("the manufacturer-specific attributes are known under code 0x1141",
      Zcl.Utils.getClusterAttribute(c, 0x5007, 0x1141)?.name === "outglowRelayDropMacFail");

async function convert(cluster, type, data, endpointId = 1) {
    const payload = {};
    const ep = device.getEndpoint(endpointId);
    for (const fz of definition.fromZigbee) {
        if ((Array.isArray(fz.cluster) ? fz.cluster.includes(cluster) : fz.cluster === cluster) && fz.type.includes(type)) {
            const msg = {type, cluster, data, endpoint: ep, device, meta: {}, linkquality: 120, groupID: 0};
            const r = await fz.convert(definition, msg, () => {}, {}, {device, logger: console, state: {}});
            Object.assign(payload, r ?? {});
        }
    }
    return payload;
}
const p = await convert("haDiagnostic", "readResponse",
                        {macTxUcastRetry: 42, relayedUcast: 7, lastMessageRssi: -61, outglowUptime: 3600, outglowResetCause: 1, outglowRelayDropMacFail: 3});
check("a read response of the Diagnostics cluster becomes state", p.radio?.mac_tx_ucast_retry === 42 && p.routing?.relayed_ucast === 7 &&
      p.radio?.last_message_rssi === -61 && p.uptime === 3600 && p.reset_cause === "watchdog" && p.routing?.relay_drop_mac_fail === 3, JSON.stringify(p));
const hp = await convert("haDiagnostic", "readResponse", {macTxUcast: 200, macTxUcastRetry: 30, macTxUcastFail: 1, outglowCoordNextHop: 0,
                                                         outglowCoordCost: 2, outglowLastStatusRx: 0, outglowPan: 0x1a62});
check("health from the counters", hp.mac_retry_rate === 15 && hp.mac_fail_rate === 0.5 && hp.link_health === "fair" &&
      hp.coordinator_route === "direct, cost 2" && hp.routing?.last_nwk_status_received === "no route" && hp.network?.nwk_pan_id === "0x1a62",
      JSON.stringify(hp));

// the tables (crates/zigbee/src/stack/diag.rs diag_bytes): a neighbor page and an event page as the bulb sends them
const nb = Buffer.alloc(60);
// 0x1a2b: LQI 200, cost 1, age 0, in use + relays, stale 3, heard 120, -71/-88 dBm, tx 40, 5 retries, 1 failure
// flags: in use, relays, link cost 2, measured; delivery 230/255
Buffer.from([0x2b, 0x1a, 200, 1, 0, 0x03 | (2 << 4) | 0x80, 3, 120, 0, 0xb9, 0xa8, 40, 5, 1, 230]).copy(nb, 0);
const evp = Buffer.alloc(24);
// newest first: #1 start 2 at 15 s a neighbor went stale (0x1a2b, LQI 80); #0 start 2 began (watchdog, resets 2)
Buffer.from([1, 0, 2, 6, 15, 0, 0, 0, 0x2b, 0x1a, 80, 0]).copy(evp, 0);
Buffer.from([0, 0, 2, 1, 0, 0, 0, 0, 1, 0, 2, 0]).copy(evp, 12);
const tb = await convert("haDiagnostic", "readResponse", {outglowNeighbors0: nb, outglowEvents0: evp, outglowExtPan: Buffer.from([1, 2, 3, 4, 5, 6, 7, 8])});
const n0 = tb.neighbor_table?.[0];
check("a neighbor table page becomes neighbor_table",
      tb.neighbor_table?.length === 1 && n0.address === "0x1a2b" && n0.lqi === 200 && n0.relays && n0.stale === 3 && n0.rx === 120 &&
          n0.rssi_avg === -71 && n0.rssi_min === -88 && n0.tx === 40 && n0.retries === 5 && n0.fails === 1 &&
          n0.link_cost === 2 && n0.cost_measured && n0.delivery === 90,
      JSON.stringify(n0));
check("an event log page becomes event_log, in words",
      tb.event_log?.length === 2 && tb.event_log[0].kind === "nb_stale" && tb.event_log[0].a === 0x1a2b && /not heard/.test(tb.event_log[0].text) &&
          /started \(watchdog\)/.test(tb.event_log[1].text),
      tb.event_log?.map((x) => x.text).join(" | "));
check("the extended PAN identifier", tb.network?.nwk_ext_pan_id === "0x0807060504030201", tb.network?.nwk_ext_pan_id);
// the next page read after two new events: it repeats #1 and #0, moved along; each stays once
const evp2 = Buffer.alloc(24);
Buffer.from([1, 0, 2, 6, 15, 0, 0, 0, 0x2b, 0x1a, 80, 0]).copy(evp2, 0);
Buffer.from([0, 0, 2, 1, 0, 0, 0, 0, 1, 0, 2, 0]).copy(evp2, 12);
const evp3 = Buffer.alloc(24);
Buffer.from([3, 0, 2, 4, 20, 0, 0, 0, 0, 0, 0, 0]).copy(evp3, 0);
Buffer.from([2, 0, 2, 3, 19, 0, 0, 0, 1, 0, 0, 0]).copy(evp3, 12);
const fz = definition.fromZigbee.find((x) => x.cluster === "haDiagnostic");
const ep = device.getEndpoint(1);
const run = (data, state) => fz.convert(definition, {type: "readResponse", cluster: "haDiagnostic", data, endpoint: ep, device, meta: {}}, () => {}, {}, {device, logger: console, state});
const s1 = run({outglowEvents0: evp3}, {});
const s2 = run({outglowEvents1: evp2}, s1);
check("the event log pages overlap: every event once, in order", s2.event_log?.map((x) => x.seq).join(",") === "3,2,1,0",
      s2.event_log?.map((x) => x.seq).join(","));
const q = await convert("genAnalogInput", "attributeReport", {presentValue: 2}, 5);
check("an Analog Input report of endpoint 5 becomes analog_in_count_5", q.analog_in_count_5 === 2, JSON.stringify(q));
const bx = await convert("haDiagnostic", "readResponse", {outglowBox3: Buffer.from([1, 2, 0xff])});
check("a black box page is published as hex", bx.black_box_page?.page === 3 && bx.black_box_page?.hex === "0102ff", JSON.stringify(bx.black_box_page));
check("black_box_read can be set", Boolean(definition.toZigbee.find((x) => x.key?.includes("black_box_read"))?.convertSet));
const t = definition.toZigbee.find((x) => x.key?.includes("diagnostics_clear"));
check("diagnostics_clear can be set", Boolean(t?.convertSet));
// Bluetooth as device options, written when they change
const optNames = (definition.options ?? []).map((o) => o.property ?? o.name);
check("Bluetooth and the diagnostics level are device options", ["bluetooth", "bluetooth_window", "diagnostics"].every((n) => optNames.includes(n)),
      optNames.join(" "));
check("no Bluetooth entities any more", !propsOf(exposesWith({diagnostics: "full"})).some((n) => ["ble_mode", "ble_active", "analog_output_8"].includes(n)));

// motion sensing: in every image, off until switched on (crates/zigbee/src/stack/sense.rs)
const SENSE_KEYS = ["motion_sensing", "occupancy", "occupancy_timeout", "motion_sensitivity", "motion_probes", "motion_auto_calibration",
                    "motion_self_tuning", "motion_calibrate", "motion_calibration_left", "motion_links", "room_occupancy"];
const hasOcc = model.endpoints.some((x) => x.ID === 1 && x.inputClusters.includes(0x0406));
const senseEx = exposesWith({});
const byName = Object.fromEntries(senseEx.map((x) => [x.property ?? x.name, x]));
if (hasOcc) {
    check("the motion sensing entities on a bulb with Occupancy Sensing", SENSE_KEYS.every((k) => byName[k]),
          SENSE_KEYS.filter((k) => !byName[k]).join(" "));
    const cat = (k) => byName[k]?.category;
    check("motion_sensing is the main control, a switch", byName.motion_sensing?.type === "binary" && cat("motion_sensing") === undefined &&
          byName.motion_sensing.value_on === "ON" && (byName.motion_sensing.access & 7) === 7);
    check("the settings are config, the links and the countdown diagnostic",
          ["occupancy_timeout", "motion_sensitivity", "motion_probes", "motion_auto_calibration", "motion_self_tuning", "motion_calibrate"].every((k) => cat(k) === "config") &&
              cat("motion_links") === "diagnostic" && cat("motion_calibration_left") === "diagnostic" && byName.motion_links.type === "list");
    check("every motion entity says what it does", SENSE_KEYS.every((k) => (byName[k]?.description ?? "").length > 20));
    const sc = Zcl.Utils.getCluster("outglowSense", 0x1141, device.customClusters);
    check("the sensing cluster's attributes are known under code 0x1141",
          sc.ID === 0xfc5e && Zcl.Utils.getClusterAttribute(sc, 0xff46, 0x1141)?.name === "lineState" && Zcl.Utils.getClusterAttribute(sc, 0x0002, 0x1141)?.name === "lines");
    const off = await convert("outglowSense", "readResponse", {probeRate: 0, sensitivity: 40});
    const on = await convert("outglowSense", "readResponse", {probeRate: 5});
    check("the rate reads as the switch", off.motion_sensing === "OFF" && off.motion_probes === 0 && off.occupancy === false && off.room_occupancy === false &&
          off.motion_sensitivity === 40 &&
          on.motion_sensing === "ON" && on.motion_probes === 5, `${JSON.stringify(off)} ${JSON.stringify(on)}`);
    // two links: 0x1a2b at -57 dBm, noise 1.5 dB, still; 0x3c4d at -71 dBm in slot 3, ON
    const lines = Buffer.from([0x2b, 0x1a, 0xc7, 12, 2, 0x00, 0x4d, 0x3c, 0xb9, 20, 30, 0x31]);
    const sens = Buffer.from([0x2b, 0x1a, 38, 38, 0x4d, 0x3c, 31, 33]);
    const lk = await convert("outglowSense", "readResponse", {lines, lineSensitivity: sens});
    const [l0, l1] = lk.motion_links ?? [];
    check("the links decode", lk.motion_links?.length === 2 && l0.address === "0x1a2b" && l0.signal === -57 && l0.noise === 1.5 && !l0.motion &&
          l0.sensitivity === 38 && l1.address === "0x3c4d" && l1.motion && l1.slot === 3 && l1.calibrated_sensitivity === 33, JSON.stringify(lk.motion_links));
    const fzs = definition.fromZigbee.filter((x) => x.cluster === "outglowSense");
    let st = {};
    for (const f of fzs) Object.assign(st, f.convert(definition, {type: "attributeReport", cluster: "outglowSense", data: {zone: 0, lineState: 0}, endpoint: ep, device, meta: {}}, () => {}, {}, {device, logger: console, state: lk}) ?? {});
    check("a report of the line state updates the links", st.motion_links?.length === 2 && st.motion_links.every((l) => !l.motion), JSON.stringify(st.motion_links));
    // the room: motion on a link to one of the bulbs named in motion_room (IEEE addresses), without it on any link
    const known = [{address: "0x1a2b", ieee_address: "0x00124b0000001a2b", slot: 0, motion: false},
                   {address: "0x3c4d", ieee_address: "0x00124b0000003c4d", slot: 3, motion: false}];
    const roomRun = (options, lineState) => {
        const o = {};
        for (const f of fzs) {
            Object.assign(o, f.convert(definition, {type: "attributeReport", cluster: "outglowSense", data: {zone: lineState ? 1 : 0, lineState}, endpoint: ep, device, meta: {}},
                                       () => {}, options, {device, logger: console, state: {motion_links: known}}) ?? {});
        }
        return o.room_occupancy;
    };
    const rooms = [roomRun({}, 1 << 3), roomRun({motion_room: "0x00124B0000003C4D"}, 1 << 3), roomRun({motion_room: "0x00124b0000001a2b, 00124b000000ffff"}, 1 << 3),
                   roomRun({motion_room: "0x00124b0000003c4d"}, 0)];
    check("room_occupancy is motion on a link to a bulb of the room", rooms.join() === "true,true,false,false", rooms.join());
    check("motion_room is a device option", (definition.options ?? []).some((o) => (o.property ?? o.name) === "motion_room"));
    // Home Assistant: the links as attributes and how many see motion as the state; the room as an occupancy sensor
    const ha = {value_template: `{{ value_json["motion_links"] | default('',True) | string | truncate(254, True, '', 0) }}`, state_topic: "zigbee2mqtt/K1"};
    definition.meta?.overrideHaDiscoveryPayload?.(ha, {});
    check("motion_links in Home Assistant: every link as an attribute, the links seeing motion counted as the state",
          ha.json_attributes_topic === "zigbee2mqtt/K1" && /value_json\.motion_links/.test(ha.json_attributes_template ?? "") && /tojson/.test(ha.json_attributes_template ?? "") &&
              /selectattr\('motion'\)/.test(ha.value_template) && !/truncate/.test(ha.value_template),
          JSON.stringify(ha));
    const hr = {value_template: `{% if value_json["room_occupancy"] %}true{% else %}false{% endif %}`, state_topic: "zigbee2mqtt/K1"};
    definition.meta?.overrideHaDiscoveryPayload?.(hr, {});
    const ho = {value_template: `{% if value_json["occupancy"] %}true{% else %}false{% endif %}`, state_topic: "zigbee2mqtt/K1"};
    definition.meta?.overrideHaDiscoveryPayload?.(ho, {});
    check("room_occupancy is an occupancy sensor in Home Assistant, nothing else changes", hr.device_class === "occupancy" && ho.device_class === undefined &&
          ho.json_attributes_topic === undefined, JSON.stringify([hr, ho]));
    // the activity of every link (0xFF47: sender, level), sent a few times a second while motion is seen
    check("the activity attributes are known under code 0x1141",
          Zcl.Utils.getClusterAttribute(sc, 0xff47, 0x1141)?.name === "lineActivity" && Zcl.Utils.getClusterAttribute(sc, 0x0007, 0x1141)?.name === "activityRate");
    let act = {};
    for (const f of fzs) {
        Object.assign(act, f.convert(definition, {type: "attributeReport", cluster: "outglowSense", data: {lineActivity: Buffer.from([0x2b, 0x1a, 37, 0x4d, 0x3c, 0]), lineState: 1}, endpoint: ep, device, meta: {}},
                                     () => {}, {}, {device, logger: console, state: lk}) ?? {});
    }
    const [a0, a1] = act.motion_links ?? [];
    check("a report of the activity sets each link's activity and motion", a0?.activity === 37 && a0.motion && a1?.activity === 0 && !a1.motion && a0.signal === -57,
          JSON.stringify(act.motion_links));
    check("motion_activity_rate is a device option", (definition.options ?? []).some((o) => (o.property ?? o.name) === "motion_activity_rate"));
    const actWrites = [];
    const actDevice = {getEndpoint: () => ({write: async (c, a, o) => actWrites.push([c, a, o]), supportsInputCluster: () => true}), isDummyDevice: false};
    const handlers = definition.onEvent ? [definition.onEvent] : [];
    for (const h of handlers) {
        await h({type: "deviceOptionsChanged", data: {device: actDevice, from: {}, to: {motion_activity_rate: 9}, deviceExposesChanged: () => {}}});
        await h({type: "deviceAnnounce", data: {device: actDevice, options: {motion_activity_rate: 2}}});
    }
    const rates = actWrites.filter((w) => w[0] === "outglowSense").map((w) => w[1].activityRate);
    check("the option writes the activity rate, at most 5, when it changes and when the bulb comes back", rates.includes(5) && rates.includes(2), JSON.stringify(actWrites));
    // the switch writes the rate
    const writes = [];
    const fakeEp = {write: async (c, a, o) => writes.push([c, a, o]), read: async () => {}};
    const tzOf = (k) => definition.toZigbee.find((x) => x.key?.includes(k));
    const r1 = await tzOf("motion_sensing").convertSet(fakeEp, "motion_sensing", "ON", {state: {motion_probes: 0}, device, mapped: definition, options: {}, message: {}});
    const r2 = await tzOf("motion_sensing").convertSet(fakeEp, "motion_sensing", "OFF", {state: {motion_probes: 5}, device, mapped: definition, options: {}, message: {}});
    check("motion_sensing ON writes 5 probes/s, OFF writes 0 and clears occupancy",
          writes[0]?.[0] === "outglowSense" && writes[0][1].probeRate === 5 && writes[0][2].manufacturerCode === 0x1141 && writes[1]?.[1].probeRate === 0 &&
              r1.state.motion_sensing === "ON" && r2.state.occupancy === false && r2.state.room_occupancy === false, JSON.stringify(writes));
    let refused = false;
    try {
        await tzOf("motion_calibrate").convertSet(fakeEp, "motion_calibrate", "start", {state: {motion_sensing: "OFF"}, device, mapped: definition, options: {}, message: {}});
    } catch (err) {
        refused = /motion_sensing on/.test(err.message);
    }
    check("a calibration with the sensing off says to switch it on", refused);
    // a bulb without Occupancy Sensing (built with --no-sense) shows none of it
    const ep1 = device.endpoints.find((x) => x.ID === 1);
    const saved = ep1.supportsInputCluster;
    ep1.supportsInputCluster = (k) => (clusterOf(k, device)?.ID === 0x0406 ? false : saved(k));
    const none = exposesWith({}).map((x) => x.property ?? x.name);
    ep1.supportsInputCluster = saved;
    check("no motion entity on a bulb without Occupancy Sensing", !SENSE_KEYS.some((k) => none.includes(k)), none.filter((k) => SENSE_KEYS.includes(k)).join(" "));
} else {
    check("no motion entity on a bulb without Occupancy Sensing", !SENSE_KEYS.some((k) => byName[k]));
}

process.exit(failed ? 1 : 0);

