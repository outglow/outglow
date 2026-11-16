use super::Stack;
use crate::hal::Hal;
use crate::util::{due, Buf, MAX_TRANSFER};

const BLOCK: usize = 64;
const BLOCK_SECURED: usize = 60;
pub(crate) const SINGLE_MAX: usize = 80;
const ACK_WAIT_MS: u32 = 1_600;
const RETRIES: u8 = 3;
const REASSEMBLY_MS: u32 = ACK_WAIT_MS * (1 + RETRIES as u32);

const FIRST: u8 = 1;
const SUBSEQUENT: u8 = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Addressing {
    pub peer: u16,
    pub dst_ep: u8,
    pub src_ep: u8,
    pub cluster: u16,
    pub profile: u16,
    pub counter: u8,
}

impl Addressing {
    const NONE: Addressing = Addressing { peer: 0xFFFF, dst_ep: 0, src_ep: 0, cluster: 0, profile: 0, counter: 0 };
}

pub(crate) struct Incoming {
    active: bool,
    done: bool,
    a: Addressing,
    total: u8,
    block_len: u8,
    last_len: u8,
    got: u32,
    expires: u32,
    buf: [u8; MAX_TRANSFER],
}

pub(crate) struct Outgoing {
    active: bool,
    secure: bool,
    a: Addressing,
    len: usize,
    block: u8,
    tries: u8,
    at: u32,
    buf: [u8; MAX_TRANSFER],
}

impl Incoming {
    pub(crate) const EMPTY: Incoming = Incoming {
        active: false,
        done: false,
        a: Addressing::NONE,
        total: 0,
        block_len: 0,
        last_len: 0,
        got: 0,
        expires: 0,
        buf: [0; MAX_TRANSFER],
    };
}

impl Outgoing {
    pub(crate) const EMPTY: Outgoing =
        Outgoing { active: false, secure: false, a: Addressing::NONE, len: 0, block: 0, tries: 0, at: 0, buf: [0; MAX_TRANSFER] };

    fn block_len(&self) -> usize {
        if self.secure {
            BLOCK_SECURED
        } else {
            BLOCK
        }
    }

    fn blocks(&self) -> u8 {
        self.len.div_ceil(self.block_len()) as u8
    }
}

impl<H: Hal> Stack<H> {
    pub(crate) fn on_fragment(&mut self, a: Addressing, kind: u8, block: u8, data: &[u8]) -> Option<()> {
        let now = self.hal.now_ms();
        let rx = &mut self.frag_in;
        if (rx.active || rx.done) && due(now, rx.expires) {
            rx.active = false;
            rx.done = false;
        }
        let index = match kind {
            FIRST => {
                if !((rx.active || rx.done) && rx.a == a) {
                    if data.is_empty() || data.len() > 255 || block == 0 || block > 32 {
                        return None;
                    }
                    if (block as usize - 1) * data.len() >= MAX_TRANSFER {
                        return None;
                    }
                    rx.active = true;
                    rx.done = false;
                    rx.a = a;
                    rx.total = block;
                    rx.block_len = data.len() as u8;
                    rx.last_len = 0;
                    rx.got = 0;
                }
                0
            }
            SUBSEQUENT => block,
            _ => return None,
        };
        if rx.a != a || !(rx.active || rx.done) {
            return None;
        }
        if rx.active {
            if index >= rx.total {
                return None;
            }
            let at = index as usize * rx.block_len as usize;
            let last = index == rx.total - 1;
            if (!last && data.len() != rx.block_len as usize) || data.is_empty() || at + data.len() > MAX_TRANSFER {
                rx.active = false;
                return None;
            }
            rx.buf[at..at + data.len()].copy_from_slice(data);
            rx.got |= 1 << index;
            if last {
                rx.last_len = data.len() as u8;
            }
            rx.expires = now.wrapping_add(REASSEMBLY_MS);
        }
        let mut ack = Buf::new();
        ack.u8(0x82);
        ack.u8(a.src_ep);
        ack.u16(a.cluster);
        ack.u16(a.profile);
        ack.u8(a.dst_ep);
        ack.u8(a.counter);
        ack.u8(kind);
        ack.u8(index);
        ack.u8(0xFF);
        self.nwk_data(a.peer, ack.as_slice());

        let rx = &mut self.frag_in;
        if rx.active && rx.got == u32::MAX >> (32 - rx.total as u32) {
            rx.active = false;
            rx.done = true;
            rx.expires = now.wrapping_add(REASSEMBLY_MS);
            let len = (rx.total as usize - 1) * rx.block_len as usize + rx.last_len as usize;
            let whole = rx.buf;
            self.aps_deliver(a.peer, a.dst_ep, a.src_ep, a.cluster, a.profile, &whole[..len], true);
        }
        Some(())
    }

    pub(crate) fn send_fragmented(&mut self, a: Addressing, payload: &[u8], secure: bool) -> bool {
        if self.frag_out.active || payload.len() > MAX_TRANSFER {
            return false;
        }
        let tx = &mut self.frag_out;
        tx.active = true;
        tx.secure = secure;
        tx.a = a;
        tx.len = payload.len();
        tx.buf[..payload.len()].copy_from_slice(payload);
        tx.block = 0;
        tx.tries = 0;
        self.send_block();
        true
    }

    fn send_block(&mut self) {
        let now = self.hal.now_ms();
        let tx = &mut self.frag_out;
        tx.at = now.wrapping_add(ACK_WAIT_MS);
        let from = tx.block as usize * tx.block_len();
        let to = (from + tx.block_len()).min(tx.len);
        let secure = tx.secure;
        let mut f = Buf::new();
        f.u8(if secure { 0xE0 } else { 0xC0 });
        f.u8(tx.a.dst_ep);
        f.u16(tx.a.cluster);
        f.u16(tx.a.profile);
        f.u8(tx.a.src_ep);
        f.u8(tx.a.counter);
        if tx.block == 0 {
            f.u8(FIRST);
            f.u8(tx.blocks());
        } else {
            f.u8(SUBSEQUENT);
            f.u8(tx.block);
        }
        let peer = tx.a.peer;
        if secure {
            let mut block = [0u8; BLOCK_SECURED];
            let n = to - from;
            block[..n].copy_from_slice(&tx.buf[from..to]);
            self.aps_secure(&mut f, &block[..n]);
        } else {
            f.bytes(&tx.buf[from..to]);
        }
        self.nwk_data(peer, f.as_slice());
    }

    pub(crate) fn on_fragment_ack(&mut self, src: u16, counter: u8, block: u8, bits: u8) {
        let tx = &mut self.frag_out;
        if !tx.active || tx.a.peer != src || tx.a.counter != counter || block != tx.block || bits & 1 == 0 {
            return;
        }
        tx.block += 1;
        tx.tries = 0;
        if tx.block >= tx.blocks() {
            tx.active = false;
        } else {
            self.send_block();
        }
    }

    pub(crate) fn frag_timers(&mut self, now: u32) {
        if self.frag_out.active && due(now, self.frag_out.at) {
            self.frag_out.tries += 1;
            if self.frag_out.tries > RETRIES {
                self.frag_out.active = false;
                self.diag.add(super::diag::k::APS_TX_UCAST_FAIL);
            } else {
                self.diag.add(super::diag::k::APS_TX_UCAST_RETRY);
                self.send_block();
            }
        }
    }
}
