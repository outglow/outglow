import { describe, expect, it } from "vitest";
import * as D from "../src/console/diag";
import { demoChars } from "../src/console/demo";

describe("console decoders", () => {
  it("reads the sample bulb's characteristics", async () => {
    const c = demoChars();
    const info = D.decodeInfo(await c.info.readValue());
    expect(info).toMatchObject({ variant: "tw", model: "ESMLFzm_w6_TW", watchdog: true, ieee: "ef:cd:ab:89:67:45:23:01" });
    const s = D.decodeState(await c.state.readValue());
    expect(s.zigbee).toMatchObject({ joined: true, address: 0x7a21, pan: 0x1a62, channel: 15, routers: 4, children: 1, rssi: -52 });
    expect(s.light).toMatchObject({ on: true, level: 178, mireds: 370, kelvin: 2700 });
    expect(s.loggedIn).toBe(false);
    const cfg = D.decodeConfig(await c.config.readValue());
    expect(cfg).toMatchObject({ hasColor: true, ctMin: 153, ctMax: 454, startUpOnOff: 255, startUpColorTemp: null });
    expect(D.decodeFrames(await c.frames.readValue())[0]).toMatchObject({ src: "0x0000", cluster: "Level", command: "Move To Level (On/Off)" });
    expect(D.decodeLog(await c.log.readValue()).map((e) => e.text)).toContain("neighbour went silent (LQI 92)");
  });

  it("decodes every counter with its type, signed ones too", async () => {
    const counters = D.decodeCounters(await demoChars().counters!.readValue());
    expect(counters.list).toHaveLength(Object.keys(D.COUNTERS).length);
    expect(counters.byId.get(0x011d)).toBe(-52);
    expect(counters.byId.get(0x0104)).toBe(812);
    expect(counters.list.find((c) => c.id === 0x5003)).toMatchObject({ name: "Router neighbours", manufacturer: true, value: 4 });
    expect(counters.macRetryRate).toBeGreaterThan(0);
  });

  it("stops at an attribute type it does not know instead of misreading the rest", () => {
    const v = D.decodeCounters(Uint8Array.of(1, 0x00, 0x00, 0x21, 5, 0, 0x01, 0x00, 0x99, 1, 2, 3));
    expect(v.list.map((c) => [c.id, c.value])).toEqual([[0, 5]]);
  });

  it("a command needs the login, identify does not", async () => {
    const c = demoChars();
    await c.cmd.writeValue(D.commands.onOff(0));
    expect(D.decodeState(await c.state.readValue())).toMatchObject({ cmdResult: 1, cmdResultText: "requires login" });
    await c.cmd.writeValue(D.commands.identify());
    expect(D.decodeState(await c.state.readValue()).cmdResult).toBe(0);
  });
});

// Bytes read from the Outglow TW image in the emulator (cargo sims -- ble_diag, BLE_DIAG_DUMP)
import { readFileSync } from "node:fs";

describe("console decoders on the firmware's own bytes", () => {
  const dump = JSON.parse(readFileSync(new URL("./fixtures/ble-diag-tw.json", import.meta.url), "utf8")) as {
    variant: string; zigbee: { ieee: string; short: number; pan: number; channel: number }; values: Record<string, string>;
  };
  const bytes = (k: string) => Uint8Array.from(dump.values[k]!.match(/../g)!.map((h) => parseInt(h, 16)));

  it("the info and state match what the coordinator knows of the bulb", () => {
    const info = D.decodeInfo(bytes("info"));
    expect(info.variant).toBe(dump.variant);
    expect(info.ieee).toBe(dump.zigbee.ieee);
    const s = D.decodeState(bytes("state"));
    expect(s.zigbee).toMatchObject({ joined: true, address: dump.zigbee.short, pan: dump.zigbee.pan, channel: dump.zigbee.channel });
  });

  it("every counter has a name and a description", () => {
    const c = D.decodeCounters(bytes("counters"));
    expect(c.list).toHaveLength(53);
    for (const a of c.list) expect(D.COUNTERS[a.id], a.idText).toBeDefined();
  });

  it("the config, frames and log decode, every log entry with a text of its own", () => {
    expect(D.decodeConfig(bytes("config")).hasColor).toBe(true);
    expect(D.decodeFrames(bytes("frames")).length).toBeGreaterThan(0);
    for (const e of D.decodeLog(bytes("log"))) expect(e.text, `code ${e.code}`).not.toMatch(/^event \d/);
  });
});
