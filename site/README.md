# Outglow site and web flasher

A static site (Vite + TypeScript, no framework): an overview, an install guide
and a Web Bluetooth flasher. GitHub Pages serves it under `/outglow/`.

```sh
cd site
npm ci
npm run dev        # http://localhost:5173/outglow/
npm test           # protocol module against flasher/awox_ota.py (needs python3 + pycryptodome)
npm run build      # typecheck, then dist/ (with ota/index.json at dist/ota/index.json)
npm run boards     # public/boards.json from crates/bulb-tc32/src/boards.rs
```

`OUTGLOW_BASE=/ npm run build` builds for a site at the domain root.

## Layout

- `index.html`, `install.html`, `flasher/index.html`: the pages.
- `src/flasher/awox-ota.ts`: the Bluetooth update protocol (packets, CRC,
  erase wait, status polling, resends, mesh login), image and bulb checks. It
  is the same protocol as `flasher/awox_ota.py`, and `test/awox-ota.test.ts`
  compares the bytes with it.
- `src/flasher/main.ts`: Web Bluetooth and the page. It identifies the bulb
  (model, hardware revision and product, plus which firmware runs, told apart
  by the OTA status length), finds its board and proposes the newest release.
- `src/flasher/bulb-art.ts`: the progress illustration.
- `public/boards.json`: the boards (generated).
- `public/firmware.json`: the flasher's release list
  (`{schema: 1, images: [{board, version, fileVersion, url, size, sha256, date, notes}]}`),
  written by `ota/make-index.py --site`.

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
