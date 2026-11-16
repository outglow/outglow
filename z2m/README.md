# Zigbee2MQTT converter

`outglow.mjs` is an optional external converter for bulbs running Outglow:
the router diagnostics grouped by topic, the start-up brightness, and on bulbs
built with motion sensing its entities. Without it Zigbee2MQTT generates a
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
