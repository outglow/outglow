#!/usr/bin/env bash
# Update the bulb over Zigbee OTA (after the first flash of our firmware
# via hardware, or — if the stock accepts plaintext — onto the factory image too).
# Requires an MQTT client (mosquitto_pub/sub) with access to the Z2M broker.
#
# Step 1 (read device fields):        ./ota_push.sh check  <id>
# Step 2 (build .ota with those fields): make -C ../firmware ota OTA_TYPE=... OTA_VER=...
# Step 3 (push our image):            ./ota_push.sh push   <id>  /abs/path/open-eglo.ota
#
# Variables: MQTT_HOST (default 127.0.0.1), MQTT_PORT (1883), BASE (zigbee2mqtt).
set -euo pipefail
MQTT_HOST="${MQTT_HOST:-127.0.0.1}"; MQTT_PORT="${MQTT_PORT:-1883}"; BASE="${BASE:-zigbee2mqtt}"
PUB="mosquitto_pub -h $MQTT_HOST -p $MQTT_PORT"
SUB="mosquitto_sub -h $MQTT_HOST -p $MQTT_PORT"

cmd="${1:-}"; dev="${2:-}"
case "$cmd" in
  check)
    [ -n "$dev" ] || { echo "usage: $0 check <id>"; exit 1; }
    echo ">> OTA check (read-only) for $dev — shows current imageType/fileVersion"
    echo ">> (response arrives on $BASE/bridge/response/device/ota_update/check)"
    timeout 25 $SUB -t "$BASE/bridge/response/device/ota_update/check" -C 1 &
    sleep 1
    $PUB -t "$BASE/bridge/request/device/ota_update/check" -m "{\"id\":\"$dev\"}"
    wait || true
    echo ">> If 'no image available' — that's normal (we have no index). What matters are"
    echo "   the fields from the Z2M log: manufacturerCode(0x1141)/imageType/fileVersion."
    ;;
  push)
    ota="${3:-}"
    [ -n "$dev" ] && [ -f "$ota" ] || { echo "usage: $0 push <id> /abs/open-eglo.ota"; exit 1; }
    echo ">> WARNING: this is a real firmware update over OTA. Make sure"
    echo "   imageType/fileVersion in $ota match (check above) and ver > stock."
    echo ">> pushing $ota to $dev"
    $PUB -t "$BASE/bridge/request/device/ota_update/update" \
         -m "{\"id\":\"$dev\",\"url\":\"$ota\"}"
    echo ">> track progress in the Z2M log (ota: downloading/apply) and in the OTA tab."
    ;;
  *) echo "usage: $0 {check|push} <id> [file.ota]"; exit 1;;
esac
