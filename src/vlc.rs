//! Table-driven VLC decoding and the reverse lookups the encoder writes
//! with, built once from the codeword lists in [`crate::tables`].

use std::sync::OnceLock;

use crate::bits::BitReader;
use crate::error::{Result, invalid};
use crate::tables::{self, code};

/// A lookup table indexed by the next `bits` bits of the stream.
pub(crate) struct Vlc {
    bits: u32,
    /// `(value, length)`; length 0 marks a bit pattern no codeword starts.
    table: Vec<(u32, u8)>,
    name: &'static str,
}

impl Vlc {
    fn new(name: &'static str, codes: impl IntoIterator<Item = (u32, u32, u32)>) -> Self {
        let codes: Vec<_> = codes.into_iter().collect();
        let bits = codes.iter().map(|c| c.1).max().unwrap_or(1);
        let mut table = vec![(0u32, 0u8); 1 << bits];
        for &(c, len, value) in &codes {
            let shift = bits - len;
            let base = (c << shift) as usize;
            for e in &mut table[base..base + (1 << shift)] {
                debug_assert_eq!(e.1, 0, "{name}: overlapping codewords");
                *e = (value, len as u8);
            }
        }
        Vlc { bits, table, name }
    }

    /// Decodes one codeword, returning its value.
    #[inline]
    pub fn decode(&self, r: &mut BitReader) -> Result<u32> {
        let (v, len) = self.table[r.peek(self.bits) as usize];
        if len == 0 {
            return Err(invalid(format!("no {} codeword matches", self.name)));
        }
        r.skip(len as usize)?;
        Ok(v)
    }
}

/// MCBPC value: `mb_type << 2 | cbpc`.
pub(crate) fn mcbpc_i() -> &'static Vlc {
    static T: OnceLock<Vlc> = OnceLock::new();
    T.get_or_init(|| {
        Vlc::new(
            "MCBPC (I)",
            tables::MCBPC_I.iter().map(|&(s, t, c)| {
                let (b, l) = code(s);
                (b, l, (t as u32) << 2 | c as u32)
            }),
        )
    })
}

pub(crate) fn mcbpc_p() -> &'static Vlc {
    static T: OnceLock<Vlc> = OnceLock::new();
    T.get_or_init(|| {
        Vlc::new(
            "MCBPC (P)",
            tables::MCBPC_P.iter().map(|&(s, t, c)| {
                let (b, l) = code(s);
                (b, l, (t as u32) << 2 | c as u32)
            }),
        )
    })
}

/// CBPY, the intra value.
pub(crate) fn cbpy() -> &'static Vlc {
    static T: OnceLock<Vlc> = OnceLock::new();
    T.get_or_init(|| {
        Vlc::new(
            "CBPY",
            tables::CBPY.iter().map(|&(s, v)| {
                let (b, l) = code(s);
                (b, l, v as u32)
            }),
        )
    })
}

/// `motion_code` magnitude.
pub(crate) fn mvd() -> &'static Vlc {
    static T: OnceLock<Vlc> = OnceLock::new();
    T.get_or_init(|| {
        Vlc::new(
            "motion vector",
            tables::MVD.iter().enumerate().map(|(i, s)| {
                let (b, l) = code(s);
                (b, l, i as u32)
            }),
        )
    })
}

pub(crate) fn dc_size(luma: bool) -> &'static Vlc {
    static L: OnceLock<Vlc> = OnceLock::new();
    static C: OnceLock<Vlc> = OnceLock::new();
    let (cell, t, name) = if luma {
        (&L, &tables::DC_SIZE_LUMA, "dct_dc_size_luminance")
    } else {
        (&C, &tables::DC_SIZE_CHROMA, "dct_dc_size_chrominance")
    };
    cell.get_or_init(|| {
        Vlc::new(
            name,
            t.iter().enumerate().map(|(i, s)| {
                let (b, l) = code(s);
                (b, l, i as u32)
            }),
        )
    })
}

pub(crate) fn dmv_length() -> &'static Vlc {
    static T: OnceLock<Vlc> = OnceLock::new();
    T.get_or_init(|| {
        Vlc::new(
            "dmv_length",
            tables::DMV_LENGTH.iter().enumerate().map(|(i, s)| {
                let (b, l) = code(s);
                (b, l, i as u32)
            }),
        )
    })
}

/// The TCOEF escape value.
pub(crate) const ESCAPE: u32 = u32::MAX;

/// TCOEF value: `last << 16 | run << 8 | level`, or [`ESCAPE`].
pub(crate) fn tcoef(intra: bool) -> &'static Vlc {
    static I: OnceLock<Vlc> = OnceLock::new();
    static P: OnceLock<Vlc> = OnceLock::new();
    let (cell, t) = if intra {
        (&I, tables::TCOEF_INTRA)
    } else {
        (&P, tables::TCOEF_INTER)
    };
    cell.get_or_init(|| {
        let esc = code(tables::TCOEF_ESCAPE);
        Vlc::new(
            "TCOEF",
            t.iter()
                .map(|&(s, last, run, level)| {
                    let (b, l) = code(s);
                    (b, l, (last as u32) << 16 | (run as u32) << 8 | level as u32)
                })
                .chain(std::iter::once((esc.0, esc.1, ESCAPE))),
        )
    })
}

/// Encoder lookup for TCOEF: `[last][run][level]` → `(code, length)`
/// without the sign bit, length 0 where the table has no code.
pub(crate) struct TcoefEnc {
    codes: Vec<(u32, u32)>,
}

impl TcoefEnc {
    #[inline]
    pub fn get(&self, last: bool, run: u32, level: u32) -> Option<(u32, u32)> {
        if run >= 64 || level == 0 || level >= 32 {
            return None;
        }
        let c = self.codes[(last as usize * 64 + run as usize) * 32 + level as usize];
        (c.1 != 0).then_some(c)
    }
}

pub(crate) fn tcoef_enc(intra: bool) -> &'static TcoefEnc {
    static I: OnceLock<TcoefEnc> = OnceLock::new();
    static P: OnceLock<TcoefEnc> = OnceLock::new();
    let (cell, t) = if intra {
        (&I, tables::TCOEF_INTRA)
    } else {
        (&P, tables::TCOEF_INTER)
    };
    cell.get_or_init(|| {
        let mut codes = vec![(0, 0); 2 * 64 * 32];
        for &(s, last, run, level) in t {
            codes[(last as usize * 64 + run as usize) * 32 + level as usize] = code(s);
        }
        TcoefEnc { codes }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::BitWriter;

    #[test]
    fn every_tcoef_codeword_decodes_to_its_event() {
        for intra in [false, true] {
            let t = if intra { tables::TCOEF_INTRA } else { tables::TCOEF_INTER };
            for &(s, last, run, level) in t {
                let (b, l) = code(s);
                let mut w = BitWriter::new();
                w.put(l, b);
                let bytes = w.into_bytes();
                let mut r = BitReader::new(&bytes);
                let v = tcoef(intra).decode(&mut r).unwrap();
                assert_eq!(v, (last as u32) << 16 | (run as u32) << 8 | level as u32);
                assert_eq!(r.pos(), l as usize);
                assert_eq!(tcoef_enc(intra).get(last == 1, run as u32, level as u32), Some((b, l)));
            }
        }
    }

    #[test]
    fn mvd_codewords_decode() {
        for (i, s) in tables::MVD.iter().enumerate() {
            let (b, l) = code(s);
            let mut w = BitWriter::new();
            w.put(l, b);
            let bytes = w.into_bytes();
            assert_eq!(mvd().decode(&mut BitReader::new(&bytes)).unwrap(), i as u32);
        }
        // Eleven zeros start no codeword.
        let z = [0u8; 4];
        assert!(mvd().decode(&mut BitReader::new(&z)).is_err());
    }
}
