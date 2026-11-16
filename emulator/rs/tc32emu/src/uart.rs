use crate::machine::Machine;

impl Machine {
    pub(crate) fn uart_tx_byte(&mut self, b: u8) {
        self.uart_tx.push(b);
    }

    pub(crate) fn uart_rx_pop(&mut self) -> u8 {
        if self.uart_rx.is_empty() {
            return 0;
        }
        self.uart_rx.remove(0)
    }

    pub(crate) fn uart_buf_cnt(&self) -> u32 {
        self.uart_rx.len().min(7) as u32
    }
}
