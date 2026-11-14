// Loads z2m/outglow.mjs against the zigbee-herdsman-converters in ZHC (as
// Zigbee2MQTT does with an external converter), checks that a bulb with the
// device model the emulator dumped (examples/device_model.rs) is matched to
// it and not to a generated or built-in definition, lists its exposes, and
// feeds its fromZigbee converters a read response of the Diagnostics cluster
// (standard and manufacturer-specific attributes) and an Analog Input
// report, as zigbee-herdsman decodes them.
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
let ex = definition.exposes;
if (typeof ex === "function") ex = ex(device, {});
const props = [];
const walk = (x) => {
    if (x.property) props.push(x.property);
    for (const f of x.features ?? []) walk(f);
};
for (const x of ex) walk(x);
console.log(`exposes (${props.length}): ${props.join(" ")}`);
const generatedNames = ["analog_in_count_2", "analog_input_3", "analog_in_percentage_4", "analog_in_count_5", "analog_in_count_6",
                        "binary_output_7", "analog_output_8", "state", "brightness", "power_on_behavior", "effect", "identify"];
const missing = generatedNames.filter((n) => !props.includes(n));
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
process.exit(failed ? 1 : 0);
