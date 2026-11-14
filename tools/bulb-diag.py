#!/usr/bin/env python3
"""What a bulb on the open Rust firmware says about itself and its network,
read through Zigbee2MQTT with the external converter z2m/open-eglo.mjs
(docs/diagnostics.md).

It presses `diagnostics_refresh` (every counter and table read anew), waits
for the state, and prints: the start (uptime, resets, why), the network
parameters, the way to the coordinator, the radio (noise floor, RSSI
histogram, MAC retries), Bluetooth, the timing worst cases, the neighbour
table and the event log. With a previous run of the same bulb on record
(~/.cache/bulb-diag/<ieee>.json) the counters show how much they grew since.

  python3 tools/bulb-diag.py 0xa4c1380000123456 [--broker HOST] [--json]
  python3 tools/bulb-diag.py 0xa4c1380000123456 --watch 60 --csv diag.csv   # a row a minute
  python3 tools/bulb-diag.py 0xa4c1380000123456 --dump [--csv rows.csv]     # the black box

Needs paho-mqtt. Reads only; the refresh is 13 + 10 Read Attributes frames.
--dump reads the black box (crates/zigbee/src/stack/blackbox.rs): 93 pages,
one Read Attributes each, about a minute; the bulb records nothing new
meanwhile. Saved raw to ~/.cache/bulb-diag/<ieee>-box-<time>.json.
"""
import argparse
import csv
import json
import os
import sys
import time

import paho.mqtt.client as mqtt

CACHE = os.path.expanduser("~/.cache/bulb-diag")
GROUPS = ("network", "radio", "routing", "aps_security", "queues", "bluetooth", "system")
COUNTERS = [
    "number_of_resets", "persistent_memory_writes", "mac_rx_bcast", "mac_tx_bcast", "mac_rx_ucast", "mac_tx_ucast",
    "mac_tx_ucast_retry", "mac_tx_ucast_fail", "aps_rx_ucast", "aps_tx_ucast_success", "aps_tx_ucast_fail",
    "route_disc_initiated", "neighbor_added", "neighbor_removed", "neighbor_stale", "nwk_fc_failure",
    "nwk_decrypt_failures", "relayed_ucast", "relay_drop_queue_full", "relay_drop_no_route", "relay_drop_mac_fail",
    "nwk_status_sent", "nwk_status_received", "cca_busy", "channel_access_failures", "ble_connections",
    "ble_disconnects", "ble_links_lost", "ble_events", "ble_events_skipped", "rssi_below_90", "rssi_90_to_80",
    "rssi_80_to_70", "rssi_70_to_60", "rssi_above_60",
]


def fetch(broker, ieee, timeout):
    """Presses the refresh and returns the state once the reads have settled
    (no new state for 3 s after the first that came in)."""
    topic = f"zigbee2mqtt/{ieee}"
    states = []
    last = [0.0]

    def on_message(_c, _u, m):
        states.append(json.loads(m.payload))
        last[0] = time.time()

    c = mqtt.Client(mqtt.CallbackAPIVersion.VERSION2)
    c.on_message = on_message
    c.connect(broker, 1883)
    c.loop_start()
    c.subscribe(topic)
    time.sleep(1)
    asked = time.time()
    states.clear()
    c.publish(topic + "/set", json.dumps({"diagnostics_refresh": "refresh"}))
    while time.time() - asked < timeout:
        time.sleep(0.5)
        if states and time.time() - last[0] > 3:
            break
    c.loop_stop()
    if not states:
        sys.exit(f"no state from {topic}: is the bulb on, and the converter installed?")
    # the converter keeps most values in groups (composites): one flat view of them
    s = dict(states[-1])
    for g in GROUPS:
        if isinstance(s.get(g), dict):
            s.update(s[g])
    return s


NWK_COMMANDS = {1: "route request", 2: "route reply", 3: "network status", 4: "leave", 5: "route record",
                6: "rejoin request", 7: "rejoin response", 8: "link status", 9: "network report", 10: "network update",
                11: "end device timeout request", 12: "end device timeout response"}
CLUSTERS = {0x0000: "Basic", 0x0003: "Identify", 0x0004: "Groups", 0x0005: "Scenes", 0x0006: "On/Off", 0x0008: "Level",
            0x0019: "OTA", 0x0300: "Color", 0x0B05: "Diagnostics", 0x000C: "Analog Input", 0x0010: "Binary Output",
            0x000D: "Analog Output", 0x0013: "ZDO Device_annce", 0x8031: "ZDO Mgmt_Lqi_rsp", 0x8032: "ZDO Mgmt_Rtg_rsp",
            0x8000: "ZDO NWK_addr_rsp", 0x8001: "ZDO IEEE_addr_rsp", 0x8004: "ZDO Simple_Desc_rsp", 0x8005: "ZDO Active_EP_rsp",
            0x0021: "Green Power", 0xFC5F: "BLE settings"}
STATUS = ["ok", "no ack", "channel busy", "held for child"]


def fetch_box(broker, ieee, timeout):
    """Reads every page of the black box; returns the blob."""
    topic = f"zigbee2mqtt/{ieee}"
    pages = {}
    last = [time.time()]

    def on_message(_c, _u, m):
        p = json.loads(m.payload).get("black_box_page")
        if p and p["page"] not in pages:
            pages[p["page"]] = bytes.fromhex(p["hex"])
            last[0] = time.time()

    c = mqtt.Client(mqtt.CallbackAPIVersion.VERSION2)
    c.on_message = on_message
    c.connect(broker, 1883)
    c.loop_start()
    c.subscribe(topic)
    time.sleep(1)
    c.publish(topic + "/set", json.dumps({"black_box_read": "all"}))
    total = None
    t0 = time.time()
    while time.time() - t0 < timeout:
        time.sleep(0.5)
        if total is None and 0 in pages:
            total = int.from_bytes(pages[0][20:22], "little")
        if total and len(pages) * 64 >= total:
            break
        if pages and time.time() - last[0] > 20:
            # a page lost: ask for the missing ones again, one by one
            want = [p for p in range(((total or 0) + 63) // 64) if p not in pages]
            for p in want[:5]:
                c.publish(topic + "/set", json.dumps({"black_box_read": p}))
            last[0] = time.time()
        sys.stderr.write(f"\rblack box: {len(pages)} pages")
    c.loop_stop()
    sys.stderr.write("\n")
    if total is None:
        sys.exit("no page 0 of the black box: is this image 0x00020004 or later, and the converter current?")
    blob = b"".join(pages.get(p, b"\0" * 64) for p in range((total + 63) // 64))[:total]
    missing = [p for p in range((total + 63) // 64) if p not in pages]
    return blob, missing


def decode_box(b):
    h = dict(version=b[0], neighbors=b[1], link_entry=b[2], ring_cap=b[3], ring_len=b[4], ring_entry=b[5],
             rows_cap=int.from_bytes(b[6:8], "little"), rows_len=int.from_bytes(b[8:10], "little"), row_entry=b[10],
             row_s=b[11] * 10, now_ms=int.from_bytes(b[12:16], "little"), ring_written=int.from_bytes(b[16:20], "little"))
    at = 24
    links = []
    for _ in range(h["neighbors"]):
        e = b[at:at + h["link_entry"]]
        at += h["link_entry"]
        short, n = int.from_bytes(e[0:2], "little"), e[2]
        if short == 0xFFFF or n == 0:
            continue
        frames = [(e[4 + i // 2] >> (4 * (i % 2))) & 0x0F for i in range(n)]
        links.append(dict(address=f"0x{short:04x}", frames=[(f & 7, bool(f & 8)) for f in frames]))
    ring = []
    for i in range(h["ring_cap"]):
        e = b[at + i * h["ring_entry"]:at + (i + 1) * h["ring_entry"]]
        if i < h["ring_len"]:
            k = e[10]
            what = (k >> 5) & 3
            idv = int.from_bytes(e[11:13], "little")
            ring.append(dict(ago_ms=(h["now_ms"] - int.from_bytes(e[0:4], "little")) & 0xFFFFFFFF,
                             mac_dst=f"0x{int.from_bytes(e[4:6], 'little'):04x}", dst=f"0x{int.from_bytes(e[6:8], 'little'):04x}",
                             src=f"0x{int.from_bytes(e[8:10], 'little'):04x}", nwk_type=["data", "command", "?", "inter-PAN"][k & 3],
                             secured=bool(k & 4), broadcast=bool(k & 8), relayed=bool(k & 16),
                             what=[None, "nwk_command", "cluster", "aps_ack"][what],
                             id=idv, name=(NWK_COMMANDS.get(idv) if what == 1 else CLUSTERS.get(idv)) if what else None,
                             seq=e[13], attempts=e[14] & 15, status=STATUS[e[14] >> 4] if e[14] >> 4 < 4 else e[14] >> 4))
    at += h["ring_cap"] * h["ring_entry"]
    rows = []
    for i in range(h["rows_len"]):
        r = b[at + i * h["row_entry"]:at + (i + 1) * h["row_entry"]]
        rows.append(dict(minute=int.from_bytes(r[0:2], "little"), tx=int.from_bytes(r[2:4], "little"),
                         rx=int.from_bytes(r[4:6], "little"), retries=r[6], fails=r[7], coord_cost=r[8] & 7,
                         coord_measured=bool(r[8] & 8), coord_direct=bool(r[8] & 16), route_disc=r[8] >> 5,
                         coord_delivery=round(r[9] * 100 / 255), stale=r[10], noise=r[11] - 256 if r[11] > 127 else r[11]))
    return h, links, ring, rows


def report_box(h, links, ring, rows, missing):
    if missing:
        print(f"(pages not read: {missing})")
    print(f"== black box v{h['version']}: {h['ring_written']} frames sent since the start, "
          f"{h['rows_len']} rows of {h['row_s']} s")
    print("== unicasts per neighbour, oldest first (digit = attempts, x = not acknowledged), and the runs of lost attempts")
    for l in links:
        seq = "".join(f"{a}" if ok else "x" for a, ok in l["frames"])
        att = []
        for a, ok in l["frames"]:
            att += [False] * (a - 1 if ok else a) + ([True] if ok else [])
        runs, run = {}, 0
        for ok in att + [True]:
            if ok:
                if run:
                    runs[run] = runs.get(run, 0) + 1
                run = 0
            else:
                run += 1
        p = sum(att) / len(att) if att else 0
        print(f"   {l['address']}: {len(l['frames']):>2} frames, {len(att):>3} attempts, delivered {100 * p:.0f}%, "
              f"runs of losses {dict(sorted(runs.items())) or '-'}")
        print(f"      {seq}")
    print("== frames sent, newest first")
    for f in reversed(ring):
        what = f["name"] or (f"{f['what']} 0x{f['id']:04x}" if f["what"] else "")
        flags = ("R" if f["relayed"] else "-") + ("B" if f["broadcast"] else "-") + ("S" if f["secured"] else "-")
        print(f"   -{f['ago_ms'] / 1000:8.1f}s  to {f['mac_dst']}  {f['src']}->{f['dst']}  {flags} {f['nwk_type']:7} {what:28} "
              f"#{f['seq']:<3} {f['attempts']} attempts, {f['status']}")
    print(f"== rows (every {h['row_s']} s): minute, unicasts sent, heard, retries, failures, coordinator, stale, noise")
    for r in rows:
        c = f"{'direct' if r['coord_direct'] else 'via'} cost {r['coord_cost']}{'m' if r['coord_measured'] else ''} p {r['coord_delivery']}%"
        print(f"   {r['minute']:>5} min  tx {r['tx']:>4}  rx {r['rx']:>5}  retry {r['retries']:>3}  fail {r['fails']:>2}  {c:28} "
              f"disc {r['route_disc']}  stale {r['stale']:>2}  noise {r['noise']}")


def pct(a, b):
    return f"{100 * a / b:.1f}%" if b else "-"


def report(s, before):
    def d(k):
        v = s.get(k)
        if v is None:
            return "?"
        if before and isinstance(before.get(k), (int, float)) and isinstance(v, (int, float)):
            return f"{v} (+{v - before[k]})"
        return str(v)

    print(f"== start: uptime {s.get('uptime')} s, resets {d('number_of_resets')}, last cause {s.get('reset_cause')}, "
          f"watchdog resets {s.get('watchdog_resets')}")
    print(f"== network: channel {s.get('nwk_channel')}, PAN {s.get('nwk_pan_id')}, ext PAN {s.get('nwk_ext_pan_id')}, "
          f"update id {s.get('nwk_update_id')}, key seq {s.get('nwk_key_seq')}, frame counter {s.get('nwk_frame_counter')}, "
          f"depth {s.get('nwk_depth')}, parent {s.get('nwk_parent')}")
    print(f"== health: {s.get('link_health')}; to the coordinator: {s.get('coordinator_route')}, many-to-one {s.get('coordinator_many_to_one')}, "
          f"routes {s.get('routes_used')}, route discoveries {d('route_disc_initiated')}")
    rx, tx = s.get("nwk_status_received"), s.get("nwk_status_sent")
    print(f"   Network Status received {d('nwk_status_received')} (last: {s.get('last_nwk_status_received') if rx else '-'}), "
          f"sent {d('nwk_status_sent')} (last: {s.get('last_nwk_status_sent') if tx else '-'})")
    t, r, f = s.get("mac_tx_ucast") or 0, s.get("mac_tx_ucast_retry") or 0, s.get("mac_tx_ucast_fail") or 0
    print(f"== radio: noise floor {s.get('noise_floor')} dBm (max {s.get('noise_floor_max')}), CCA busy {d('cca_busy')}, "
          f"channel access failures {d('channel_access_failures')}")
    print(f"   MAC unicasts sent {d('mac_tx_ucast')}, retries {d('mac_tx_ucast_retry')} ({pct(r, t)}), failed {d('mac_tx_ucast_fail')} ({pct(f, t)})")
    hist = [s.get(k) or 0 for k in ("rssi_below_90", "rssi_90_to_80", "rssi_80_to_70", "rssi_70_to_60", "rssi_above_60")]
    total = sum(hist) or 1
    print("   RSSI of frames heard: " + "  ".join(f"{lab} {pct(v, total)}" for lab, v in zip(("<-90", "-90..-80", "-80..-70", "-70..-60", ">-60"), hist)))
    print(f"   neighbours added {d('neighbor_added')}, removed {d('neighbor_removed')}, stale {d('neighbor_stale')}; "
          f"NWK counter failures {d('nwk_fc_failure')}, decrypt failures {d('nwk_decrypt_failures')}")
    print(f"   relayed {d('relayed_ucast')}; lost: queue {d('relay_drop_queue_full')}, no route {d('relay_drop_no_route')}, "
          f"next hop {d('relay_drop_mac_fail')}")
    ev, sk = s.get("ble_events") or 0, s.get("ble_events_skipped") or 0
    print(f"== Bluetooth: connections {d('ble_connections')}, disconnects {d('ble_disconnects')}, links lost {d('ble_links_lost')}, "
          f"events {d('ble_events')}, skipped {d('ble_events_skipped')} ({pct(sk, ev + sk)}), radio lent {s.get('ble_ms')} ms")
    print(f"== worst cases: main loop {s.get('loop_max')} µs (watchdog 2 000 000), BLE loan {s.get('ble_lend_max')} µs, "
          f"flash erase {s.get('flash_busy_max')} µs; stack never used {s.get('stack_free')} B (interrupts {s.get('irq_stack_free')} B)")
    print("== neighbours (L = LQI, cost = what it says of our frames, age in link status periods, st = times stale,\n   link = cost routing uses, m = measured, deliv = attempts acknowledged)")
    print(f"   {'addr':7} {'LQI':>3} {'cost':>4} {'age':>3} {'flags':5} {'st':>3} {'rx':>5} {'RSSI avg/min':>12} {'tx':>4} {'retry':>5} {'fail':>4} {'link':>4} {'deliv':>5}")
    for n in s.get("neighbor_table") or []:
        flags = ("R" if n.get("relays") else "-") + ("L" if n.get("in_link_status") else "-") + ("C" if n.get("child") else "-")
        print(f"   {n['address']:7} {n['lqi']:>3} {n['out_cost']:>4} {n['age']:>3} {flags:5} {n['stale']:>3} {n['rx']:>5} "
              f"{n['rssi_avg']:>5}/{n['rssi_min']:<6} {n['tx']:>4} {n['retries']:>5} {n['fails']:>4} "
              f"{str(n.get('link_cost', '?')) + ('m' if n.get('cost_measured') else ''):>4} "
              f"{(str(n['delivery']) + '%') if n.get('delivery') is not None else '-':>5}")
    print("== events, newest first (#start, seconds since that start)")
    for e in s.get("event_log") or []:
        print(f"   {e.get('text') or e}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("ieee")
    ap.add_argument("--broker", default=os.environ.get("MQTT_BROKER", "localhost"),
                    help="MQTT broker of Zigbee2MQTT (default: $MQTT_BROKER or localhost)")
    ap.add_argument("--timeout", type=float, default=30)
    ap.add_argument("--json", action="store_true", help="the raw state instead of the report")
    ap.add_argument("--watch", type=float, help="every this many seconds, a row of counters (with --csv)")
    ap.add_argument("--csv")
    ap.add_argument("--dump", action="store_true", help="read and decode the black box")
    a = ap.parse_args()
    os.makedirs(CACHE, exist_ok=True)
    if a.dump:
        blob, missing = fetch_box(a.broker, a.ieee, max(a.timeout, 240))
        h, links, ring, rows = decode_box(blob)
        out = os.path.join(CACHE, f"{a.ieee}-box-{time.strftime('%Y%m%d-%H%M%S')}.json")
        json.dump(dict(header=h, links=links, ring=ring, rows=rows, missing=missing, raw=blob.hex()), open(out, "w"))
        if a.json:
            print(json.dumps(dict(header=h, links=links, ring=ring, rows=rows), indent=2))
        else:
            report_box(h, links, ring, rows, missing)
        if a.csv:
            with open(a.csv, "w", newline="") as f:
                w = csv.DictWriter(f, fieldnames=list(rows[0].keys()) if rows else ["minute"])
                w.writeheader()
                w.writerows(rows)
        print(f"(saved {out})")
        return
    cache = os.path.join(CACHE, a.ieee + ".json")
    if a.watch:
        out = open(a.csv, "a", newline="") if a.csv else sys.stdout
        w = csv.writer(out)
        if not a.csv or os.path.getsize(a.csv) == 0:
            w.writerow(["time", "uptime", "noise_floor", *COUNTERS])
        while True:
            s = fetch(a.broker, a.ieee, a.timeout)
            w.writerow([time.strftime("%Y-%m-%d %H:%M:%S"), s.get("uptime"), s.get("noise_floor"), *[s.get(k) for k in COUNTERS]])
            out.flush()
            time.sleep(a.watch)
    s = fetch(a.broker, a.ieee, a.timeout)
    if a.json:
        print(json.dumps(s, indent=2))
        return
    before = None
    if os.path.exists(cache):
        prev = json.load(open(cache))
        # only the same start of the bulb: the counters do not survive a power cut
        if prev.get("number_of_resets") == s.get("number_of_resets"):
            before = prev
            print(f"(growth since {prev.get('_when')})")
    s_store = dict(s, _when=time.strftime("%Y-%m-%d %H:%M:%S"))
    report(s, before)
    json.dump(s_store, open(cache, "w"))


if __name__ == "__main__":
    main()
