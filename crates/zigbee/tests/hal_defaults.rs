use zigbee::hal::{Hal, HalCounters, NvItem, ResetCause, RxMeta, TxStatus};

#[derive(Default)]
struct Bare {
    stored: Vec<(NvItem, Vec<u8>)>,
}

impl Hal for Bare {
    fn now_ms(&mut self) -> u32 {
        0
    }
    fn random(&mut self) -> u32 {
        4
    }
    fn ieee(&mut self) -> u64 {
        1
    }
    fn set_channel(&mut self, _channel: u8) {}
    fn set_filter(&mut self, _pan: u16, _short: u16) {}
    fn transmit(&mut self, _mpdu: &[u8]) -> TxStatus {
        TxStatus::Ok
    }
    fn receive(&mut self, _buf: &mut [u8; 127]) -> Option<RxMeta> {
        None
    }
    fn nv_load(&mut self, _item: NvItem, _buf: &mut [u8]) -> Option<usize> {
        None
    }
    fn nv_store(&mut self, item: NvItem, data: &[u8]) {
        self.stored.push((item, data.to_vec()));
    }
    fn nv_erase(&mut self) {}
}

#[test]
fn a_hal_without_the_optional_parts_reports_that_it_has_none() {
    let mut h = Bare::default();
    assert_eq!(h.noise_dbm(), None, "no noise floor");
    assert_eq!(h.energy_detect(15), 0, "no energy scan");
    assert!(!h.ota_begin(1000), "nowhere to put an image");
    assert!(!h.ota_resume(1000));
    assert_eq!(h.tx_attempts(), 0);
    assert_eq!(h.counters(), HalCounters::default());
    assert!(h.black_box().is_none());
    assert_eq!(h.reset_cause(), ResetCause::PowerOn);
    let mut buf = [0u8; 8];
    assert_eq!(h.retained(&mut buf), None, "nothing kept over a restart");
}

#[test]
fn two_items_stored_together_are_stored_one_after_the_other_by_default() {
    let mut h = Bare::default();
    h.nv_store_pair(NvItem::Network, &[1, 2], NvItem::Counters, &[3]);
    assert_eq!(h.stored, vec![(NvItem::Network, vec![1, 2]), (NvItem::Counters, vec![3])]);
}

#[test]
fn the_optional_outputs_of_a_bare_hal_do_nothing() {
    let mut h = Bare::default();
    h.indirect_pending(&[1], &[2]);
    h.poll_children(&[1]);
    h.ota_write(0, &[1, 2]);
    h.ota_activate();
    h.transmit_at(&[1], 10);
    h.trace(zigbee::hal::Trace::default());
    h.retain(&[1]);
    h.zcl_received(1, 6, 0, 0x11, 255, -40);
    h.sense_listen(0x5E55);
    assert!(h.stored.is_empty());
    let mut buf = [0u8; 4];
    assert_eq!(h.retained(&mut buf), None, "what was kept is not given back");
}
