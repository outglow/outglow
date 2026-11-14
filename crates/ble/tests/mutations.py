#!/usr/bin/env python3
"""Mutation check of the BLE crate's host tests.

Each entry breaks the crate in one place (a check removed, a rule ignored, a
value wrong). With the change in, `cargo test -p ble` must fail: a mutation
that passes shows a rule no test holds the code to. The source is restored
after every run.

  python3 crates/ble/tests/mutations.py
  ONLY="OTA|login" python3 crates/ble/tests/mutations.py   # those whose names contain a word

Every `old` text must occur exactly once in its file; after a change to that
code the entry is reported as stale and has to be written anew.
Exit code 0 = every mutation was caught.
"""
import os
import subprocess
import sys

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
SRC = os.path.join(REPO, "crates", "ble", "src")

# (what goes wrong, file under crates/ble/src, text to replace, replacement)
MUTATIONS = [
    # link layer
    ('link: no channel remapping', 'pdu.rs',
     '    if is_used(map, *unmapped) {\n        return *unmapped;\n    }', '    return *unmapped;\n    #[allow(unreachable_code)]\n    if is_used(map, *unmapped) {\n        return *unmapped;\n    }'),
    ('link: hop not applied to skipped events', 'll.rs',
     '                self.stats.events_skipped += 1;\n                self.link.advance();\n                continue;',
     '                self.stats.events_skipped += 1;\n                self.link.unmapped = (self.link.unmapped + 37 - self.link.p.hop) % 37;\n                self.link.advance();\n                continue;'),
    ('link: transmit window offset ignored at connection', 'll.rs',
     'l.anchor = end.wrapping_add(UNIT + p.win_offset as u32 * UNIT);', 'l.anchor = end.wrapping_add(UNIT);'),
    ('link: transmit window size ignored', 'll.rs',
     '        l.window = p.win_size as u32 * UNIT;\n        l.established = false;', '        l.window = 0;\n        l.established = false;'),
    ('link: no window widening', 'll.rs',
     '        elapsed_ms * ppm * TICKS_PER_US / 1000\n', '        elapsed_ms * ppm * 0\n'),
    ('link: anchor not taken from the central', 'll.rs',
     '                l.anchor = anchor;\n                l.since = 0;', '                l.since = 0;'),
    ('link: connection update ignored', 'll.rs',
     '                    self.link.update = Some(u);', '                    let _ = u;'),
    ('link: connection update applied at once', 'll.rs',
     '            if u.instant == self.event {', '            if true {'),
    ('link: update without its window offset', 'll.rs',
     'self.anchor = self.expected().wrapping_add(u.win_offset as u32 * UNIT);', 'self.anchor = self.expected();'),
    ('link: channel map update ignored', 'll.rs',
     '                    self.link.map_update = Some((map, instant));', '                    let _ = (map, instant);'),
    ('link: channel map applied at once', 'll.rs',
     '            if instant == self.event {\n                self.p.channel_map = map;', '            if true {\n                self.p.channel_map = map;'),
    ('link: a passed instant accepted', 'll.rs',
     'if ahead >= 32767 || !(6..=3200).contains(&u.interval) {', 'if !(6..=3200).contains(&u.interval) {'),
    ('link: a change heard on its instant ends the connection', 'll.rs',
     'if ahead >= 32767 || !(6..=3200).contains(&u.interval) {', 'if ahead == 0 || ahead >= 32767 || !(6..=3200).contains(&u.interval) {'),
    ('link: a connection update heard one event before its instant is never applied', 'll.rs',
     '                    self.link.update = Some(u);\n                    self.link.apply_due();', '                    self.link.update = Some(u);'),
    ('link: a channel map heard one event before its instant is never applied', 'll.rs',
     '                    self.link.map_update = Some((map, instant));\n                    self.link.apply_due();', '                    self.link.map_update = Some((map, instant));'),
    ('link: a connection update heard on its instant is ignored', 'll.rs',
     '                    self.link.p.interval = u.interval;\n                    self.link.p.latency = u.latency;', '                    self.link.p.latency = u.latency;'),
    ('link: supervision not restarted with new parameters', 'll.rs',
     '                self.alive = self.anchor;\n', ''),
    ('link: no bound on the window widening', 'll.rs',
     'if l.drift(expected) >= (l.interval() / 2).saturating_sub(T_IFS) {', 'if false {'),
    ('link: own termination given up after half a second', 'll.rs',
     'let timeout = self.link.p.timeout as u32 * 10 * TICKS_PER_MS;\n            self.link.closing', 'let timeout = 500 * TICKS_PER_MS;\n            self.link.closing'),
    ('link: parameter request right after connecting', 'll.rs',
     'const PARAM_REQUEST_AFTER: u32 = 5000 * TICKS_PER_MS;', 'const PARAM_REQUEST_AFTER: u32 = 1000 * TICKS_PER_MS;'),
    ('link: a cut name advertised as the complete one', 'll.rs',
     'if shown.len() == name.len() { 0x09 } else { 0x08 }', '0x09'),
    ('link: no supervision timeout', 'll.rs',
     '                now.wrapping_sub(l.alive) > l.p.timeout as u32 * 10 * TICKS_PER_MS && reached(now, l.alive)\n',
     '                false\n'),
    ('link: unestablished connection waits the whole supervision timeout', 'll.rs',
     '                l.event >= ESTABLISH_EVENTS\n',
     '                now.wrapping_sub(l.alive) > l.p.timeout as u32 * 10 * TICKS_PER_MS && reached(now, l.alive)\n'),
    ('link: unestablished connection given up after six intervals, not six events', 'll.rs',
     '                l.event >= ESTABLISH_EVENTS\n',
     '                now.wrapping_sub(l.alive) > ESTABLISH_EVENTS as u32 * l.interval() && reached(now, l.alive)\n'),
    ('link: LL_TERMINATE_IND ignored', 'll.rs',
     '            LL_TERMINATE_IND => self.link.terminated = true,', '            LL_TERMINATE_IND => {}'),
    ('link: version sent every time', 'll.rs',
     '                if !self.link.version_sent {', '                if true {'),
    ('link: unknown control packets not answered', 'll.rs',
     '                self.hal.tx_push(pdu::LLID_CONTROL, &[LL_UNKNOWN_RSP, opcode]);', ''),
    ('link: encryption request not rejected', 'll.rs',
     '            LL_ENC_REQ => {', '            LL_ENC_REQ if false => {'),
    ('link: parameter request although the interval is right', 'll.rs',
     '                if self.link.p.interval != WANTED_INTERVAL {', '                if true {'),
    ('link: own terminate never completes', 'll.rs',
     '                if self.hal.tx_idle() || reached(now, deadline) {', '                if false && (self.hal.tx_idle() || reached(now, deadline)) {'),
    ('link: advertising goes on in a connection', 'll.rs',
     '        if self.link.state == LinkState::Advertising {\n            let now = self.hal.now();\n            if !reached(now, self.link.next_adv) {\n                return;\n            }',
     '        {\n            let now = self.hal.now();\n            if !reached(now, self.link.next_adv) {\n                if self.link.state == LinkState::Connected { self.connection_events(); }\n                return;\n            }'),
    ('link: invalid CONNECT_IND accepted', 'pdu.rs',
     '    valid.then_some(c)', '    let _ = valid;\n    Some(c)'),
    ('link: limited discoverable mode for good', 'll.rs',
     '            self.link.limited = false;\n', ''),
    ('link: general discoverable from power-up', 'll.rs',
     '            limited: true,\n', '            limited: false,\n'),
    ('link: wrong product in the advertisement', 'll.rs',
     '        put(&self.identity.product.to_le_bytes());', '        put(&0x20a0u16.to_le_bytes());'),
    # attribute server
    ('att: writes to read-only attributes accepted', 'att.rs',
     '        if !a.writable {\n            return Err(ERR_WRITE_NOT_PERMITTED);\n        }', ''),
    ('att: errors sent for commands', 'att.rs',
     '            Err(_) if opcode & 0x40 != 0 => return,', ''),
    ('att: MTU not limited to ours', 'att.rs',
     'self.host.mtu = theirs.clamp(MTU_DEFAULT, MTU_MAX);', 'self.host.mtu = MTU_MAX.max(theirs).min(MTU_MAX);'),
    ('att: mixed UUID sizes in one Find Information response', 'att.rs',
     '                    if this != format || out.room() < 2 + len {', '                    if out.room() < 2 + len {'),
    ('att: service group ends at the service itself', 'att.rs',
     '    (handle + 1..=LAST_HANDLE).find(|h| is_service(*h)).map_or(LAST_HANDLE, |h| h - 1)', '    handle'),
    ('att: blob offset not checked', 'att.rs',
     '                if offset > n {\n                    return Err((handle, ERR_INVALID_OFFSET));\n                }', '                let offset = offset.min(n);'),
    ('l2cap: only the first fragment of a response is sent', 'att.rs',
     '        for (i, fragment) in frame.chunks(LL_PAYLOAD_MAX).enumerate() {', '        for (i, fragment) in frame.chunks(LL_PAYLOAD_MAX).enumerate().take(1) {'),
    ('l2cap: pairing requests ignored', 'att.rs',
     '            CID_SMP if payload.first().is_some_and(|code| (0x01..=0x0e).contains(code) && *code != 0x05) => {', '            CID_SMP if false => {'),
    ('smp: only pairing requests refused', 'att.rs',
     'payload.first().is_some_and(|code| (0x01..=0x0e).contains(code) && *code != 0x05)', 'payload.first() == Some(&0x01)'),
    ('smp: a Pairing Failed is answered with another', 'att.rs',
     'payload.first().is_some_and(|code| (0x01..=0x0e).contains(code) && *code != 0x05)', 'payload.first().is_some_and(|code| (0x01..=0x0e).contains(code))'),
    ('smp: reserved codes answered', 'att.rs',
     'payload.first().is_some_and(|code| (0x01..=0x0e).contains(code) && *code != 0x05)', 'payload.first().is_some_and(|code| *code != 0x05)'),
    ('l2cap: requests with odd codes left unanswered', 'att.rs',
     '0x01 | 0x13 | 0x15 | 0x16 | 0x18 | 0x1a => {}', '0x01 | 0x13 | 0x15 | 0x16 | 0x17 | 0x18 | 0x1a => {}'),
    ('l2cap: responses answered with a reject', 'att.rs',
     '0x01 | 0x13 | 0x15 | 0x16 | 0x18 | 0x1a => {}', '0x01 | 0x13 | 0x16 | 0x18 | 0x1a => {}'),
    ('l2cap: a frame whose header is split is lost', 'att.rs',
     '        if h.have < 4 {\n            return;\n        }', '        if h.have < 4 {\n            h.open = false;\n            return;\n        }'),
    ('l2cap: a frame with surplus octets processed', 'att.rs',
     '        if h.have > h.want {\n            return;\n        }\n', ''),
    ('l2cap: a signalling packet over MTUsig dropped silently', 'att.rs',
     '            if cid == CID_SIGNALING {', '            if false {'),
    ('att: client configuration of any length accepted', 'att.rs',
     '                if value.len() != 2 {\n                    return Err(ERR_INVALID_LENGTH);\n                }\n', ''),
    ('att: unsupported request reported with a handle', 'att.rs',
     '            _ => return Err((0, ERR_REQUEST_NOT_SUPPORTED)),', '            _ => return Err((u16_at(p, 1), ERR_REQUEST_NOT_SUPPORTED)),'),
    ('att: a type of the wrong size reported with the start handle', 'att.rs',
     'let kind = Wanted::parse(&p[5..]).ok_or((0, ERR_INVALID_PDU))?;', 'let kind = Wanted::parse(&p[5..]).ok_or((u16_at(p, 1), ERR_INVALID_PDU))?;'),
    ('att: octets after a fixed-length request ignored', 'att.rs',
     'let exact = |n: usize| if p.len() == n {', 'let exact = |n: usize| if p.len() >= n {'),
    ('status: the standard notification switch is ignored', 'mesh.rs',
     '        self.mesh.status_on || self.host.notifications_enabled()', '        self.mesh.status_on'),
    ('status: a second switch repeats the first notification', 'mesh.rs',
     '        if now != was {', '        if now || now != was {'),
    # mesh profile
    ('login: any password accepted', 'mesh.rs',
     '        (enc[..8] == *proof).then_some(block)', '        let _ = (enc, proof);\n        Some(block)'),
    ('login: factory passwords still work after pairing', 'mesh.rs',
     '            let block = if m.paired {', '            let block = if false {'),
    ('login: the "1234" password not accepted', 'mesh.rs',
     '.or_else(|| m.login_block(&padded(DEFAULT_PASSWORD), random, proof))', ''),
    ('login: session survives the connection', 'mesh.rs',
     '        m.session = None;\n        m.status_on = false;', '        m.status_on = false;'),
    ('command: checksum not checked', 'mesh.rs',
     '        if mic[..2] != packet[3..5] {', '        if false {'),
    ('command: repeated packets carried out again', 'mesh.rs',
     '        if packet[..3] == self.mesh.last_sn {\n            return;\n        }', ''),
    ('command: other bulbs\' commands carried out', 'mesh.rs',
     '        if to_me || dst == 0xffff {\n            self.mesh_command(op, &params, false);', '        {\n            self.mesh_command(op, &params, false);'),
    ('command: brightness turns the light on', 'mesh.rs',
     'self.push_command(if self.light.on { Command::MoveToLevel { level, transition_ms } } else { Command::SetLevelWhileOff(level) });',
     'self.push_command(Command::MoveToLevel { level, transition_ms });'),
    ('command: temperature scale upside down', 'mesh.rs',
     'let mireds = CT_MIN + (span * v.min(127) as u32 / 127) as u16;', 'let mireds = CT_MAX - (span * v.min(127) as u32 / 127) as u16;'),
    ('command: temperature on the dimmable bulb', 'mesh.rs',
     '            (OP_TEMPERATURE, Some(&v)) if self.identity.tunable => {', '            (OP_TEMPERATURE, Some(&v)) => {'),
    ('command: block format without its checksum', 'mesh.rs',
     '        if !(1..=14).contains(&len) || crc8_6c(&plain[1..2 + len]) != plain[0] {', '        if !(1..=14).contains(&len) {'),
    ('command: tunnelled ZCL for another bulb carried out', 'mesh.rs',
     '        if !for_us {\n            return;\n        }', ''),
    ('status: no notification on a change', 'mesh.rs',
     '        if self.mesh.reported != Some(self.light) || self.mesh.status_due {', '        if self.mesh.status_due {'),
    ('status: notifications without the app asking', 'mesh.rs',
     '        if !self.mesh_status_enabled() {\n            return true;\n        }', ''),
    ('status: brightness reported unscaled', 'mesh.rs',
     '    ((level as u16 + 1) / 2).max(1) as u8', '    level'),
    ('pairing: new network not stored', 'mesh.rs',
     '                m.att_pair[0] = 0x07;\n                self.mesh_save();', '                m.att_pair[0] = 0x07;'),
    ('pairing: accepted without a login', 'mesh.rs',
     '            let Some(key) = self.mesh.session else { return };\n            let mut block = [0u8; 16];\n            block.copy_from_slice(&value[1..17]);\n            let field',
     '            let key = self.mesh.session.unwrap_or([0; 16]);\n            let mut block = [0u8; 16];\n            block.copy_from_slice(&value[1..17]);\n            let field'),
    ('reset: the connection stays up', 'mesh.rs',
     '            self.mesh.disconnect_at = Some(self.hal.now().wrapping_add(RESET_DISCONNECT));', ''),
    ('address: assigned address not kept', 'mesh.rs',
     '                self.mesh.address = u16::from_le_bytes([p[0], p[1]]);\n                self.mesh_save();', '                self.mesh.address = u16::from_le_bytes([p[0], p[1]]);'),
    # update
    ('OTA: image checksum not checked', 'ota.rs',
     ' && u32::from_le_bytes(self.tail) == self.crc', ''),
    ('OTA: magic not checked', 'ota.rs',
     '                self.marked = false;', ''),
    ('OTA: bootable mark written during the transfer', 'ota.rs',
     '            buf[(BOOT_MARK - offset) as usize] = 0xff;', ''),
    ('OTA: packet checksum not checked', 'ota.rs',
     '        if crc16_modbus(body) != u16::from_le_bytes([crc[0], crc[1]]) {', '        if false {'),
    ('OTA: blocks out of order accepted', 'ota.rs',
     '        if index != self.ota.next || block.len()', '        if block.len()'),
    ('OTA: a repeated block is an error', 'ota.rs',
     '        if index.wrapping_add(1) == self.ota.next {\n            // the last block again\n            return;\n        }', ''),
    ('OTA: end packet with a wrong count accepted', 'ota.rs',
     '                && (index == 0xffff || index == self.ota.next)\n', ''),
    ('OTA: restart at once, before the status can be read', 'ota.rs',
     '            self.ota.timer = Some(self.hal.now().wrapping_add(RESTART_AFTER));', '            self.hal.ota_activate();'),
    ('OTA: a failed image is started', 'ota.rs',
     '            State::Done if !self.ota.failed => {', '            State::Done => {'),
    ('OTA: an image the flash did not keep is reported good', 'ota.rs',
     '                self.ota.failed = !self.hal.ota_activate();', '                self.hal.ota_activate();'),
    ('OTA: transfer survives a lost connection', 'ota.rs',
     '        if self.ota.active() {\n            self.ota = Ota::new();\n        }', ''),
    ('OTA: image larger than the staging area accepted', 'ota.rs',
     '!(0x40..=self.hal.ota_capacity()).contains(&self.ota.declared)', 'self.ota.declared < 0x40'),
    ('OTA: sectors not erased before writing', 'ota.rs',
     '        self.ota_erase_to(end);\n        self.hal.ota_write(offset, buf);', '        self.hal.ota_write(offset, buf);'),
    # mesh on air
    ('air: mesh radio time spent in the factory network too', 'air.rs',
     '        if !self.mesh.is_paired() {', '        if false {'),
    ('air: checksum of air packets not checked', 'air.rs',
     "    mesh_mic(ltk, &n, &p[O_OP..O_MIC])[..4] == p[O_MIC..]", "    let _ = mesh_mic(ltk, &n, &p[O_OP..O_MIC]);\n    true"),
    ('air: a burst the radio left out counts as one of the train', 'air.rs',
     '            if !sent && slot.skipped < SKIPPED_MAX {', '            if false {'),
    ('air: repeats carried out again', 'air.rs',
     '        if self.air.seen(&sn, src, op) {\n            return;\n        }', ''),
    ('air: nothing passed on', 'air.rs',
     '        if !to_me && p[O_HOPS] > 0 {', '        if false {'),
    ('air: commands for one bulb carried out by all', 'air.rs',
     '        } else if to_everyone || to_me {', '        } else if true {'),
    ('air: status requests not answered', 'air.rs',
     '            if op == OP_STATUS_REQUEST {', '            if false {'),
    ('air: answers not handed to the app', 'air.rs',
     '                self.mesh_notify_from_air(sn, src, &payload);', ''),
    ('air: app commands for other bulbs stay here', 'mesh.rs',
     '        if dst != 0 && !to_me {\n            self.air_bridge(&sn, dst, op, &params);\n        }', ''),
    ('air: commands for the connected bulb alone go on air', 'mesh.rs',
     '        if dst != 0 && !to_me {\n            self.air_bridge', '        if true {\n            self.air_bridge'),
    ('air: one access code for every network', 'air.rs',
     'self.air.access_code = mesh_access_code(self.mesh.name_padded(), self.mesh.password_padded());', 'self.air.access_code = 0x1234_5678;'),
]


def main():
    only = os.environ.get("ONLY")
    missed, stale, ran = [], [], 0
    for name, path, old, new in MUTATIONS:
        if only and not any(word in name for word in only.split("|")):
            continue
        file = os.path.join(SRC, path)
        source = open(file).read()
        if source.count(old) != 1:
            print(f"STALE   {name}: the text occurs {source.count(old)} times in {path}", flush=True)
            stale.append(name)
            continue
        open(file, "w").write(source.replace(old, new))
        try:
            r = subprocess.run(["cargo", "test", "-p", "ble"], cwd=REPO, capture_output=True, text=True)
        finally:
            open(file, "w").write(source)
        ran += 1
        out = r.stdout + r.stderr
        failed = [line.split()[1] for line in out.splitlines() if line.startswith("test ") and line.rstrip().endswith("FAILED")]
        if "error[" in out or "could not compile" in out:
            print(f"BROKEN  {name}: does not compile", flush=True)
            stale.append(name)
        elif failed:
            print(f"caught  {name}: {len(failed)} test(s), e.g. {failed[0]}", flush=True)
        else:
            print(f"MISSED  {name}", flush=True)
            missed.append(name)
    print(f"{ran} mutations run: {ran - len(missed)} caught, {len(missed)} missed, {len(stale)} stale")
    return 0 if not missed and not stale else 1


if __name__ == "__main__":
    sys.exit(main())
