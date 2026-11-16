use super::aps::{ENDPOINT, PROFILE_HA, PROFILE_WILDCARD, PROFILE_ZDO};
use super::parent::CAP_FFD;
use super::{Binding, Stack, ROUTE_ACTIVE, ROUTE_FREE};
use crate::hal::Hal;
use crate::mac;
use crate::nwk;
use crate::util::{Buf, Reader};

const SUCCESS: u8 = 0x00;
const INVALID_REQUEST: u8 = 0x80;
const DEVICE_NOT_FOUND: u8 = 0x81;
const INVALID_EP: u8 = 0x82;
const NOT_ACTIVE: u8 = 0x83;
const NOT_SUPPORTED: u8 = 0x84;
const NO_ENTRY: u8 = 0x88;
const TABLE_FULL: u8 = 0x8C;
const UNKNOWN_DEVICE: u8 = 0xC8;

const NWK_ADDR_REQ: u16 = 0x0000;
const IEEE_ADDR_REQ: u16 = 0x0001;
const NODE_DESC_REQ: u16 = 0x0002;
const POWER_DESC_REQ: u16 = 0x0003;
const SIMPLE_DESC_REQ: u16 = 0x0004;
const ACTIVE_EP_REQ: u16 = 0x0005;
const MATCH_DESC_REQ: u16 = 0x0006;
const DEVICE_ANNCE: u16 = 0x0013;
const PARENT_ANNCE: u16 = 0x001F;
const BIND_REQ: u16 = 0x0021;
const UNBIND_REQ: u16 = 0x0022;
const MGMT_LQI_REQ: u16 = 0x0031;
const MGMT_RTG_REQ: u16 = 0x0032;
const MGMT_BIND_REQ: u16 = 0x0033;
const MGMT_LEAVE_REQ: u16 = 0x0034;
const MGMT_PERMIT_JOINING_REQ: u16 = 0x0036;
const MGMT_NWK_UPDATE_REQ: u16 = 0x0038;
const RESPONSE: u16 = 0x8000;
const PARENT_ANNCE_CHILDREN: usize = (super::frag::SINGLE_MAX - 2) / 8;
const PARENT_ANNCE_RSP_CHILDREN: usize = (super::frag::SINGLE_MAX - 3) / 8;

const STACK_REVISION: u16 = 22;
const MAX_FRAME: u16 = 82;

impl<H: Hal> Stack<H> {
    pub(crate) fn on_zdo(&mut self, src: u16, cluster: u16, p: &[u8], unicast: bool) {
        let mut r = Reader::new(p);
        let Some(seq) = r.u8() else {
            return;
        };
        if cluster & RESPONSE != 0 {
            let _ = self.on_zdo_response(src, cluster, r);
            return;
        }
        if cluster == DEVICE_ANNCE {
            if let (Some(short), Some(ext)) = (r.u16(), r.u64()) {
                self.address_seen(short, ext);
                self.learn_address(ext, short);
                self.gp_device_announced(short, ext);
                if let Some(i) = self.child_by_ext(ext) {
                    if self.children[i].short != short {
                        self.remove_child(i);
                    }
                }
            }
            return;
        }
        if cluster == PARENT_ANNCE {
            let _ = self.on_parent_announce(src, seq, r);
            return;
        }
        let mut rsp = Buf::new();
        rsp.u8(seq);
        if self.zdo_request(cluster, r, unicast, &mut rsp).is_some() {
            self.aps_data(src, 0, cluster | RESPONSE, PROFILE_ZDO, 0, rsp.as_slice());
        }
    }

    fn zdo_request(&mut self, cluster: u16, mut r: Reader, unicast: bool, rsp: &mut Buf) -> Option<()> {
        let me = self.nib.short;
        match cluster {
            NWK_ADDR_REQ | IEEE_ADDR_REQ => {
                let by_ext = cluster == NWK_ADDR_REQ;
                let (ext, short) = if by_ext { (r.u64()?, 0xFFFF) } else { (u64::MAX, r.u16()?) };
                let request_type = r.u8()?;
                let start = r.u8()? as usize;
                let child = self.end_devices().find(|c| if by_ext { c.ext == ext } else { c.short == short });
                let found = match child {
                    Some(c) => Some((c.ext, c.short)),
                    None if (by_ext && ext == self.ext) || (!by_ext && short == me) => Some((self.ext, me)),
                    None => None,
                };
                let (status, ext, short) = match (found, request_type) {
                    (None, _) => (DEVICE_NOT_FOUND, ext, short),
                    (Some((ext, short)), 0) => (SUCCESS, ext, short),
                    (Some(_), 1) => (SUCCESS, self.ext, me),
                    (Some(found), _) if by_ext => (INVALID_REQUEST, found.0, found.1),
                    (Some(_), _) => (INVALID_REQUEST, self.ext, me),
                };
                if status != SUCCESS && !unicast {
                    return None;
                }
                rsp.u8(status);
                rsp.u64(ext);
                rsp.u16(short);
                if status == SUCCESS && request_type == 1 {
                    let count_at = rsp.len();
                    rsp.u8(0);
                    if self.end_devices().next().is_some() {
                        rsp.u8(start as u8);
                        let mut count = 0;
                        for c in self.end_devices().skip(start) {
                            rsp.u16(c.short);
                            count += 1;
                        }
                        rsp.set(count_at, count);
                    }
                }
            }
            NODE_DESC_REQ | POWER_DESC_REQ | ACTIVE_EP_REQ | SIMPLE_DESC_REQ => {
                let about = r.u16()?;
                if about != me {
                    if !unicast {
                        return None;
                    }
                    rsp.u8(DEVICE_NOT_FOUND);
                    rsp.u16(about);
                    if cluster != NODE_DESC_REQ && cluster != POWER_DESC_REQ {
                        rsp.u8(0);
                    }
                    return Some(());
                }
                match cluster {
                    NODE_DESC_REQ => {
                        rsp.u8(SUCCESS);
                        rsp.u16(me);
                        rsp.u8(0x01);
                        rsp.u8(0x40);
                        rsp.u8(mac::CAPABILITY_ROUTER);
                        rsp.u16(self.identity.manufacturer_code);
                        rsp.u8(MAX_FRAME as u8);
                        rsp.u16(crate::util::MAX_TRANSFER as u16);
                        rsp.u16(STACK_REVISION << 9);
                        rsp.u16(crate::util::MAX_TRANSFER as u16);
                        rsp.u8(0);
                    }
                    POWER_DESC_REQ => {
                        rsp.u8(SUCCESS);
                        rsp.u16(me);
                        rsp.u16(0xC110);
                    }
                    ACTIVE_EP_REQ => {
                        rsp.u8(SUCCESS);
                        rsp.u16(me);
                        rsp.u8(2 + super::objects::ENDPOINTS.len() as u8);
                        rsp.u8(ENDPOINT);
                        for ep in super::objects::ENDPOINTS {
                            rsp.u8(ep);
                        }
                        rsp.u8(super::gp::GP_ENDPOINT);
                    }
                    _ => {
                        let endpoint = r.u8()?;
                        if endpoint == super::gp::GP_ENDPOINT {
                            rsp.u8(SUCCESS);
                            rsp.u16(me);
                            self.gp_simple_descriptor(rsp);
                            return Some(());
                        }
                        if let Some(o) = super::objects::object(endpoint) {
                            rsp.u8(SUCCESS);
                            rsp.u16(me);
                            rsp.u8(10);
                            rsp.u8(endpoint);
                            rsp.u16(PROFILE_HA);
                            rsp.u16(super::objects::device_id(o));
                            rsp.u8(1);
                            rsp.u8(1);
                            rsp.u16(o.cluster);
                            rsp.u8(0);
                            return Some(());
                        }
                        if endpoint != ENDPOINT {
                            if !unicast {
                                return None;
                            }
                            rsp.u8(if matches!(endpoint, 0x00 | 0xFF) { INVALID_EP } else { NOT_ACTIVE });
                            rsp.u16(me);
                            rsp.u8(0);
                            return Some(());
                        }
                        rsp.u8(SUCCESS);
                        rsp.u16(me);
                        let clusters = self.in_clusters();
                        rsp.u8(10 + 2 * clusters.len() as u8);
                        rsp.u8(ENDPOINT);
                        rsp.u16(PROFILE_HA);
                        rsp.u16(self.device_id());
                        rsp.u8(1);
                        rsp.u8(clusters.len() as u8);
                        for c in clusters {
                            rsp.u16(*c);
                        }
                        rsp.u8(1);
                        rsp.u16(super::ota::OTA);
                    }
                }
            }
            MATCH_DESC_REQ => {
                let about = r.u16()?;
                let profile = r.u16()?;
                let mut matched = 0u16;
                for _ in 0..r.u8()? {
                    let c = r.u16()?;
                    if self.in_clusters().contains(&c) {
                        matched |= 1;
                    }
                    for (i, o) in super::objects::OBJECTS.iter().enumerate() {
                        if o.cluster == c {
                            matched |= 2 << i;
                        }
                    }
                }
                if !((about == me || nwk::is_broadcast(about)) && (profile == PROFILE_HA || profile == PROFILE_WILDCARD)) {
                    matched = 0;
                }
                if matched == 0 && !unicast {
                    return None;
                }
                rsp.u8(SUCCESS);
                rsp.u16(me);
                rsp.u8(matched.count_ones() as u8);
                if matched & 1 != 0 {
                    rsp.u8(ENDPOINT);
                }
                for (i, o) in super::objects::OBJECTS.iter().enumerate() {
                    if matched & 2 << i != 0 {
                        rsp.u8(o.ep);
                    }
                }
            }
            BIND_REQ | UNBIND_REQ => {
                if !unicast {
                    return None;
                }
                let src_ext = r.u64()?;
                let src_ep = r.u8()?;
                let cluster_id = r.u16()?;
                let entry = match r.u8()? {
                    1 => Binding { used: true, src_ep, cluster: cluster_id, group: true, dst: r.u16()? as u64, dst_ep: 0 },
                    3 => Binding { used: true, src_ep, cluster: cluster_id, group: false, dst: r.u64()?, dst_ep: r.u8()? },
                    _ => {
                        rsp.u8(NOT_SUPPORTED);
                        return Some(());
                    }
                };
                let status = if src_ext != self.ext {
                    NOT_SUPPORTED
                } else if src_ep != ENDPOINT && super::objects::object(src_ep).is_none() {
                    INVALID_EP
                } else {
                    let found = self.bindings.iter().position(|b| *b == entry);
                    let free = self.bindings.iter().position(|b| !b.used);
                    match (cluster == BIND_REQ, found, free) {
                        (true, Some(_), _) => SUCCESS,
                        (true, None, Some(i)) => {
                            self.bindings[i] = entry;
                            self.tables_dirty = true;
                            SUCCESS
                        }
                        (true, None, None) => TABLE_FULL,
                        (false, Some(i), _) => {
                            self.bindings[i].used = false;
                            self.tables_dirty = true;
                            SUCCESS
                        }
                        (false, None, _) => NO_ENTRY,
                    }
                };
                rsp.u8(status);
            }
            MGMT_LQI_REQ => {
                let start = r.u8()? as usize;
                let neighbors = self.neighbors.iter().filter(|n| n.used).count();
                let children = self.children.iter().filter(|c| c.used && c.joined && c.capability & CAP_FFD == 0).count();
                rsp.u8(SUCCESS);
                rsp.u8((neighbors + children) as u8);
                rsp.u8(start as u8);
                let count_at = rsp.len();
                rsp.u8(0);
                let mut count = 0;
                for index in start..(start + 3).min(neighbors + children) {
                    let (ext, short, device, rx_on, relation, depth, lqi) = if index < neighbors {
                        let n = self.neighbors.iter().filter(|n| n.used).nth(index)?;
                        let relation = if n.short == self.nib.parent { 0 } else { 2 };
                        (n.ext, n.short, if n.short == 0 { 0 } else { 1 }, 1, relation, if n.short == 0 { 0 } else { self.nib.depth }, n.lqi)
                    } else {
                        let c = self.children.iter().filter(|c| c.used && c.joined && c.capability & CAP_FFD == 0).nth(index - neighbors)?;
                        (c.ext, c.short, 2, if c.sleepy() { 0 } else { 1 }, 1, self.nib.depth + 1, 255)
                    };
                    rsp.u64(self.nib.ext_pan);
                    rsp.u64(ext);
                    rsp.u16(short);
                    rsp.u8(device | rx_on << 2 | relation << 4);
                    rsp.u8(2);
                    rsp.u8(depth);
                    rsp.u8(lqi);
                    count += 1;
                }
                rsp.set(count_at, count);
            }
            MGMT_RTG_REQ => {
                let start = r.u8()? as usize;
                let total = self.routes.iter().filter(|e| e.status != ROUTE_FREE).count();
                rsp.u8(SUCCESS);
                rsp.u8(total as u8);
                rsp.u8(start as u8);
                let count_at = rsp.len();
                rsp.u8(0);
                let mut count = 0;
                for e in self.routes.iter().filter(|e| e.status != ROUTE_FREE).skip(start).take(10) {
                    rsp.u16(e.dst);
                    let status = if e.status == ROUTE_ACTIVE { 0 } else { 1 };
                    rsp.u8(status | (e.many_to_one as u8) << 4 | (e.record_due as u8) << 5);
                    rsp.u16(e.next);
                    count += 1;
                }
                rsp.set(count_at, count);
            }
            MGMT_BIND_REQ => {
                let start = r.u8()? as usize;
                let total = self.bindings.iter().filter(|b| b.used).count();
                rsp.u8(SUCCESS);
                rsp.u8(total as u8);
                rsp.u8(start as u8);
                let count_at = rsp.len();
                rsp.u8(0);
                let mut count = 0;
                for b in self.bindings.iter().filter(|b| b.used).skip(start).take(3) {
                    rsp.u64(self.ext);
                    rsp.u8(b.src_ep);
                    rsp.u16(b.cluster);
                    if b.group {
                        rsp.u8(1);
                        rsp.u16(b.dst as u16);
                    } else {
                        rsp.u8(3);
                        rsp.u64(b.dst);
                        rsp.u8(b.dst_ep);
                    }
                    count += 1;
                }
                rsp.set(count_at, count);
            }
            MGMT_LEAVE_REQ => {
                if !unicast {
                    return None;
                }
                let who = r.u64()?;
                let flags = r.u8().unwrap_or(0);
                if who == 0 || who == self.ext {
                    rsp.u8(SUCCESS);
                    let now = self.hal.now_ms();
                    self.leave_at = Some(now.wrapping_add(500));
                    self.leave_rejoin = flags & 0x80 != 0;
                } else if let Some(i) = self.child_by_ext(who) {
                    self.dismiss_child(i, (flags & 0x80) >> 2 | (flags & 0x40) << 1);
                    rsp.u8(SUCCESS);
                } else {
                    rsp.u8(UNKNOWN_DEVICE);
                }
            }
            MGMT_PERMIT_JOINING_REQ => {
                let seconds = r.u8()?.min(0xFE);
                let now = self.hal.now_ms();
                self.permit_until = match seconds {
                    0 => None,
                    s => Some(now.wrapping_add(s as u32 * 1000)),
                };
                if !unicast {
                    return None;
                }
                rsp.u8(SUCCESS);
            }
            MGMT_NWK_UPDATE_REQ => {
                let channels = r.u32()?;
                let duration = r.u8()?;
                match duration {
                    0xFE => {
                        let update_id = r.u8()?;
                        let channel = channels.trailing_zeros() as u8;
                        if (11..=26).contains(&channel) {
                            self.nib.update_id = update_id;
                            let now = self.hal.now_ms();
                            self.channel_at = Some((now.wrapping_add(9_000), channel));
                        }
                        return None;
                    }
                    0xFF => {
                        self.nib.update_id = r.u8()?;
                        self.manager = r.u16()?;
                        return None;
                    }
                    0..=5 => {
                        if !unicast {
                            return None;
                        }
                        let channels = channels & 0x07FF_F800;
                        rsp.u8(SUCCESS);
                        rsp.u32(channels);
                        rsp.u16(self.stats.tx as u16);
                        rsp.u16(self.stats.tx_failed as u16);
                        rsp.u8(channels.count_ones() as u8);
                        for channel in 11..=26u8 {
                            if channels & (1 << channel) != 0 {
                                let energy = self.hal.energy_detect(channel);
                                rsp.u8(energy);
                            }
                        }
                        self.hal.set_channel(self.nib.channel);
                    }
                    _ => {
                        if !unicast {
                            return None;
                        }
                        rsp.u8(INVALID_REQUEST);
                    }
                }
            }
            _ => {
                if !unicast {
                    return None;
                }
                rsp.u8(NOT_SUPPORTED);
            }
        }
        Some(())
    }

    fn on_zdo_response(&mut self, src: u16, cluster: u16, mut r: Reader) -> Option<()> {
        let status = r.u8()?;
        match cluster & !RESPONSE {
            MATCH_DESC_REQ if status == SUCCESS => {
                r.skip(2)?;
                if r.u8()? >= 1 {
                    let endpoint = r.u8()?;
                    self.ota_server_found(src, endpoint);
                }
            }
            NODE_DESC_REQ if self.tclk_step == 2 => {
                if status != SUCCESS || r.u16()? != 0x0000 {
                    return None;
                }
                r.skip(8)?;
                let revision = r.u16()? >> 9;
                if revision >= 21 {
                    self.tclk_advance(3);
                } else {
                    self.nib.tclk_state = 3;
                    self.store_network();
                    self.tclk_step = 0;
                    self.steering_done();
                }
            }
            NWK_ADDR_REQ if status == SUCCESS => {
                let ext = r.u64()?;
                let short = r.u16()?;
                self.learn_address(ext, short);
            }
            PARENT_ANNCE if status == SUCCESS => {
                for _ in 0..r.u8()? {
                    let ext = r.u64()?;
                    if let Some(i) = self.child_by_ext(ext) {
                        if !self.child_alive(i) {
                            self.remove_child(i);
                        }
                    }
                }
            }
            _ => {}
        }
        Some(())
    }

    fn end_devices(&self) -> impl Iterator<Item = &super::parent::Child> {
        self.children.iter().filter(|c| c.used && c.joined && c.capability & CAP_FFD == 0)
    }

    fn child_alive(&self, i: usize) -> bool {
        let since = self.children[i].heard.wrapping_sub(self.booted);
        since != 0 && since < 0x8000_0000
    }

    pub(crate) fn parent_announce(&mut self) {
        let mut p = Buf::new();
        p.u8(self.next_zdo_seq());
        let count_at = p.len();
        p.u8(0);
        let mut count = 0;
        let mut i = self.parent_annce_from;
        self.parent_annce_from = 0;
        while i < self.children.len() {
            let c = self.children[i];
            if c.used && c.capability & super::parent::CAP_FFD == 0 && !self.child_alive(i) {
                if count == PARENT_ANNCE_CHILDREN {
                    self.parent_annce_from = i;
                    let now = self.hal.now_ms();
                    let wait = super::join::PARENT_ANNCE_BASE_MS + self.hal.random() % super::join::PARENT_ANNCE_JITTER_MS;
                    self.parent_annce_at = Some(now.wrapping_add(wait));
                    break;
                }
                p.u64(c.ext);
                count += 1;
            }
            i += 1;
        }
        if count == 0 {
            return;
        }
        p.set(count_at, count as u8);
        self.aps_data(nwk::BROADCAST_ROUTERS, 0, PARENT_ANNCE, PROFILE_ZDO, 0, p.as_slice());
    }

    fn on_parent_announce(&mut self, src: u16, seq: u8, mut r: Reader) -> Option<()> {
        if src == self.nib.short {
            return Some(());
        }
        let mut rsp = Buf::new();
        let mut count = 0;
        let n = r.u8()?;
        if r.remaining() < n as usize * 8 {
            return None;
        }
        for k in 0..n {
            let ext = r.u64()?;
            if let Some(i) = self.child_by_ext(ext) {
                if self.child_alive(i) {
                    if count == 0 {
                        rsp = Buf::new();
                        rsp.u8(seq);
                        rsp.u8(SUCCESS);
                        rsp.u8(0);
                    }
                    rsp.u64(ext);
                    count += 1;
                }
            }
            if count != 0 && (count == PARENT_ANNCE_RSP_CHILDREN || k + 1 == n) {
                rsp.set(2, count as u8);
                self.aps_data(src, 0, PARENT_ANNCE | RESPONSE, PROFILE_ZDO, 0, rsp.as_slice());
                count = 0;
            }
        }
        Some(())
    }

    fn next_zdo_seq(&mut self) -> u8 {
        self.zdo_seq = self.zdo_seq.wrapping_add(1);
        self.zdo_seq
    }

    pub(crate) fn device_announce(&mut self) {
        let mut p = Buf::new();
        p.u8(self.next_zdo_seq());
        p.u16(self.nib.short);
        p.u64(self.ext);
        p.u8(mac::CAPABILITY_ROUTER);
        self.aps_data(nwk::BROADCAST_RX_ON, 0, DEVICE_ANNCE, PROFILE_ZDO, 0, p.as_slice());
    }

    pub(crate) fn zdo_permit_joining_request(&mut self, seconds: u8) {
        let seq = self.next_zdo_seq();
        self.aps_data(nwk::BROADCAST_ROUTERS, 0, MGMT_PERMIT_JOINING_REQ, PROFILE_ZDO, 0, &[seq, seconds, 0x01]);
    }

    pub(crate) fn zdo_node_desc_request(&mut self) {
        let seq = self.next_zdo_seq();
        self.aps_data(0x0000, 0, NODE_DESC_REQ, PROFILE_ZDO, 0, &[seq, 0x00, 0x00]);
    }

    pub(crate) fn zdo_nwk_addr_request(&mut self, ext: u64) {
        let mut p = Buf::new();
        p.u8(self.next_zdo_seq());
        p.u64(ext);
        p.u8(0);
        p.u8(0);
        self.aps_data(nwk::BROADCAST_RX_ON, 0, NWK_ADDR_REQ, PROFILE_ZDO, 0, p.as_slice());
    }

    fn learn_address(&mut self, ext: u64, short: u16) {
        if let Some(e) = self.addr_map.iter_mut().find(|e| e.0 == ext) {
            e.1 = short;
            return;
        }
        let i = self.addr_next as usize % self.addr_map.len();
        self.addr_map[i] = (ext, short);
        self.addr_next = self.addr_next.wrapping_add(1);
    }

    pub(crate) fn resolve(&self, ext: u64) -> Option<u16> {
        if ext == self.nib.tc_ext {
            return Some(0x0000);
        }
        if let Some(n) = self.neighbors.iter().find(|n| n.used && n.ext == ext) {
            return Some(n.short);
        }
        self.addr_map.iter().find(|e| e.0 == ext && e.1 != 0xFFFF).map(|e| e.1)
    }
}
