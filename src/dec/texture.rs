//! Block texture decoding: the intra DC differential (6.3.8) and the TCOEF
//! run / level events with the three escape modes (7.4.1).

use crate::bits::BitReader;
use crate::error::{Result, invalid};
use crate::tables::{lmax, rmax};
use crate::vlc::{self, ESCAPE};

/// `dct_dc_size` then `dct_dc_differential` (and the marker after a size
/// above 8).
pub(crate) fn read_dc_diff(r: &mut BitReader, luma: bool) -> Result<i32> {
    let size = vlc::dc_size(luma).decode(r)?;
    if size == 0 {
        return Ok(0);
    }
    let code = r.read(size)? as i32;
    let v = if code >> (size - 1) == 0 { code - ((1 << size) - 1) } else { code };
    if size > 8 {
        r.marker("after dct_dc_differential")?;
    }
    Ok(v)
}

#[inline]
fn unpack(v: u32) -> (bool, u32, u32) {
    (v >> 16 != 0, (v >> 8) & 0xff, v & 0xff)
}

/// Reads TCOEF events into `blk` (raster order) through `scan`, from scan
/// position `start`, until the event with `last` set. `intra_table`
/// selects Table B-16 over B-17; `short_header` selects H.263's escape (a
/// fixed-length `last`, `run` and 8-bit `level`) over MPEG-4's three modes.
pub(crate) fn read_coeffs(
    r: &mut BitReader,
    blk: &mut [i16; 64],
    scan: &[u8; 64],
    start: usize,
    intra_table: bool,
    short_header: bool,
) -> Result<()> {
    let table = vlc::tcoef(intra_table);
    let mut i = start;
    loop {
        let v = table.decode(r)?;
        let (last, run, level): (bool, u32, i32) = if v != ESCAPE {
            let (last, run, level) = unpack(v);
            let neg = r.read_bit()?;
            (last, run, if neg { -(level as i32) } else { level as i32 })
        } else if short_header {
            let last = r.read_bit()?;
            let run = r.read(6)?;
            let level = r.read(8)? as u8 as i8 as i32;
            if level == 0 || level == -128 {
                return Err(invalid(format!("escaped level {level} in a short-header block")));
            }
            (last, run, level)
        } else if !r.read_bit()? {
            // Type 1: level offset by LMAX.
            let v = table.decode(r)?;
            if v == ESCAPE {
                return Err(invalid("escape inside an escape"));
            }
            let (last, run, level) = unpack(v);
            let neg = r.read_bit()?;
            let level = (level + lmax(intra_table, last, run as usize)) as i32;
            (last, run, if neg { -level } else { level })
        } else if !r.read_bit()? {
            // Type 2: run offset by RMAX + 1.
            let v = table.decode(r)?;
            if v == ESCAPE {
                return Err(invalid("escape inside an escape"));
            }
            let (last, run, level) = unpack(v);
            let neg = r.read_bit()?;
            let rm = rmax(intra_table, last, level).ok_or_else(|| invalid("RMAX of an uncoded level"))?;
            (last, run + rm + 1, if neg { -(level as i32) } else { level as i32 })
        } else {
            // Type 3: fixed length.
            let last = r.read_bit()?;
            let run = r.read(6)?;
            r.marker("before an escaped level")?;
            let level = ((r.read(12)? << 20) as i32) >> 20;
            r.marker("after an escaped level")?;
            if level == 0 {
                return Err(invalid("escaped level 0"));
            }
            (last, run, level)
        };
        i += run as usize;
        if i >= 64 {
            return Err(invalid("coefficients run past the end of the block"));
        }
        blk[scan[i] as usize] = level as i16;
        i += 1;
        if last {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::BitWriter;
    use crate::tables::{TCOEF_ESCAPE, TCOEF_INTER, TCOEF_INTRA, ZIGZAG, code};

    fn put(w: &mut BitWriter, s: &str) {
        let (b, l) = code(s);
        w.put(l, b);
    }

    /// The code of `(last, run, level)` in a table.
    fn find(t: &[(&'static str, u8, u8, u8)], last: u8, run: u8, level: u8) -> &'static str {
        t.iter().find(|e| (e.1, e.2, e.3) == (last, run, level)).unwrap().0
    }

    fn decode(w: BitWriter, intra: bool, short: bool) -> [i16; 64] {
        let bytes = w.into_bytes();
        let mut b = [0i16; 64];
        read_coeffs(&mut BitReader::new(&bytes), &mut b, &ZIGZAG, 0, intra, short).unwrap();
        b
    }

    /// Escape type 1 (7.4.1.3): ESC, 0, then a table code whose level is
    /// offset by LMAX(last, run) — intra (1, 0, 1) means level 1 + 8.
    #[test]
    fn escape_mode_1() {
        let mut w = BitWriter::new();
        put(&mut w, TCOEF_ESCAPE);
        w.put(1, 0);
        put(&mut w, find(TCOEF_INTRA, 1, 0, 1));
        w.put(1, 1); // negative
        let b = decode(w, true, false);
        assert_eq!(b[0], -9);
        // Inter (1, 1, 1): LMAX(1, 1) = 2, so level 3, after one zero.
        let mut w = BitWriter::new();
        put(&mut w, TCOEF_ESCAPE);
        w.put(1, 0);
        put(&mut w, find(TCOEF_INTER, 1, 1, 1));
        w.put(1, 0);
        let b = decode(w, false, false);
        assert_eq!(b[ZIGZAG[1] as usize], 3);
    }

    /// Escape type 2: ESC, 10, then a code whose run is offset by
    /// RMAX(last, level) + 1 — inter (0, 0, 2) means run 0 + 10 + 1.
    #[test]
    fn escape_mode_2() {
        let mut w = BitWriter::new();
        put(&mut w, TCOEF_ESCAPE);
        w.put(2, 0b10);
        put(&mut w, find(TCOEF_INTER, 1, 0, 2));
        w.put(1, 0);
        let b = decode(w, false, false);
        // RMAX(1, 2) = 1 for inter: run 0 + 1 + 1 = 2.
        assert_eq!(b[ZIGZAG[2] as usize], 2);
        let mut w = BitWriter::new();
        put(&mut w, TCOEF_ESCAPE);
        w.put(2, 0b10);
        put(&mut w, find(TCOEF_INTRA, 1, 3, 1));
        w.put(1, 1);
        let b = decode(w, true, false);
        // RMAX(1, 1) = 20 for intra: run 3 + 21 = 24.
        assert_eq!(b[ZIGZAG[24] as usize], -1);
    }

    /// Escape type 3: ESC, 11, last, 6-bit run, marker, 12-bit two's
    /// complement level, marker.
    #[test]
    fn escape_mode_3() {
        let mut w = BitWriter::new();
        put(&mut w, TCOEF_ESCAPE);
        w.put(2, 0b11);
        w.put(1, 0);
        w.put(6, 5);
        w.put(1, 1);
        w.put(12, (-300i32 as u32) & 0xfff);
        w.put(1, 1);
        put(&mut w, TCOEF_ESCAPE);
        w.put(2, 0b11);
        w.put(1, 1);
        w.put(6, 0);
        w.put(1, 1);
        w.put(12, 2047);
        w.put(1, 1);
        let b = decode(w, false, false);
        assert_eq!(b[ZIGZAG[5] as usize], -300);
        assert_eq!(b[ZIGZAG[6] as usize], 2047);
        // A missing marker is an error.
        let mut w = BitWriter::new();
        put(&mut w, TCOEF_ESCAPE);
        w.put(2, 0b11);
        w.put(8, 0);
        w.put(14, 1 << 1);
        let bytes = w.into_bytes();
        let mut b = [0i16; 64];
        assert!(read_coeffs(&mut BitReader::new(&bytes), &mut b, &ZIGZAG, 0, false, false).is_err());
    }

    /// The short video header's escape (H.263 5.4.2): last, 6-bit run,
    /// 8-bit signed level; 0 and -128 are forbidden.
    #[test]
    fn short_header_escape() {
        let mut w = BitWriter::new();
        put(&mut w, TCOEF_ESCAPE);
        w.put(1, 1);
        w.put(6, 3);
        w.put(8, (-100i32 as u32) & 0xff);
        let b = decode(w, false, true);
        assert_eq!(b[ZIGZAG[3] as usize], -100);
        for bad in [0u32, 0x80] {
            let mut w = BitWriter::new();
            put(&mut w, TCOEF_ESCAPE);
            w.put(1, 1);
            w.put(6, 0);
            w.put(8, bad);
            let bytes = w.into_bytes();
            let mut b = [0i16; 64];
            assert!(read_coeffs(&mut BitReader::new(&bytes), &mut b, &ZIGZAG, 0, false, true).is_err());
        }
    }

    #[test]
    fn run_past_the_block_is_an_error() {
        let mut w = BitWriter::new();
        for _ in 0..3 {
            put(&mut w, find(TCOEF_INTER, 0, 26, 1));
            w.put(1, 0);
        }
        let bytes = w.into_bytes();
        let mut b = [0i16; 64];
        assert!(read_coeffs(&mut BitReader::new(&bytes), &mut b, &ZIGZAG, 0, false, false).is_err());
    }

    /// dct_dc_differential (Table B-15 semantics): a leading 0 bit marks a
    /// negative value, `code - (2^size - 1)`; sizes above 8 end in a marker.
    #[test]
    fn dc_differential() {
        let cases: &[(&str, &str, i32)] = &[
            ("011", "", 0),
            ("11", "1", 1),
            ("11", "0", -1),
            ("10", "10", 2),
            ("10", "00", -3),
            ("0000 0000 1", "0000000000 1", -1023),
        ];
        for &(size, bits, v) in cases {
            let mut w = BitWriter::new();
            put(&mut w, size);
            if !bits.is_empty() {
                put(&mut w, bits);
            }
            let bytes = w.into_bytes();
            assert_eq!(read_dc_diff(&mut BitReader::new(&bytes), true).unwrap(), v, "{size} {bits}");
        }
    }
}
