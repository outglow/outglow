# Zigbee2MQTT converter

`outglow.mjs` is an optional external converter for bulbs running Outglow:
the router diagnostics grouped by topic, the start-up brightness, and the
motion sensing switch and settings. Without it Zigbee2MQTT generates a
definition from what the bulb shows (the light, the main router values on
endpoints 2–6, the settings on endpoints 7–8).

It needs Zigbee2MQTT 2.x. From 2.11, allow external converters first in
`configuration.yaml`:

```yaml
advanced:
  enable_external_js: true
```

One command for any installation (needs `curl`, `jq`, `mosquitto_pub`; add
`-u`/`-P` for a broker with a password, change `zigbee2mqtt` for another base
topic):

```sh
curl -fsSL https://outglow.github.io/z2m/outglow.mjs \
  | jq -Rs '{name: "outglow.mjs", code: .}' \
  | mosquitto_pub -h YOUR_BROKER -t zigbee2mqtt/bridge/request/converter/save -s
```

Zigbee2MQTT saves it in its `external_converters` folder and loads it;
`mosquitto_sub -h YOUR_BROKER -t zigbee2mqtt/bridge/converters -C 1` lists
it. To remove it, send `{"name": "outglow.mjs"}` to
`zigbee2mqtt/bridge/request/converter/remove`.

Other ways: in the Home Assistant add-on's frontend, *Settings → Dev console →
External converters* (name it `outglow.mjs`, paste the file); or copy the file
into the `external_converters` folder next to `configuration.yaml` (Docker:
`./data/external_converters/`, Linux: `/opt/zigbee2mqtt/data/external_converters/`)
and restart Zigbee2MQTT. A bulb already paired picks it up after a restart of
Zigbee2MQTT or a new interview. See Zigbee2MQTT's
[guide to external converters](https://www.zigbee2mqtt.io/advanced/more/external_converters.html).

## Motion sensing (experimental)

Every Outglow image can notice people moving from how the radio signal between
bulbs changes, and reports it as Occupancy. It is **off** out of the box: the
bulb then sends nothing extra. It has run in the emulator only, not yet on a
real bulb.

- `motion_sensing` (ON/OFF): the switch. Switch it on in at least two bulbs
  in the same room; each senses along its links to the others.
- `occupancy`, `occupancy_timeout` (seconds without motion until it clears).
- `motion_auto_calibration` (on by default): a day after the switch-on every
  link takes the sensitivity of its quietest hour.
- `motion_calibrate` (start/stop) and `motion_calibration_left`: calibrate now
  instead, with the room empty and still for 60 s.
- `motion_sensitivity` (0–100, higher is more sensitive, 0 notices nothing):
  every link at once; set by hand it switches the automatic calibration off.
- `motion_self_tuning`: once a day a small step after the background noise.
- `motion_probes` (advanced): probe frames a second, 5 when switched on; 0 is
  off.
- `motion_links` (diagnostic): the bulbs it senses with, each link's signal,
  noise, sensitivity and whether it sees motion now. Refresh it from the
  device page, or with `diagnostics_refresh`. In Home Assistant its state is
  how many links see motion now, and the links are its attribute `links`.
- `room_occupancy` and the device option `motion_room`: list the IEEE
  addresses of the other bulbs of the room in `motion_room`, and
  `room_occupancy` counts only the links to them, so a walk past a bulb of
  the next room does not light this one. Without the option it is motion on
  any link.

Without the converter: Occupancy Sensing (`0x0406`) and the manufacturer
cluster `0xFC5E` (code `0x1141`); attribute `0x0001` is the probe rate.
