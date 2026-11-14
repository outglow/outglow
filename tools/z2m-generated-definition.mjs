// What Zigbee2MQTT makes of a bulb it has no definition for: the device
// model the emulator dumps (emulator/rs/scenarios/examples/device_model.rs)
// behind a mocked zigbee-herdsman Device/Endpoint, handed to the real
// zigbee-herdsman-converters (findByDevice with generateForUnknown, which is
// what Zigbee2MQTT calls; generateDefinition.ts makes the definition). Prints
// the matched or generated definition, its exposes and the generated source;
// with --compare FILE.mjs (an external converter, read only) also that
// converter's exposes and what one has that the other has not.
//
//   node tools/z2m-generated-definition.mjs model.json [--compare converter.mjs ...] [--json]
//
// ZHC: checkout of zigbee-herdsman-converters with dist/ built and
// node_modules installed (default ~/oss/zigbee-herdsman-converters).

import {createRequire} from "node:module";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const ZHC = process.env.ZHC ?? path.join(os.homedir(), "oss", "zigbee-herdsman-converters");
const require = createRequire(path.join(ZHC, "package.json"));
const zhc = require(path.join(ZHC, "dist", "index.js"));
const {Zcl} = require("zigbee-herdsman");

const args = process.argv.slice(2);
const modelPath = args[0];
const compare = [];
for (let i = 1; i < args.length; i++) {
    if (args[i] === "--compare") compare.push(args[++i]);
}
const asJson = args.includes("--json");
const model = JSON.parse(fs.readFileSync(modelPath, "utf8"));

const POWER = {0: "Unknown", 1: "Mains (single phase)", 2: "Mains (3 phase)", 3: "Battery", 4: "DC Source"};

function clusterOf(key) {
    try {
        return Zcl.Utils.getCluster(key, undefined, {});
    } catch {
        return undefined;
    }
}

class Endpoint {
    constructor(device, e) {
        this.ID = e.ID;
        this.profileID = e.profileID;
        this.deviceID = e.deviceID;
        this.inputClusters = e.inputClusters;
        this.outputClusters = e.outputClusters;
        this.deviceIeeeAddress = device.ieeeAddr;
        this.deviceNetworkAddress = device.networkAddress;
        this.attrs = e.attributes ?? {};
        this.device = device;
        this.binds = [];
        this.configuredReportings = [];
        this.clusters = {};
        this.reads = [];
    }
    getDevice() {
        return this.device;
    }
    getInputClusters() {
        return this.inputClusters.map((c) => clusterOf(c)).filter((c) => c && !c.name.match(/^\d+$/));
    }
    getOutputClusters() {
        return this.outputClusters.map((c) => clusterOf(c)).filter((c) => c && !c.name.match(/^\d+$/));
    }
    supportsInputCluster(key) {
        const c = clusterOf(key);
        return c !== undefined && this.inputClusters.includes(c.ID);
    }
    supportsOutputCluster(key) {
        const c = clusterOf(key);
        return c !== undefined && this.outputClusters.includes(c.ID);
    }
    attributeValue(cluster, attribute) {
        const c = clusterOf(cluster);
        if (!c) return undefined;
        const a = typeof attribute === "number" ? Object.values(c.attributes).find((x) => x.ID === attribute) : c.attributes[attribute];
        if (!a) return undefined;
        return this.attrs[String(c.ID)]?.[String(a.ID)]?.value;
    }
    getClusterAttributeValue(cluster, attribute) {
        return this.attributeValue(cluster, attribute);
    }
    // what the generator reads that it did not find in the cache
    async read(cluster, attributes) {
        const out = {};
        for (const a of attributes) {
            this.reads.push(`${cluster}.${a}`);
            const v = this.attributeValue(cluster, a);
            if (v === undefined) throw new Error(`UNSUPPORTED_ATTRIBUTE ${cluster}.${a}`);
            out[a] = v;
        }
        return out;
    }
}

const device = {
    ieeeAddr: model.ieeeAddr,
    networkAddress: model.networkAddress,
    type: model.type,
    manufacturerID: model.manufacturerID,
    manufacturerName: model.manufacturerName,
    modelID: model.modelID,
    softwareBuildID: model.softwareBuildID,
    dateCode: model.dateCode,
    zclVersion: model.zclVersion,
    applicationVersion: model.applicationVersion,
    stackVersion: model.stackVersion,
    hardwareVersion: model.hardwareVersion,
    powerSource: POWER[model.powerSource] ?? "Unknown",
    interviewCompleted: true,
    interviewState: "SUCCESSFUL",
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
device.endpoints = model.endpoints.map((e) => new Endpoint(device, e));

/// The exposes of a definition for this device: [{name, type, access, unit, min, max, features}].
function exposesOf(definition) {
    let ex = definition.exposes;
    if (typeof ex === "function") ex = ex(device, {});
    const flat = (e) => ({
        type: e.type,
        name: e.property ?? e.name,
        endpoint: e.endpoint,
        access: e.access,
        unit: e.unit,
        min: e.value_min,
        max: e.value_max,
        label: e.label,
        values: e.values,
        category: e.category,
        features: e.features?.map(flat),
    });
    return ex.map(flat);
}

function describe(e, indent = "  ") {
    const bits = [`${e.type}`, e.name + (e.endpoint ? ` (ep ${e.endpoint})` : "")];
    if (e.label) bits.push(`"${e.label}"`);
    if (e.unit) bits.push(`[${e.unit}]`);
    if (e.min !== undefined || e.max !== undefined) bits.push(`${e.min ?? ""}..${e.max ?? ""}`);
    if (e.values) bits.push(`{${e.values.join(", ")}}`);
    if (e.access !== undefined) bits.push(`access ${e.access}`);
    if (e.category) bits.push(`category ${e.category}`);
    let s = `${indent}${bits.join(" ")}\n`;
    for (const f of e.features ?? []) s += describe(f, `${indent}  `);
    return s;
}

/// The property names of the exposes, features included.
function names(list) {
    const out = new Set();
    const walk = (e, prefix) => {
        // (the property of an endpoint's expose already ends in _<endpoint>)
        const n = e.name;
        if (e.features?.length && (e.type === "light" || e.type === "switch")) {
            for (const f of e.features) walk(f, "");
        } else if (e.features?.length) {
            out.add(prefix + n);
            for (const f of e.features) out.add(`${n}.${f.name}`);
        } else {
            out.add(prefix + n);
        }
    };
    for (const e of list) walk(e, "");
    return out;
}

const definition = await zhc.findByDevice(device, true);
const source = definition.generated ? await zhc.generateExternalDefinitionSource(device) : null;
const ours = exposesOf(definition);
const result = {
    model: definition.model,
    vendor: definition.vendor,
    description: definition.description,
    generated: Boolean(definition.generated),
    exposes: ours,
    source,
    comparisons: [],
};
for (const file of compare) {
    // the converter imports zigbee-herdsman-converters by package name: resolve it to ZHC
    const text = fs
        .readFileSync(file, "utf8")
        .replaceAll("zigbee-herdsman-converters/lib/", `file://${path.join(ZHC, "dist", "lib")}/`)
        .replace(/\.\/?lib\/(\w+)"/g, '$1.js"');
    const fixed = text.replace(/(file:\/\/[^"]+\/lib\/\w+)"/g, '$1.js"');
    const tmp = path.join(os.tmpdir(), `z2m-compare-${process.pid}-${path.basename(file)}`);
    fs.writeFileSync(tmp, fixed);
    const ext = (await import(`file://${tmp}`)).default;
    fs.unlinkSync(tmp);
    const prepared = zhc.prepareDefinition(ext);
    const theirs = exposesOf(prepared);
    const a = names(ours);
    const b = names(theirs);
    result.comparisons.push({
        file,
        model: prepared.model,
        exposes: theirs,
        onlyGenerated: [...a].filter((x) => !b.has(x)).sort(),
        onlyConverter: [...b].filter((x) => !a.has(x)).sort(),
    });
}

if (asJson) {
    console.log(JSON.stringify(result, null, 2));
} else {
    console.log(`${result.generated ? "GENERATED" : "MATCHED"} definition: ${result.vendor} ${result.model} — ${result.description}`);
    console.log("exposes:");
    for (const e of ours) process.stdout.write(describe(e));
    if (source) console.log(`\ngenerated source:\n${source}`);
    for (const c of result.comparisons) {
        console.log(`\n== against ${c.file} (${c.model}):`);
        for (const e of c.exposes) process.stdout.write(describe(e));
        console.log(`only in the generated definition: ${c.onlyGenerated.join(", ") || "-"}`);
        console.log(`only in the converter: ${c.onlyConverter.join(", ") || "-"}`);
    }
}
