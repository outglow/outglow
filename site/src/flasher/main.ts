// The browser flasher: Web Bluetooth transport, bulb detection, release
// proposal and the progress display around awox-ota.ts (the protocol, shared
// with the tests).

import { BADGES, productsFor, type BulbsFile } from "../bulbs.ts";
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
  detected: el("detected"),
  pickOutglow: el<HTMLInputElement>("pick-outglow"), pickOriginal: el<HTMLInputElement>("pick-original"),
  optOutglow: el("opt-outglow"), optOriginal: el("opt-original"),
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
  /** the image is the vendor's original firmware from the site's own copy (hash checked) */
  original: boolean;
  /** the site's copy of the original firmware (original/manifest.json) */
  originals: Original[];
  /** what the loaded image is, for the start button */
  label: string;
  /** why the last download failed, shown under the choice */
  loadError: string;
  /** the products of every board (bulbs.json), for the picture of the bulb found */
  bulbs: BulbsFile | null;
  /** which choices the connected bulb has */
  can: { outglow: boolean; original: boolean };
  busy: boolean;
  abort: AbortController | null;
}

const state: State = {
  boards: [], manifest: { schema: 1, images: [] }, device: null, chars: null, bulb: null, board: undefined,
  img: null, info: null, local: false, original: false, originals: [], label: "", loadError: "", can: { outglow: false, original: false }, bulbs: null, busy: false, abort: null,
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
  state.original = false;
  state.label = "";
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

type Original = { board: string; file: string; size: number; sha256: string; firmware_version: string };

const ORIGINAL_BASE = () => new URL(`${BASE}original/`, location.href);

async function loadOriginals(): Promise<void> {
  try {
    const r = await fetch(new URL("manifest.json", ORIGINAL_BASE()), { cache: "no-cache" });
    if (r.ok) state.originals = ((await r.json()) as { images: Original[] }).images;
  } catch {
    // not on this site: the option stays unavailable
  }
}

/** What can go on the connected bulb: Outglow's newest release and the original firmware. */
function propose(): void {
  fillReleases();
  const b = state.bulb!;
  const latest = state.board ? ota.latestFor(state.manifest, state.board.name) : undefined;
  const original = state.board ? state.originals.find((o) => o.board === state.board!.name) : undefined;
  if (!state.board) {
    ui.proposal.textContent = `No matching Outglow build was found for ${b.model ?? "this model"}, hardware ${b.revision ?? "unknown"}.`;
  } else {
    ui.proposal.textContent = b.running === "outglow" ? "This bulb runs Outglow." :
      b.running === "stock" ? "This bulb runs its original firmware." : "";
  }
  state.can = { outglow: !!latest, original: !!original };
  ui.pickOutglow.disabled = !latest;
  ui.optOutglow.textContent = latest
    ? `Version ${latest.version}${b.running === "outglow" ? ": update" : ""}`
    : state.board ? "No release for this bulb yet" : "—";
  ui.pickOriginal.disabled = !original;
  ui.optOriginal.textContent = original
    ? `${original.firmware_version} from the manufacturer`
    : state.board ? "Not available on this site" : "—";
  // the obvious choice: Outglow, unless there is nothing to install but the original
  const pick = latest ? ui.pickOutglow : original && b.running === "outglow" ? ui.pickOriginal : null;
  ui.pickOutglow.checked = pick === ui.pickOutglow;
  ui.pickOriginal.checked = pick === ui.pickOriginal;
  void loadChoice();
}

/** Forgets the image loaded before: a new choice starts empty. */
function clearImage(note: string): number {
  state.img = null;
  state.info = null;
  state.original = false;
  state.local = false;
  state.label = "";
  state.loadError = "";
  ui.image.textContent = note;
  render();
  return ++loadSeq;
}

/** Every load gets a number; one that finishes after a newer choice is dropped. */
let loadSeq = 0;

/** Downloads what the choice says. */
async function loadChoice(): Promise<void> {
  if (ui.pickOriginal.checked) return restoreOriginal();
  if (ui.pickOutglow.checked && state.board) {
    if (!ui.release.value) {
      const latest = ota.latestFor(state.manifest, state.board.name);
      if (latest) ui.release.value = JSON.stringify(latest);
    }
    return chooseRelease();
  }
}

async function chooseRelease(): Promise<void> {
  if (!ui.release.value) return;
  const r = JSON.parse(ui.release.value) as ota.Release;
  const seq = clearImage(`Downloading Outglow ${r.version}…`);
  try {
    log(`downloading Outglow ${r.version} for ${r.board}`);
    const res = await fetch(new URL(r.url, new URL(BASE, location.href)));
    if (!res.ok) throw new Error(`download failed: HTTP ${res.status}`);
    const bytes = new Uint8Array(await res.arrayBuffer());
    if (r.size && bytes.length !== r.size) throw new Error(`downloaded ${bytes.length} B, the release says ${r.size} B`);
    if (r.sha256 && (await sha256Hex(bytes)) !== r.sha256.toLowerCase()) throw new Error("SHA-256 differs from the release list");
    if (seq !== loadSeq) return;
    takeImage(bytes, `${r.board} ${r.version}`, false);
    if (state.info) state.label = `Install Outglow ${r.version}`;
  } catch (e) {
    if (seq !== loadSeq) return;
    log((e as Error).message, "err");
    state.loadError = `Outglow ${r.version} could not be downloaded: ${(e as Error).message}`;
  }
  render();
}

/** The original firmware of the connected bulb's board, from the site's copy. */
async function restoreOriginal(): Promise<void> {
  const e = state.board && state.originals.find((o) => o.board === state.board!.name);
  if (!e) return;
  const seq = clearImage(`Downloading the original firmware ${e.firmware_version}…`);
  try {
    log(`downloading the original firmware ${e.firmware_version} for ${e.board}`);
    const r = await fetch(new URL(e.file, ORIGINAL_BASE()));
    if (!r.ok) throw new Error(`download failed: HTTP ${r.status}`);
    const bytes = new Uint8Array(await r.arrayBuffer());
    if (bytes.length !== e.size || (await sha256Hex(bytes)) !== e.sha256) throw new Error("the original firmware does not match its checksum");
    if (seq !== loadSeq) return;
    takeImage(bytes, `original ${e.board} ${e.firmware_version}`, false);
    if (state.info && !state.info.outglow) {
      state.original = true;
      state.label = "Restore the original firmware";
    }
  } catch (err) {
    if (seq !== loadSeq) return;
    log((err as Error).message, "err");
    state.loadError = `The original firmware could not be downloaded: ${(err as Error).message}`;
  }
  render();
}

// ---- the update ---------------------------------------------------------------------------

function problems(): string[] {
  const out: string[] = [];
  if (!state.chars || !state.bulb) out.push("Connect to a bulb.");
  if (!state.img || !state.info) out.push("Choose a firmware image.");
  if (state.info && !state.info.outglow && !state.original && !(state.local && ui.stock.checked)) {
    out.push("This is the original firmware: use Restore original firmware, or allow a local file under Advanced options.");
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
        "The bulb should restart to install it. This tool has not confirmed that the new firmware booted. Give it a minute, then switch it off and on if it stays dark.", "ok");
    phase("reboot", "done");
  } catch (e) {
    phase(current, "failed");
    art.set(ui.bar.value / 100, "failed");
    log(`update failed: ${(e as Error).message}. The tool could not confirm completion. Check whether the bulb reconnects before retrying ` +
        "(switch the bulb off and on first).", "err");
  } finally {
    await lock?.release().catch(() => {});
    state.busy = false;
    render();
  }
}

/** The bulb found: a picture and the name of the product it most likely is. */
function renderDetected(b: ota.Bulb | null): void {
  const found = b && state.bulbs && b.model && b.revision ? productsFor(state.bulbs, b.model, b.revision) : null;
  const best = found?.products[0];
  ui.detected.hidden = !found;
  if (!found) {
    ui.detected.replaceChildren();
    return;
  }
  const make = <K extends keyof HTMLElementTagNameMap>(tag: K, cls?: string, text?: string) =>
    Object.assign(document.createElement(tag), cls ? { className: cls } : {}, text !== undefined ? { textContent: text } : {});
  const pic = make("div", "detected-pic");
  if (best?.image) {
    const img = make("img");
    img.src = BASE + best.image;
    img.alt = "";
    pic.append(img);
  }
  const text = make("div", "detected-text");
  text.append(make("span", "muted small", "Found"));
  if (best) {
    text.append(make("strong", undefined, best.name));
    text.append(make("span", "muted", `${best.brand} ${best.article}`));
    const others = found.products.length - 1;
    if (others > 0) {
      const a = make("a", "small", `or one of ${others} other product${others > 1 ? "s" : ""} with this board`);
      a.href = `${BASE}bulbs.html?q=${encodeURIComponent(found.b.board)}`;
      a.target = "_blank";
      text.append(a);
    }
  } else {
    text.append(make("strong", undefined, found.b.board));
    text.append(make("span", "muted", "No product known with this board yet"));
  }
  text.append(make("span", `badge kind-${found.b.kind.replace("outglow-", "")}`, BADGES[found.b.kind] ?? found.b.kind));
  ui.detected.replaceChildren(pic, text);
}

/** A label, a value and an optional badge kind. */
type Fact = [string, string, ("ok" | "bad" | "neutral")?];

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
  ui.pickOutglow.disabled = state.busy || !state.can.outglow;
  ui.pickOriginal.disabled = state.busy || !state.can.original;
  ui.pickOutglow.closest("label")?.classList.toggle("off", ui.pickOutglow.disabled);
  ui.pickOriginal.closest("label")?.classList.toggle("off", ui.pickOriginal.disabled);
  ui.start.textContent = state.label || (state.local && state.info ? (state.info.outglow ? "Install this image" : "Install this original firmware") : "Start the update");
  ui.anyRevisionConfirm.hidden = !ui.anyRevision.checked;
  const b = state.bulb;
  renderDetected(connected && b ? b : null);
  if (connected && b) {
    const rows: Fact[] = [
      ["Model", b.model ?? "unknown"],
      ["Hardware", b.revision ?? "unknown"],
      ["Board", state.board ? state.board.name : "not supported", state.board ? "ok" : "bad"],
      ["Running", b.running === "outglow" ? "Outglow" : b.running === "stock" ? "Original firmware" : "Unknown",
        b.running === "outglow" ? "ok" : b.running === "stock" ? "neutral" : "bad"],
      [b.running === "outglow" ? "Reported version" : "Firmware", (b.firmware ?? "unknown") +
        (b.running === "outglow" ? " (the original's, for the vendor app)" : "")],
    ];
    if (b.product !== null) rows.push(["Product", hexId(b.product)]);
    ui.bulb.replaceChildren(facts(rows));
  } else {
    ui.bulb.replaceChildren(Object.assign(document.createElement("p"), {
      className: "muted",
      textContent: "Not connected. Power the bulb on and keep it nearby. Disconnect other apps before connecting here.",
    }));
  }
  ui.image.textContent = state.info && state.img
    ? `Ready: ${state.local ? "local file, " : ""}${state.info.outglow ? "Outglow" : "original firmware"} for ` +
      `${state.info.model} ${state.info.revision}, ${Math.round(state.img.length / 1024)} KB, checked.`
    : state.loadError || ui.image.textContent || "";
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
  await loadOriginals();
  try {
    state.bulbs = (await (await fetch(`${BASE}bulbs.json`)).json()) as BulbsFile;
  } catch {
    // no pictures, the facts are enough
  }
  ui.connect.onclick = () => connect().catch((e: Error) => {
    phase("connect", "failed");
    log(`connect: ${e.message}`, "err");
  });
  ui.disconnect.onclick = () => state.device?.gatt?.disconnect();
  ui.release.onchange = () => {
    ui.pickOutglow.checked = true;
    void chooseRelease();
  };
  ui.pickOutglow.onchange = () => void loadChoice();
  ui.pickOriginal.onchange = () => void loadChoice();
  ui.file.onchange = async () => {
    const f = ui.file.files?.[0];
    if (!f) return;
    ui.pickOutglow.checked = ui.pickOriginal.checked = false;
    clearImage("");
    takeImage(new Uint8Array(await f.arrayBuffer()), f.name, true);
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
