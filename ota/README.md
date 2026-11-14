# Zigbee OTA index for Zigbee2MQTT

`index.json` lists the Outglow images in the format of
[Koenkk/zigbee-OTA](https://github.com/Koenkk/zigbee-OTA) (the index Zigbee2MQTT
uses by default). Point Zigbee2MQTT at it, and bulbs running Outglow are
offered new versions.

The site build publishes it at `https://outglow.github.io/outglow/ota/index.json`;
the files it points to are GitHub release assets.

## Zigbee2MQTT configuration

```yaml
ota:
  zigbee_ota_override_index_location: https://outglow.github.io/outglow/ota/index.json
```

Entries of the override index take precedence over the default index. Both stay
in use, so the bulbs' other neighbours keep getting their vendors' updates.

Zigbee2MQTT offers updates only to devices whose definition says they support
OTA. For Outglow that definition is the external converter
[`z2m/outglow.mjs`](../z2m/outglow.mjs), so install it.

## What updates on its own and what does not

**What the bulb does.** It asks for a new image by itself (`crates/zigbee/src/stack/ota.rs`):

- 30 seconds after it starts or joins, then once every 24 hours;
- also a moment after the coordinator sends it an Image Notify.

It finds the upgrade server with a Match Descriptor request (the coordinator
first) and sends Query Next Image with its manufacturer code (`0x1141`), image
type (per board, `0x0390`–`0x03a4`) and file version. It takes any offered
image of its own manufacturer code and image type whose version differs from
the one it runs, so a downgrade works too.

The download asks for 48-byte blocks and resumes after a power cut. The image
is made bootable only after its CRC checks out. The bulb then waits for the
server's Upgrade End Response to say when to switch; if the server stays
silent, it switches anyway after three tries.

**What Zigbee2MQTT does: it does not install updates on its own.** When the
bulb's query arrives (at most once per `update_check_interval`, default
1440 minutes), Zigbee2MQTT looks the bulb up in the indexes, publishes
`{"update": {"state": "available"}}` and answers the bulb "no image
available". The download starts only when someone asks for it:

- the **Update** button in the frontend, or `zigbee2mqtt/bridge/request/device/ota_update/update` with `{"id": "<bulb>"}`;
- a **scheduled** update: `zigbee2mqtt/bridge/request/device/ota_update/schedule`
  with `{"id": "<bulb>"}`. Zigbee2MQTT then answers the bulb's next query (within
  a day, or 30 s after a power cycle) with the image. The schedule survives
  restarts of Zigbee2MQTT;
- an automation, e.g. in Home Assistant: on the bulb's update entity turning
  `on`, call `update.install` (or publish one of the requests above).

So "bulbs update themselves" needs one of these. A Home Assistant automation
that installs every Outglow update (one bulb at a time, at night) comes
closest. Zigbee2MQTT's own warning applies: an update takes 10 minutes or
more, loads the network, and the bulb restarts at the end, which switches it
on if its power-on behaviour says so.

`ota: disable_automatic_update_check: true` stops Zigbee2MQTT from checking
when the bulb asks. Manual checks and scheduled updates still work.

## Making the index

```sh
cargo xtask index --version 0.2.4 --tag v0.2.4 --notes "..." \
    --site site/public/firmware.json --assets release-assets/ \
    target/tc32*/bulb-tc32.ota
```

For each `bulb-tc32.ota` (one per board, `cargo xtask build --all`) it:

- reads the OTA header and the board tag of the image inside;
- adds an entry with `fileName`, `fileVersion`, `fileSize`, `url`, `imageType`,
  `manufacturerCode`, `sha512`, `otaHeaderString`, `modelId` (the model id the
  image reports: `outglow-dimm`, `outglow-tw`, …) and `releaseNotes`;
- writes the Bluetooth image next to it into the web flasher's release list.

Entries of other versions stay, so the previous release remains available for
a downgrade (Zigbee2MQTT's downgrade topics).

`modelId` makes Zigbee2MQTT match only devices that report that model. Bulbs on
the original firmware never match. They have no Zigbee OTA client anyway: the
first install is over Bluetooth.

`0x1141` is Telink's manufacturer code and many devices share it, but no image
type in the default index uses the range `0x0390`–`0x03a4`. With `modelId`
added, an image cannot reach a foreign device.

The `index.json` checked in here is an example made from a local build (file
version `0x00020000`). Until the first release is published, its URL does not
exist.
