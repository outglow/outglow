//! The diagnostics report: what the converter (z2m/open-eglo.mjs) made of
//! the Diagnostics cluster and its manufacturer attributes, one state of
//! Zigbee2MQTT flattened.

use std::fmt::Write;

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde::ser::{Serialize, SerializeMap, Serializer};
use serde_json::{Map, Value};

use super::py;

/// The groups (composites) the converter keeps most values in.
pub const GROUPS: [&str; 7] = ["network", "radio", "routing", "aps_security", "queues", "bluetooth", "system"];

/// The counters of a `--watch` row.
pub const COUNTERS: [&str; 35] = [
    "number_of_resets", "persistent_memory_writes", "mac_rx_bcast", "mac_tx_bcast", "mac_rx_ucast", "mac_tx_ucast",
    "mac_tx_ucast_retry", "mac_tx_ucast_fail", "aps_rx_ucast", "aps_tx_ucast_success", "aps_tx_ucast_fail",
    "route_disc_initiated", "neighbor_added", "neighbor_removed", "neighbor_stale", "nwk_fc_failure",
    "nwk_decrypt_failures", "relayed_ucast", "relay_drop_queue_full", "relay_drop_no_route", "relay_drop_mac_fail",
    "nwk_status_sent", "nwk_status_received", "cca_busy", "channel_access_failures", "ble_connections",
    "ble_disconnects", "ble_links_lost", "ble_events", "ble_events_skipped", "rssi_below_90", "rssi_90_to_80",
    "rssi_80_to_70", "rssi_70_to_60", "rssi_above_60",
];

/// A state of Zigbee2MQTT with its keys in the order they came (the raw
/// view prints them so; nested objects come sorted, as Zigbee2MQTT sends them).
#[derive(Clone, Default)]
pub struct State {
    pub keys: Vec<String>,
    pub map: Map<String, Value>,
}

impl State {
    pub fn insert(&mut self, k: String, v: Value) {
        if !self.map.contains_key(&k) {
            self.keys.push(k.clone());
        }
        self.map.insert(k, v);
    }
}

impl<'de> Deserialize<'de> for State {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = State;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<State, A::Error> {
                let mut s = State::default();
                while let Some((k, v)) = a.next_entry::<String, Value>()? {
                    s.insert(k, v);
                }
                Ok(s)
            }
        }
        d.deserialize_map(V)
    }
}

impl Serialize for State {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(self.keys.len()))?;
        for k in &self.keys {
            m.serialize_entry(k, &self.map[k])?;
        }
        m.end()
    }
}

/// One flat view of a state: the groups' values on top as well (new keys
/// after the others, as Python's `dict.update` puts them).
pub fn flatten(state: State) -> State {
    let mut s = state.clone();
    for g in GROUPS {
        if let Some(Value::Object(o)) = state.map.get(g) {
            for (k, v) in o {
                s.insert(k.clone(), v.clone());
            }
        }
    }
    s
}

fn pct(a: f64, b: f64) -> String {
    if b != 0.0 {
        format!("{:.1}%", 100.0 * a / b)
    } else {
        "-".into()
    }
}

/// The report of state `s`; with `before` (an earlier state of the same
/// start of the bulb) the counters show how much they grew since.
pub fn report(s: &Map<String, Value>, before: Option<&Map<String, Value>>) -> String {
    let g = |k: &str| py::get(s, k);
    let d = |k: &str| -> String {
        let v = match s.get(k) {
            None | Some(Value::Null) => return "?".into(),
            Some(v) => v,
        };
        if let (Some(b), Value::Number(n)) = (before.filter(|b| !b.is_empty()), v) {
            if let Some(Value::Number(p)) = b.get(k) {
                let grew = match (n.as_i64(), p.as_i64()) {
                    (Some(x), Some(y)) => (x - y).to_string(),
                    _ => py::float(n.as_f64().unwrap_or(0.0) - p.as_f64().unwrap_or(0.0)),
                };
                return format!("{} (+{grew})", py::str(v));
            }
        }
        py::str(v)
    };
    let n = |k: &str| py::num(s.get(k));
    let mut o = String::new();
    let w = &mut o;
    let _ = writeln!(
        w,
        "== start: uptime {} s, resets {}, last cause {}, watchdog resets {}",
        g("uptime"),
        d("number_of_resets"),
        g("reset_cause"),
        g("watchdog_resets")
    );
    let _ = writeln!(
        w,
        "== network: channel {}, PAN {}, ext PAN {}, update id {}, key seq {}, frame counter {}, depth {}, parent {}",
        g("nwk_channel"),
        g("nwk_pan_id"),
        g("nwk_ext_pan_id"),
        g("nwk_update_id"),
        g("nwk_key_seq"),
        g("nwk_frame_counter"),
        g("nwk_depth"),
        g("nwk_parent")
    );
    let _ = writeln!(
        w,
        "== health: {}; to the coordinator: {}, many-to-one {}, routes {}, route discoveries {}",
        g("link_health"),
        g("coordinator_route"),
        g("coordinator_many_to_one"),
        g("routes_used"),
        d("route_disc_initiated")
    );
    let last = |count: &str, last: &str| if py::truthy(s.get(count)) { g(last) } else { "-".into() };
    let _ = writeln!(
        w,
        "   Network Status received {} (last: {}), sent {} (last: {})",
        d("nwk_status_received"),
        last("nwk_status_received", "last_nwk_status_received"),
        d("nwk_status_sent"),
        last("nwk_status_sent", "last_nwk_status_sent")
    );
    let (t, r, f) = (n("mac_tx_ucast"), n("mac_tx_ucast_retry"), n("mac_tx_ucast_fail"));
    let _ = writeln!(
        w,
        "== radio: noise floor {} dBm (max {}), CCA busy {}, channel access failures {}",
        g("noise_floor"),
        g("noise_floor_max"),
        d("cca_busy"),
        d("channel_access_failures")
    );
    let _ = writeln!(
        w,
        "   MAC unicasts sent {}, retries {} ({}), failed {} ({})",
        d("mac_tx_ucast"),
        d("mac_tx_ucast_retry"),
        pct(r, t),
        d("mac_tx_ucast_fail"),
        pct(f, t)
    );
    let hist = ["rssi_below_90", "rssi_90_to_80", "rssi_80_to_70", "rssi_70_to_60", "rssi_above_60"].map(n);
    let sum: f64 = hist.iter().sum();
    let total = if sum == 0.0 { 1.0 } else { sum };
    let labels = ["<-90", "-90..-80", "-80..-70", "-70..-60", ">-60"];
    let _ = writeln!(
        w,
        "   RSSI of frames heard: {}",
        labels.iter().zip(hist).map(|(l, v)| format!("{l} {}", pct(v, total))).collect::<Vec<_>>().join("  ")
    );
    let _ = writeln!(
        w,
        "   neighbours added {}, removed {}, stale {}; NWK counter failures {}, decrypt failures {}",
        d("neighbor_added"),
        d("neighbor_removed"),
        d("neighbor_stale"),
        d("nwk_fc_failure"),
        d("nwk_decrypt_failures")
    );
    let _ = writeln!(
        w,
        "   relayed {}; lost: queue {}, no route {}, next hop {}",
        d("relayed_ucast"),
        d("relay_drop_queue_full"),
        d("relay_drop_no_route"),
        d("relay_drop_mac_fail")
    );
    let (ev, sk) = (n("ble_events"), n("ble_events_skipped"));
    let _ = writeln!(
        w,
        "== Bluetooth: connections {}, disconnects {}, links lost {}, events {}, skipped {} ({}), radio lent {} ms",
        d("ble_connections"),
        d("ble_disconnects"),
        d("ble_links_lost"),
        d("ble_events"),
        d("ble_events_skipped"),
        pct(sk, ev + sk),
        g("ble_ms")
    );
    let _ = writeln!(
        w,
        "== worst cases: main loop {} µs (watchdog 2 000 000), BLE loan {} µs, flash erase {} µs; stack never used {} B (interrupts {} B)",
        g("loop_max"),
        g("ble_lend_max"),
        g("flash_busy_max"),
        g("stack_free"),
        g("irq_stack_free")
    );
    let _ = writeln!(
        w,
        "== neighbours (L = LQI, cost = what it says of our frames, age in link status periods, st = times stale,\n   link = cost routing uses, m = measured, deliv = attempts acknowledged)"
    );
    let _ = writeln!(
        w,
        "   {:7} {:>3} {:>4} {:>3} {:5} {:>3} {:>5} {:>12} {:>4} {:>5} {:>4} {:>4} {:>5}",
        "addr", "LQI", "cost", "age", "flags", "st", "rx", "RSSI avg/min", "tx", "retry", "fail", "link", "deliv"
    );
    let empty = Vec::new();
    let table = s.get("neighbor_table").and_then(Value::as_array).unwrap_or(&empty);
    for nb in table {
        let e = Map::new();
        let nb = nb.as_object().unwrap_or(&e);
        let v = |k: &str| py::get(nb, k);
        let flag = |k: &str, c: &'static str| if py::truthy(nb.get(k)) { c } else { "-" };
        let flags = format!("{}{}{}", flag("relays", "R"), flag("in_link_status", "L"), flag("child", "C"));
        let link = format!(
            "{}{}",
            nb.get("link_cost").map(py::str).unwrap_or_else(|| "?".into()),
            if py::truthy(nb.get("cost_measured")) { "m" } else { "" }
        );
        let deliv = match nb.get("delivery") {
            None | Some(Value::Null) => "-".into(),
            Some(d) => format!("{}%", py::str(d)),
        };
        let _ = writeln!(
            w,
            "   {:7} {:>3} {:>4} {:>3} {:5} {:>3} {:>5} {:>5}/{:<6} {:>4} {:>5} {:>4} {:>4} {:>5}",
            v("address"),
            v("lqi"),
            v("out_cost"),
            v("age"),
            flags,
            v("stale"),
            v("rx"),
            v("rssi_avg"),
            v("rssi_min"),
            v("tx"),
            v("retries"),
            v("fails"),
            link,
            deliv
        );
    }
    let _ = writeln!(w, "== events, newest first (#start, seconds since that start)");
    for e in s.get("event_log").and_then(Value::as_array).unwrap_or(&empty) {
        let text = e.get("text").filter(|t| py::truthy(Some(t))).unwrap_or(e);
        let _ = writeln!(w, "   {}", py::str(text));
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(text: &str) -> State {
        serde_json::from_str(text).unwrap()
    }

    /// A made-up state (crates/outglow/src/diag/testdata) and what
    /// the earlier Python tool (bulb-diag.py) printed of it, with and without an earlier run.
    #[test]
    fn reports_as_the_python_tool() {
        let s = flatten(state(include_str!("testdata/state.json")));
        let before: Map<String, Value> = serde_json::from_str(include_str!("testdata/before.json")).unwrap();
        assert_eq!(report(&s.map, Some(&before)), include_str!("testdata/report.txt"));
        assert_eq!(report(&s.map, None), include_str!("testdata/report.first.txt"));
    }

    #[test]
    fn flattens_groups_after_the_keys_that_came() {
        let s = flatten(state(r#"{"z": 1, "radio": {"cca_busy": 4, "z": 2}, "a": 3}"#));
        assert_eq!(s.keys, ["z", "radio", "a", "cca_busy"]);
        assert_eq!(s.map["z"], 2);
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            r#"{"z":2,"radio":{"cca_busy":4,"z":2},"a":3,"cca_busy":4}"#
        );
    }
}
