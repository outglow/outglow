// The browser flasher: Web Bluetooth transport, bulb detection, release
// proposal and the progress display around awox-ota.ts (the protocol, shared
// with the tests).

import * as ota from "./awox-ota.ts";
import { BulbArt } from "./bulb-art.ts";

const BASE = import.meta.env.BASE_URL;
const DEVINFO = "device_information";

function el<T extends HTMLElement = HTMLElement>(id: string): T {
  const e = document.getElementById(id);
  if (!e) throw new Error(`#${id} missing`);
  return e as T;
}

const ui = {
  support: el("support"), connect: el<HTMLButtonElement>("connect"), disconnect: el<HTMLButtonElement>("disconnect"),
  bulb: el("bulb"), proposal: el("proposal"), release: el<HTMLSelectElement>("release"), image: el("image"),
  problems: el("problems"), confirm: el<HTMLInputElement>("confirm"), start: el<HTMLButtonElement>("start"),
  stop: el<HTMLButtonElement>("stop"), file: el<HTMLInputElement>("file"), stock: el<HTMLInputElement>("stock"),
  login: el<HTMLInputElement>("login"), meshName: el<HTMLInputElement>("mesh-name"), mac: el<HTMLInputElement>("mac"),
  password: el<HTMLInputElement>("password"), chunk: el<HTMLSelectElement>("chunk"), delay: el<HTMLInputElement>("delay"),
  resends: el<HTMLInputElement>("resends"), anyRevision: el<HTMLInputElement>("any-revision"),
  anyRevisionConfirm: el("any-revision-confirm"), anyRevisionText: el<HTMLInputElement>("any-revision-text"),
  allDevices: el<HTMLInputElement>("all-devices"), verbose: el<HTMLInputElement>("verbose"),
  bar: el<HTMLProgressElement>("bar"), pct: el("pct"), speed: el("speed"), eta: el("eta"),
  phases: el("phases"), log: el("log"), copy: el<HTMLButtonElement>("copy"), download: el<HTMLButtonElement>("download"),
};
const art = new BulbArt(el("art"));

type PhaseName = "connect" | "login" | "erasing" | "writing" | "verifying" | "reboot";
type PhaseState = "active" | "done" | "failed" | "skipped" | "";

interface State {
  boards: ota.Board[];
  manifest: ota.Manifest;
  device: BluetoothDevice | null;
  chars: Record<ota.Char, BluetoothRemoteGATTCharacteristic> | null;
  bulb: ota.Bulb | null;
  board: ota.Board | undefined;
  img: Uint8Array | null;
  info: ota.ImageInfo | null;
  local: boolean;
  busy: boolean;
  abort: AbortController | null;
}

const state: State = {
  boards: [], manifest: { schema: 1, images: [] }, device: null, chars: null, bulb: null, board: undefined,
  img: null, info: null, local: false, busy: false, abort: null,
};
const lines: string[] = [];

function log(line: string, cls?: "ok" | "err", verbose = false): void {
  const text = `${new Date().toLocaleTimeString()}  ${line}`;
  lines.push(text);
  if (verbose && !ui.verbose.checked) return;
  const d = document.createElement("div");
  d.textContent = text;
  if (cls) d.className = cls;
  ui.log.append(d);
  ui.log.scrollTop = ui.log.scrollHeight;
}

function phase(name: PhaseName, s: PhaseState): void {
  const li = ui.phases.querySelector<HTMLElement>(`[data-phase="${name}"]`);
  if (li) li.dataset.state = s;
}

function resetPhases(): void {
  for (const li of ui.phases.querySelectorAll<HTMLElement>("li")) li.dataset.state = "";
}

const hexId = (n: number | null) => (n === null ? "unknown" : `0x${n.toString(16).padStart(4, "0")}`);
const macText = (m: Uint8Array) => Array.from(m, (b) => b.toString(16).padStart(2, "0")).join(":");

function parseMac(s: string): Uint8Array | null {
  const h = s.replace(/[^0-9a-f]/gi, "");
  return h.length === 12 ? ota.hex(h) : null;
}

// ---- Bluetooth -------------------------------------------------------------------

class WebBluetoothTransport implements ota.Transport {
  writes = 0;
  reads = 0;
  constructor(private readonly chars: Record<ota.Char, BluetoothRemoteGATTCharacteristic>) {}
  async write(code: ota.Char, data: Uint8Array): Promise<void> {
    const ch = this.chars[code];
    const buf = data as Uint8Array<ArrayBuffer>;
    if (code === ota.OTA && ch.properties.writeWithoutResponse) await ch.writeValueWithoutResponse(buf);
    else await ch.writeValueWithResponse(buf);
    this.writes++;
  }
  async read(code: ota.Char): Promise<Uint8Array> {
    this.reads++;
    const v = await this.chars[code].readValue();
    return new Uint8Array(v.buffer.slice(v.byteOffset, v.byteOffset + v.byteLength));
  }
  sleep(ms: number): Promise<void> {
    return new Promise((r) => setTimeout(r, ms));
  }
}

/** Product and MAC from the advertisement, where the browser shows it (Chrome needs a flag for this). */
async function advertisement(device: BluetoothDevice): Promise<{ product: number; mac6: Uint8Array } | null> {
  if (!("watchAdvertisements" in device)) return null;
  return new Promise((resolve) => {
    const ctl = new AbortController();
    let settled = false;
    const done = (v: { product: number; mac6: Uint8Array } | null) => {
      if (settled) return;
      settled = true;
      ctl.abort();
      device.removeEventListener("advertisementreceived", on);
      resolve(v);
    };
    const on = (ev: Event) => {
      const md = (ev as BluetoothAdvertisingEvent).manufacturerData?.get(ota.AWOX_VENDOR);
      if (md) done(ota.parseAdvertisement(new Uint8Array(md.buffer, md.byteOffset, md.byteLength)));
    };
    device.addEventListener("advertisementreceived", on);
    device.watchAdvertisements({ signal: ctl.signal }).catch(() => done(null));
    setTimeout(() => done(null), 4000);
  });
}

async function readText(service: BluetoothRemoteGATTService, uuid: string): Promise<string | null> {
  try {
    const v = await (await service.getCharacteristic(uuid)).readValue();
    return new TextDecoder().decode(v).replace(/\0+$/, "").trim();
  } catch {
    return null;
  }
}

async function connect(): Promise<void> {
  resetPhases();
  phase("connect", "active");
  const services = [ota.TELINK_SERVICE, DEVINFO];
  const options: RequestDeviceOptions = ui.allDevices.checked
    ? { acceptAllDevices: true, optionalServices: services, optionalManufacturerData: [ota.AWOX_VENDOR] }
    : { filters: [{ manufacturerData: [{ companyIdentifier: ota.AWOX_VENDOR }] }], optionalServices: services,
        optionalManufacturerData: [ota.AWOX_VENDOR] };
  const device = await navigator.bluetooth.requestDevice(options);
  state.device = device;
  device.addEventListener("gattserverdisconnected", () => {
    if (!state.busy) log("disconnected");
    state.chars = null;
    render();
  });
  log(`selected ${device.name ?? "(no name)"}`);
  const adv = await advertisement(device);
  log(adv ? `advertisement: product ${hexId(adv.product)}, MAC ${macText(adv.mac6)}`
    : "this browser does not show the advertisement: the board is identified from Device Information", undefined, !adv);
  if (adv && !ui.mac.value) ui.mac.value = macText(adv.mac6);
  const server = await device.gatt!.connect();
  const mesh = await server.getPrimaryService(ota.TELINK_SERVICE);
  const chars = {
    [ota.PAIR]: await mesh.getCharacteristic(ota.telinkUuid(ota.PAIR)),
    [ota.OTA]: await mesh.getCharacteristic(ota.telinkUuid(ota.OTA)),
  } as Record<ota.Char, BluetoothRemoteGATTCharacteristic>;
  let model: string | null = null, revision: string | null = null, firmware: string | null = null;
  try {
    const di = await server.getPrimaryService(DEVINFO);
    model = await readText(di, "model_number_string");
    revision = await readText(di, "hardware_revision_string");
    firmware = await readText(di, "firmware_revision_string");
  } catch {
    log("the bulb has no Device Information service", "err");
  }
  const idle = new ota.Status(await new WebBluetoothTransport(chars).read(ota.OTA));
  state.chars = chars;
  state.bulb = { product: adv?.product ?? null, mac6: adv?.mac6 ?? null, model, revision, firmware, running: idle.layout };
  state.board = ota.identify(state.bulb, state.boards);
  log(`bulb: ${model ?? "?"} hardware ${revision ?? "?"}, firmware ${firmware ?? "?"}, ` +
      `running ${running(idle.layout)}; OTA status ${ota.toHex(idle.raw)}`, undefined);
  if (idle.state) log(`the OTA channel is not idle: ${idle.describe()}`, "err");
  phase("connect", "done");
  propose();
  render();
}

function running(l: ota.Layout): string {
  return l === "stock" ? "the original firmware" : l === "outglow" ? "Outglow" : "unknown firmware";
}

// ---- releases and images -----------------------------------------------------------------

async function sha256Hex(data: Uint8Array): Promise<string> {
  return ota.toHex(new Uint8Array(await crypto.subtle.digest("SHA-256", data as BufferSource)));
}

function takeImage(bytes: Uint8Array, source: string, local: boolean): void {
  state.img = null;
  state.info = null;
  try {
    const info = ota.checkImage(bytes, state.boards);
    state.img = bytes;
    state.info = info;
    state.local = local;
    log(`image ${source}: ${bytes.length} B for ${info.model} ${info.revision} ` +
        `(${info.outglow ? "Outglow" : "original firmware"}), header and CRC ok`, "ok");
  } catch (e) {
    log(`image ${source} refused: ${(e as Error).message}`, "err");
  }
  render();
}

async function loadManifest(): Promise<void> {
  try {
    const r = await fetch(`${BASE}firmware.json`, { cache: "no-cache" });
    state.manifest = (await r.json()) as ota.Manifest;
  } catch {
    log("no release list (firmware.json): local files only", undefined, true);
  }
  fillReleases();
}

function fillReleases(): void {
  const list = state.board ? state.manifest.images.filter((r) => r.board === state.board!.name) : state.manifest.images;
  const sorted = [...list].sort((a, b) => parseInt(b.fileVersion, 16) - parseInt(a.fileVersion, 16));
  ui.release.replaceChildren(new Option("—", ""), ...sorted.map((r) => new Option(`${r.board} — ${r.version}`, JSON.stringify(r))));
}

/** The newest release for the connected bulb's board, chosen and downloaded. */
function propose(): void {
  fillReleases();
  const b = state.bulb!;
  if (!state.board) {
    ui.proposal.textContent = `This bulb (${b.model ?? "?"}, hardware ${b.revision ?? "?"}, product ${hexId(b.product)}) ` +
      "is not a board Outglow supports. Nothing is proposed.";
    return;
  }
  const latest = ota.latestFor(state.manifest, state.board.name);
  if (!latest) {
    ui.proposal.textContent = `Board ${state.board.name}, running ${running(b.running)}. ` +
      "There is no release for it yet; a local image can be chosen under Advanced options.";
    return;
  }
  ui.proposal.textContent = `Board ${state.board.name}, running ${running(b.running)}. Proposed: Outglow ${latest.version}` +
    (latest.notes ? ` — ${latest.notes}` : "") + ".";
  ui.release.value = JSON.stringify(latest);
  void chooseRelease();
}

async function chooseRelease(): Promise<void> {
  if (!ui.release.value) return;
  const r = JSON.parse(ui.release.value) as ota.Release;
  try {
    log(`downloading ${r.url}`);
    const res = await fetch(new URL(r.url, new URL(BASE, location.href)));
    if (!res.ok) throw new Error(`download failed: HTTP ${res.status}`);
    const bytes = new Uint8Array(await res.arrayBuffer());
    if (r.size && bytes.length !== r.size) throw new Error(`downloaded ${bytes.length} B, the release says ${r.size} B`);
    if (r.sha256 && (await sha256Hex(bytes)) !== r.sha256.toLowerCase()) throw new Error("SHA-256 differs from the release list");
    takeImage(bytes, `${r.board} ${r.version}`, false);
  } catch (e) {
    log((e as Error).message, "err");
  }
}

// ---- the update ---------------------------------------------------------------------------

function problems(): string[] {
  const out: string[] = [];
  if (!state.chars || !state.bulb) out.push("Connect to a bulb.");
  if (!state.img || !state.info) out.push("Choose a firmware image.");
  if (state.info && !state.info.outglow && !(state.local && ui.stock.checked)) {
    out.push("This is the original firmware: allow it under Advanced options to go back to it.");
  }
  if (state.bulb && state.info) {
    let p = ota.matchBulb(state.info, state.bulb);
    const accepted = ui.anyRevision.checked && ui.anyRevisionText.value.trim().toLowerCase() === "i accept";
    if (accepted) p = p.filter((s) => !s.includes("hardware revision"));
    out.push(...p);
    if (state.bulb.product === null && !state.bulb.model) out.push("The bulb's model could not be read, so the image cannot be checked against it.");
    try {
      ota.pickChunk(state.bulb.running, chunk(), ota.CHUNK_CAP.stock + 7);
    } catch (e) {
      out.push((e as Error).message);
    }
  }
  if (ui.login.checked && !parseMac(ui.mac.value) && !ui.password.value.trim()) {
    out.push("Mesh login needs the bulb's MAC or a password (Advanced options), or switch the login off.");
  }
  if (!ui.confirm.checked) out.push("Tick the confirmation.");
  return out;
}

function chunk(): number | undefined {
  return ui.chunk.value ? Number(ui.chunk.value) : undefined;
}

async function start(): Promise<void> {
  const chars = state.chars!;
  const img = state.img!;
  const t = new WebBluetoothTransport(chars);
  state.busy = true;
  state.abort = new AbortController();
  for (const p of ["login", "erasing", "writing", "verifying", "reboot"] as const) phase(p, "");
  art.set(0, "busy");
  render();
  let lock: WakeLockSentinel | null = null;
  let current: PhaseName = "login";
  const enter = (p: PhaseName) => {
    if (current !== p) phase(current, "done");
    current = p;
    phase(p, "active");
  };
  try {
    lock = await navigator.wakeLock?.request("screen").catch(() => null);
    if (ui.login.checked) {
      enter("login");
      const pw = ui.password.value.trim()
        ? ota.hex(ui.password.value.replace(/[^0-9a-f]/gi, ""))
        : await ota.securePassword(parseMac(ui.mac.value)!);
      await t.write(ota.PAIR, await ota.loginPacket(ui.meshName.value || "unpaired", pw, crypto.getRandomValues(new Uint8Array(8))));
      const rsp = await t.read(ota.PAIR);
      if (rsp[0] !== 0x0d) {
        throw new ota.OtaError(`mesh login refused (${ota.toHex(rsp)}): a bulb paired to the vendor app has another mesh name and password`);
      }
      log("mesh login ok");
    } else {
      phase("login", "skipped");
    }
    const t0 = performance.now();
    let tWrite = 0;
    await ota.runOta(t, img, {
      chunk: chunk(),
      delayMs: Number(ui.delay.value) || 0,
      maxResends: Number(ui.resends.value) || 10,
      signal: state.abort.signal,
      log: (s, verbose) => log(s, undefined, verbose),
      phase: (p) => {
        enter(p);
        if (p === "writing" && !tWrite) tWrite = performance.now();
      },
      progress: (done, total) => {
        const f = done / total;
        ui.bar.value = 100 * f;
        art.set(f, "busy");
        ui.pct.textContent = `${(100 * f).toFixed(1)} %`;
        const s = (performance.now() - tWrite) / 1000;
        const bytes = done * (chunk() ?? ota.BLOCK);
        if (s > 2) {
          ui.speed.textContent = `${(bytes / s / 1024).toFixed(2)} KB/s`;
          ui.eta.textContent = `${Math.ceil((s / done) * (total - done))} s left`;
        }
      },
    });
    phase(current, "done");
    enter("reboot");
    art.set(1, "done");
    ui.eta.textContent = "";
    log(`the bulb has the image: ${t.writes} writes, ${t.reads} reads, ${Math.round((performance.now() - t0) / 1000)} s. ` +
        "It restarts by itself to install it; give it a minute, then switch it off and on if it stays dark.", "ok");
    phase("reboot", "done");
  } catch (e) {
    phase(current, "failed");
    art.set(ui.bar.value / 100, "failed");
    log(`update failed: ${(e as Error).message}. The bulb keeps the firmware it had; it can be tried again ` +
        "(switch the bulb off and on first).", "err");
  } finally {
    await lock?.release().catch(() => {});
    state.busy = false;
    render();
  }
}

/** A label, a value and an optional badge kind. */
type Fact = [string, string, ("ok" | "bad" | "info")?];

function facts(rows: Fact[]): HTMLElement {
  const dl = document.createElement("dl");
  for (const [k, v, kind] of rows) {
    const dt = Object.assign(document.createElement("dt"), { textContent: k });
    const dd = document.createElement("dd");
    if (kind) dd.append(Object.assign(document.createElement("span"), { className: `badge ${kind}`, textContent: v }));
    else dd.textContent = v;
    dl.append(dt, dd);
  }
  return dl;
}

function render(): void {
  const connected = !!state.chars;
  ui.connect.disabled = state.busy || connected || !navigator.bluetooth;
  ui.disconnect.disabled = state.busy || !connected;
  ui.stop.disabled = !state.busy;
  ui.release.disabled = state.busy;
  ui.anyRevisionConfirm.hidden = !ui.anyRevision.checked;
  const b = state.bulb;
  if (connected && b) {
    const rows: Fact[] = [
      ["Model", b.model ?? "unknown"],
      ["Hardware", b.revision ?? "unknown"],
      ["Board", state.board ? state.board.name : "not supported", state.board ? "ok" : "bad"],
      ["Running", b.running === "outglow" ? "Outglow" : b.running === "stock" ? "Original firmware" : "Unknown",
        b.running === "outglow" ? "ok" : b.running === "stock" ? "info" : "bad"],
      [b.running === "outglow" ? "Reported version" : "Firmware", (b.firmware ?? "unknown") +
        (b.running === "outglow" ? " (the original's, for the vendor app)" : "")],
    ];
    if (b.product !== null) rows.push(["Product", hexId(b.product)]);
    ui.bulb.replaceChildren(facts(rows));
  } else {
    ui.bulb.replaceChildren(Object.assign(document.createElement("p"), {
      className: "muted",
      textContent: "Not connected. Switch the bulb off and on first: the original firmware accepts Bluetooth for about 20 minutes after power-on.",
    }));
  }
  ui.image.textContent = state.info && state.img
    ? `${state.info.model} ${state.info.revision}, ${state.img.length} B, ${state.info.outglow ? "Outglow" : "original firmware"}` +
      `${state.local ? " (local file)" : ""}`
    : "No image chosen.";
  const p = state.busy ? [] : problems();
  ui.problems.replaceChildren(...p.map((s) => Object.assign(document.createElement("li"), { textContent: s })));
  ui.start.disabled = state.busy || p.length > 0;
}

function download(): void {
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([lines.join("\n") + "\n"], { type: "text/plain" }));
  a.download = `outglow-flasher-${new Date().toISOString().replace(/[:.]/g, "-")}.log`;
  a.click();
  URL.revokeObjectURL(a.href);
}

async function main(): Promise<void> {
  if (!navigator.bluetooth) ui.support.hidden = false;
  try {
    state.boards = (await (await fetch(`${BASE}boards.json`)).json()) as ota.Board[];
  } catch {
    log("could not load the board list (boards.json)", "err");
  }
  await loadManifest();
  ui.connect.onclick = () => connect().catch((e: Error) => {
    phase("connect", "failed");
    log(`connect: ${e.message}`, "err");
  });
  ui.disconnect.onclick = () => state.device?.gatt?.disconnect();
  ui.release.onchange = () => void chooseRelease();
  ui.file.onchange = async () => {
    const f = ui.file.files?.[0];
    if (f) takeImage(new Uint8Array(await f.arrayBuffer()), f.name, true);
  };
  for (const e of [ui.login, ui.mac, ui.password, ui.confirm, ui.stock, ui.anyRevision, ui.anyRevisionText, ui.chunk, ui.meshName]) {
    e.addEventListener("input", render);
  }
  ui.start.onclick = () => void start();
  ui.stop.onclick = () => state.abort?.abort();
  ui.copy.onclick = () => void navigator.clipboard.writeText(lines.join("\n"));
  ui.download.onclick = download;
  window.addEventListener("beforeunload", (e) => {
    if (state.busy) e.preventDefault();
  });
  render();
}

void main();
