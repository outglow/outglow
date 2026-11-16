pub const MAX_MPDU: usize = 125;

#[derive(Clone, Copy)]
pub struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Reader<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Reader { b, i: 0 }
    }
    pub fn pos(&self) -> usize {
        self.i
    }
    pub fn remaining(&self) -> usize {
        self.b.len() - self.i
    }
    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.i.checked_add(n)?;
        let s = self.b.get(self.i..end)?;
        self.i = end;
        Some(s)
    }
    pub fn skip(&mut self, n: usize) -> Option<()> {
        self.take(n).map(|_| ())
    }
    pub fn rest(&mut self) -> &'a [u8] {
        let s = self.b.get(self.i..).unwrap_or(&[]);
        self.i = self.b.len();
        s
    }
    pub fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|s| s[0])
    }
    pub fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|s| u16::from_le_bytes([s[0], s[1]]))
    }
    pub fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    pub fn u64(&mut self) -> Option<u64> {
        self.take(8).map(|s| {
            let mut a = [0u8; 8];
            a.copy_from_slice(s);
            u64::from_le_bytes(a)
        })
    }
    pub fn key(&mut self) -> Option<[u8; 16]> {
        self.take(16).map(|s| {
            let mut a = [0u8; 16];
            a.copy_from_slice(s);
            a
        })
    }
}

#[derive(Clone, Copy)]
pub struct Buffer<const N: usize> {
    d: [u8; N],
    n: u16,
    overflow: bool,
}

pub type Buf = Buffer<MAX_MPDU>;
pub type Long = Buffer<MAX_TRANSFER>;

pub const MAX_TRANSFER: usize = 250;

impl<const N: usize> Buffer<N> {
    pub const fn new() -> Self {
        Buffer { d: [0; N], n: 0, overflow: false }
    }
    pub fn from_slice(s: &[u8]) -> Self {
        let mut b = Self::new();
        b.bytes(s);
        b
    }
    pub fn len(&self) -> usize {
        self.n as usize
    }
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
    pub fn ok(&self) -> bool {
        !self.overflow
    }
    pub fn room(&self) -> usize {
        N - self.n as usize
    }
    pub fn as_slice(&self) -> &[u8] {
        &self.d[..self.n as usize]
    }
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.d[..self.n as usize]
    }
    pub fn clear(&mut self) {
        self.n = 0;
        self.overflow = false;
    }
    pub fn truncate(&mut self, n: usize) {
        if n < self.n as usize {
            self.n = n as u16;
        }
    }
    pub fn bytes(&mut self, s: &[u8]) {
        let n = self.n as usize;
        match self.d.get_mut(n..n + s.len()) {
            Some(dst) => {
                dst.copy_from_slice(s);
                self.n = (n + s.len()) as u16;
            }
            None => self.overflow = true,
        }
    }
    pub fn u8(&mut self, v: u8) {
        self.bytes(&[v]);
    }
    pub fn u16(&mut self, v: u16) {
        self.bytes(&v.to_le_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
    pub fn set(&mut self, i: usize, v: u8) {
        if let Some(b) = self.d[..self.n as usize].get_mut(i) {
            *b = v;
        }
    }
    pub fn get(&self, i: usize) -> u8 {
        self.as_slice().get(i).copied().unwrap_or(0)
    }
}

impl<const N: usize> Default for Buffer<N> {
    fn default() -> Self {
        Self::new()
    }
}

pub fn due(now: u32, at: u32) -> bool {
    now.wrapping_sub(at) as i32 >= 0
}
