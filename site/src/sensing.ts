// The motion sensing page: a room with three bulbs and a person walking
// between them, and one link's detector drawn as it decides. The detector is
// the firmware's (crates/sense): at most one kept sample per 80 ms, the
// median of the 5 before, weight = floor(|x - median| * 4 / threshold) up
// to 3, the sum of the last 16 weights against M (`params`), held for
// 10 ms * (100 - s) before the link reports motion. The radio channel is a
// toy: a steady level with a little noise, which a moving body near the
// line between two bulbs makes wobble by several dB.

type Pt = { x: number; y: number };

const BULBS: Pt[] = [
  { x: 0.16, y: 0.3 },
  { x: 0.84, y: 0.3 },
  { x: 0.5, y: 0.86 },
];
const NAMES = ["A", "B", "C"];
const LINKS: [number, number][] = [
  [0, 1],
  [1, 2],
  [2, 0],
];
/// Probes per second (the default rate once sensing is on).
const RATE = 5;
const HIST = 5;
const WIN = 16;
const MIN_GAP_MS = 80;
/// Shortened for the demo (the firmware's default is 30 s).
const OFF_DELAY_MS = 3000;
const CHART_MS = 10_000;

/// Threshold [1/4 dB] and M for a sensitivity (crates/sense `params`).
function params(s: number): [number, number] {
  s = Math.min(100, Math.max(0, Math.round(s)));
  if (s >= 50) return [4, 1 + Math.floor(((100 - s) * 7 + 25) / 50)];
  return [5 + Math.floor(((49 - s) * 35 + 24) / 48), 8];
}

/// The person's walk: points and how long to take to reach each (ms); a
/// repeated point is standing still.
const WALK: [Pt, number][] = [
  [{ x: -0.08, y: 0.62 }, 0],
  [{ x: 0.42, y: 0.12 }, 3200],
  [{ x: 0.42, y: 0.12 }, 6500],
  [{ x: 0.7, y: 0.62 }, 2600],
  [{ x: 1.1, y: 0.7 }, 2200],
  [{ x: 1.1, y: 0.7 }, 3500],
];
const WALK_MS = WALK.reduce((a, w) => a + w[1], 0);

function personAt(t: number): { p: Pt; moving: boolean } {
  let u = ((t % WALK_MS) + WALK_MS) % WALK_MS;
  for (let i = 1; i < WALK.length; i++) {
    const [to, d] = WALK[i]!;
    const from = WALK[i - 1]![0];
    if (u <= d) {
      const f = d ? u / d : 1;
      const p = { x: from.x + (to.x - from.x) * f, y: from.y + (to.y - from.y) * f };
      return { p, moving: from.x !== to.x || from.y !== to.y };
    }
    u -= d;
  }
  return { p: WALK[0]![0], moving: false };
}

function distToSegment(p: Pt, a: Pt, b: Pt): number {
  const dx = b.x - a.x, dy = b.y - a.y;
  const l2 = dx * dx + dy * dy;
  const f = Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / l2));
  return Math.hypot(p.x - (a.x + f * dx), p.y - (a.y + f * dy));
}

// a reproducible pseudo-random source, so the demo looks the same each time
let seed = 0x2545f491;
function rand(): number {
  seed ^= seed << 13;
  seed ^= seed >>> 17;
  seed ^= seed << 5;
  return ((seed >>> 0) % 1_000_000) / 1_000_000;
}
function gauss(): number {
  return Math.sqrt(-2 * Math.log(rand() + 1e-9)) * Math.cos(2 * Math.PI * rand());
}

interface Sample { t: number; rssi: number; median: number | null; weight: number; on: boolean }

class Link {
  hist: number[] = [];
  dev: number[] = [];
  last = -1e9;
  ruleSince: number | null = null;
  lastHit = -1e9;
  on = false;
  samples: Sample[] = [];
  constructor(public a: number, public b: number, public base: number) {}

  /// The disturbance a person causes on this link now: 0..1.
  near(p: Pt): number {
    return Math.max(0, 1 - distToSegment(p, BULBS[this.a]!, BULBS[this.b]!) / 0.17);
  }

  frame(t: number, p: Pt, moving: boolean, s: number) {
    if (t - this.last < MIN_GAP_MS) return;
    this.last = t;
    const e = this.near(p);
    const rssi = Math.round(this.base + 0.7 * gauss() - 5 * e + (moving ? 9 * e * gauss() : 0.4 * e * gauss()));
    let median: number | null = null;
    let weight = 0;
    if (this.hist.length === HIST) {
      median = [...this.hist].sort((x, y) => x - y)[2]!;
      const d = Math.min(63, Math.abs(rssi - median));
      this.dev.push(d);
      if (this.dev.length > WIN) this.dev.shift();
      const [thr, m] = params(s);
      weight = Math.min(3, Math.floor((d * 4) / thr));
      const sum = this.dev.reduce((acc, x) => acc + Math.min(3, Math.floor((x * 4) / thr)), 0);
      if (sum >= m) {
        if (this.ruleSince === null) this.ruleSince = t;
        this.lastHit = t;
        if (t - this.ruleSince >= 10 * (100 - s)) this.on = true;
      } else {
        this.ruleSince = null;
      }
    }
    if (this.on && t - this.lastHit > OFF_DELAY_MS) this.on = false;
    this.hist.push(rssi);
    if (this.hist.length > HIST) this.hist.shift();
    this.samples.push({ t, rssi, median, weight, on: this.on });
    while (this.samples.length && this.samples[0]!.t < t - CHART_MS - 500) this.samples.shift();
  }

  sum(s: number): number {
    const [thr] = params(s);
    return this.dev.reduce((acc, x) => acc + Math.min(3, Math.floor((x * 4) / thr)), 0);
  }
}

const links = LINKS.map(([a, b], i) => new Link(a, b, [-63, -69, -66][i]!));
const room = document.getElementById("room") as HTMLCanvasElement;
const chart = document.getElementById("link") as HTMLCanvasElement;
const slider = document.getElementById("s") as HTMLInputElement;
const sOut = document.getElementById("s-out")!;
const paramsOut = document.getElementById("params")!;
const roomState = document.getElementById("room-state")!;
const pauseBtn = document.getElementById("pause") as HTMLButtonElement;

let s = Number(slider.value);
function showParams() {
  const [thr, m] = params(s);
  sOut.textContent = String(s);
  paramsOut.textContent = `threshold ${(thr / 4).toLocaleString("en", { maximumFractionDigits: 2 })} dB · M = ${m} · held for ${10 * (100 - s)} ms`;
}
slider.addEventListener("input", () => {
  s = Number(slider.value);
  showParams();
});
showParams();

let colors: Record<string, string> = {};
function readColors() {
  const cs = getComputedStyle(document.documentElement);
  for (const k of ["bg", "surface", "text", "muted", "line", "accent", "accent-soft", "ok"]) {
    colors[k] = cs.getPropertyValue(`--${k}`).trim() || "#888";
  }
}
readColors();
matchMedia("(prefers-color-scheme: dark)").addEventListener("change", readColors);

function fit(c: HTMLCanvasElement, ratio: number): CanvasRenderingContext2D {
  const w = c.clientWidth;
  const h = Math.round(w * ratio);
  const dpr = window.devicePixelRatio || 1;
  if (c.width !== Math.round(w * dpr) || c.height !== Math.round(h * dpr)) {
    c.width = Math.round(w * dpr);
    c.height = Math.round(h * dpr);
    c.style.height = `${h}px`;
  }
  const g = c.getContext("2d")!;
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  return g;
}

function drawRoom(t: number, p: Pt, moving: boolean) {
  const w = room.clientWidth;
  const g = fit(room, w < 520 ? 0.75 : 0.42);
  const h = room.clientHeight;
  const X = (q: Pt) => q.x * w;
  const Y = (q: Pt) => q.y * h;
  g.clearRect(0, 0, w, h);
  // links
  for (const l of links) {
    const a = BULBS[l.a]!, b = BULBS[l.b]!;
    g.lineWidth = l.on ? 4 : 1.5;
    g.strokeStyle = l.on ? colors.accent! : colors.line!;
    g.setLineDash(l.on ? [] : [5, 6]);
    g.beginPath();
    g.moveTo(X(a), Y(a));
    g.lineTo(X(b), Y(b));
    g.stroke();
  }
  g.setLineDash([]);
  // probes: every bulb sends RATE per second, staggered; a dot travels to each other bulb
  const period = 1000 / RATE;
  for (let i = 0; i < BULBS.length; i++) {
    const phase = ((t + (i * period) / BULBS.length) % period) / period;
    if (phase > 0.75) continue;
    const f = phase / 0.75;
    for (let j = 0; j < BULBS.length; j++) {
      if (j === i) continue;
      const a = BULBS[i]!, b = BULBS[j]!;
      g.fillStyle = colors.muted!;
      g.globalAlpha = 0.85 - 0.6 * f;
      g.beginPath();
      g.arc(X(a) + (X(b) - X(a)) * f, Y(a) + (Y(b) - Y(a)) * f, 2.6, 0, 2 * Math.PI);
      g.fill();
    }
  }
  g.globalAlpha = 1;
  // bulbs
  for (let i = 0; i < BULBS.length; i++) {
    const q = BULBS[i]!;
    const grad = g.createRadialGradient(X(q), Y(q), 2, X(q), Y(q), 26);
    grad.addColorStop(0, colors.accent!);
    grad.addColorStop(1, "transparent");
    g.fillStyle = grad;
    g.beginPath();
    g.arc(X(q), Y(q), 26, 0, 2 * Math.PI);
    g.fill();
    g.fillStyle = colors.surface!;
    g.strokeStyle = colors.accent!;
    g.lineWidth = 2;
    g.beginPath();
    g.arc(X(q), Y(q), 11, 0, 2 * Math.PI);
    g.fill();
    g.stroke();
    g.fillStyle = colors.text!;
    g.font = "600 12px system-ui, sans-serif";
    g.textAlign = "center";
    g.textBaseline = "middle";
    g.fillText(NAMES[i]!, X(q), Y(q) + 0.5);
  }
  // the person
  if (p.x > -0.05 && p.x < 1.05) {
    const px = X(p), py = Y(p);
    const step = moving ? Math.sin(t / 110) * 3 : 0;
    g.fillStyle = colors.text!;
    g.beginPath();
    g.arc(px, py - 13, 6, 0, 2 * Math.PI);
    g.fill();
    g.lineWidth = 4;
    g.lineCap = "round";
    g.strokeStyle = colors.text!;
    g.beginPath();
    g.moveTo(px, py - 6);
    g.lineTo(px, py + 8);
    g.moveTo(px, py + 8);
    g.lineTo(px - 4 + step, py + 18);
    g.moveTo(px, py + 8);
    g.lineTo(px + 4 - step, py + 18);
    g.stroke();
    g.lineCap = "butt";
  }
}

function drawChart(t: number) {
  const w = chart.clientWidth;
  const g = fit(chart, w < 520 ? 0.85 : 0.4);
  const h = chart.clientHeight;
  const l = links[0]!;
  const [thr, m] = params(s);
  const L = 56, R = 12, T = 10;
  const motionH = 10, barsH = 34, gap = 10;
  const plotB = h - motionH - barsH - 2 * gap - 18;
  const x = (ts: number) => L + ((ts - (t - CHART_MS)) / CHART_MS) * (w - L - R);
  const lo = -84, hi = -48;
  const y = (r: number) => T + ((hi - r) / (hi - lo)) * (plotB - T);
  g.clearRect(0, 0, w, h);
  g.font = "11px system-ui, sans-serif";
  g.textBaseline = "middle";
  g.textAlign = "right";
  for (let r = -80; r <= -50; r += 10) {
    g.strokeStyle = colors.line!;
    g.lineWidth = 1;
    g.beginPath();
    g.moveTo(L, y(r));
    g.lineTo(w - R, y(r));
    g.stroke();
    g.fillStyle = colors.muted!;
    g.fillText(`${r}`, L - 6, y(r));
  }
  g.save();
  g.translate(10, (T + plotB) / 2);
  g.rotate(-Math.PI / 2);
  g.textAlign = "center";
  g.fillText("dBm", 0, 0);
  g.restore();
  const vis = l.samples.filter((q) => q.t >= t - CHART_MS);
  // threshold band around the median
  g.fillStyle = colors["accent-soft"]!;
  g.globalAlpha = 0.8;
  for (let i = 1; i < vis.length; i++) {
    const a = vis[i - 1]!, b = vis[i]!;
    if (a.median === null || b.median === null) continue;
    const d = thr / 4;
    g.beginPath();
    g.moveTo(x(a.t), y(a.median + d));
    g.lineTo(x(b.t), y(b.median + d));
    g.lineTo(x(b.t), y(b.median - d));
    g.lineTo(x(a.t), y(a.median - d));
    g.closePath();
    g.fill();
  }
  g.globalAlpha = 1;
  // median
  g.strokeStyle = colors.accent!;
  g.lineWidth = 2;
  g.beginPath();
  let started = false;
  for (const q of vis) {
    if (q.median === null) continue;
    if (!started) {
      g.moveTo(x(q.t), y(q.median));
      started = true;
    } else g.lineTo(x(q.t), y(q.median));
  }
  g.stroke();
  // samples
  g.fillStyle = colors.text!;
  for (const q of vis) {
    g.beginPath();
    g.arc(x(q.t), y(Math.max(lo, Math.min(hi, q.rssi))), 2.4, 0, 2 * Math.PI);
    g.fill();
  }
  // weights
  const barsT = plotB + gap;
  const unit = barsH / 3;
  for (const q of vis) {
    if (!q.weight) continue;
    g.fillStyle = q.weight >= 2 ? colors.accent! : colors.muted!;
    g.fillRect(x(q.t) - 2.5, barsT + barsH - q.weight * unit, 5, q.weight * unit);
  }
  g.fillStyle = colors.muted!;
  g.textAlign = "right";
  g.fillText("weight", L - 6, barsT + barsH / 2);
  // motion strip
  const motT = barsT + barsH + gap;
  g.fillStyle = colors.line!;
  g.fillRect(L, motT, w - L - R, motionH);
  g.fillStyle = colors.accent!;
  for (let i = 1; i < vis.length; i++) {
    if (vis[i]!.on) g.fillRect(x(vis[i - 1]!.t), motT, x(vis[i]!.t) - x(vis[i - 1]!.t) + 0.5, motionH);
  }
  g.fillStyle = colors.muted!;
  g.fillText("motion", L - 6, motT + motionH / 2);
  // the window sum against M, now
  const sum = l.sum(s);
  g.textAlign = "left";
  g.fillStyle = sum >= m ? colors.accent! : colors.muted!;
  g.font = "600 12px system-ui, sans-serif";
  g.fillText(`last 16 weights: ${sum} of M = ${m}${l.on ? " · motion" : ""}`, L, h - 8);
}

let simT = 0;
let last = performance.now();
let paused = matchMedia("(prefers-reduced-motion: reduce)").matches;

function setPaused(v: boolean) {
  paused = v;
  pauseBtn.textContent = v ? "Play" : "Pause";
  pauseBtn.setAttribute("aria-pressed", String(v));
}
pauseBtn.addEventListener("click", () => setPaused(!paused));
setPaused(paused);

/// Runs the radio side from `simT` to `end`: a probe from each sender every 1000 / RATE ms.
function advance(end: number) {
  for (let ts = simT + 20; ts <= end; ts += 20) {
    const { p, moving } = personAt(ts);
    if (Math.floor(ts / (1000 / RATE)) !== Math.floor((ts - 20) / (1000 / RATE))) {
      for (const l of links) l.frame(ts, p, moving, s);
    }
  }
  simT = end;
}

// `?at=ms` starts at that moment of the walk (and paused); with motion
// reduced the demo starts paused too, on a moment worth seeing
const at = Number(new URLSearchParams(location.search).get("at"));
if (at > 0) {
  advance(at);
  setPaused(true);
} else if (paused) {
  advance(4200);
}

function tick(now: number) {
  const dt = Math.min(100, now - last);
  last = now;
  if (!paused) advance(simT + dt);
  const { p, moving } = personAt(simT);
  drawRoom(simT, p, moving);
  drawChart(simT);
  const any = links.some((l) => l.on);
  roomState.textContent = any ? "Occupied" : "Empty";
  roomState.classList.toggle("on", any);
  requestAnimationFrame(tick);
}
requestAnimationFrame(tick);
