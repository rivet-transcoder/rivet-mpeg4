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
