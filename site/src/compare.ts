// The comparison data (public/compare.json, written by `cargo xtask compare`)
// and what the comparison page computes from it: tallies per area and per
// section, the tests that differ, the links to the standards.

export type Verdict = "PASS" | "FAIL" | "NOTE" | "INCONCLUSIVE" | "N/A" | "ERROR" | "WITHHELD";

export interface Result {
  stock: Verdict;
  outglow: Verdict;
  stock_note?: string;
  outglow_note?: string;
}

export interface Test {
  id: string;
  suite: "zigbee" | "ble";
  module: string;
  area: string;
  doc: string;
  clause: string;
  requirement: string;
  level: "shall" | "should" | "reading";
  mandatory: boolean;
  ambiguous: string | null;
  withheld: "security" | "access" | null;
  results: Record<string, Result>;
}

export interface Target {
  id: string;
  board: string;
  name: string;
  stock: { firmware_version: string; hw_revision?: string; sha256: string; source: string; from?: string; app_version?: string };
  outglow: { image_type: string; sha256: string };
}

export interface CompareFile {
  schema: number;
  generated: string;
  how: string;
  source: string;
  outglow: { commit: string; commit_date: string; describe: string; url: string };
  cpi: number;
  suites: { id: string; covers: string }[];
  withheld: { security: string; access: string; policy: string };
  verdicts: Record<string, string>;
  targets: Target[];
  tests: Test[];
}

export type Side = "stock" | "outglow";

/** The order of the summary's areas (an area the data has and this list lacks comes last). */
export const AREAS = [
  "Network layer",
  "Parent role (end devices)",
  "Application support (APS)",
  "Device and service discovery (ZDO)",
  "Joining (Base Device Behavior)",
  "Zigbee security",
  "Leaving, removal and binding",
  "ZCL: reading, writing, reporting",
  "ZCL: Basic and Identify",
  "ZCL: On/Off and Level Control",
  "ZCL: Color Control",
  "ZCL: Groups and Scenes",
  "ZCL: Diagnostics",
  "Green Power",
  "OTA updates",
  "Bluetooth link layer",
  "Bluetooth L2CAP",
  "Bluetooth ATT",
  "Bluetooth GATT and GAP",
  "Bluetooth pairing and encryption",
];

export interface Tally {
  /** tests in the group */
  total: number;
  /** tests that apply to this image (not N/A, not withheld) */
  applicable: number;
  pass: number;
  fail: number;
  error: number;
  note: number;
  inconclusive: number;
  na: number;
  withheld: number;
}

export function tally(tests: Test[], target: string, side: Side): Tally {
  const t: Tally = { total: 0, applicable: 0, pass: 0, fail: 0, error: 0, note: 0, inconclusive: 0, na: 0, withheld: 0 };
  for (const x of tests) {
    const v = x.results[target]?.[side];
    if (!v) continue;
    t.total++;
    if (v === "WITHHELD") t.withheld++;
    else if (v === "N/A") t.na++;
    else {
      t.applicable++;
      if (v === "PASS") t.pass++;
      else if (v === "FAIL") t.fail++;
      else if (v === "ERROR") t.error++;
      else if (v === "NOTE") t.note++;
      else t.inconclusive++;
    }
  }
  return t;
}

/** "12 of 15 passed" and what the rest were, in words. */
export function tallyText(t: Tally): string {
  if (t.total === 0) return "no tests";
  if (t.withheld === t.total) return "not published";
  if (t.applicable === 0) return t.na === t.total ? "not applicable" : `${t.na} not applicable, ${t.withheld} not published`;
  const rest: string[] = [];
  if (t.fail) rest.push(`${t.fail} failed`);
  if (t.error) rest.push(`${t.error} test ${t.error === 1 ? "error" : "errors"}`);
  if (t.note) rest.push(`${t.note} ${t.note === 1 ? "note" : "notes"}`);
  if (t.inconclusive) rest.push(`${t.inconclusive} inconclusive`);
  if (t.na) rest.push(`${t.na} not applicable`);
  if (t.withheld) rest.push(`${t.withheld} not published`);
  return `${t.pass} of ${t.applicable} passed` + (rest.length ? ` (${rest.join(", ")})` : "");
}

/** Whether a section's token selects a test: an id prefix ("NWK-LS"), a whole id, or a module name. */
export function selects(token: string, t: Test): boolean {
  return t.module === token || t.id === token || t.id.startsWith(token.endsWith("-") ? token : token + "-");
}

/** The tests of a section, by its tokens; `taken` collects what a section already showed. */
export function sectionTests(tests: Test[], tokens: string[], taken?: Set<string>): Test[] {
  const out = tests.filter((t) => !taken?.has(t.id) && tokens.some((k) => selects(k, t)));
  out.forEach((t) => taken?.add(t.id));
  return out;
}

/** Definite failures on Outglow where the original firmware passed. */
export function losses(tests: Test[], target: string): Test[] {
  return tests.filter((t) => t.results[target]?.stock === "PASS" && t.results[target]?.outglow === "FAIL");
}

/** The documents the tests cite, with the publisher's copy. */
export const STANDARDS: Record<string, { name: string; url: string }> = {
  R22: { name: "Zigbee R22", url: "https://csa-iot.org/wp-content/uploads/2022/01/docs-05-3474-22-0csg-zigbee-specification-1.pdf" },
  ZCL8: { name: "ZCL 8", url: "https://csa-iot.org/wp-content/uploads/2022/01/07-5123-08-Zigbee-Cluster-Library-1.pdf" },
  BDB: { name: "BDB 3.0.1", url: "https://csa-iot.org/wp-content/uploads/2022/12/16-02828-012-PRO-BDB-v3.0.1-Specification.pdf" },
  ZLO: {
    name: "Lighting & Occupancy 1.0",
    url: "https://csa-iot.org/wp-content/uploads/2022/01/docs-15-0014-05-0plo-LightingOccupancyDevice-Specification-V1.0-1.pdf",
  },
  GP: { name: "Green Power Basic 1.1.2", url: "https://csa-iot.org/wp-content/uploads/2024/03/14-0563-19_Green-Power-Basic_specification_v1.1.2.pdf" },
  Core: { name: "Bluetooth Core 5.4", url: "https://www.bluetooth.com/specifications/specs/core-specification-5-4/" },
  CSS: { name: "Core Specification Supplement", url: "https://www.bluetooth.com/specifications/specs/core-specification-supplement/" },
};

/** The standard a test's document names (the first one when it names two). */
export function standardOf(doc: string): { name: string; url: string } | undefined {
  const d = doc.trim();
  if (d.startsWith("R22")) return STANDARDS.R22;
  if (d.startsWith("ZCL8")) return STANDARDS.ZCL8;
  if (d.startsWith("BDB")) return STANDARDS.BDB;
  if (d.startsWith("ZLO")) return STANDARDS.ZLO;
  if (d.startsWith("GP")) return STANDARDS.GP;
  if (d.startsWith("Core")) return STANDARDS.Core;
  if (d.startsWith("CSS")) return STANDARDS.CSS;
  return undefined;
}

/** The reference as the page writes it: "Zigbee R22 3.6.5", "Bluetooth Core 5.4 Vol 6 Part B §4.5.2". */
export function reference(t: Test): string {
  const doc = t.doc
    .replace(/^R22\b/, "Zigbee R22")
    .replace(/^ZCL8\b/, "ZCL 8")
    .replace(/^ZLO\b/, "Lighting & Occupancy 1.0")
    .replace(/^Core 5\.4\b/, "Bluetooth Core 5.4")
    .replace(/^CSS 11\b/, "Core Specification Supplement 11");
  return t.suite === "ble" ? `${doc} §${t.clause}` : `${doc} ${t.clause}`;
}

export const VERDICT_WORD: Record<Verdict, string> = {
  PASS: "pass",
  FAIL: "fail",
  NOTE: "note",
  INCONCLUSIVE: "inconclusive",
  "N/A": "not applicable",
  ERROR: "test error",
  WITHHELD: "not published",
};
