// The console: one bulb over Web Bluetooth, its diagnostic service read every
// second (crates/ble/src/diag.rs), light control and settings after a mesh login.
import * as D from "./diag";
import { demoChars, type Char, type Chars } from "./demo";
import { AWOX_VENDOR, TELINK_SERVICE, loginPacket, parseAdvertisement, securePassword, telinkUuid, PAIR } from "../flasher/awox-ota";

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const hex4 = (v: number) => "0x" + v.toString(16).padStart(4, "0");
const num = (v: number) => v.toLocaleString("en-GB");
const pct = (r: number) => (r * 100).toFixed(1) + " %";
const kelvin = (m: number) => Math.round(1e6 / m / 10) * 10;
const params = new URLSearchParams(location.search);

let device: BluetoothDevice | null = null;
let mac6: Uint8Array | null = null;
let chars: Chars | null = null;
let timer: number | undefined;
let tick = 0;
let state: D.State | null = null;
let holdUntil = 0; // after the user moves a control, don't overwrite it with a read for a moment
const hold = () => { holdUntil = Date.now() + 2500; };
const openCounters = new Set<number>();

function status(text: string, cls?: "on" | "err") {
  $("status-text").textContent = text;
  $("status").className = "pill" + (cls ? " " + cls : "");
}
function message(text: string) {
  $("message").hidden = !text;
  $("message").textContent = text;
}
const tag = (text: string | number, cls = "") => `<span class="tag ${cls}">${text}</span>`;
const loggedInSets = ["control-set", "config-set", "service-set"];

function renderInfo(i: D.Info) {
  $("title").textContent = i.model || "Console";
  $("subtitle").textContent = `firmware ${i.firmware}`;
  $("model").textContent = i.model;
  $("fw").textContent = i.firmware;
  $("variant").textContent = i.variant === "tw" ? "Brightness and white-tone controls" : "Brightness controls";
  $("ieee").textContent = i.ieee;
  $("has-wd").innerHTML = i.watchdog ? tag("on", "good") : tag("off");
}

function renderState(s: D.State) {
  state = s;
  $("uptime").textContent = D.formatDuration(s.uptime);
  $("cause").textContent = "since a " + s.resetCauseText;
  $("wd").innerHTML = s.watchdogResets ? tag(s.watchdogResets, "warn") : "0";
  $("boots").textContent = String(s.bootCount);
  $("step").textContent = s.resetStep ? `step ${s.resetStep} of 7` : "not started";

  const z = s.zigbee;
  $("zb-state").textContent = z.joined ? "In a network" : "Not in a network";
  $("zb-sub").textContent = z.joined ? `channel ${z.channel}` : "not currently joined; pairing may need to be enabled";
  $("zb-addr").textContent = z.address !== null ? hex4(z.address) : "–";
  $("zb-pan").textContent = z.pan !== null ? hex4(z.pan) : "–";
  $("zb-chan").textContent = String(z.channel);
  $("zb-neigh").innerHTML = `${z.routers <= 1 && z.joined ? tag(z.routers, "warn") : z.routers} routers, ${z.children} connected end devices`;
  $("zb-signal").textContent = z.lqi ? `Quality ${z.lqi} of 255; strength ${z.rssi} dBm` : "–";
  $("lqi-bar").style.width = Math.round((z.lqi * 100) / 255) + "%";

  const m = s.mac;
  $("retry-rate").innerHTML = m.tx ? `${pct(m.retryRate)} ${m.retryRate > 0.3 ? tag("many retries", "bad") : m.retryRate > 0.15 ? tag("elevated retries", "warn") : tag("few retries", "good")}` : "–";
  $("mac-tx").textContent = num(m.tx);
  $("mac-rx").textContent = num(m.rx);
  $("mac-retry").textContent = num(m.retry);
  $("mac-fail").innerHTML = m.fail ? `${num(m.fail)} ${m.failRate > 0.01 ? tag(pct(m.failRate), "warn") : ""}` : "0";

  const l = s.light;
  $("light-state").textContent = l.on ? `${l.percent} %` : "Off";
  $("light-sub").textContent = (l.on ? "on" : `saved brightness ${l.percent} %`) + (l.kelvin ? `, ${l.kelvin} K` : "");
  $("toggle").textContent = l.on ? "Turn off" : "Turn on";
  const level = $<HTMLInputElement>("level-in"), ct = $<HTMLInputElement>("ct-in");
  if (Date.now() > holdUntil) {
    level.value = String(l.level);
    if (l.mireds) ct.value = String(l.mireds);
  }
  $("level-out").textContent = `${Math.round((+level.value * 100) / 254)} %`;
  if (l.mireds) $("ct-out").textContent = `${kelvin(+ct.value)} K`;

  $("login-card").hidden = s.loggedIn;
  for (const id of loggedInSets) $<HTMLFieldSetElement>(id).disabled = !s.loggedIn;
}

function renderConfig(c: D.Config) {
  for (const id of ["ct-field", "su-ct-field", "opt-ct-exec-row", "opt-couple-row"]) $(id).hidden = !c.hasColor;
  if (c.hasColor) for (const id of ["ct-in", "su-ct"]) { $<HTMLInputElement>(id).min = String(c.ctMin); $<HTMLInputElement>(id).max = String(c.ctMax); }
  if (Date.now() <= holdUntil) return;
  $<HTMLSelectElement>("su-onoff").value = String(c.startUpOnOff);
  const fixed = c.startUpLevel !== 0xff && c.startUpLevel !== 0;
  $<HTMLSelectElement>("su-level-mode").value = fixed ? "set" : String(c.startUpLevel);
  $("su-level").hidden = !fixed;
  if (fixed) $<HTMLInputElement>("su-level").value = String(c.startUpLevel);
  $<HTMLSelectElement>("su-ct-mode").value = c.startUpColorTemp ? "set" : "prev";
  $("su-ct").hidden = !c.startUpColorTemp;
  if (c.startUpColorTemp) $<HTMLInputElement>("su-ct").value = String(c.startUpColorTemp);
  $<HTMLInputElement>("opt-exec").checked = c.executeIfOff;
  $<HTMLInputElement>("opt-couple").checked = c.coupleColorTemp;
  $<HTMLInputElement>("opt-ct-exec").checked = c.colorExecuteIfOff;
}

const ago = (uptime: number, t: number) => D.formatDuration(Math.max(0, uptime - t)) + " ago";

function renderFrames(frames: D.Frame[], uptime: number) {
  $("frames").innerHTML = frames.length
    ? frames.map((f) => `<tr><td class="muted">${ago(uptime, f.time)}</td><td>${f.src === "0x0000" ? "coordinator" : f.src}</td>` +
      `<td>${f.cluster}</td><td>${f.command}${f.manufacturer ? " (manufacturer)" : ""}</td><td>Quality ${f.lqi} of 255; ${f.rssi} dBm</td></tr>`).join("")
    : `<tr><td colspan="5" class="muted">No command has arrived since the start.</td></tr>`;
}

function renderLog(entries: D.LogEntry[], uptime: number) {
  $("log").innerHTML = entries.length
    ? entries.map((e) => `<li><time>${e.previous ? "before this start" : ago(uptime, e.time)}</time><span>${e.text}</span></li>`).join("")
    : `<li class="muted">Nothing yet.</li>`;
}

function renderCounters(c: D.Counters) {
  $("counters-card").hidden = false;
  $("counters-sub").textContent = `Relative to transmitted direct messages: retries ${pct(c.macRetryRate)}, failed sends ${pct(c.macFailRate)}. A message can be retried more than once, so these are ratios of counts, not percentages of affected messages.`;
  $("counters").innerHTML = c.list.map((a) => {
    const open = openCounters.has(a.id);
    return `<tr><td><span class="feature">${a.name}<button type="button" class="info" data-counter="${a.id}" aria-expanded="${open}" ` +
      `aria-label="About: ${a.name}">i</button></span></td><td class="muted mono">${a.idText}${a.manufacturer ? " · Outglow" : ""}</td>` +
      `<td class="num">${num(a.value)}</td></tr>` +
      `<tr class="more"${open ? "" : " hidden"}><td colspan="3">${a.what}${a.healthy ? ` <strong>How to read it:</strong> ${a.healthy}` : ""}</td></tr>`;
  }).join("");
}

$("counters").addEventListener("click", (ev) => {
  const b = (ev.target as HTMLElement).closest<HTMLButtonElement>("button[data-counter]");
  if (!b) return;
  const id = +b.dataset.counter!;
  const open = !openCounters.has(id);
  if (open) openCounters.add(id); else openCounters.delete(id);
  b.setAttribute("aria-expanded", String(open));
  (b.closest("tr")!.nextElementSibling as HTMLElement).hidden = !open;
});

async function poll() {
  if (!chars) return;
  try {
    const s = D.decodeState(await chars.state.readValue());
    renderState(s);
    if (tick++ % 2 === 0) {
      renderConfig(D.decodeConfig(await chars.config.readValue()));
      renderFrames(D.decodeFrames(await chars.frames.readValue()), s.uptime);
      renderLog(D.decodeLog(await chars.log.readValue()), s.uptime);
      if (chars.counters) {
        try { renderCounters(D.decodeCounters(await chars.counters.readValue())); } catch { chars.counters = null; }
      }
    }
  } catch (e) {
    if (chars) message("Could not read the bulb: " + (e as Error).message);
  }
}

/** A command, then the result the bulb puts in state[39]. */
async function send(bytes: Uint8Array, what: string) {
  if (!chars) return;
  try {
    const before = state ? state.cmdCount : -1;
    await chars.cmd.writeValue(bytes as BufferSource);
    const s = D.decodeState(await chars.state.readValue());
    renderState(s);
    if (s.cmdCount === before) message(`${what}: the bulb did not confirm the command.`);
    else if (s.cmdResult) message(`${what}: ${s.cmdResultText}.`);
    else message("");
  } catch (e) {
    message(`${what}: ${(e as Error).message}`);
  }
}

async function tryLogin(name: string, password: string | Uint8Array): Promise<boolean> {
  if (!chars) return false;
  await chars.pair.writeValue(await loginPacket(name, password, crypto.getRandomValues(new Uint8Array(8))) as BufferSource);
  return (await chars.pair.readValue()).getUint8(0) === 0x0d;
}

async function login() {
  const name = $<HTMLInputElement>("mesh-name").value || "unpaired", pass = $<HTMLInputElement>("mesh-pass").value;
  try {
    let ok = await tryLogin(name, pass);
    // a bulb never added to the app: newer products derive the password from their address
    if (!ok && name === "unpaired" && mac6) ok = await tryLogin(name, await securePassword(mac6));
    if (!ok) { message("Login refused: wrong network name or password."); return; }
    message("");
    await poll();
  } catch (e) {
    message("Login: " + (e as Error).message);
  }
}

function stop(text?: string) {
  clearInterval(timer);
  chars = null;
  status(text ?? "disconnected", "err");
  $("connect").textContent = "Connect again";
  $<HTMLButtonElement>("identify").disabled = true;
  for (const id of loggedInSets) $<HTMLFieldSetElement>(id).disabled = true;
  $("login-card").hidden = true;
}

async function start(c: Chars, name: string) {
  chars = c;
  $("counters-card").hidden = true;
  renderInfo(D.decodeInfo(await c.info.readValue()));
  status("connected: " + name, "on");
  $("connect").textContent = "Disconnect";
  $<HTMLButtonElement>("identify").disabled = false;
  $<HTMLDetailsElement>("about").open = false;
  message("");
  tick = 0;
  await poll();
  timer = window.setInterval(poll, 1000);
}

function wrap(c: BluetoothRemoteGATTCharacteristic): Char {
  return { readValue: () => c.readValue(), writeValue: (v) => c.writeValue(v) };
}

async function open(dev: BluetoothDevice) {
  status("connecting…");
  const gatt = await dev.gatt!.connect();
  let svc: BluetoothRemoteGATTService;
  try {
    svc = await gatt.getPrimaryService(D.UUID.service);
  } catch {
    gatt.disconnect();
    throw new Error("The console service could not be opened. Check that this bulb runs an Outglow version with console support and that the Bluetooth connection is working.");
  }
  const mesh = await gatt.getPrimaryService(TELINK_SERVICE);
  const c: Partial<Chars> = { pair: wrap(await mesh.getCharacteristic(telinkUuid(PAIR))), counters: null };
  for (const k of ["info", "state", "log", "cmd", "frames", "config"] as const) c[k] = wrap(await svc.getCharacteristic(D.UUID[k]));
  try { c.counters = wrap(await svc.getCharacteristic(D.UUID.counters)); } catch { c.counters = null; }
  await start(c as Chars, dev.name ?? "bulb");
}

async function advertisedMac(dev: BluetoothDevice): Promise<Uint8Array | null> {
  if (!("watchAdvertisements" in dev)) return null;
  return new Promise((resolve) => {
    const ctl = new AbortController();
    const done = (v: Uint8Array | null) => { ctl.abort(); dev.removeEventListener("advertisementreceived", on); resolve(v); };
    const on = (ev: Event) => {
      const md = (ev as BluetoothAdvertisingEvent).manufacturerData?.get(AWOX_VENDOR);
      if (md) done(parseAdvertisement(new Uint8Array(md.buffer, md.byteOffset, md.byteLength))?.mac6 ?? null);
    };
    dev.addEventListener("advertisementreceived", on);
    dev.watchAdvertisements({ signal: ctl.signal }).catch(() => done(null));
    setTimeout(() => done(null), 3000);
  });
}

async function connect() {
  if (device?.gatt?.connected) { device.gatt.disconnect(); return; }
  message("");
  try {
    if (!device) {
      device = await navigator.bluetooth.requestDevice({
        filters: [{ manufacturerData: [{ companyIdentifier: AWOX_VENDOR }] }],
        optionalServices: [D.UUID.service, TELINK_SERVICE],
      });
      device.addEventListener("gattserverdisconnected", () => stop());
      mac6 = await advertisedMac(device);
    }
    await open(device);
  } catch (e) {
    if ((e as Error).name === "NotFoundError") { status("not connected"); return; } // the chooser was closed
    status("error", "err");
    message((e as Error).message);
  }
}

// ---- controls ----------------------------------------------------------------------
for (const b of document.querySelectorAll<HTMLButtonElement>("h2 > button.info")) {
  b.addEventListener("click", () => {
    const open = b.getAttribute("aria-expanded") !== "true";
    b.setAttribute("aria-expanded", String(open));
    $(b.getAttribute("aria-controls")!).hidden = !open;
  });
}
$("connect").addEventListener("click", connect);
$("identify").addEventListener("click", () => send(D.commands.identify(), "Blink"));
$("login").addEventListener("click", login);
$("toggle").addEventListener("click", () => send(D.commands.onOff(state?.light.on ? 0 : 1), "Light"));
const levelIn = $<HTMLInputElement>("level-in"), ctIn = $<HTMLInputElement>("ct-in");
levelIn.addEventListener("input", () => { hold(); $("level-out").textContent = `${Math.round((+levelIn.value * 100) / 254)} %`; });
levelIn.addEventListener("change", () => { hold(); send(D.commands.level(+levelIn.value), "Brightness"); });
ctIn.addEventListener("input", () => { hold(); $("ct-out").textContent = `${kelvin(+ctIn.value)} K`; });
ctIn.addEventListener("change", () => { hold(); send(D.commands.colorTemp(+ctIn.value), "Colour"); });
$("su-onoff").addEventListener("change", () => { hold(); send(D.commands.startUpOnOff(+$<HTMLSelectElement>("su-onoff").value), "Setting"); });
const sendSuLevel = () => {
  hold();
  const mode = $<HTMLSelectElement>("su-level-mode").value;
  $("su-level").hidden = mode !== "set";
  const fixed = Math.max(1, Math.min(254, +$<HTMLInputElement>("su-level").value || 254));
  send(D.commands.startUpLevel(mode === "set" ? fixed : +mode), "Setting");
};
$("su-level-mode").addEventListener("change", sendSuLevel);
$("su-level").addEventListener("change", sendSuLevel);
const sendSuCt = () => {
  hold();
  const set = $<HTMLSelectElement>("su-ct-mode").value === "set";
  $("su-ct").hidden = !set;
  send(D.commands.startUpColorTemp(set ? +$<HTMLInputElement>("su-ct").value : 0xffff), "Setting");
};
$("su-ct-mode").addEventListener("change", sendSuCt);
$("su-ct").addEventListener("change", sendSuCt);
const sendLevelOptions = () => {
  hold();
  send(D.commands.levelOptions(($<HTMLInputElement>("opt-exec").checked ? 1 : 0) | ($<HTMLInputElement>("opt-couple").checked ? 2 : 0)), "Setting");
};
$("opt-exec").addEventListener("change", sendLevelOptions);
$("opt-couple").addEventListener("change", sendLevelOptions);
$("opt-ct-exec").addEventListener("change", () => { hold(); send(D.commands.colorOptions($<HTMLInputElement>("opt-ct-exec").checked ? 1 : 0), "Setting"); });
$("reboot").addEventListener("click", () => send(D.commands.reboot(), "Restart"));
let armed = 0;
$("factory").addEventListener("click", () => {
  const b = $("factory");
  if (Date.now() < armed) {
    armed = 0;
    b.textContent = "Factory reset";
    send(D.commands.factoryReset(), "Factory reset");
  } else {
    armed = Date.now() + 5000;
    b.textContent = "Tap again to reset";
    setTimeout(() => { if (Date.now() >= armed) b.textContent = "Factory reset"; }, 5100);
  }
});

if (params.has("demo") || location.hash === "#demo") {
  $("demo-note").hidden = false;
  $<HTMLButtonElement>("connect").disabled = true;
  void start(demoChars(), "sample bulb");
} else if (!navigator.bluetooth) {
  $("support").hidden = false;
  $<HTMLButtonElement>("connect").disabled = true;
}
