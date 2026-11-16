// public/compare.json (cargo xtask compare) and the comparison page: no
// withheld result of the original firmware leaks, every test has a result on
// every bulb, every test falls in a section of why.html, and the tallies add up.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, test } from "vitest";
import { type CompareFile, losses, reference, sectionTests, selects, standardOf, tally, tallyText, type Test } from "../src/compare.ts";

const site = join(dirname(fileURLToPath(import.meta.url)), "..");
const data = JSON.parse(readFileSync(join(site, "public", "compare.json"), "utf8")) as CompareFile;
const html = readFileSync(join(site, "why.html"), "utf8");

describe("compare.json", () => {
  test("has both filament bulbs and both suites", () => {
    expect(data.targets.map((t) => t.board)).toEqual(expect.arrayContaining(["ESMLFzm_w6_Dimm-4.63", "ESMLFzm_w6_TW-4.63"]));
    expect(new Set(data.tests.map((t) => t.suite))).toEqual(new Set(["zigbee", "ble"]));
    for (const t of data.targets) expect(t.stock.firmware_version).toMatch(/\S/);
  });

  test("a withheld test never carries the original firmware's result or observation", () => {
    const held = data.tests.filter((t) => t.withheld);
    expect(held.length).toBeGreaterThan(0);
    for (const t of held) {
      for (const r of Object.values(t.results)) {
        expect(r.stock, t.id).toBe("WITHHELD");
        expect(r.stock_note, t.id).toBeUndefined();
      }
    }
    // and nothing else is withheld
    for (const t of data.tests.filter((x) => !x.withheld)) {
      for (const r of Object.values(t.results)) expect(r.stock, t.id).not.toBe("WITHHELD");
    }
  });

  test("the security tests are withheld, whatever their result", () => {
    for (const t of data.tests) {
      if (t.module === "security_keys" || t.module === "smp_pairing" || t.id.startsWith("NWK-SEC-") || t.id.includes("LEAVE-0") && !t.id.startsWith("PAR-")) {
        expect(t.withheld, t.id).not.toBeNull();
      }
    }
  });

  test("every test has a result on every bulb", () => {
    for (const t of data.tests) for (const g of data.targets) expect(t.results[g.id], `${t.id} ${g.id}`).toBeDefined();
  });

  test("no path of the machine that generated it", () => {
    expect(JSON.stringify(data)).not.toMatch(/\/Users\/|\/home\/runner|C:\\\\/);
  });

  test("every reference names a document the page links", () => {
    for (const t of data.tests) {
      expect(standardOf(t.doc), `${t.id}: ${t.doc}`).toBeDefined();
      expect(reference(t)).toMatch(/\d/);
    }
  });
});

describe("why.html", () => {
  const sections = [...html.matchAll(/<section class="std"( data-first)? data-tests="([^"]+)"/g)].map((m) => ({
    first: !!m[1],
    tokens: m[2]!.split(/\s+/),
  }));

  test("every test is in exactly one section", () => {
    const taken = new Set<string>();
    for (const s of [...sections.filter((s) => s.first), ...sections.filter((s) => !s.first)]) sectionTests(data.tests, s.tokens, taken);
    const missing = data.tests.filter((t) => !taken.has(t.id)).map((t) => t.id);
    expect(missing).toEqual([]);
  });

  test("withheld tests fall in the sections that say so", () => {
    const taken = new Set<string>();
    const held = new Set<string>();
    for (const s of sections.filter((s) => s.first)) for (const t of sectionTests(data.tests, s.tokens, taken)) held.add(t.id);
    expect(data.tests.filter((t) => t.withheld).map((t) => t.id).filter((id) => !held.has(id))).toEqual([]);
  });

  test("every claim names tests that exist", () => {
    const ids = new Set(data.tests.map((t) => t.id));
    for (const m of html.matchAll(/data-claim="([^"]+)"/g)) for (const id of m[1]!.split(/\s+/)) expect(ids.has(id), id).toBe(true);
  });
});

describe("tallies", () => {
  const mk = (stock: string, outglow: string): Test =>
    ({ id: "X-01", module: "m", results: { a: { stock, outglow } } }) as unknown as Test;

  test("count applicable tests only", () => {
    const tests = [mk("PASS", "PASS"), mk("FAIL", "PASS"), mk("N/A", "PASS"), mk("WITHHELD", "PASS")];
    expect(tallyText(tally(tests, "a", "stock"))).toBe("1 of 2 passed (1 failed, 1 not applicable, 1 not published)");
    expect(tallyText(tally(tests, "a", "outglow"))).toBe("4 of 4 passed");
    expect(losses([mk("PASS", "N/A"), mk("PASS", "PASS")], "a")).toHaveLength(1);
  });

  test("a token selects by prefix, id or module", () => {
    const t = { id: "NWK-LS-01", module: "nwk_routing" } as Test;
    expect(selects("NWK-LS", t)).toBe(true);
    expect(selects("NWK-L", t)).toBe(false);
    expect(selects("nwk_routing", t)).toBe(true);
    expect(selects("NWK-LS-01", t)).toBe(true);
  });
});
