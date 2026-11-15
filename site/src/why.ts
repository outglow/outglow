// The comparison page: each row's "i" opens its explanation; the first one
// holds the LED duty cycle of one bulb for every brightness level and colour
// temperature, original firmware and Outglow on one chart.
import data from "./compare-tw.json";

type Point = [number, number];
interface Sweep {
  label: string;
  unit: string;
  ticks: number[];
  original: Point[];
  outglow: Point[];
}

const SWEEPS: Record<"level" | "ct", Sweep> = {
  level: {
    label: "Level",
    unit: "duty cycle, both channels",
    ticks: [1, 50, 100, 150, 200, 254],
    original: data.level.original.map((y, i) => [i + 1, y]),
    outglow: data.level.outglow.map((y, i) => [i + 1, y]),
  },
  ct: {
    label: "Mired",
    unit: "share of the light on the warm channel",
    ticks: [153, 200, 250, 300, 350, 400, 454],
    original: data.colourTemperature.mireds.map((m, i) => [m, data.colourTemperature.original[i]!]),
    outglow: data.colourTemperature.mireds.map((m, i) => [m, data.colourTemperature.outglow[i]!]),
  },
};

const NS = "http://www.w3.org/2000/svg";
const W = 720, H = 300, L = 46, R = 14, T = 12, B = 34;
const svg = document.getElementById("curve") as unknown as SVGSVGElement;
const readout = document.getElementById("curve-readout")!;
const diff = document.getElementById("curve-diff")!;

function el(name: string, attrs: Record<string, string | number>, text?: string): SVGElement {
  const e = document.createElementNS(NS, name);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
  if (text !== undefined) e.textContent = text;
  svg.appendChild(e);
  return e;
}

function largestDifference(a: Point[], b: Point[], from = 0): number {
  let max = 0;
  for (let i = from; i < a.length; i++) max = Math.max(max, Math.abs(a[i]![1] - b[i]![1]));
  return max;
}

function draw(key: "level" | "ct"): void {
  const s = SWEEPS[key];
  svg.replaceChildren();
  const x0 = s.original[0]![0], x1 = s.original[s.original.length - 1]![0];
  const sx = (x: number) => L + ((x - x0) / (x1 - x0)) * (W - L - R);
  const sy = (y: number) => T + (1 - y / 100) * (H - T - B);
  for (const y of [0, 25, 50, 75, 100]) {
    el("line", { x1: L, x2: W - R, y1: sy(y), y2: sy(y), class: "grid" });
    el("text", { x: L - 8, y: sy(y) + 4, "text-anchor": "end" }, `${y} %`);
  }
  for (const x of s.ticks) el("text", { x: sx(x), y: H - B + 18, "text-anchor": "middle" }, String(x));
  el("text", { x: W - R, y: H - 4, "text-anchor": "end" }, key === "level" ? "Brightness level" : "Mired (cold → warm)");
  const path = (p: Point[]) => p.map(([x, y], i) => `${i ? "L" : "M"}${sx(x).toFixed(1)} ${sy(y).toFixed(1)}`).join(" ");
  el("path", { d: path(s.original), class: "line-original" });
  el("path", { d: path(s.outglow), class: "line-outglow" });

  if (key === "level") {
    // level 1 is the one deliberate difference: Zigbee says the light switches off there
    el("circle", { cx: sx(1), cy: sy(s.original[0]![1]), r: 4, class: "dot-original" });
    el("circle", { cx: sx(1), cy: sy(s.outglow[0]![1]), r: 4, class: "dot-outglow" });
    el("text", { x: sx(1) + 10, y: sy(0) - 10, class: "note" }, "level 1: original stays at 1.5 %, Outglow switches off");
    diff.textContent = `Levels 2–254: largest difference ${largestDifference(s.original, s.outglow, 1).toFixed(2)} points`;
  } else {
    diff.textContent = `Largest difference ${largestDifference(s.original, s.outglow).toFixed(2)} points`;
  }

  const cursor = el("line", { x1: 0, x2: 0, y1: T, y2: H - B, class: "cursor", visibility: "hidden" });
  const hit = el("rect", { x: L, y: T, width: W - L - R, height: H - T - B, fill: "transparent" });
  hit.addEventListener("pointermove", (ev) => {
    const r = svg.getBoundingClientRect();
    const v = x0 + ((((ev as PointerEvent).clientX - r.left) / r.width) * W - L) / (W - L - R) * (x1 - x0);
    let i = 0;
    s.original.forEach((p, j) => { if (Math.abs(p[0] - v) < Math.abs(s.original[i]![0] - v)) i = j; });
    const [x, a] = s.original[i]!, b = s.outglow[i]![1];
    cursor.setAttribute("x1", String(sx(x)));
    cursor.setAttribute("x2", String(sx(x)));
    cursor.setAttribute("visibility", "visible");
    readout.textContent = `${s.label} ${x}: original ${a.toFixed(2)} %, Outglow ${b.toFixed(2)} % (${s.unit})`;
  });
  hit.addEventListener("pointerleave", () => cursor.setAttribute("visibility", "hidden"));
}

const buttons = { level: document.getElementById("sweep-level")!, ct: document.getElementById("sweep-ct")! };
for (const key of ["level", "ct"] as const) {
  buttons[key].addEventListener("click", () => {
    for (const k of ["level", "ct"] as const) buttons[k].setAttribute("aria-pressed", String(k === key));
    draw(key);
  });
}
draw("level");

for (const b of document.querySelectorAll<HTMLButtonElement>("button.info")) {
  b.addEventListener("click", () => {
    const more = document.getElementById(b.getAttribute("aria-controls")!)!;
    const open = b.getAttribute("aria-expanded") !== "true";
    b.setAttribute("aria-expanded", String(open));
    more.hidden = !open;
  });
}
