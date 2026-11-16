// The comparison page: the "i" rows of the table at the top, and everything
// that comes from the test results (public/compare.json, `cargo xtask
// compare`): the summary per area, a line and a list of tests under every
// section, the claims about the original firmware that its results still
// bear out, the losses, the versions tested, and the table of diagnostic
// fields (from the console's own descriptions).

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
  const cls = v === "PASS" ? "v-pass" : v === "FAIL" || v === "ERROR" ? "v-fail" : v === "WITHHELD" ? "v-held" : "v-other";
  const who = side === "stock" ? "Original" : "Outglow";
  return el("span", { class: `badge ${cls}` }, `${who}: ${VERDICT_WORD[v as keyof typeof VERDICT_WORD] ?? v}`);
}

function testItem(t: Test, r: Result): HTMLElement {
  const std = standardOf(t.doc);
  const ref = std ? el("a", { href: std.url, rel: "noopener" }, reference(t)) : el("span", {}, reference(t));
  const li = el(
    "li",
    {},
    el("div", { class: "t-head" }, el("code", {}, t.id), badge(r.stock, "stock"), badge(r.outglow, "outglow")),
    el("p", { class: "t-req" }, t.requirement),
    el("p", { class: "t-ref small" }, ref, t.level === "should" ? " (a recommendation)" : t.level === "reading" ? " (depends on how the clause is read)" : ""),
  );
  if (r.stock_note) li.append(el("p", { class: "t-obs small" }, el("strong", {}, "Original: "), r.stock_note));
  if (r.outglow_note) li.append(el("p", { class: "t-obs small" }, el("strong", {}, "Outglow: "), r.outglow_note));
  if (t.ambiguous) li.append(el("p", { class: "t-obs small muted" }, "Reading of the clause: ", t.ambiguous));
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
    `original ${tallyText(tally(tests, target, "stock"))}; Outglow ${tallyText(tally(tests, target, "outglow"))}.`,
  );
}

const sections = [...document.querySelectorAll<HTMLElement>("section.std")];

function render(data: CompareFile, target: string) {
  const tests = data.tests;
  const tgt = data.targets.find((t) => t.id === target)!;
  for (const s of document.querySelectorAll("[data-stock-version]")) s.textContent = `${tgt.stock.firmware_version} on ${tgt.board}`;

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
      el("span", { class: "frac" }, t.applicable ? `${t.pass}/${t.applicable}` : t.withheld ? "–" : "n/a"),
      el("span", { class: "bar", "aria-hidden": "true" }, el("span", { style: `width:${Math.round(frac * 100)}%` })),
      el("span", { class: "visually-hidden" }, tallyText(t)),
    );
  };
  const rows = areas.map((a) => {
    const mine = tests.filter((t) => t.area === a);
    return el("tr", {}, el("th", { scope: "row" }, a), cell(mine, "stock"), cell(mine, "outglow"));
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
      "Passed of the tests that apply to each firmware. “n/a”: the firmware lacks that optional part. “–”: not published. ",
      "Failed, notes and inconclusive results are in each section below.",
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
    s.append(tallyLine(mine, target), testList(mine, target, `The ${mine.length} test${mine.length === 1 ? "" : "s"}`));
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
          testList(rest, target, `The ${rest.length} test${rest.length === 1 ? "" : "s"}`),
        ),
      );
    }
  }

  // losses: what the original passes and Outglow does not
  const lost = losses(tests, target);
  const lbox = document.getElementById("losses")!;
  lbox.replaceChildren(
    el(
      "p",
      {},
      el("strong", {}, "In the tests: "),
      lost.length
        ? `${lost.length} test${lost.length === 1 ? "" : "s"} the original firmware passes and Outglow does not, on the ${tgt.name.replace(/ \(.*\)$/, "").toLowerCase()}:`
        : "no test that the original firmware passes fails on Outglow.",
    ),
  );
  if (lost.length) {
    lbox.append(testList(lost, target, "Show them"));
    lbox.append(
      el(
        "p",
        { class: "muted small" },
        "“Not applicable” on Outglow means it lacks an optional part the original has (for example a Bluetooth procedure it never starts), not that it broke a rule.",
      ),
    );
  }
}

function versions(data: CompareFile) {
  const rows = data.targets.map((t) =>
    el(
      "tr",
      {},
      el("td", {}, t.name.replace(/ \(.*\)$/, ""), el("br"), el("code", { class: "small" }, t.board)),
      el("td", {}, el("strong", {}, t.stock.firmware_version), el("br"), el("span", { class: "small muted" }, `from ${t.stock.from ?? "the vendor app"} ${t.stock.app_version ?? ""}`)),
      el("td", {}, el("a", { href: data.outglow.url }, data.outglow.describe || data.outglow.commit), el("br"), el("span", { class: "small muted" }, `${data.outglow.commit_date}, image type ${t.outglow.image_type}`)),
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
      `Results of ${data.generated}, from the emulator (${data.how}). `,
      "The original images are the ones the vendor's app installs, kept in ",
      el("a", { href: "https://github.com/outglow/original-firmware" }, "outglow/original-firmware"),
      "; every version listed there for these bulbs is tested, and a new one is tested by the next build of this site. ",
      "Each image's SHA-256 is in ",
      el("a", { href: `${import.meta.env.BASE_URL}compare.json` }, "compare.json"),
      ".",
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
      el("div", { class: "t-head" }, el("code", {}, hex(id)), el("strong", {}, c?.name ?? spec)),
      el("p", { class: "small muted" }, spec, " · Zigbee2MQTT: ", el("code", {}, z2m)),
      el("p", { class: "t-req" }, c?.what ?? ""),
      ...(c?.healthy ? [el("p", { class: "small" }, el("strong", {}, "Healthy: "), c.healthy)] : []),
    );
  };
  document.getElementById("fields")!.replaceChildren(
    el("h4", {}, "Standard attributes (ZCL 8 Tables 3-130, 3-131)"),
    el("ul", { class: "tests fields" }, ...ZCL.map(([id, spec, z]) => item(id, spec, z))),
    el("h4", {}, "Manufacturer attributes (code 0x1141)"),
    el(
      "ul",
      { class: "tests fields" },
      ...Object.entries(MANUFACTURER_KEYS).map(([id, key]) => item(Number(id), "manufacturer-specific", `${key} (converter)`)),
    ),
    el(
      "p",
      { class: "small muted" },
      "Over Zigbee the bulb has more manufacturer attributes from 0x5015 (signal-strength histogram, noise floor, the way to the coordinator, the neighbour table, the event log); the converter shows them in its groups.",
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
    p.replaceChildren(...parts);
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
