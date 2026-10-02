//! Bit-level reading and writing, most significant bit first (clause 5.2 of
//! ISO/IEC 14496-2: every syntax element is written MSB first).

use crate::error::{Result, invalid};

/// Reads bits from a byte slice. Peeking past the end yields zero bits;
/// reading past it is an error, so a truncated VOP fails instead of decoding
/// zeros forever.
#[derive(Clone)]
pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0 }
    }

    /// Bit position from the start of the buffer.
    #[inline]
    pub fn pos(&self) -> usize {
        self.pos
    }

    #[inline]
    pub fn set_pos(&mut self, pos: usize) {
        self.pos = pos;
    }

    /// Total bits in the buffer.
    #[inline]
    pub fn len_bits(&self) -> usize {
        self.data.len() * 8
    }

    /// Bits left before the end (negative positions never occur; past the
    /// end this is 0).
    #[inline]
    pub fn left(&self) -> usize {
        self.len_bits().saturating_sub(self.pos)
    }

    /// The next 64 bits at `pos`, zero-padded past the end.
    #[inline]
    fn window(&self, pos: usize) -> u64 {
        let byte = pos >> 3;
        let bit = (pos & 7) as u32;
        let v = if byte + 8 <= self.data.len() {
            u64::from_be_bytes(self.data[byte..byte + 8].try_into().unwrap())
        } else {
            let mut b = [0u8; 8];
            if byte < self.data.len() {
                let n = self.data.len() - byte;
                b[..n].copy_from_slice(&self.data[byte..]);
            }
            u64::from_be_bytes(b)
        };
        v << bit
    }

    /// The next `n` bits (0..=32) without consuming them.
    #[inline]
    pub fn peek(&self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        (self.window(self.pos) >> (64 - n)) as u32
    }

    /// `n` bits (0..=32) starting `offset` bits ahead, without consuming.
    #[inline]
    pub fn peek_at(&self, offset: usize, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        (self.window(self.pos + offset) >> (64 - n)) as u32
    }

    #[inline]
    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.pos += n;
        if self.pos > self.len_bits() {
            return Err(invalid("data runs past the end of the buffer"));
        }
        Ok(())
    }

    /// Reads `n` bits (0..=32).
    #[inline]
    pub fn read(&mut self, n: u32) -> Result<u32> {
        let v = self.peek(n);
        self.skip(n as usize)?;
        Ok(v)
    }

    #[inline]
    pub fn read_bit(&mut self) -> Result<bool> {
        Ok(self.read(1)? != 0)
    }

    /// A marker bit, which must be 1.
    pub fn marker(&mut self, what: &str) -> Result<()> {
        if self.read(1)? != 1 {
            return Err(invalid(format!("marker bit missing {what}")));
        }
        Ok(())
    }

    /// A marker bit that encoders are known to get wrong: read, not checked.
    pub fn lenient_marker(&mut self) -> Result<()> {
        self.read(1).map(|_| ())
    }

    /// Advances to the next byte boundary (no-op when aligned).
    pub fn align(&mut self) {
        self.pos = (self.pos + 7) & !7;
    }

    /// `nextbits_bytealigned()` of clause 5.2.4: the position of the bits
    /// after the stuffing that would follow, without consuming anything.
    /// Stuffing is a `0` then `1`s up to the byte boundary; from an aligned
    /// position it is the whole byte `0111 1111`, so a following start code
    /// or resync marker is looked for after that byte when it is there.
    pub fn stuffing_end(&self) -> Option<usize> {
        let to_align = 8 - (self.pos & 7);
        let stuffing = self.peek(to_align as u32);
        let expected = (1u32 << (to_align - 1)) - 1; // 0 followed by ones
        if stuffing == expected {
            Some(self.pos + to_align)
        } else {
            None
        }
    }
}

/// Writes bits MSB first into a growing byte vector.
#[derive(Default, Clone)]
pub(crate) struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    nbits: u32,
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Writes the low `n` bits (0..=32) of `v`.
    #[inline]
    pub fn put(&mut self, n: u32, v: u32) {
        if n == 0 {
            return;
        }
        debug_assert!(n <= 32);
        let v = (v as u64) & ((1u64 << n) - 1);
        self.acc = (self.acc << n) | v;
        self.nbits += n;
        while self.nbits >= 8 {
            self.nbits -= 8;
            self.out.push((self.acc >> self.nbits) as u8);
        }
        self.acc &= (1u64 << self.nbits) - 1;
    }

    /// Bits written so far.
    pub fn len_bits(&self) -> usize {
        self.out.len() * 8 + self.nbits as usize
    }

    pub fn is_aligned(&self) -> bool {
        self.nbits == 0
    }

    /// `next_start_code()` stuffing: a zero bit, then ones to the boundary
    /// (a whole `0111 1111` byte when already aligned).
    pub fn stuff(&mut self) {
        self.put(1, 0);
        while !self.is_aligned() {
            self.put(1, 1);
        }
    }

    /// Zero bits to the byte boundary (for headers that end aligned by
    /// definition, and the short video header's end of picture).
    pub fn align_zero(&mut self) {
        while !self.is_aligned() {
            self.put(1, 0);
        }
    }

    /// Appends whole bytes; the writer must be aligned.
    pub fn put_bytes(&mut self, b: &[u8]) {
        debug_assert!(self.is_aligned());
        self.out.extend_from_slice(b);
    }

    pub fn into_bytes(mut self) -> Vec<u8> {
        self.align_zero();
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read() {
        let mut w = BitWriter::new();
        w.put(3, 0b101);
        w.put(17, 0x1_2345);
        w.put(32, 0xdead_beef);
        w.put(1, 1);
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.read(3).unwrap(), 0b101);
        assert_eq!(r.read(17).unwrap(), 0x1_2345);
        assert_eq!(r.read(32).unwrap(), 0xdead_beef);
        assert_eq!(r.read(1).unwrap(), 1);
        assert!(r.read(8).is_err());
    }

    #[test]
    fn stuffing_rules() {
        // Unaligned: 0 then ones.
        let mut w = BitWriter::new();
        w.put(3, 0b110);
        w.stuff();
        assert_eq!(w.len_bits(), 8);
        let b = w.into_bytes();
        assert_eq!(b, [0b1100_1111]);
        let mut r = BitReader::new(&b);
        r.skip(3).unwrap();
        assert_eq!(r.stuffing_end(), Some(8));
        // Aligned: a whole 0111 1111 byte.
        let mut w = BitWriter::new();
        w.put(8, 0xaa);
        w.stuff();
        assert_eq!(w.into_bytes(), [0xaa, 0x7f]);
    }
}
