// public/bulbs.json against public/boards.json and the pictures on disk: every
// board and revision exists, kinds agree, every picture is there, small, and
// credited, and the page's filter and search behave.

import { existsSync, readFileSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, test } from "vitest";
import type { Board } from "../src/flasher/awox-ota.ts";
import { type BulbsFile, GROUPS, matches } from "../src/bulbs.ts";

const pub = join(dirname(fileURLToPath(import.meta.url)), "..", "public");
const boards = JSON.parse(readFileSync(join(pub, "boards.json"), "utf8")) as Board[];
const bulbs = JSON.parse(readFileSync(join(pub, "bulbs.json"), "utf8")) as BulbsFile;

describe("bulbs.json", () => {
  test("covers every board exactly once, with matching kinds", () => {
    const listed = bulbs.boards.flatMap((b) => b.revisions.map((r) => `${b.board}-${r}`)).sort();
    expect(listed).toEqual(boards.map((b) => b.name).sort());
    for (const b of bulbs.boards) {
      for (const r of b.revisions) {
        const real = boards.find((x) => x.name === `${b.board}-${r}`);
        expect(real?.kind, b.board).toBe(b.kind);
      }
      expect(GROUPS[b.kind], b.kind).toBeDefined();
    }
  });

  test("only ESMLFzm_w6_Dimm is marked as tested on hardware", () => {
    expect(bulbs.boards.filter((b) => b.hardware_tested).map((b) => b.board)).toEqual(["ESMLFzm_w6_Dimm"]);
  });

  test("every product is complete and every picture exists and is credited", () => {
    for (const b of bulbs.boards) {
      for (const p of b.products) {
        const id = `${b.board} ${p.brand} ${p.article}`;
        expect(p.name, id).toBeTruthy();
        expect(p.shape, id).toBeTruthy();
        expect(["confirmed", "likely", "inferred"], id).toContain(p.confidence);
        expect(p.source.length, id).toBeGreaterThan(0);
        if (p.image) {
          const file = join(pub, p.image);
          expect(existsSync(file), file).toBe(true);
          expect(statSync(file).size, file).toBeLessThan(80 * 1024);
          expect(bulbs.images[p.image], `credit for ${p.image}`).toBeDefined();
        }
      }
    }
  });
});

describe("filter and search", () => {
  const b = bulbs.boards.find((x) => x.board === "ESMLFzm_w6_TW");
  const p = b?.products.find((x) => x.article === "12242");
  test("finds the ST64 tunable white filament bulb", () => {
    expect(b && p).toBeTruthy();
    if (!b || !p) return;
    expect(matches(b, p, "all", "12242")).toBe(true);
    expect(matches(b, p, "tunable", "")).toBe(true);
    expect(matches(b, p, "colour", "")).toBe(false);
    expect(matches(b, p, "all", "esmlfzm_w6_tw-4.63")).toBe(true);
    expect(matches(b, p, "all", "12245")).toBe(false);
  });
});
