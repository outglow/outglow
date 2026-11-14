// SPDX-License-Identifier: Apache-2.0
//! The package variables of go/ble that the environment sets
//! (TC32EMU_SUSPEND_RF, TC32EMU_RX_BUSY, TC32EMU_TXFIFO_CLEAR,
//! TC32EMU_CENTRAL_SKIP_EVERY, TC32EMU_AES_LOG). Go reads them once at
//! package init and its tests assign them; here they are per thread, read
//! from the environment on first use, and settable for the tests.

use std::cell::Cell;
use std::env;

fn choice(name: &str, def: &'static str, allowed: &[&'static str]) -> &'static str {
    let v = env::var(name).unwrap_or_default();
    if v.is_empty() {
        return def;
    }
    for a in allowed {
        if *a == v {
            return a;
        }
    }
    panic!("{}: {}", name, v);
}

thread_local! {
    static SUSPEND_RF: Cell<Option<&'static str>> = const { Cell::new(None) };
    static RX_BUSY: Cell<Option<&'static str>> = const { Cell::new(None) };
    static TXFIFO_CLEAR: Cell<Option<&'static str>> = const { Cell::new(None) };
    static SKIP_EVERY: Cell<Option<i64>> = const { Cell::new(None) };
    static AES_LOG: Cell<Option<bool>> = const { Cell::new(None) };
}

/// SuspendRF: "keep" (default) or "lost".
pub fn suspend_rf() -> &'static str {
    SUSPEND_RF.with(|c| match c.get() {
        Some(v) => v,
        None => {
            let v = choice("TC32EMU_SUSPEND_RF", "keep", &["keep", "lost"]);
            c.set(Some(v));
            v
        }
    })
}
pub fn set_suspend_rf(v: &'static str) {
    SUSPEND_RF.with(|c| c.set(Some(v)));
}

/// RxBusy: "off" (default), "air" or "stuck".
pub fn rx_busy() -> &'static str {
    RX_BUSY.with(|c| match c.get() {
        Some(v) => v,
        None => {
            let v = choice("TC32EMU_RX_BUSY", "off", &["off", "air", "stuck"]);
            c.set(Some(v));
            v
        }
    })
}
pub fn set_rx_busy(v: &'static str) {
    RX_BUSY.with(|c| c.set(Some(v)));
}

/// TxFIFOClear: "position" (default) or "zero".
pub fn txfifo_clear() -> &'static str {
    TXFIFO_CLEAR.with(|c| match c.get() {
        Some(v) => v,
        None => {
            let v = choice("TC32EMU_TXFIFO_CLEAR", "position", &["position", "zero"]);
            c.set(Some(v));
            v
        }
    })
}
pub fn set_txfifo_clear(v: &'static str) {
    TXFIFO_CLEAR.with(|c| c.set(Some(v)));
}

/// CentralSkipEvery: 0 (default) never skips.
pub fn central_skip_every() -> i64 {
    SKIP_EVERY.with(|c| match c.get() {
        Some(v) => v,
        None => {
            let s = env::var("TC32EMU_CENTRAL_SKIP_EVERY").unwrap_or_default();
            let v = if s.is_empty() {
                0
            } else {
                match s.parse::<i64>() {
                    Ok(n) if n >= 0 => n,
                    _ => panic!("TC32EMU_CENTRAL_SKIP_EVERY: {}", s),
                }
            };
            c.set(Some(v));
            v
        }
    })
}
pub fn set_central_skip_every(v: i64) {
    SKIP_EVERY.with(|c| c.set(Some(v)));
}

/// TC32EMU_AES_LOG set (Go looks it up at every block; the environment does not change).
pub fn aes_log() -> bool {
    AES_LOG.with(|c| match c.get() {
        Some(v) => v,
        None => {
            let v = !env::var("TC32EMU_AES_LOG").unwrap_or_default().is_empty();
            c.set(Some(v));
            v
        }
    })
}

/// Reads every setting now (Go panics on a bad value at package init).
pub fn init() {
    suspend_rf();
    rx_busy();
    txfifo_clear();
    central_skip_every();
    aes_log();
}
