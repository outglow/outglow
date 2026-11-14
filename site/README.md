# Outglow site and web flasher

A static site (Vite + TypeScript, no framework): an overview, a list of
supported bulbs, an install guide and a Web Bluetooth flasher. GitHub Pages serves it at https://outglow.github.io/ (built with the base `/` by
`outglow/outglow.github.io`, `.github/workflows/site.yml`) and still under `/outglow/`
(the project site, for links and Zigbee2MQTT configurations made before).

```sh
cd site
npm ci
npm run dev        # http://localhost:5173/outglow/
npm test           # protocol module against `outglow ble vectors` (the Rust flasher; needs cargo),
                   # and bulbs.json against boards.json and the pictures
npm run build      # typecheck, then dist/ (with ota/index.json at dist/ota/index.json)
npm run boards     # public/boards.json (cargo xtask boards: all board tables)
```

`OUTGLOW_BASE=/ npm run build` builds for a site at the domain root.

## Layout

- `index.html`, `bulbs.html`, `install.html`, `flasher/index.html`: the pages.
- `src/flasher/awox-ota.ts`: the Bluetooth update protocol (packets, CRC,
  erase wait, status polling, resends, mesh login), image and bulb checks. It
  is the same protocol as `crates/outglow/src/flash/protocol.rs`, and `test/awox-ota.test.ts`
  compares the bytes with it.
- `src/flasher/main.ts`: Web Bluetooth and the page. It identifies the bulb
  (model, hardware revision and product, plus which firmware runs, told apart
  by the OTA status length), finds its board and proposes the newest release.
- `src/flasher/bulb-art.ts`: the progress illustration.
- `public/boards.json`: the boards (generated).
- `public/bulbs.json`, `public/bulbs/`, `src/bulbs.ts`: the supported bulbs page
  (hand-written, see below).
- `public/firmware.json`: the flasher's release list
  (`{schema: 1, images: [{board, version, fileVersion, url, size, sha256, date, notes}]}`),
  written by `cargo xtask index --site`.

## Supported bulbs (`bulbs.json`)

Which real products each board runs on. Written by hand; `test/bulbs.test.ts`
checks it against `boards.json` and the picture files.

```jsonc
{
  "schema": 1,
  "boards": [{
    "board": "ESMLFzm_w6_TW",        // module name, as the bulb reports it
    "revisions": ["4.63"],           // hardware revisions with an image (board = module-revision)
    "kind": "outglow-tw",          // as in boards.json; the page groups dimm / tw / colour
    "hardware_tested": false,        // has Outglow run on a real bulb of this board
    "note": "…",                     // optional, shown when no product is confirmed
    "products": [{
      "brand": "EGLO", "article": "12242",
      "name": "connect.z ST64 filament bulb, amber",
      "shape": "ST64 filament", "socket": "E27",
      "light": "tunable white 2200–6500 K",
      "image": "bulbs/filament-st64-amber-spiral.webp",   // relative to the site root, or null
      "datasheet": "https://tools.eglo.com/tds/eng-GB/12242",  // maker's datasheet, or null
      "confidence": "confirmed",     // confirmed | likely | inferred
      "source": [{"label": "Zigbee2MQTT 12242", "url": "https://…"}],
      "note": "…"                    // optional caveat shown on the card
    }]
  }],
  "images": { "bulbs/…webp": {"label": "where the picture came from", "url": "https://…"} }
}
```

- `confirmed`: a device of that article was seen reporting the module name
  (Zigbee2MQTT definitions and device reports, the Zigbee device database
  at zigbee.blakadder.com, or a bulb we have).
- `likely`: same family and light as a confirmed product, module not reported.
- `inferred`: only the module name points to it (`ESMLzm_c5_E14`: 5 W colour, E14).

The same article has shipped with different modules, and vendor firmware
updates change the reported name (`EBF_RGB_Zm` → `EBF_RGB_Zm_CLP`), so the
flasher reads the board from the bulb; the list only helps to find candidates.

Pictures: one per article (`bulbs/<brand>-<article>.webp`), 512×512 WebP with
a transparent background, under 80 KB. Most come from EGLO's technical
datasheets (`tools.eglo.com/tds/eng-GB/<article>`), whose product picture has
its own transparency mask; they were only trimmed, centred and scaled. The
AwoX bulbs have no picture there: their photos on white (from
zigbee.blakadder.com) were cut out with `rembg` (model `isnet-general-use`),
joined with a flood fill from the edges where the glass is clear. Each
picture's origin is in `images` and listed on the page.

## Firmware files and CORS

Browsers cannot download GitHub release assets: they carry no CORS headers.
So the Pages workflow has to copy each release's `…-ota.bin` assets into
`dist/firmware/<tag>/` before deploying. The URLs in `firmware.json` are
relative to the site for that reason. The Zigbee OTA index (`ota/index.json`)
can point straight at the release assets, because Zigbee2MQTT downloads them
server-side.

No firmware binaries are committed.

## Browser support

Web Bluetooth exists in Chrome and Edge on desktop and on Android. Safari and
every browser on iOS lack it. The advertisement (product id and MAC) is only
visible with `watchAdvertisements`, which Chrome keeps behind
`chrome://flags/#enable-experimental-web-platform-features`. Without it, the
bulb is identified from Device Information (model and hardware revision), and
the MAC for the mesh login has to be typed in: it is the first six bytes of
the bulb's Zigbee IEEE address, as Zigbee2MQTT shows it.

The flasher was tested against a model of the bulb in the unit tests and in a
browser without a bulb. It has not yet flashed a real bulb.
