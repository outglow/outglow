//! What Wireshark makes of the Rust stacks' frames (emulator/wireshark_check.py
//! and emulator/ble_wireshark_check.py): Wireshark's dissectors are an
//! implementation of the same specifications unrelated to ours, so a
//! misreading that sits on both sides of our own tests shows up here.
//!
//! `wireshark` runs `cargo test -p zigbee` with ZIGBEE_PCAP_DIR set (every
//! host test leaves a capture and the list of who sent each frame), then
//! checks every capture with tshark: nothing of a bulb malformed, undecrypted,
//! with an expert warning or undissected bytes; objections to test-device
//! frames only where listed; decoded fields as expected in named tests. With
//! WIRESHARK_PCAPS=a.pcap,b.pcap it checks captures of emulator scenarios
//! instead (see `Air::pcap`). `ble_wireshark` does the same for `cargo test -p
//! ble` (BLE_PCAP_DIR). Both need tshark 4.x; without it they fail with a
//! clear message.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{SimCtx, SimDef};
use crate::runner::repo_root;
use crate::session::{Stop, R};

pub fn def() -> SimDef {
    SimDef { name: "wireshark", doc: "tshark reads every frame of the Zigbee host tests (or of given captures)", run: run_zigbee }
}

pub fn def_ble() -> SimDef {
    SimDef { name: "ble_wireshark", doc: "tshark reads every packet of the BLE host tests", run: run_ble }
}

// ------------------------------------------------------------------ Zigbee
/// Network and link keys the tests and scenarios use.
const KEYS: [&str; 6] = [
    "01030507090b0d0f00020406080a0c0d",
    "5a6967426565416c6c69616e63653039",
    "d0d1d2d3d4d5d6d7d8d9dadbdcdddedf",
    "2122232425262728292a2b2c2d2e2f30",
    "77777777777777777777777777777777",
    "66b6900981e1ee3ca4206b6b861c02bb",
];
/// Keys of Green Power devices in the tests and scenarios.
const GP_KEYS: [&str; 3] = ["c0c1c2c3c4c5c6c7c8c9cacbcccdcecf", "505152535455565758595a5b5c5d5e5f", "01030507090b0d0f00020406080a0c0d"];

/// Frames of test devices that Wireshark objects to, and why that is intended.
const INTENDED: [(&str, &str, &str); 21] = [
    ("a_network_key_change_missed_while_off", "Encrypted Payload", "the coordinator moved to a key the capture has not shown yet"),
    ("replayed_and_forged_frames_are_dropped", "Encrypted Payload", "a forged frame"),
    ("data_encrypted_with_the_link_key", "Encrypted Payload", "data under a key that is not the bulb's link key"),
    ("a_request_under_a_key_the_bulb_does_not_have", "Touchlink Master Key", "a request under key index 4 to a bulb without that key"),
    ("a_build_that_was_given_the_master_key", "Touchlink Master Key", "a stand-in for the master key, which Wireshark does not have"),
    ("commands_for_a_light_that_is_off_follow_the_options", "Malformed", "Move to Level without the option fields, as older senders have it"),
    ("frames_that_are_only_authenticated_are_checked_too", "", "a frame tampered with on purpose"),
    ("shared_keys_come_from", "", "frames under keys the bulb must not accept"),
    ("a_paired_device_is_forwarded_decrypted_and_only_when_fresh", "", "frames under a wrong key"),
    ("commissioning_mode_tunnels_what_a_sink_can_use", "", "frames no device should send"),
    ("commands_and_attribute_access_can_be_discovered", "bytes left over", "a command the cluster does not have, sent to see it refused"),
    ("a_cluster_command_cut_short", "Malformed", "View Scene without the scene identifier, Get Group Membership short of the groups it counts"),
    ("every_command_that_turns_the_light_on_sets_global_scene_control", "Malformed", "Move to Level (with On/Off) without the option fields, as older senders have it"),
    ("level_commands_and_scenes_that_switch_the_light", "Malformed", "Move to Level (with On/Off) without the option fields, as older senders have it"),
    ("with_on_off_the_lowest_level_is_off", "Malformed", "Move to Level and Step (with On/Off) without the option fields, as older senders have it"),
    ("a_frame_that_could_not_be_checked_is_reported", "", "Green Power frames under a shared key nobody gave the proxy"),
    ("frames_that_are_not_a_devices_to_send_open_no_receive_window", "", "Green Power frames no device sends: forgeries and impossible options"),
    ("a_key_transport_recorded_earlier_is_not_taken", "Encrypted Payload", "a recorded key transport played back under a key that is gone"),
    ("a_network_without_a_trust_center_has_no_key_commands", "Malformed", "a Tunnel command with a made-up inner frame"),
    ("a_scan_request_that_is_not_from_a_link_initiator_is_ignored", "Malformed", "a Scan Request cut short before the touchlink information"),
    ("addresses_in_the_header_are_checked_and_unknown_commands_answered", "Unknown Command", "a NWK command that does not exist, sent to see it answered"),
];

/// Fields as Wireshark decodes them, in frames the bulb sent: (capture, display filter, at least).
const EXPECT: &[(&str, &str, usize)] = &[
    ("joins_announces_and_exchanges_the_link_key", "wpan.cmd == 0x01", 1),
    ("joins_announces_and_exchanges_the_link_key", "zbee_aps.zdp_cluster == 0x0013 && zbee_zdp.ext_addr == a4:c1:38:00:00:00:00:01", 1),
    ("joins_announces_and_exchanges_the_link_key", "zbee_aps.cmd.id == 0x08 && zbee_aps.cmd.key_type == 4", 1),
    ("joins_announces_and_exchanges_the_link_key", "zbee_aps.cmd.id == 0x0f && zbee_aps.cmd.key_type == 4 && zbee_aps.cmd.key_hash", 1),
    ("answers_beacon_requests_after_a_random_pause", "zbee_beacon.protocol == 0 && zbee_beacon.profile == 2 && zbee_beacon.router == 1 && zbee_beacon.ext_panid == dd:dd:dd:dd:dd:dd:dd:dd", 1),
    ("leaves_on_request_and_can_join_again", "zbee_nwk.cmd.id == 0x04 && zbee_nwk.cmd.leave.request == 0", 1),
    ("leave_with_rejoin_brings_the_bulb_back_under_its_key", "zbee_nwk.cmd.id == 0x06 && zbee_nwk.security == 1", 1),
    ("a_network_key_change_missed_while_off", "zbee_nwk.cmd.id == 0x06 && zbee_nwk.security == 0", 1),
    ("discovers_routes_in_both_directions", "zbee_nwk.cmd.id == 0x01 && zbee_nwk.cmd.route.dest == 0x0000", 1),
    ("discovers_routes_in_both_directions", "zbee_nwk.cmd.id == 0x02", 1),
    ("many_to_one_route_and_route_record_then_source_routing", "zbee_nwk.cmd.id == 0x05", 1),
    ("link_status_lists_live_neighbors_with_both_costs", "zbee_nwk.cmd.id == 0x08 && zbee_nwk.cmd.link.count >= 1 && zbee_nwk.cmd.link.address == 0x0000", 1),
    ("another_device_under_the_bulbs_address_makes_it_take_a_new_one", "zbee_nwk.cmd.id == 0x03 && zbee_nwk.cmd.status == 0x0d", 1),
    ("a_foreign_network_under_the_same_pan_identifier", "zbee_nwk.cmd.id == 0x09", 1),
    ("a_sleepy_end_device_joins_and_is_reported", "wpan.cmd == 0x02", 1),
    ("a_sleepy_end_device_joins_and_is_reported", "zbee_aps.cmd.id == 0x06 && zbee_aps.cmd.update_status == 1", 1),
    ("a_device_with_the_key_rejoins_without_permit_joining", "zbee_nwk.cmd.id == 0x07 && zbee_nwk.cmd.rejoin_status == 0", 1),
    ("a_child_that_stops_polling_is_forgotten_and_told_to_rejoin", "zbee_nwk.cmd.id == 0x04 && zbee_nwk.cmd.leave.request == 1 && zbee_nwk.cmd.leave.rejoin == 1", 1),
    ("an_orphaned_child_is_told_where_its_network_is", "wpan.cmd == 0x08", 1),
    ("after_a_restart_children_that_moved_are_given_up", "zbee_aps.zdp_cluster == 0x001f", 1),
    ("after_a_restart_children_that_moved_are_given_up", "zbee_aps.zdp_cluster == 0x801f && zbee_zdp.status == 0", 1),
    ("interview_descriptors", "zbee_aps.zdp_cluster == 0x8002 && zbee_zdp.node.type == 1 && zbee_zdp.node.manufacturer == 0x1141", 1),
    ("interview_descriptors", "zbee_aps.zdp_cluster == 0x8004 && zbee_zdp.profile == 0x0104 && zbee_zdp.in_cluster == 0x0008", 1),
    ("interview_descriptors", "zbee_aps.zdp_cluster == 0x8005 && zbee_zdp.ep_count == 9", 1),
    ("reads_basic_and_light_attributes", "zbee_zcl.cmd.id == 0x01 && zbee_zcl.attr.status == 0x00", 2),
    ("reports_bound_attributes_on_change_and_periodically", "zbee_zcl.cmd.id == 0x0a && zbee_zcl_general.onoff.attr_id == 0x0000", 1),
    ("reports_bound_attributes_on_change_and_periodically", "zbee_zcl.cmd.id == 0x07", 1),
    ("commands_and_attribute_access_can_be_discovered", "zbee_zcl.cmd.id == 0x12", 3),
    ("commands_and_attribute_access_can_be_discovered", "zbee_zcl.cmd.id == 0x16", 1),
    ("scenes_take_transition_times_in_tenths_and_can_be_copied", "zbee_zcl_general.scenes.cmd.srv_tx.id == 0x41", 1),
    ("scenes_take_transition_times_in_tenths_and_can_be_copied", "zbee_zcl_general.scenes.cmd.srv_tx.id == 0x42", 1),
    ("data_encrypted_with_the_link_key", "zbee_aps.security == 1 && zbee_zcl.cmd.id == 0x0b", 1),
    ("a_long_request_arrives_in_fragments_and_the_long_answer_leaves_in_fragments", "zbee_aps.fragment.count >= 3 && zbee_zcl.cmd.id == 0x01", 1),
    ("a_long_request_arrives_in_fragments_and_the_long_answer_leaves_in_fragments", "zbee_aps.type == 2 && zbee_aps.block_acks", 3),
    ("downloads_checks_and_activates_a_newer_image", "zbee_zcl_general.ota.cmd.srv_rx.id == 0x01 && zbee_zcl_general.ota.manufacturer_code == 0x1141 && zbee_zcl_general.ota.image.type == 0x0390", 1),
    ("downloads_checks_and_activates_a_newer_image", "zbee_zcl_general.ota.cmd.srv_rx.id == 0x03 && zbee_zcl_general.ota.file.offset", 10),
    ("downloads_checks_and_activates_a_newer_image", "zbee_zcl_general.ota.cmd.srv_rx.id == 0x06 && zbee_zcl_general.ota.status == 0x00", 1),
    ("a_damaged_image_is_reported_and_never_activated", "zbee_zcl_general.ota.cmd.srv_rx.id == 0x06 && zbee_zcl_general.ota.status != 0x00", 1),
    ("a_scan_from_close_by_is_answered_with_the_bulbs_network", "zbee_zcl_general.touchlink.tx_cmd_id == 0x01 && zbee_zcl_general.touchlink.key_bitmask.dev == 1 && zbee_zcl_general.touchlink.key_bitmask.cert == 1 && zbee_zcl_general.touchlink.profile_id == 0x0104", 1),
    ("a_scan_from_close_by_is_answered_with_the_bulbs_network", "zbee_zcl_general.touchlink.tx_cmd_id == 0x03", 1),
    ("a_join_request_moves_the_bulb_to_the_initiators_network", "zbee_zcl_general.touchlink.tx_cmd_id == 0x13 && zbee_zcl_general.touchlink.status == 0", 1),
    ("a_new_bulb_starts_a_network_for_the_initiator", "zbee_zcl_general.touchlink.tx_cmd_id == 0x11 && zbee_zcl_general.touchlink.status == 0", 1),
    ("a_new_bulb_starts_a_network_for_the_initiator", "zbee_aps.cmd.id == 0x05 && zbee_aps.cmd.key_type == 1", 1),
    ("a_bulb_on_a_network_with_a_trust_center_is_not_for_the_taking", "zbee_zcl_general.touchlink.tx_cmd_id == 0x13 && zbee_zcl_general.touchlink.status == 1", 1),
    ("commissioning_frames_are_tunnelled_to_the_sink", "zbee_zcl_general.gp.comm_notif.opt.proxy_info == 1 && zbee_zcl_general.gp.comm_notif.opt.secur_failed == 0 && zbee_nwk_gp.cmd.comm.dev_id == 2", 1),
    ("a_paired_device_is_forwarded_decrypted_and_only_when_fresh", "zbee_zcl_general.gp.notif.opt.also_unicast == 1 && zbee_zcl_general.gp.notif.opt.secur_lev == 3 && zbee_zcl_general.gp.notif.opt.proxy_info == 1", 2),
    ("group_notifications_of_two_proxies_collapse_into_one", "zbee_zcl_general.gp.notif.opt.also_comm_grp == 1 && zbee_aps.group == 0x0b84 && zbee_nwk.src == 0x2345", 1),
    ("the_proxy_table_can_be_read_back", "zbee_zcl_general.gp.proxy_sink_tbl_resp.status == 0 && zbee_zcl_general.gp.proxy_sink_tbl_resp.entries_count == 2", 1),
    ("the_proxy_table_can_be_read_back", "zbee_zcl_general.gp.proxy_tbl.entry.opt.entry_active == 1", 3),
    ("a_device_known_by_its_ieee_address", "zbee_zcl_general.gp.notif.opt.app_id == 2 && zbee_zcl_general.gp.gpd_ieee == 00:11:22:33:44:55:ff:fe", 1),
    ("the_sinks_answer_is_sent_in_the_devices_receive_window", "zbee_nwk_gp.fc_ext_direction == 1 && zbee_nwk_gp.command_id == 0xf0 && zbee_nwk_gp.cmd.comm_reply.pan_id == 0x1a62", 1),
    ("a_channel_configuration_is_delivered_on_the_channel", "zbee_nwk_gp.frame_type == 1 && zbee_nwk_gp.command_id == 0xf3", 1),
];

/// Frames of the device under test of which Wireshark reads a field wrong.
const QUIRKS: [(&str, &str, &str); 5] = [
    (
        "",
        "wpan.dst_pan == 0x5e55",
        "the motion sensing's probes (crates/sense/src/probe.rs): broadcasts to a PAN of their own with the C \
         firmware's payload, not NWK frames, which Wireshark reads as NWK and finds cut short",
    ),
    (
        "",
        "zbee_zcl_general.touchlink.tx_cmd_id == 0x03",
        "Device Information Response: Wireshark 4.6 reads the transaction identifier twice and so runs out of step \
         (its current source does not); the record is as Telink's target sends it (emulator/rust_touchlink_sim.py compares the bytes)",
    ),
    (
        "address_requests_are_answered_for_end_device_children",
        "zbee_aps.zdp_cluster == 0x8001 && zbee_zdp.assoc_device_count == 0",
        "IEEE_addr_rsp, extended, with a start index past the bulb's children: none follow, but there are some, so \
         StartIndex is in the frame (R22 Table 2-92); Wireshark reads StartIndex only after a count other than 0",
    ),
    (
        "addresses_in_the_header_are_checked_and_unknown_commands_answered",
        "zbee_nwk.cmd.id == 0x03 && zbee_nwk.cmd.status == 0x13",
        "Network Status 0x13 (Unknown Command, R22 3.6.13): status and the command identifier, no destination address \
         (R22/R23 3.4.3.3.2: only for a routing failure or an address conflict); Wireshark does not know 0x13 and reads an address",
    ),
    (
        "addresses_in_the_header_are_checked_and_unknown_commands_answered",
        "zbee_nwk.cmd.id == 0x3f",
        "the test device's broadcast of a NWK command that does not exist, relayed as it came; Wireshark reads its payload as R23 TLVs",
    ),
];



fn have_tshark() -> bool {
    Command::new("tshark").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

struct Zb {
    master: Vec<String>,
}

impl Zb {
    fn tshark(&self, pcap: &Path, args: &[&str]) -> Result<String, String> {
        let mut cmd = Command::new("tshark");
        cmd.arg("-r").arg(pcap);
        for k in KEYS {
            cmd.args(["-o", &format!("uat:zigbee_pc_keys:\"{k}\",\"Normal\",\"test\"")]);
        }
        for k in &self.master {
            cmd.args(["-o", &format!("uat:zigbee_pc_keys:\"{k}\",\"Normal\",\"Touchlink Master Key\"")]);
        }
        for k in GP_KEYS {
            cmd.args(["-o", &format!("uat:zigbee_gp_keys:\"{k}\",\"Normal\",\"test\"")]);
        }
        let r = cmd.args(args).output().map_err(|e| e.to_string())?;
        if !r.status.success() {
            let e = String::from_utf8_lossy(&r.stderr).trim().chars().take(300).collect::<String>();
            return Err(format!("tshark {}: {e}", args.join(" ")));
        }
        Ok(String::from_utf8_lossy(&r.stdout).to_string())
    }

    /// Numbers of the frames a display filter selects.
    fn matching(&self, pcap: &Path, filter: &str) -> Result<BTreeSet<usize>, String> {
        let out = self.tshark(pcap, &["-Y", filter, "-T", "fields", "-e", "frame.number"])?;
        Ok(out.split_whitespace().filter_map(|x| x.parse().ok()).collect())
    }

    /// Rules 1 and 2: (frames, objections as (frame, who, what)).
    fn check_capture(&self, pcap: &Path, senders: Option<&[String]>) -> Result<(usize, Vec<(usize, String, String)>), String> {
        let name = pcap.file_name().unwrap().to_string_lossy().to_string();
        let out = self.tshark(pcap, &["-T", "fields", "-E", "separator=|", "-e", "frame.number", "-e", "frame.protocols", "-e",
                                      "_ws.expert.message", "-e", "_ws.expert.severity", "-e", "wpan.fcs_ok", "-e", "_ws.col.info"])?;
        let mut quirk = BTreeSet::new();
        for (prefix, flt, _) in QUIRKS {
            if name.starts_with(prefix) {
                quirk.extend(self.matching(pcap, flt)?);
            }
        }
        let from_gpd = if senders.is_some() {
            BTreeSet::new()
        } else {
            self.matching(pcap, "zbee_nwk_gp && (zbee_nwk_gp.fc_extension == 0 || zbee_nwk_gp.fc_ext_direction == 0)")?
        };
        let mut objections = Vec::new();
        let mut frames = 0;
        for line in out.lines() {
            let mut f: Vec<&str> = line.splitn(6, '|').collect();
            f.resize(6, "");
            let (number, protocols, messages, severities, fcs_ok, info) = (f[0], f[1], f[2], f[3], f[4], f[5]);
            frames += 1;
            let n: usize = number.parse().unwrap_or(0);
            let who = senders.map(|s| s[n - 1].clone()).unwrap_or_else(|| "B".into());
            let mut what: Vec<String> = Vec::new();
            let worst = severities
                .split(',')
                .filter(|x| !x.is_empty())
                .map(|x| x.strip_prefix("0x").map(|h| i64::from_str_radix(h, 16).unwrap_or(0)).unwrap_or_else(|| x.parse().unwrap_or(0)))
                .max()
                .unwrap_or(0);
            if info.contains("[Malformed") || protocols.contains("_ws.malformed") {
                what.push("malformed".into());
            }
            if worst >= 0x600000 {
                what.push(messages.into());
            }
            if protocols.ends_with(":data") && !protocols.contains("zbee_aps:data") && !quirk.contains(&n) {
                what.push("bytes left over after the last dissector".into());
            }
            if protocols.ends_with(":data") && protocols.contains("zbee_aps:data") && !info.contains("fragment") {
                what.push("payload not dissected".into());
            }
            if fcs_ok == "0" || fcs_ok == "False" {
                what.push("bad frame check sequence".into());
            }
            if quirk.contains(&n) {
                what.retain(|w| w == "bad frame check sequence");
            }
            if from_gpd.contains(&n) {
                what.retain(|w| w != "bytes left over after the last dissector");
            }
            if what.is_empty() {
                continue;
            }
            let text = what.join("; ");
            if who == "T" || senders.is_none() {
                let intended = INTENDED.iter().any(|(p, needle, _)| name.starts_with(p) && text.contains(needle));
                if intended && who == "T" {
                    continue;
                }
            }
            let info70: String = info.chars().take(70).collect();
            objections.push((n, who, format!("{info70}: {text}")));
        }
        Ok((frames, objections))
    }
}

fn tmpdir(prefix: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("{prefix}{}-{}", std::process::id(), std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)));
    let _ = std::fs::create_dir_all(&d);
    d
}

fn run_zigbee(cx: &mut SimCtx) -> R<()> {
    let mut zb = Zb { master: Vec::new() };
    // the touchlink master key, when given (TOUCHLINK_MASTER_KEY, as for the build); never printed
    if let Ok(k) = std::env::var("TOUCHLINK_MASTER_KEY") {
        zb.master.push(k.to_lowercase());
    }
    if !have_tshark() {
        return Err(Stop::Error("tshark not found".into()));
    }
    let pcaps_env = std::env::var("WIRESHARK_PCAPS").unwrap_or_default();
    let bad = if pcaps_env.is_empty() {
        host(cx, &zb)?
    } else {
        captures(cx, &zb, &pcaps_env.split(',').map(PathBuf::from).collect::<Vec<_>>())?
    };
    cx.say(&format!("== result: {}", if bad == 0 { "PASS".to_string() } else { format!("FAIL ({bad})") }));
    cx.results.push(("wireshark".into(), bad == 0, String::new()));
    Ok(())
}

fn host(cx: &mut SimCtx, zb: &Zb) -> R<usize> {
    let work = tmpdir("zigbee-pcap-");
    cx.say(&format!("== cargo test -p zigbee, captures in {}", work.display()));
    let r = Command::new("cargo")
        .args(["test", "-p", "zigbee"])
        .current_dir(repo_root())
        .env("ZIGBEE_PCAP_DIR", &work)
        .output()
        .map_err(|e| Stop::Error(format!("cargo: {e}")))?;
    if !r.status.success() {
        let out = String::from_utf8_lossy(&r.stdout).to_string();
        let tail: String = out.chars().rev().take(2000).collect::<Vec<_>>().into_iter().rev().collect();
        cx.say(&tail);
        let _ = std::fs::remove_dir_all(&work);
        return Err(Stop::Error("the host tests fail; nothing to dissect".into()));
    }
    let mut pcaps: Vec<String> = std::fs::read_dir(&work)
        .map_err(|e| Stop::Error(e.to_string()))?
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
        .filter(|f| f.ends_with(".pcap"))
        .collect();
    pcaps.sort();
    let senders_of = |f: &str| -> Vec<String> {
        let p = work.join(format!("{}.senders", &f[..f.len() - 5]));
        std::fs::read_to_string(p).unwrap_or_default().split_whitespace().map(String::from).collect()
    };
    let mut bad = 0usize;
    let (mut frames_total, mut bulb_total) = (0usize, 0usize);
    for f in &pcaps {
        let path = work.join(f);
        let senders = senders_of(f);
        let (frames, objections) = zb.check_capture(&path, Some(&senders)).map_err(Stop::Error)?;
        frames_total += frames;
        bulb_total += senders.iter().filter(|s| *s == "B").count();
        for (n, who, what) in objections {
            bad += 1;
            let w = if who == "B" { "bulb" } else { "test device, not on the list of intended ones" };
            cx.say(&format!("FAIL {f} frame {n} ({w}): {what}"));
        }
    }
    let state = if bad == 0 { "nothing malformed, undecrypted or left undissected".to_string() } else { format!("{bad} objections") };
    cx.say(&format!("   {} captures, {frames_total} frames, {bulb_total} of them sent by bulbs: {state}", pcaps.len()));
    for (prefix, flt, minimum) in EXPECT {
        let files: Vec<&String> = pcaps.iter().filter(|f| f.starts_with(prefix)).collect();
        if files.is_empty() {
            bad += 1;
            cx.say(&format!("FAIL no capture named {prefix}*"));
            continue;
        }
        let mut count: i64 = 0;
        for f in files {
            let senders = senders_of(f);
            match zb.matching(&work.join(f), flt) {
                Ok(m) => count += m.iter().filter(|n| senders.get(**n - 1).map(|s| s == "B").unwrap_or(false)).count() as i64,
                Err(e) => {
                    cx.say(&format!("FAIL {prefix}: {e}"));
                    count = -1;
                    break;
                }
            }
        }
        if count < *minimum as i64 {
            bad += 1;
            cx.say(&format!("FAIL {prefix}: {count} bulb frames match '{flt}', expected at least {minimum}"));
        }
    }
    cx.say(&format!("   {} expectations on decoded fields: {}", EXPECT.len(), if bad == 0 { "all met" } else { "see above" }));
    for (_, flt, why) in QUIRKS {
        cx.say(&format!("   note ({flt}): {why}"));
    }
    let _ = std::fs::remove_dir_all(&work);
    Ok(bad)
}

fn captures(cx: &mut SimCtx, zb: &Zb, paths: &[PathBuf]) -> R<usize> {
    let mut bad = 0;
    for path in paths {
        let (frames, objections) = zb.check_capture(path, None).map_err(Stop::Error)?;
        let m = |f: &str| zb.matching(path, f).map(|s| s.len()).map_err(Stop::Error);
        let fcs = m("wpan.fcs_ok == 1")?;
        let zigbee = m("zbee_nwk || zbee_nwk_gp || zbee_beacon")?;
        let secured = m("zbee_nwk.security == 1")?;
        let decrypted = m("zbee.sec.decryption_key || zbee.sec.key")?;
        cx.say(&format!(
            "== {}: {frames} frames, frame check sequence good in {fcs}, {zigbee} Zigbee frames, {secured} secured at NWK level ({decrypted} decrypted)",
            path.display()
        ));
        for (n, _, what) in objections {
            bad += 1;
            cx.say(&format!("FAIL frame {n}: {what}"));
        }
        if fcs != frames {
            bad += 1;
            cx.say(&format!("FAIL {} frames without a good frame check sequence", frames - fcs));
        }
    }
    Ok(bad)
}

// ------------------------------------------------------------------ BLE
const BULB: &str = "btle_rf.pdu_type == 3";
const BULB_ADV: &str = "btle.advertising_address == a4:c1:38:11:22:33 && btle.advertising_header.pdu_type in {0x00, 0x04}";

const BLE_INTENDED: [(&str, &str); 8] = [
    ("the_version_is_exchanged_once_and_odd_control_packets_are_refused", ""),
    ("a_change_whose_instant_has_passed_ends_the_connection", ""),
    ("refuses_pairing_and_unknown_signaling", ""),
    ("requests_that_cannot_be_served_get_the_right_error", ""),
    ("a_larger_mtu_gives_longer_responses_in_fragments", ""),
    ("requests_of_the_wrong_length_are_invalid", ""),
    ("a_frame_that_does_not_match_its_length_is_discarded", ""),
    ("a_signalling_packet_over_the_signalling_mtu_is_rejected", ""),
];
const BLE_QUIRKS: [(&str, &str); 2] = [
    ("a_larger_mtu_gives_longer_responses_in_fragments", "exceed current ATT_MTU"),
    ("the_version_is_exchanged_once_and_odd_control_packets_are_refused", "control procedure"),
];

const BLE_EXPECT: &[(&str, &str, usize)] = &[
    ("advertises_like_the_stock_bulb", "btle.advertising_header.pdu_type == 0x00 && btcommon.eir_ad.entry.device_name == \"unpaired\" && btcommon.eir_ad.entry.company_id == 0x0160 && btcommon.eir_ad.entry.uuid_16 == 0x1800 && btcommon.eir_ad.entry.flags.le_limited_discoverable_mode == 1 && btcommon.eir_ad.entry.flags.bredr_not_supported == 1", 8),
    ("advertises_like_the_stock_bulb", "btle.advertising_header.pdu_type == 0x04 && btcommon.eir_ad.entry.device_name == \"unpaired\"", 8),
    ("services_and_characteristics_sit_at_the_stock_handles", "btatt.opcode == 0x11 && btatt.uuid16 == 0x1800 && btatt.uuid16 == 0x180a", 1),
    ("services_and_characteristics_sit_at_the_stock_handles", "btatt.opcode == 0x11 && btatt.uuid128 == 10:19:0d:0c:0b:0a:09:08:07:06:05:04:03:02:01:00 && btatt.handle == 0x0010 && btatt.group_end_handle == 0x001f", 1),
    ("services_and_characteristics_sit_at_the_stock_handles", "btatt.opcode == 0x09 && btatt.uuid128 == 11:19:0d:0c:0b:0a:09:08:07:06:05:04:03:02:01:00 && btatt.handle == 0x0012 && btatt.characteristic_properties == 0x1a", 1),
    ("services_and_characteristics_sit_at_the_stock_handles", "btatt.opcode == 0x09 && btatt.uuid128 == 14:19:0d:0c:0b:0a:09:08:07:06:05:04:03:02:01:00 && btatt.handle == 0x001e && btatt.characteristic_properties == 0x0a", 1),
    ("services_and_characteristics_sit_at_the_stock_handles", "btatt.opcode == 0x01 && btatt.error_code == 0x0a", 3),
    ("services_and_characteristics_sit_at_the_stock_handles", "btatt.opcode == 0x05 && btatt.uuid16 == 0x2902 && btatt.handle == 0x0013", 1),
    ("device_information_reads_like_the_stock", "btatt.opcode == 0x07 && btatt.handle == 0x0007", 1),
    ("requests_that_cannot_be_served_get_the_right_error", "btatt.opcode == 0x01 && btatt.error_code == 0x01", 2),
    ("requests_that_cannot_be_served_get_the_right_error", "btatt.opcode == 0x01 && btatt.error_code == 0x03", 1),
    ("requests_that_cannot_be_served_get_the_right_error", "btatt.opcode == 0x01 && btatt.error_code == 0x06", 1),
    ("requests_that_cannot_be_served_get_the_right_error", "btatt.opcode == 0x01 && btatt.error_code == 0x07", 1),
    ("requests_that_cannot_be_served_get_the_right_error", "btatt.opcode == 0x01 && btatt.error_code == 0x10", 1),
    ("a_larger_mtu_gives_longer_responses_in_fragments", "btatt.opcode == 0x03 && btatt.server_rx_mtu == 64", 2),
    ("a_larger_mtu_gives_longer_responses_in_fragments", "btatt.opcode == 0x05 && btl2cap.length == 62 && btatt.handle == 0x000f", 1),
    ("light_commands_work_after_login_and_changes_are_notified", "btatt.opcode == 0x1b && btatt.handle == 0x0012", 5),
    ("light_commands_work_after_login_and_changes_are_notified", "btatt.opcode == 0x13", 6),
    ("answers_the_control_procedures_of_a_central", "btle.control_opcode == 0x0c && btle.control.version_number == 0x08 && btle.control.company_id == 0x0211", 1),
    ("answers_the_control_procedures_of_a_central", "btle.control_opcode == 0x09", 1),
    ("answers_the_control_procedures_of_a_central", "btle.control_opcode == 0x07 && btle.control.unknown_type == 0x12", 1),
    ("answers_the_control_procedures_of_a_central", "btle.control_opcode == 0x07 && btle.control.unknown_type == 0x14", 1),
    ("answers_the_control_procedures_of_a_central", "btle.control_opcode == 0x07 && btle.control.unknown_type == 0x16", 1),
    ("the_version_is_exchanged_once_and_odd_control_packets_are_refused", "btle.control_opcode == 0x0c", 1),
    ("the_version_is_exchanged_once_and_odd_control_packets_are_refused", "btle.control_opcode == 0x07 && btle.control.unknown_type == 0x0f", 1),
    ("answers_the_control_procedures_of_a_central", "btle.control_opcode == 0x0d && btle.control.error_code == 0x1a", 1),
    ("asks_for_a_slower_connection_once", "btl2cap.cmd_code == 0x12 && btl2cap.min_interval == 40 && btl2cap.max_interval == 40 && btl2cap.timeout_multiplier == 400", 1),
    ("refuses_pairing_and_unknown_signaling", "btsmp.opcode == 0x05 && btsmp.reason == 0x05", 2),
    ("refuses_pairing_and_unknown_signaling", "btl2cap.cmd_code == 0x01 && btl2cap.rej_reason == 0x0000", 2),
    ("a_long_name_is_advertised_as_shortened", "btle.advertising_header.pdu_type == 0x00 && btcommon.eir_ad.entry.type == 0x08 && btcommon.eir_ad.entry.device_name == \"SixteenByteN\"", 1),
    ("notifications_can_be_enabled_the_standard_way_alone", "btatt.opcode == 0x1b && btatt.handle == 0x0012", 3),
    ("a_mesh_reset_returns_to_the_factory_network_and_disconnects", "btle.control_opcode == 0x02 && btle.control.error_code == 0x13", 1),
    ("an_image_is_staged_checked_and_started", "btatt.opcode == 0x0b && btatt.handle == 0x001b", 4),
];

fn btshark(pcap: &Path, args: &[&str]) -> Result<String, String> {
    let r = Command::new("tshark").arg("-r").arg(pcap).args(args).output().map_err(|e| e.to_string())?;
    if !r.status.success() {
        let e = String::from_utf8_lossy(&r.stderr).trim().chars().take(300).collect::<String>();
        return Err(format!("tshark {}: {e}", args.join(" ")));
    }
    Ok(String::from_utf8_lossy(&r.stdout).to_string())
}

fn bcount(pcap: &Path, filter: &str) -> Result<usize, String> {
    Ok(btshark(pcap, &["-Y", filter, "-T", "fields", "-e", "frame.number"])?.split_whitespace().count())
}

/// Per packet: (number, from the bulb, what Wireshark objects to).
fn bobjections(pcap: &Path) -> Result<Vec<(usize, bool, String)>, String> {
    let out = btshark(pcap, &["-T", "fields", "-E", "separator=|", "-E", "aggregator=;", "-e", "frame.number", "-e", "btle_rf.pdu_type",
                              "-e", "btle.advertising_header.pdu_type", "-e", "frame.protocols", "-e", "_ws.expert.message",
                              "-e", "_ws.expert.severity", "-e", "_ws.malformed", "-e", "btle.unknown_data", "-e", "_ws.col.info"])?;
    let mut rows = Vec::new();
    for line in out.lines() {
        let mut f: Vec<&str> = line.split('|').collect();
        f.resize(9, "");
        let (number, kind, adv_type, protocols, messages, severities, malformed, unknown, info) =
            (f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7], f[8]);
        let bulb = kind == "3" || (kind == "0" && (adv_type == "0x00" || adv_type == "0x04"));
        let mut what: Vec<String> = Vec::new();
        if !malformed.is_empty() || protocols.contains("_ws.malformed") {
            what.push("malformed".into());
        }
        if !unknown.is_empty() {
            what.push("undissected data".into());
        }
        for (m, s) in messages.split(';').zip(severities.split(';')) {
            if !s.is_empty() && s.parse::<i64>().unwrap_or(0) >= 0x600000 {
                what.push(m.into());
            }
        }
        if !what.is_empty() {
            let info70: String = info.chars().take(70).collect();
            rows.push((number.parse().unwrap_or(0), bulb, format!("{}  [{info70}]", what.join("; "))));
        }
    }
    Ok(rows)
}

fn run_ble(cx: &mut SimCtx) -> R<()> {
    if !have_tshark() {
        return Err(Stop::Error("tshark not found".into()));
    }
    let dir = tmpdir("ble-pcap-");
    let r = Command::new("cargo")
        .args(["test", "-p", "ble"])
        .current_dir(repo_root())
        .env("BLE_PCAP_DIR", &dir)
        .output()
        .map_err(|e| Stop::Error(format!("cargo: {e}")))?;
    if !r.status.success() {
        let out = String::from_utf8_lossy(&r.stdout).to_string();
        let tail: String = out.chars().rev().take(2000).collect::<Vec<_>>().into_iter().rev().collect();
        cx.say(&tail);
        let _ = std::fs::remove_dir_all(&dir);
        return Err(Stop::Error("cargo test -p ble failed".into()));
    }
    let mut caps: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|e| Stop::Error(e.to_string()))?
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
        .filter(|f| f.ends_with(".pcap"))
        .collect();
    caps.sort();
    let e = |x: String| Stop::Error(x);
    let mut failures: Vec<String> = Vec::new();
    let (mut packets, mut bulb, mut att) = (0, 0, 0);
    for name in &caps {
        let path = dir.join(name);
        let test = &name[..name.len() - 5];
        packets += bcount(&path, "frame").map_err(e)?;
        bulb += bcount(&path, &format!("({BULB}) || ({BULB_ADV})")).map_err(e)?;
        att += bcount(&path, &format!("({BULB}) && btatt")).map_err(e)?;
        for (number, is_bulb, what) in bobjections(&path).map_err(e)? {
            let listed: &[(&str, &str)] = if is_bulb { &BLE_QUIRKS } else { &BLE_INTENDED };
            if listed.iter().any(|(t, needle)| test.starts_with(t) && what.contains(needle)) {
                continue;
            }
            if is_bulb {
                failures.push(format!("{name} #{number}: bulb packet: {what}"));
            } else {
                failures.push(format!("{name} #{number}: test central packet not on the list of intended ones: {what}"));
            }
        }
    }
    // a test with several bulbs leaves several captures: <test>.pcap, <test>.1.pcap, ...
    let mut by_test: Vec<(String, Vec<PathBuf>)> = Vec::new();
    for name in &caps {
        let stem = &name[..name.len() - 5];
        let test = match stem.rsplit_once('.') {
            Some((a, b)) if !b.is_empty() && b.chars().all(|c| c.is_ascii_digit()) => a.to_string(),
            _ => stem.to_string(),
        };
        match by_test.iter_mut().find(|(t, _)| *t == test) {
            Some(x) => x.1.push(dir.join(name)),
            None => by_test.push((test, vec![dir.join(name)])),
        }
    }
    let mut checked = 0;
    for (test, filter, at_least) in BLE_EXPECT {
        let Some((_, paths)) = by_test.iter().find(|(t, _)| t == test) else {
            failures.push(format!("no capture of {test}"));
            continue;
        };
        let scope = if filter.contains("advertising_header") { BULB_ADV } else { BULB };
        let mut n = 0;
        for p in paths {
            n += bcount(p, &format!("({scope}) && ({filter})")).map_err(e)?;
        }
        checked += 1;
        if n < *at_least {
            failures.push(format!("{test}: {n} bulb packets match `{filter}`, expected at least {at_least}"));
        }
    }
    cx.say(&format!("{} captures, {packets} packets, {bulb} from bulbs ({att} with ATT), {checked} expectations", caps.len()));
    for f in &failures {
        cx.say(&format!("  FAIL  {f}"));
    }
    cx.say(&if failures.is_empty() { "PASS".to_string() } else { format!("{} objection(s)", failures.len()) });
    cx.results.push(("ble_wireshark".into(), failures.is_empty(), String::new()));
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
