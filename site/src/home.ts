// The board table of the overview page, from public/boards.json.

import type { Board } from "./flasher/awox-ota.ts";

const KINDS: Record<string, string> = {
  "open-eglo-dimm": "dimmable white",
  "open-eglo-tw": "tunable white",
  "open-eglo-rgbtw": "colour + tunable white",
  "open-eglo-rgbw": "colour + white",
  "open-eglo-ebf": "colour",
};
const hex = (n: number) => "0x" + n.toString(16).padStart(4, "0");

async function main(): Promise<void> {
  const body = document.getElementById("boards");
  if (!body) return;
  try {
    const boards = (await (await fetch(`${import.meta.env.BASE_URL}boards.json`)).json()) as Board[];
    body.replaceChildren(...boards.map((b) => {
      const tr = document.createElement("tr");
      for (const v of [b.name, KINDS[b.kind] ?? b.kind, hex(b.ble_product), hex(b.image_type)]) {
        const td = document.createElement("td");
        td.textContent = v;
        tr.append(td);
      }
      return tr;
    }));
  } catch {
    body.innerHTML = '<tr><td colspan="4">The list did not load.</td></tr>';
  }
}

void main();
