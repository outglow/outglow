// The comparison page: the "i" rows of the table at the top, and everything
// that comes from the test results (public/compare.json, `cargo xtask
// compare`): the summary per area, a line and a list of tests under every
// section, the claims about the original firmware that its results still
// bear out, the losses, the versions tested, and the table of diagnostic
// fields (from the console's own descriptions). Reader-facing check copy
// lives in compare-copy.ts; raw observations remain in the generated data.

import {
  AREAS,
  type CompareFile,
  losses,
  reference,
  type Result,
  sectionTests,
  type Side,
  STANDARDS,
  standardOf,
  tally,
  tallyText,
  type Test,
  VERDICT_WORD,
} from "./compare.ts";
import { EXAMPLES } from "./compare-examples.ts";
import { CHECKS, behaviour } from "./compare-copy.ts";
import { COUNTERS } from "./console/diag.ts";

for (const b of document.querySelectorAll<HTMLButtonElement>("button.info")) {
  b.addEventListener("click", () => {
    const more = document.getElementById(b.getAttribute("aria-controls")!)!;
    const open = b.getAttribute("aria-expanded") !== "true";
    b.setAttribute("aria-expanded", String(open));
    more.hidden = !open;
  });
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, attrs: Record<string, string> = {}, ...kids: (Node | string)[]) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
  e.append(...kids);
  return e;
}

function badge(v: string, side: Side): HTMLElement {
  const cls = v === "PASS" ? "v-pass" : v === "FAIL" ? "v-fail" : v === "WITHHELD" ? "v-held" : "v-other";
  const who = side === "stock" ? "Original" : "Outglow";
  return el("span", { class: `badge ${cls}` }, `${who}: ${VERDICT_WORD[v as keyof typeof VERDICT_WORD] ?? v}`);
}

/** Plain-language expectations and outcomes; exact measurements stay in the data file. */
function testItem(t: Test, r: Result): HTMLElement {
  const std = standardOf(t.doc);
  const ref = std ? el("a", { href: std.url, rel: "noopener" }, reference(t)) : el("span", {}, reference(t));
  const li = el(
    "li", {},
    el("p", { class: "t-req" }, el("strong", {}, "What we check: "),
      CHECKS[t.id] ?? "A new check has been added; its plain-English explanation is not available yet."),
    el("div", { class: "t-head" }, badge(r.stock, "stock"), badge(r.outglow, "outglow")),
  );
  for (const side of ["stock", "outglow"] as const) {
    li.append(el("p", { class: "t-obs" }, el("strong", {}, side === "stock" ? "Original firmware: " : "Outglow: "), behaviour(t, r, side)));
  }
  if (EXAMPLES[t.id]) li.append(el("p", { class: "t-example" },
    el("strong", {}, "What could happen: "), EXAMPLES[t.id]!));
  if (t.level !== "shall" || t.ambiguous) {
    li.append(el("p", { class: "small muted" }, t.level === "should"
      ? "This checks a recommendation, rather than a mandatory rule."
      : "This passage has more than one reasonable reading. Treat the result as a qualified interpretation, not an unambiguous failure."));
  }
  li.append(el("details", { class: "test-reference" },
    el("summary", {}, "Source in the standard"),
    el("p", { class: "t-ref small" }, ref,
      ". The numbers identify the document version and section used for this check."),
  ));
  return li;
}

function testList(tests: Test[], target: string, label: string): HTMLElement {
  const ul = el("ul", { class: "tests" });
  for (const t of tests) {
    const r = t.results[target];
    if (r) ul.append(testItem(t, r));
  }
  return el("details", { class: "tests-wrap" }, el("summary", {}, label), ul);
}

function tallyLine(tests: Test[], target: string): HTMLElement {
  return el(
    "p",
    { class: "tally small" },
    el("span", { class: "who" }, "Tests: "),
    `original firmware ${tallyText(tally(tests, target, "stock"))}; Outglow ${tallyText(tally(tests, target, "outglow"))}.`,
  );
}

const sections = [...document.querySelectorAll<HTMLElement>("section.std")];

/** What each area of the summary covers, in plain words. */
const AREA_ABOUT: Record<string, string> = {
  "Network layer": "How the bulb works as a Zigbee router: finding routes, relaying frames for others, keeping track of its neighbours, and noticing address and network identifier conflicts.",
  "Parent role (end devices)": "Battery devices (sensors, remotes) that join through the bulb: letting them in, holding their messages until they wake up, and letting them move to another parent.",
  "Application support (APS)": "Receipt confirmations, repeated commands, group messages and long messages split into parts.",
  "Device and service discovery (ZDO)": "How the hub learns what the bulb is, which functions it offers, which devices are nearby and how to reach them.",
  "Joining (Base Device Behavior)": "Joining a network the way Zigbee 3.0 asks: which channels it scans, and opening the network for the next device afterwards.",
  "Zigbee security": "Getting and replacing encryption keys, rejecting repeated old messages and returning securely to the network.",
  "Leaving, removal and binding": "Who may remove the bulb or change its saved direct connections to other devices.",
  "ZCL: reading, writing, reporting": "Reading and changing settings, reporting changes and explaining unsupported commands.",
  "ZCL: Basic and Identify": "What the bulb says about itself (manufacturer, model, version), and blinking when the hub asks it to show itself.",
  "ZCL: On/Off and Level Control": "Switching and dimming: on, off, toggle, fades, switching off after a time, and the level after power returns.",
  "ZCL: Color Control": "Colour temperature on tunable white bulbs: moving and stepping it, its limits, and what the bulb reports.",
  "ZCL: Groups and Scenes": "Groups of bulbs addressed with one message, and scenes stored in the bulb and recalled with one command.",
  "ZCL: Diagnostics": "Counters of radio and network activity that the hub can read. Only Outglow offers this standard service.",
  "Green Power": "Advertising the service that forwards messages from battery-free switches. This check does not exercise forwarding itself.",
  "OTA updates": "Requesting update details after the Zigbee hub announces available firmware. This check does not install an update.",
  "Bluetooth link layer": "The Bluetooth radio side: advertising, accepting a connection, keeping it, and the control procedures inside it.",
  "Bluetooth L2CAP": "Assembling complete Bluetooth messages from radio packets and answering channel-management requests.",
  "Bluetooth ATT": "Reading and changing Bluetooth values and giving useful answers when a request cannot be carried out.",
  "Bluetooth GATT and GAP": "How a phone discovers available Bluetooth features, checks permissions and subscribes to updates.",
  "Bluetooth pairing and encryption": "Refusing pairing and encryption requests, what the bulb says it supports, and how it ends a connection itself.",
};

/** Data keeps its protocol area names; the page uses everyday descriptions. */
const AREA_NAME: Record<string, string> = {
  "Network layer": "Forwarding messages through the network",
  "Parent role (end devices)": "Helping battery sensors and remotes",
  "Application support (APS)": "Delivering commands and recognising repeats",
  "Device and service discovery (ZDO)": "Discovering devices and routes",
  "Joining (Base Device Behavior)": "Joining a network",
  "Zigbee security": "Zigbee encryption and secure joining",
  "Leaving, removal and binding": "Leaving, removal and saved device connections",
  "ZCL: reading, writing, reporting": "Reading settings and reporting changes",
  "ZCL: Basic and Identify": "Device information and identification",
  "ZCL: On/Off and Level Control": "Switching and brightness",
  "ZCL: Color Control": "Warmer and cooler white light",
  "ZCL: Groups and Scenes": "Groups and saved light scenes",
  "ZCL: Diagnostics": "Troubleshooting readings",
  "Green Power": "Battery-free switches",
  "OTA updates": "Wireless firmware updates",
  "Bluetooth link layer": "Bluetooth discovery and connections",
  "Bluetooth L2CAP": "Assembling Bluetooth messages",
  "Bluetooth ATT": "Reading and changing Bluetooth values",
  "Bluetooth GATT and GAP": "Bluetooth features and subscriptions",
  "Bluetooth pairing and encryption": "Bluetooth pairing and encryption",
};

/** Areas whose test list is open in the summary (kept when the bulb changes). */
const openAreas = new Set<string>();

/** Order inside an area: where the two differ first, then what fails, then the rest. */
function rank(r: Result): number {
  if (r.stock !== r.outglow && r.stock !== "WITHHELD") return 0;
  if (r.stock !== "PASS" || r.outglow !== "PASS") return 1;
  return 2;
}

function render(data: CompareFile, target: string) {
  const tests = data.tests;
  const tgt = data.targets.find((t) => t.id === target)!;
  for (const s of document.querySelectorAll("[data-stock-version]")) s.textContent = `${tgt.stock.firmware_version} on the ${tgt.name.replace(/ \(.*\)$/, "").toLowerCase()}`;

  // summary per area
  const areas = [...new Set(tests.map((t) => t.area))].sort((a, b) => {
    const ia = AREAS.indexOf(a), ib = AREAS.indexOf(b);
    return (ia < 0 ? 999 : ia) - (ib < 0 ? 999 : ib);
  });
  const cell = (mine: Test[], side: Side) => {
    const t = tally(mine, target, side);
    const frac = t.applicable ? t.pass / t.applicable : 0;
    return el(
      "td",
      {},
      el("span", { class: "frac" }, t.applicable ? `${t.pass} of ${t.applicable} passed` : t.withheld ? "Not published" : "Not applicable"),
      el("span", { class: "bar", "aria-hidden": "true" }, el("span", { style: `width:${Math.round(frac * 100)}%` })),
      el("span", { class: "visually-hidden" }, tallyText(t)),
    );
  };
  const rows = areas.flatMap((a, i) => {
    const mine = tests.filter((t) => t.area === a && t.results[target]);
    const id = `area-${i}`;
    const open = openAreas.has(a);
    const toggle = el(
      "button",
      { type: "button", class: "area-toggle", "aria-expanded": String(open), "aria-controls": id },
      el("span", { class: "chev", "aria-hidden": "true" }),
      AREA_NAME[a] ?? a,
    );
    const sorted = [...mine].sort((x, y) => rank(x.results[target]!) - rank(y.results[target]!));
    const ul = el("ul", { class: "tests" });
    for (const t of sorted) ul.append(testItem(t, t.results[target]!));
    const more = el(
      "tr",
      { class: "area-more", id },
      el(
        "td",
        { colspan: "3" },
        el("p", { class: "area-about" }, AREA_ABOUT[a] ?? ""),
        el("p", { class: "small muted" }, `${mine.length} test${mine.length === 1 ? "" : "s"}; those where the two differ first.`),
        ul,
      ),
    );
    more.hidden = !open;
    toggle.addEventListener("click", () => {
      const now = toggle.getAttribute("aria-expanded") !== "true";
      toggle.setAttribute("aria-expanded", String(now));
      more.hidden = !now;
      if (now) openAreas.add(a);
      else openAreas.delete(a);
    });
    return [el("tr", { class: "area-row" }, el("th", { scope: "row" }, toggle), cell(mine, "stock"), cell(mine, "outglow")), more];
  });
  const suiteName = (suite: string) => (suite === "zigbee" ? "All Zigbee tests" : "All Bluetooth tests");
  const total = (suite: string) => {
    const mine = tests.filter((t) => t.suite === suite);
    return el("tr", { class: "total" }, el("th", { scope: "row" }, suiteName(suite)), cell(mine, "stock"), cell(mine, "outglow"));
  };
  const totalLine = (suite: string) => {
    const mine = tests.filter((t) => t.suite === suite);
    return el(
      "li",
      {},
      el("strong", {}, `${suiteName(suite)}: `),
      `original ${tallyText(tally(mine, target, "stock"))}; Outglow ${tallyText(tally(mine, target, "outglow"))}.`,
    );
  };
  const summary = document.getElementById("summary")!;
  summary.replaceChildren(
    el(
      "div",
      { class: "table-wrap summary-table" },
      el(
        "table",
        {},
        el("caption", { class: "visually-hidden" }, `Tests passed per area, ${tgt.name}`),
        el("thead", {}, el("tr", {}, el("th", {}, "Area"), el("th", {}, "Original"), el("th", {}, "Outglow"))),
        el("tbody", {}, ...rows, total("zigbee"), total("ble")),
      ),
    ),
    el("ul", { class: "caveats small" }, totalLine("zigbee"), totalLine("ble")),
    el(
      "p",
      { class: "muted small" },
      "Counts include only checks that apply to each firmware and have published results. ",
      "Open an area to read what we check and how both firmwares behave. Technical references are available separately.",
    ),
  );

  // sections: the ones that claim tests first (security), then in order of the page
  const taken = new Set<string>();
  const order = [...sections.filter((s) => s.hasAttribute("data-first")), ...sections.filter((s) => !s.hasAttribute("data-first"))];
  for (const s of order) {
    const mine = sectionTests(tests, (s.dataset.tests ?? "").split(/\s+/).filter(Boolean), taken);
    s.querySelector(".tally")?.remove();
    s.querySelector(".tests-wrap")?.remove();
    for (const c of s.querySelectorAll<HTMLElement>("[data-claim]")) {
      const ids = (c.dataset.claim ?? "").split(/\s+/);
      c.hidden = !mine.some((t) => ids.includes(t.id) && t.results[target] && !["PASS", "WITHHELD"].includes(t.results[target]!.stock));
    }
    const stockSide = s.querySelector<HTMLElement>(".sides li");
    stockSide?.querySelector(".auto")?.remove();
    const claims = stockSide ? [...stockSide.querySelectorAll<HTMLElement>("[data-claim]")] : [];
    if (stockSide && claims.length && claims.every((c) => c.hidden)) {
      stockSide.append(el("span", { class: "auto" }, `In these tests: ${tallyText(tally(mine, target, "stock"))}.`));
    }
    if (!mine.length) continue;
    s.append(tallyLine(mine, target), testList(mine, target, `Read the ${mine.length} check${mine.length === 1 ? "" : "s"}`));
  }
  // tests no section names (a new module, say): listed as they are
  for (const [suite, at] of [["zigbee", "zigbee-other"], ["ble", "ble-other"]] as const) {
    const rest = tests.filter((t) => t.suite === suite && !taken.has(t.id));
    const box = document.getElementById(at)!;
    box.replaceChildren();
    if (rest.length) {
      box.append(
        el(
          "section",
          { class: "std" },
          el("h3", {}, "Other tests"),
          el("p", {}, "Tests added after this page was written."),
          tallyLine(rest, target),
          testList(rest, target, `Read the ${rest.length} check${rest.length === 1 ? "" : "s"}`),
        ),
      );
    }
  }

  // Only definite failures belong here. Optional and unexercised procedures
  // must not turn a "not applicable" result into a claimed regression.
  const lost = losses(tests, target);
  const lbox = document.getElementById("losses")!;
  lbox.replaceChildren(
    el(
      "p",
      {},
      el("strong", {}, "Checks that stock passes and Outglow fails: "),
      lost.length
        ? `${lost.length} for the ${tgt.name.replace(/ \(.*\)$/, "").toLowerCase()}.`
        : "none in the published results for this bulb. This comparison uses only definite pass and fail results on both sides.",
    ),
  );
  if (lost.length) {
    lbox.append(testList(lost, target, "Read these failures"));
  }
  const notApplicable = tests.filter((t) => t.results[target]?.stock === "PASS" && t.results[target]?.outglow === "N/A");
  if (notApplicable.length) {
    lbox.append(el("p", {},
      "Some checks pass on stock but are not applicable to Outglow in this run. This can mean an optional feature is absent or a procedure was not started; it is not a failed check."));
    lbox.append(testList(notApplicable, target, `Why ${notApplicable.length} checks are not applicable to Outglow`));
  }
}

function versions(data: CompareFile) {
  const rows = data.targets.map((t) =>
    el(
      "tr",
      {},
      el("td", {}, t.name.replace(/ \(.*\)$/, ""), el("br"), el("span", { class: "small" }, `Hardware model identifier: ${t.board}`)),
      el("td", {}, el("strong", {}, t.stock.firmware_version), el("br"), el("span", { class: "small muted" }, `from ${t.stock.from ?? "the vendor app"} ${t.stock.app_version ?? ""}`)),
      el("td", {}, el("a", { href: data.outglow.url }, data.outglow.describe || data.outglow.commit), el("br"), el("span", { class: "small muted" }, `Source revision dated ${data.outglow.commit_date}`)),
    ),
  );
  document.getElementById("versions-table")!.replaceChildren(
    el(
      "div",
      { class: "table-wrap compare versions" },
      el(
        "table",
        {},
        el("thead", {}, el("tr", {}, el("th", {}, "Bulb"), el("th", {}, "Original firmware"), el("th", {}, "Outglow"))),
        el("tbody", {}, ...rows),
      ),
    ),
    el(
      "p",
      { class: "muted small" },
      `Results generated ${data.generated} in the emulator. `,
      "The original firmware files came from the vendor’s app. These findings apply to the versions and hardware listed above; a different release may behave differently.",
    ),
  );
  document.getElementById("data-note")!.textContent =
    `Results of ${data.generated}: Outglow ${data.outglow.describe || data.outglow.commit} (${data.outglow.commit_date}); ` +
    `original ${[...new Set(data.targets.map((t) => t.stock.firmware_version))].join(", ")}. Measured in the emulator, not on bulbs.`;
}

// ZCL 8 names and Zigbee2MQTT's (zigbee-herdsman's) names of the standard
// attributes, the converter's keys of the manufacturer ones (z2m/outglow.mjs)
const ZCL: [number, string, string][] = [
  [0x0000, "NumberOfResets", "numberOfResets"], [0x0001, "PersistentMemoryWrites", "persistentMemoryWrites"],
  [0x0100, "MacRxBcast", "macRxBcast"], [0x0101, "MacTxBcast", "macTxBcast"], [0x0102, "MacRxUcast", "macRxUcast"],
  [0x0103, "MacTxUcast", "macTxUcast"], [0x0104, "MacTxUcastRetry", "macTxUcastRetry"], [0x0105, "MacTxUcastFail", "macTxUcastFail"],
  [0x0106, "APSRxBcast", "aPSRxBcast"], [0x0107, "APSTxBcast", "aPSTxBcast"], [0x0108, "APSRxUcast", "aPSRxUcast"],
  [0x0109, "APSTxUcastSuccess", "aPSTxUcastSuccess"], [0x010a, "APSTxUcastRetry", "aPSTxUcastRetry"], [0x010b, "APSTxUcastFail", "aPSTxUcastFail"],
  [0x010c, "RouteDiscInitiated", "routeDiscInitiated"], [0x010d, "NeighborAdded", "neighborAdded"], [0x010e, "NeighborRemoved", "neighborRemoved"],
  [0x010f, "NeighborStale", "neighborStale"], [0x0110, "JoinIndication", "joinIndication"], [0x0111, "ChildMoved", "childMoved"],
  [0x0112, "NWKFCFailure", "nwkFcFailure"], [0x0113, "APSFCFailure", "apsFcFailure"], [0x0114, "APSUnauthorizedKey", "apsUnauthorizedKey"],
  [0x0115, "NWKDecryptFailures", "nwkDecryptFailures"], [0x0116, "APSDecryptFailures", "apsDecryptFailures"],
  [0x0117, "PacketBufferAllocateFailures", "packetBufferAllocateFailures"], [0x0118, "RelayedUcast", "relayedUcast"],
  [0x0119, "PhytoMACqueuelimitreached", "phyToMacQueueLimitReached"], [0x011a, "PacketValidatedropcount", "packetValidateDropCount"],
  [0x011b, "AverageMACRetryPerAPSMessageSent", "averageMacRetryPerApsMessageSent"], [0x011c, "LastMessageLQI", "lastMessageLqi"],
  [0x011d, "LastMessageRSSI", "lastMessageRssi"],
];
const MANUFACTURER_KEYS: Record<number, string> = {
  0x5000: "uptime", 0x5001: "reset_cause", 0x5002: "watchdog_resets", 0x5003: "neighbors", 0x5004: "children",
  0x5005: "relay_drop_queue_full", 0x5006: "relay_drop_no_route", 0x5007: "relay_drop_mac_fail", 0x5008: "relay_drop_duplicate",
  0x5009: "nwk_status_sent", 0x500a: "broadcast_drop_table_full", 0x500b: "broadcast_queue_full", 0x500c: "cca_busy",
  0x500d: "channel_access_failures", 0x500e: "rx_queue_peak", 0x500f: "indirect_peak", 0x5010: "indirect_dropped",
  0x5011: "indirect_expired", 0x5012: "ble_lends", 0x5013: "ble_ms", 0x5014: "flash_erases",
};

function fields() {
  const hex = (v: number) => "0x" + v.toString(16).padStart(4, "0");
  const item = (id: number, spec: string, z2m: string) => {
    const c = COUNTERS[id];
    return el(
      "li",
      {},
      el("div", { class: "t-head" }, el("strong", {}, c?.name ?? spec)),
      el("p", { class: "t-req" }, c?.what ?? ""),
      ...(c?.healthy ? [el("p", { class: "small" }, el("strong", {}, "How to read it: "), c.healthy)] : []),
      el("details", {}, el("summary", {}, "Names for looking up this reading"),
        el("p", { class: "small muted" }, "Zigbee reading identifier: ", el("code", {}, hex(id)),
          ". This hexadecimal number identifies the reading; it is not its measured value. Standard name: ", spec,
          ". Zigbee2MQTT field name: ", el("code", {}, z2m), ".")),
    );
  };
  document.getElementById("fields")!.replaceChildren(
    el("p", {}, "Some diagnostic names use protocol abbreviations. MAC refers to individual radio transmissions between neighbours; APS refers to complete commands or reports; NWK refers to forwarding and protecting messages across the network. A frame is one radio message, a unicast is addressed to one device, and a broadcast is sent to many devices."),
    el("p", {}, "A child is an end device connected through this bulb, often a battery-powered sensor or remote. A watchdog is a timer that restarts the bulb if its software stops responding. Flash is permanent storage. LQI describes reception quality; RSSI describes signal strength. Signal strength uses dBm, a radio power scale where a more negative reading means a weaker signal."),
    el("h4", {}, "Standard Zigbee readings"),
    el("ul", { class: "tests fields" }, ...ZCL.map(([id, spec, z]) => item(id, spec, z))),
    el("h4", {}, "Additional Outglow readings"),
    el(
      "ul",
      { class: "tests fields" },
      ...Object.entries(MANUFACTURER_KEYS).map(([id, key]) => item(Number(id), "manufacturer-specific", `${key} (converter)`)),
    ),
    el(
      "p",
      { class: "small muted" },
      "Over Zigbee, Outglow also reports signal-strength patterns, background radio noise, its route to the hub, nearby devices and an event log. The converter groups these readings for you.",
    ),
  );
}

// the references under each section's heading, linked to the documents
function linkRefs() {
  const names: [string, string][] = [
    ["Zigbee R22", "R22"], ["ZCL 8", "ZCL8"], ["BDB 3.0.1", "BDB"], ["Lighting & Occupancy 1.0", "ZLO"],
    ["Green Power Basic 1.1.2", "GP"], ["Bluetooth Core 5.4", "Core"],
  ];
  for (const p of document.querySelectorAll<HTMLElement>("p.ref")) {
    const text = p.textContent ?? "";
    const parts: (Node | string)[] = [];
    let rest = text;
    for (;;) {
      let best: [number, string, string] | undefined;
      for (const [n, k] of names) {
        const i = rest.indexOf(n);
        if (i >= 0 && (!best || i < best[0])) best = [i, n, k];
      }
      if (!best) break;
      const [i, n, k] = best;
      parts.push(rest.slice(0, i), el("a", { href: STANDARDS[k]!.url, rel: "noopener" }, n));
      rest = rest.slice(i + n.length);
    }
    parts.push(rest);
    p.replaceChildren("Document versions and section numbers: ", ...parts);
    const details = el("details", { class: "section-reference" }, el("summary", {}, "Other standard sections checked here"));
    p.before(details);
    details.append(p);
  }
}

async function main() {
  linkRefs();
  fields();
  let data: CompareFile;
  try {
    data = (await (await fetch(`${import.meta.env.BASE_URL}compare.json`)).json()) as CompareFile;
  } catch {
    document.getElementById("summary")!.textContent = "The test results could not be loaded.";
    return;
  }
  versions(data);
  const pick = document.getElementById("targets")!;
  let current = data.targets[0]?.id ?? "";
  const buttons = data.targets.map((t) => {
    const b = el("button", { type: "button", "aria-pressed": String(t.id === current) }, t.name.replace(/ \(.*\)$/, "").replace(/ bulb$/, ""));
    if (data.targets.filter((x) => x.board === t.board).length > 1) b.append(` ${t.stock.firmware_version}`);
    b.addEventListener("click", () => {
      current = t.id;
      for (const x of buttons) x.setAttribute("aria-pressed", String(x === b));
      render(data, current);
    });
    return b;
  });
  pick.replaceChildren(...buttons);
  render(data, current);
}

void main();
