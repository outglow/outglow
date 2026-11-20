use super::parent::{Child, CHILD_ADDRESSES};
use super::{Binding, BleMode, Nib, Stack, BINDINGS, BLE_WINDOW_MAX_MIN, CHILDREN, COUNTER_STEP, GROUPS};
use crate::aes::Aes128;
use crate::hal::{Hal, NvItem};
use crate::util::{pack, Reader};

const NETWORK_VERSION: u8 = 1;
const TABLES_VERSION: u8 = 2;
const SCENES_VERSION: u8 = 3;
const CHILDREN_VERSION: u8 = 1;
const SETTINGS_VERSION: u8 = 1;
const ITEM_MAX: usize = 160;
const CHILDREN_PER_ITEM: usize = 13;
const _: () = assert!(CHILDREN_PER_ITEM == (ITEM_MAX - 1) / 12, "children of 12 bytes after the version byte");
const _: () = assert!(CHILDREN <= 3 * CHILDREN_PER_ITEM, "three items (NvItem::children) hold the child table");
pub(crate) const CHILDREN_PARTS: usize = CHILDREN.div_ceil(CHILDREN_PER_ITEM);
pub(crate) const NO_CHILDREN: u32 = digest(&[CHILDREN_VERSION]);

const fn digest(b: &[u8]) -> u32 {
    let mut h = 0x811C_9DC5u32;
    let mut i = 0;
    while i < b.len() {
        h = (h ^ b[i] as u32).wrapping_mul(0x0100_0193);
        i += 1;
    }
    h
}

struct Blob {
    d: [u8; ITEM_MAX],
    n: usize,
}

impl Blob {
    fn new() -> Self {
        Blob { d: [0; ITEM_MAX], n: 0 }
    }
    fn put(&mut self, s: &[u8]) {
        if let Some(dst) = self.d.get_mut(self.n..self.n + s.len()) {
            dst.copy_from_slice(s);
            self.n += s.len();
        }
    }
    fn u8(&mut self, v: u8) {
        self.put(&[v]);
    }
    fn u16(&mut self, v: u16) {
        self.put(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.put(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.put(&v.to_le_bytes());
    }
    fn slice(&self) -> &[u8] {
        &self.d[..self.n]
    }
}

impl<H: Hal> Stack<H> {
    pub(crate) fn nv_put(&mut self, item: NvItem, data: &[u8]) {
        self.hal.nv_store(item, data);
        self.nv_written();
    }

    pub(crate) fn store_network(&mut self) {
        let n = &self.nib;
        let mut b = Blob::new();
        b.u8(NETWORK_VERSION);
        b.u8(n.channel);
        b.u16(n.pan);
        b.u16(n.short);
        b.u64(n.ext_pan);
        b.u8(n.update_id);
        b.u8(n.depth);
        b.u16(n.parent);
        b.put(&n.key);
        b.u8(n.key_seq);
        b.u8(pack(&[(n.alt_valid as u16, 0), (n.alt_old as u16, 1)]) as u8);
        b.u8(n.alt_seq);
        b.put(&n.alt_key);
        b.u64(n.tc_ext);
        b.put(&n.tclk);
        b.put(&n.tclk_new);
        b.u8(n.tclk_state);
        self.nv_put(NvItem::Network, b.slice());
    }

    pub(crate) fn load_network(&mut self) -> bool {
        let mut raw = [0u8; ITEM_MAX];
        let Some(len) = self.hal.nv_load(NvItem::Network, &mut raw) else {
            return false;
        };
        let parsed = (|| {
            let mut r = Reader::new(raw.get(..len)?);
            if r.u8()? != NETWORK_VERSION {
                return None;
            }
            let alt;
            Some(Nib {
                channel: r.u8()?,
                pan: r.u16()?,
                short: r.u16()?,
                ext_pan: r.u64()?,
                update_id: r.u8()?,
                depth: r.u8()?,
                parent: r.u16()?,
                key: r.key()?,
                key_seq: r.u8()?,
                alt_valid: {
                    alt = r.u8()?;
                    alt & 1 != 0
                },
                alt_old: alt & 2 != 0,
                alt_seq: r.u8()?,
                alt_key: r.key()?,
                tc_ext: r.u64()?,
                tclk: r.key()?,
                tclk_new: r.key()?,
                tclk_state: r.u8()?,
            })
        })();
        match parsed {
            Some(nib) if (11..=26).contains(&nib.channel) && nib.short < 0xFFF8 => {
                self.nib = nib;
                self.key = Aes128::with_engine(&nib.key, H::AES);
                true
            }
            _ => false,
        }
    }

    pub(crate) fn store_counters(&mut self) {
        let mut b = Blob::new();
        b.u32(self.fc_stored);
        b.u32(self.aps_fc_stored);
        self.nv_put(NvItem::Counters, b.slice());
    }

    pub(crate) fn store_settings(&mut self) {
        let mut b = Blob::new();
        b.u8(SETTINGS_VERSION);
        b.u8(self.ble_mode.to_u8());
        b.u16(self.ble_window);
        self.hal.nv_store(NvItem::Settings, b.slice());
    }

    pub(crate) fn load_settings(&mut self) {
        let mut raw = [0u8; 4];
        if let Some(4) = self.hal.nv_load(NvItem::Settings, &mut raw) {
            let mut r = Reader::new(&raw);
            let (Some(SETTINGS_VERSION), Some(mode), Some(window)) = (r.u8(), r.u8(), r.u16()) else {
                return;
            };
            if let (Some(mode), true) = (BleMode::from_u8(mode), (1..=BLE_WINDOW_MAX_MIN).contains(&window)) {
                self.ble_mode = mode;
                self.ble_window = window;
            }
        }
    }

    pub(crate) fn load_counters(&mut self) {
        let mut raw = [0u8; 8];
        if let Some(8) = self.hal.nv_load(NvItem::Counters, &mut raw) {
            let mut r = Reader::new(&raw);
            self.fc_out = r.u32().unwrap_or(0);
            self.aps_fc_out = r.u32().unwrap_or(0);
        }
        self.fc_stored = self.fc_out.wrapping_add(COUNTER_STEP);
        self.aps_fc_stored = self.aps_fc_out.wrapping_add(COUNTER_STEP);
        let boot = self.diag_start();
        let mut b = Blob::new();
        b.u32(self.fc_stored);
        b.u32(self.aps_fc_stored);
        self.hal.nv_store_pair(NvItem::Counters, b.slice(), NvItem::Boot, &boot);
        self.nv_written();
    }

    pub(crate) fn store_tables(&mut self) {
        let mut b = Blob::new();
        b.u8(TABLES_VERSION);
        for g in self.groups {
            b.u16(g);
        }
        for bind in self.bindings {
            b.u8(pack(&[(bind.used as u16, 0), (bind.group as u16, 1), ((bind.src_ep.wrapping_sub(1) & 0x3F) as u16, 2)]) as u8);
            b.u16(bind.cluster);
            b.u64(bind.dst);
            b.u8(bind.dst_ep);
        }
        for r in self.reports {
            b.u16(r.min_s);
            b.u16(r.max_s);
            b.u16(r.change);
        }
        b.u8(self.level_options);
        b.u8(self.color_options);
        b.u8(self.on_level);
        b.u8(self.allow_children as u8);
        self.nv_put(NvItem::Tables, b.slice());
    }

    pub(crate) fn load_tables(&mut self) {
        let mut raw = [0u8; ITEM_MAX];
        let Some(len) = self.hal.nv_load(NvItem::Tables, &mut raw) else {
            return;
        };
        let mut groups = [0u16; GROUPS];
        let mut bindings = self.bindings;
        let mut reports = self.reports;
        let ok = (|| {
            let mut r = Reader::new(raw.get(..len)?);
            if r.u8()? != TABLES_VERSION {
                return None;
            }
            for g in groups.iter_mut() {
                *g = r.u16()?;
            }
            for bind in bindings.iter_mut().take(BINDINGS) {
                let flags = r.u8()?;
                *bind = Binding {
                    used: flags & 1 != 0,
                    group: flags & 2 != 0,
                    src_ep: (flags >> 2) + 1,
                    cluster: r.u16()?,
                    dst: r.u64()?,
                    dst_ep: r.u8()?,
                };
            }
            for rep in reports.iter_mut() {
                rep.min_s = r.u16()?;
                rep.max_s = r.u16()?;
                rep.change = r.u16()?;
            }
            self.level_options = r.u8().unwrap_or(0);
            self.color_options = r.u8().unwrap_or(super::zcl::COLOR_OPTIONS_DEFAULT);
            self.on_level = r.u8().unwrap_or(0xFF);
            self.allow_children = r.u8().is_none_or(|v| v != 0);
            Some(())
        })();
        if ok.is_some() {
            self.groups = groups;
            self.bindings = bindings;
            self.reports = reports;
        }
    }

    pub(crate) fn store_scenes(&mut self) {
        let mut b = Blob::new();
        b.u8(SCENES_VERSION);
        for s in self.scenes.iter().filter(|s| s.used) {
            b.u16(s.group);
            b.u8(s.id);
            b.u8(pack(&[(s.on as u16, 0), (s.omitted as u16, 1)]) as u8);
            b.u8(s.level);
            b.u16(s.color_temp);
            b.u16(s.transition);
            b.u8(s.tenths);
            b.u8(s.colour.mode);
            b.u8(s.colour.hue);
            b.u8(s.colour.sat);
            b.u16(s.colour.x);
            b.u16(s.colour.y);
        }
        self.nv_put(NvItem::Scenes, b.slice());
    }

    pub(crate) fn load_scenes(&mut self) {
        let mut raw = [0u8; ITEM_MAX];
        let Some(len) = self.hal.nv_load(NvItem::Scenes, &mut raw) else {
            return;
        };
        let mut r = Reader::new(&raw[..len.min(ITEM_MAX)]);
        let version = r.u8();
        if !matches!(version, Some(2) | Some(SCENES_VERSION)) {
            return;
        }
        for slot in self.scenes.iter_mut() {
            let (Some(group), Some(id), Some(on), Some(level), Some(color_temp), Some(transition), Some(tenths)) =
                (r.u16(), r.u8(), r.u8(), r.u8(), r.u16(), r.u16(), r.u8())
            else {
                break;
            };
            let colour = if version == Some(SCENES_VERSION) {
                let (Some(mode), Some(hue), Some(sat), Some(x), Some(y)) = (r.u8(), r.u8(), r.u8(), r.u16(), r.u16()) else {
                    break;
                };
                super::SceneColour { mode, hue, sat, x, y }
            } else {
                super::SceneColour::TEMPERATURE
            };
            *slot = super::Scene { used: true, group, id, on: on & 1 != 0, omitted: on >> 1, level, color_temp, colour, transition, tenths };
        }
    }

    pub(crate) fn store_children(&mut self) {
        for k in 0..CHILDREN_PARTS {
            let mut b = Blob::new();
            b.u8(CHILDREN_VERSION);
            let part = &self.children[k * CHILDREN_PER_ITEM..((k + 1) * CHILDREN_PER_ITEM).min(CHILDREN)];
            for c in part.iter().filter(|c| c.used && c.joined) {
                b.u64(c.ext);
                b.u16(c.short);
                b.u8(c.capability);
                b.u8(c.timeout);
            }
            let d = digest(b.slice());
            if d != self.children_stored[k] {
                self.nv_put(NvItem::children(k), b.slice());
                self.children_stored[k] = d;
            }
        }
    }

    pub(crate) fn load_children(&mut self, now: u32) {
        for k in 0..CHILDREN_PARTS {
            let mut raw = [0u8; ITEM_MAX];
            let Some(len) = self.hal.nv_load(NvItem::children(k), &mut raw) else {
                continue;
            };
            let len = len.min(ITEM_MAX);
            self.children_stored[k] = digest(&raw[..len]);
            let mut r = Reader::new(&raw[..len]);
            if r.u8() != Some(CHILDREN_VERSION) {
                continue;
            }
            let never = self.never_a_child();
            let part = &mut self.children[k * CHILDREN_PER_ITEM..((k + 1) * CHILDREN_PER_ITEM).min(CHILDREN)];
            for slot in part.iter_mut() {
                let (Some(ext), Some(short), Some(capability), Some(timeout)) = (r.u64(), r.u16(), r.u8(), r.u8()) else {
                    break;
                };
                let used = CHILD_ADDRESSES.contains(&short) && !never.contains(&ext);
                *slot = Child { used, ext, short, capability, joined: true, timeout, heard: now, ..Child::EMPTY };
            }
        }
    }
}

#[cfg(test)]
mod tests;
