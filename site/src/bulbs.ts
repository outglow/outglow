// The "Supported bulbs" page: product cards from public/bulbs.json, with a
// filter by light type and a search over article numbers, names and boards.

export type Confidence = "confirmed" | "likely" | "inferred";

export interface Source {
  label: string;
  url?: string;
}

export interface Product {
  brand: string;
  article: string;
  name: string;
  shape: string;
  socket: string | null;
  light: string;
  image: string | null;
  /** The maker's technical datasheet, or null. */
  datasheet: string | null;
  confidence: Confidence;
  source: Source[];
  note?: string;
}

export interface BulbBoard {
  board: string;
  revisions: string[];
  kind: string;
  hardware_tested: boolean;
  note?: string;
  products: Product[];
}

export interface BulbsFile {
  schema: 1;
  boards: BulbBoard[];
  images: Record<string, Source>;
}

/** The filter groups of the page, by board kind. */
export const GROUPS: Record<string, "dimmable" | "tunable" | "colour"> = {
  "outglow-dimm": "dimmable",
  "outglow-tw": "tunable",
  "outglow-rgbtw": "colour",
  "outglow-rgbw": "colour",
  "outglow-ebf": "colour",
};

export const BADGES: Record<string, string> = {
  "outglow-dimm": "Dimmable white",
  "outglow-tw": "Tunable white",
  "outglow-rgbtw": "Colour + tunable white",
  "outglow-rgbw": "Colour + white",
  "outglow-ebf": "White + colour ring",
};

const CONFIDENCE: Record<Confidence, string> = {
  confirmed: "Confirmed: a device of this model reports this board",
  likely: "Likely: same product family and light, not seen reporting the board",
  inferred: "Inferred from the module name; unconfirmed",
};

const RANK: Record<Confidence, number> = { confirmed: 0, likely: 1, inferred: 2 };

/** The products known with a board (model and hardware revision), the surest first. */
export function productsFor(data: BulbsFile, model: string, revision: string): { b: BulbBoard; products: Product[] } | null {
  const b = data.boards.find((x) => x.board === model && x.revisions.includes(revision));
  if (!b) return null;
  return { b, products: [...b.products].sort((x, y) => RANK[x.confidence] - RANK[y.confidence]) };
}

/** Lower-case text a search term is matched against. */
export function haystack(b: BulbBoard, p: Product): string {
  return [p.brand, p.article, p.name, p.shape, p.socket ?? "", b.board, ...b.revisions.map((r) => `${b.board}-${r}`)]
    .join(" ")
    .toLowerCase();
}

/** Whether a product card passes the kind filter and the search text. */
export function matches(b: BulbBoard, p: Product, group: string, query: string): boolean {
  if (group !== "all" && GROUPS[b.kind] !== group) return false;
  const q = query.trim().toLowerCase();
  if (!q) return true;
  const text = haystack(b, p);
  return q.split(/\s+/).every((t) => text.includes(t));
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, cls?: string, text?: string): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function link(href: string, text: string): HTMLAnchorElement {
  const a = el("a", undefined, text);
  a.href = href;
  a.rel = "noopener";
  return a;
}

function card(b: BulbBoard, p: Product, base: string): HTMLElement {
  const c = el("article", "bulb");
  c.dataset.group = GROUPS[b.kind] ?? "";

  const fig = el("div", "bulb-pic");
  if (p.image) {
    const img = el("img");
    img.src = base + p.image;
    img.alt = `${p.brand} ${p.article}`;
    img.loading = "lazy";
    img.width = 256;
    img.height = 256;
    fig.append(img);
  } else {
    fig.append(el("span", "muted", "No picture"));
  }
  c.append(fig);

  const body = el("div", "bulb-body");
  const title = el("h3", undefined, p.name);
  body.append(title);
  body.append(el("p", "bulb-article", `${p.brand} ${p.article}`));

  const badges = el("div", "badges");
  badges.append(el("span", `badge kind-${GROUPS[b.kind] ?? "other"}`, BADGES[b.kind] ?? b.kind));
  const conf = el("span", `badge conf-${p.confidence}`, p.confidence);
  conf.title = CONFIDENCE[p.confidence];
  badges.append(conf);
  body.append(badges);

  const dl = el("dl");
  const row = (k: string, v: Node | string) => {
    dl.append(el("dt", undefined, k));
    const dd = el("dd");
    dd.append(v);
    dl.append(dd);
  };
  row("Shape", p.shape + (p.socket ? `, ${p.socket}` : ""));
  row("Light", p.light);
  const boardText = el("span");
  boardText.append(el("code", undefined, b.board));
  if (b.revisions.length) boardText.append(` hardware ${b.revisions.join(", ")}`);
  row("Board", boardText);
  const src = el("span");
  p.source.forEach((s, i) => {
    if (i) src.append("; ");
    src.append(s.url ? link(s.url, s.label) : s.label);
  });
  row("Source", src);
  body.append(dl);
  if (p.note) body.append(el("p", "bulb-note muted", p.note));
  const links = el("div", "bulb-links");
  if (p.datasheet) {
    const a = link(p.datasheet, "Datasheet");
    a.className = "button";
    links.append(a);
  }
  if (links.childElementCount) body.append(links);
  c.append(body);
  return c;
}

async function main(): Promise<void> {
  const grid = document.getElementById("bulbs");
  const unknown = document.getElementById("unknown");
  const count = document.getElementById("count");
  const search = document.getElementById("search") as HTMLInputElement | null;
  const credits = document.getElementById("credits");
  if (!grid || !search) return;
  const base = import.meta.env.BASE_URL;
  let data: BulbsFile;
  try {
    data = (await (await fetch(`${base}bulbs.json`)).json()) as BulbsFile;
  } catch {
    grid.textContent = "The list did not load.";
    return;
  }

  const cards: { b: BulbBoard; p: Product; node: HTMLElement }[] = [];
  for (const b of data.boards) for (const p of b.products) cards.push({ b, p, node: card(b, p, base) });
  grid.replaceChildren(...cards.map((c) => c.node));

  if (unknown) {
    const lone = data.boards.filter((b) => !b.products.some((p) => p.confidence !== "inferred"));
    unknown.replaceChildren(...lone.map((b) => {
      const li = el("li");
      li.append(el("code", undefined, b.board), ` (${b.revisions.join(", ")}): ${BADGES[b.kind] ?? b.kind}. `);
      li.append(b.note ?? (b.products.length ? "Only inferred products so far." : "No product known."));
      return li;
    }));
  }

  if (credits) {
    const used = new Set(cards.map((c) => c.p.image).filter((i): i is string => !!i));
    credits.replaceChildren(...[...used].sort().map((img) => {
      const li = el("li");
      const s = data.images[img];
      li.append(el("code", undefined, img.replace(/^bulbs\//, "")), ": ");
      li.append(s?.url ? link(s.url, s.label) : (s?.label ?? "unknown"));
      return li;
    }));
  }

  let group = "all";
  const buttons = [...document.querySelectorAll<HTMLButtonElement>("[data-filter]")];
  const apply = () => {
    let shown = 0;
    for (const c of cards) {
      const ok = matches(c.b, c.p, group, search.value);
      c.node.hidden = !ok;
      if (ok) shown++;
    }
    if (count) count.textContent = `${shown} of ${cards.length} products`;
    for (const btn of buttons) btn.setAttribute("aria-pressed", String(btn.dataset.filter === group));
  };
  for (const btn of buttons) {
    btn.addEventListener("click", () => {
      group = btn.dataset.filter ?? "all";
      apply();
    });
  }
  search.addEventListener("input", apply);
  const start = new URLSearchParams(location.search).get("q");
  if (start) search.value = start;
  apply();
}

if (typeof document !== "undefined") void main();
