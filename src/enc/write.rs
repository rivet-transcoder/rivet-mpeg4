//! Writing macroblock-layer syntax: the VLCs of Annex B, the intra DC
//! differential, motion vector differences and TCOEF events with the three
//! escape modes, chosen the way 7.4.1 decodes them.

use crate::bits::BitWriter;
use crate::tables::{self, code, lmax, rmax};
use crate::vlc::{rvlc_enc, tcoef_enc};

/// Writes a codeword given as the standard prints it.
#[inline]
pub(crate) fn put_code(w: &mut BitWriter, s: &str) {
    let (b, l) = code(s);
    w.put(l, b);
}

pub(crate) fn put_mcbpc_i(w: &mut BitWriter, mb_type: u8, cbpc: u8) {
    let e = tables::MCBPC_I
        .iter()
        .find(|e| e.1 == mb_type && e.2 == cbpc)
        .expect("MCBPC (I)");
    put_code(w, e.0);
}

pub(crate) fn put_mcbpc_p(w: &mut BitWriter, mb_type: u8, cbpc: u8) {
    let e = tables::MCBPC_P
        .iter()
        .find(|e| e.1 == mb_type && e.2 == cbpc)
        .expect("MCBPC (P)");
    put_code(w, e.0);
}

/// CBPY of an intra macroblock; an inter one writes `15 - cbpy`.
pub(crate) fn put_cbpy(w: &mut BitWriter, v: u8) {
    put_code(w, tables::CBPY[v as usize].0);
}

/// `dct_dc_size` and `dct_dc_differential` for a DC difference.
pub(crate) fn put_dc_diff(w: &mut BitWriter, diff: i32, luma: bool) {
    let a = diff.unsigned_abs();
    let size = 32 - a.leading_zeros();
    put_code(
        w,
        if luma {
            tables::DC_SIZE_LUMA[size as usize]
        } else {
            tables::DC_SIZE_CHROMA[size as usize]
        },
    );
    if size > 0 {
        let v = if diff >= 0 {
            diff as u32
        } else {
            (diff + (1 << size) - 1) as u32
        };
        w.put(size, v);
        if size > 8 {
            w.put(1, 1);
        }
    }
}

/// A motion vector difference component, already wrapped into
/// `[-32 f, 32 f - 1]`.
pub(crate) fn put_mvd(w: &mut BitWriter, d: i32, fcode: u32) {
    if d == 0 {
        put_code(w, tables::MVD[0]);
        return;
    }
    let rs = fcode - 1;
    let a = d.unsigned_abs();
    let (m, res) = if rs == 0 {
        (a, 0)
    } else {
        (((a - 1) >> rs) + 1, (a - 1) & ((1 << rs) - 1))
    };
    put_code(w, tables::MVD[m as usize]);
    w.put(1, (d < 0) as u32);
    if rs > 0 {
        w.put(rs, res);
    }
}

/// Wraps a vector difference into the range `fcode` codes (the decoder's
/// modular addition undoes it).
pub(crate) fn wrap_diff(d: i32, fcode: u32) -> i32 {
    crate::dec::vop::wrap_mv(d, fcode)
}

/// Writes the coefficients of `levels` (raster order) in `scan` order from
/// position `start` as TCOEF events. Returns false (writing nothing) when
/// there are none.
pub(crate) fn put_coeffs(
    w: &mut BitWriter,
    levels: &[i16; 64],
    scan: &[u8; 64],
    start: usize,
    intra_table: bool,
) -> bool {
    let mut events: Vec<(u32, i32)> = Vec::with_capacity(16);
    let mut run = 0;
    for &z in &scan[start..] {
        let v = levels[z as usize];
        if v == 0 {
            run += 1;
        } else {
            events.push((run, v as i32));
            run = 0;
        }
    }
    if events.is_empty() {
        return false;
    }
    let enc = tcoef_enc(intra_table);
    let n = events.len();
    for (i, &(run, level)) in events.iter().enumerate() {
        let last = i + 1 == n;
        let a = level.unsigned_abs();
        let sign = (level < 0) as u32;
        if let Some((c, l)) = enc.get(last, run, a) {
            w.put(l, c);
            w.put(1, sign);
            continue;
        }
        let lm = lmax(intra_table, last, run as usize);
        if lm > 0
            && a > lm
            && let Some((c, l)) = enc.get(last, run, a - lm)
        {
            put_code(w, tables::TCOEF_ESCAPE);
            w.put(1, 0);
            w.put(l, c);
            w.put(1, sign);
            continue;
        }
        if let Some(rm) = rmax(intra_table, last, a)
            && run > rm
            && let Some((c, l)) = enc.get(last, run - rm - 1, a)
        {
            put_code(w, tables::TCOEF_ESCAPE);
            w.put(2, 0b10);
            w.put(l, c);
            w.put(1, sign);
            continue;
        }
        put_code(w, tables::TCOEF_ESCAPE);
        w.put(2, 0b11);
        w.put(1, last as u32);
        w.put(6, run);
        w.put(1, 1);
        w.put(12, (level as u32) & 0xfff);
        w.put(1, 1);
    }
    true
}

/// One TCOEF event with the reversible table (Table B-23): its code and
/// sign, or the reversible escape `0000 1`, `last`, `run`, marker, 11-bit
/// `|level|`, marker, `0000 s`. Levels beyond 2047 are clipped.
pub(crate) fn put_rvlc_event(w: &mut BitWriter, last: bool, run: u32, level: i32, intra: bool) {
    let a = level.unsigned_abs().min(2047);
    let sign = (level < 0) as u32;
    if let Some((c, l)) = rvlc_enc(intra).get(last, run, a) {
        w.put(l, c);
        w.put(1, sign);
        return;
    }
    w.put(5, 0b00001);
    w.put(1, last as u32);
    w.put(6, run);
    w.put(1, 1);
    w.put(11, a);
    w.put(1, 1);
    w.put(4, 0);
    w.put(1, sign);
}

/// [`put_coeffs`] with the reversible table.
pub(crate) fn put_coeffs_rvlc(
    w: &mut BitWriter,
    levels: &[i16; 64],
    scan: &[u8; 64],
    start: usize,
    intra_table: bool,
) -> bool {
    let mut events: Vec<(u32, i32)> = Vec::with_capacity(16);
    let mut run = 0;
    for &z in &scan[start..] {
        let v = levels[z as usize];
        if v == 0 {
            run += 1;
        } else {
            events.push((run, v as i32));
            run = 0;
        }
    }
    let n = events.len();
    for (i, &(run, level)) in events.iter().enumerate() {
        put_rvlc_event(w, i + 1 == n, run, level, intra_table);
    }
    n > 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::BitReader;
    use crate::dec::texture_for_tests::read_coeffs;
    use crate::tables::{ALT_HORIZONTAL, ZIGZAG};

    /// Every escape mode the writer picks reads back to the same block.
    #[test]
    fn coefficients_round_trip_through_every_escape_mode() {
        let cases: Vec<Vec<(usize, i16)>> = vec![
            vec![(0, 1)],
            vec![(0, 13)], // intra run 0 up to 27 direct; inter 12 then type 1
            vec![(0, 30)], // type 1 in both (27 + 3, 12 + 18: not inter)
            vec![(0, 400), (5, -2047)], // type 3
            vec![(20, 1), (63, 1)], // long runs: type 2
            vec![(3, -3), (40, 2), (50, -1)],
            vec![(1, 28), (2, 11), (9, 4)],
        ];
        for intra in [false, true] {
            for case in &cases {
                for scan in [&ZIGZAG, &ALT_HORIZONTAL] {
                    let mut lv = [0i16; 64];
                    for &(pos, v) in case {
                        lv[scan[pos] as usize] = v;
                    }
                    let mut w = BitWriter::new();
                    assert!(put_coeffs(&mut w, &lv, scan, 0, intra));
                    let n = w.len_bits();
                    let b = w.into_bytes();
                    let mut r = BitReader::new(&b);
                    let mut got = [0i16; 64];
                    read_coeffs(&mut r, &mut got, scan, 0, intra, false).unwrap();
                    assert_eq!(got, lv, "{case:?} intra {intra}");
                    assert_eq!(r.pos(), n);
                }
            }
        }
    }

    #[test]
    fn dc_and_mvd_round_trip() {
        for luma in [true, false] {
            for d in -2047..=2047 {
                let mut w = BitWriter::new();
                put_dc_diff(&mut w, d, luma);
                let b = w.into_bytes();
                let v = crate::dec::texture_for_tests::read_dc_diff(&mut BitReader::new(&b), luma)
                    .unwrap();
                assert_eq!(v, d);
            }
        }
        for fcode in 1..=7u32 {
            let f = 1 << (fcode - 1);
            for d in -32 * f..32 * f {
                let mut w = BitWriter::new();
                put_mvd(&mut w, d, fcode);
                let b = w.into_bytes();
                let v = crate::dec::texture_for_tests::read_mvd(&mut BitReader::new(&b), fcode)
                    .unwrap();
                assert_eq!(v, d, "fcode {fcode}");
            }
        }
    }

    /// Every event of every reversible table, and escaped ones (long runs,
    /// large levels, both signs, last or not), read back the same forwards
    /// and backwards, and the backward reader ends exactly where the
    /// event began.
    #[test]
    fn reversible_events_read_the_same_both_ways() {
        use crate::bits::BackReader;
        use crate::dec::texture_for_tests::{read_rvlc_event, read_rvlc_event_back};
        let mut events = Vec::new();
        for &(_, i, p) in crate::tables::RVLC_TCOEF {
            for e in [i, p] {
                events.push((e.0 == 1, e.1 as u32, e.2 as i32));
                events.push((e.0 == 1, e.1 as u32, -(e.2 as i32)));
            }
        }
        for last in [false, true] {
            for (run, level) in [(0, 28), (0, 2047), (63, 1), (45, -1), (20, -300), (9, 3)] {
                events.push((last, run, level));
            }
        }
        for intra in [true, false] {
            let mut w = BitWriter::new();
            let mut ends = Vec::new();
            for &(last, run, level) in &events {
                put_rvlc_event(&mut w, last, run, level, intra);
                ends.push(w.len_bits());
            }
            let n = w.len_bits();
            let b = w.into_bytes();
            let mut r = BitReader::new(&b);
            let mut got = Vec::new();
            for _ in &events {
                got.push(read_rvlc_event(&mut r, intra).unwrap());
            }
            assert_eq!(got, events, "forwards, intra {intra}");
            assert_eq!(r.pos(), n);
            let mut br = BackReader::new(&b, n, 0);
            let mut back = Vec::new();
            for k in (0..events.len()).rev() {
                assert_eq!(br.pos(), ends[k]);
                back.push(read_rvlc_event_back(&mut br, intra).unwrap());
            }
            back.reverse();
            assert_eq!(back, events, "backwards, intra {intra}");
            assert_eq!(br.pos(), 0);
        }
    }

    /// The escape forms Annex E.1.4.4.1 calls illegal are refused in both
    /// directions: level 0, an event the table codes, a leading `0000 0`, a
    /// missing marker.
    #[test]
    fn illegal_reversible_escapes() {
        use crate::bits::BackReader;
        use crate::dec::texture_for_tests::{read_rvlc_event, read_rvlc_event_back};
        let esc = |lead: u32, last: u32, run: u32, m1: u32, level: u32, m2: u32| {
            let mut w = BitWriter::new();
            w.put(5, lead);
            w.put(1, last);
            w.put(6, run);
            w.put(1, m1);
            w.put(11, level);
            w.put(1, m2);
            w.put(5, 0b00001);
            (w.len_bits(), w.into_bytes())
        };
        // A legal one first: (0, 0, -40) inter.
        let (n, b) = esc(1, 0, 0, 1, 40, 1);
        assert_eq!(
            read_rvlc_event(&mut BitReader::new(&b), false).unwrap(),
            (false, 0, -40)
        );
        assert_eq!(
            read_rvlc_event_back(&mut BackReader::new(&b, n, 0), false).unwrap(),
            (false, 0, -40)
        );
        for (lead, last, run, m1, level, m2) in [
            (1, 0, 0, 1, 0, 1),  // level 0
            (1, 0, 0, 1, 1, 1),  // (0, 0, 1) has a code
            (0, 0, 0, 1, 40, 1), // leading 0000 0
            (1, 0, 0, 0, 40, 1), // marker
            (1, 0, 0, 1, 40, 0), // marker
        ] {
            let (n, b) = esc(lead, last, run, m1, level, m2);
            assert!(read_rvlc_event(&mut BitReader::new(&b), false).is_err());
            assert!(read_rvlc_event_back(&mut BackReader::new(&b, n, 0), false).is_err());
        }
    }
}
