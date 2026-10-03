//! The normative tables of ISO/IEC 14496-2 Annex B and clause 7, transcribed
//! as data: the variable length codes, the scans, the default quantiser
//! matrices, the escape-mode limits and the DC scaler.
//!
//! Codewords are written as the standard prints them, MSB first, spaces for
//! legibility only. Where a code carries a trailing sign bit (`s` in the
//! standard), the codeword here is the part before it.

/// MCBPC for I-VOPs (Table B-6; H.263 Table 8). Value: `(mb_type, cbpc)`,
/// `mb_type` 3 = intra, 4 = intra+q, [`MB_STUFFING`] for stuffing.
pub(crate) const MCBPC_I: &[(&str, u8, u8)] = &[
    ("1", 3, 0),
    ("001", 3, 1),
    ("010", 3, 2),
    ("011", 3, 3),
    ("0001", 4, 0),
    ("0000 01", 4, 1),
    ("0000 10", 4, 2),
    ("0000 11", 4, 3),
    ("0000 0000 1", MB_STUFFING, 0),
];

/// MCBPC for P-VOPs and S-VOPs (Table B-7; H.263 Table 7). `mb_type` 0 inter,
/// 1 inter+q, 2 inter4v, 3 intra, 4 intra+q.
pub(crate) const MCBPC_P: &[(&str, u8, u8)] = &[
    ("1", 0, 0),
    ("0011", 0, 1),
    ("0010", 0, 2),
    ("0001 01", 0, 3),
    ("011", 1, 0),
    ("0000 111", 1, 1),
    ("0000 110", 1, 2),
    ("0000 0010 1", 1, 3),
    ("010", 2, 0),
    ("0000 101", 2, 1),
    ("0000 100", 2, 2),
    ("0000 0101", 2, 3),
    ("0001 1", 3, 0),
    ("0000 0100", 3, 1),
    ("0000 0011", 3, 2),
    ("0000 011", 3, 3),
    ("0001 00", 4, 0),
    ("0000 0010 0", 4, 1),
    ("0000 0001 1", 4, 2),
    ("0000 0001 0", 4, 3),
    ("0000 0000 1", MB_STUFFING, 0),
];

/// The `mb_type` of a stuffing MCBPC.
pub(crate) const MB_STUFFING: u8 = 255;

/// CBPY (Table B-8; H.263 Table 9), indexed by the intra-macroblock value;
/// an inter macroblock's CBPY is 15 minus it. Bit 3 is block 0.
pub(crate) const CBPY: &[(&str, u8)] = &[
    ("0011", 0),
    ("0010 1", 1),
    ("0010 0", 2),
    ("1001", 3),
    ("0001 1", 4),
    ("0111", 5),
    ("0000 10", 6),
    ("1011", 7),
    ("0001 0", 8),
    ("0000 11", 9),
    ("0101", 10),
    ("1010", 11),
    ("0100", 12),
    ("1000", 13),
    ("0110", 14),
    ("11", 15),
];

/// `motion_code` magnitudes 0..=32 (Table B-12; H.263 Table 14). Every
/// nonzero magnitude is followed by a sign bit, `1` meaning negative.
pub(crate) const MVD: [&str; 33] = [
    "1",
    "01",
    "001",
    "0001",
    "0000 11",
    "0000 101",
    "0000 100",
    "0000 011",
    "0000 0101 1",
    "0000 0101 0",
    "0000 0100 1",
    "0000 0100 01",
    "0000 0100 00",
    "0000 0011 11",
    "0000 0011 10",
    "0000 0011 01",
    "0000 0011 00",
    "0000 0010 11",
    "0000 0010 10",
    "0000 0010 01",
    "0000 0010 00",
    "0000 0001 11",
    "0000 0001 10",
    "0000 0001 01",
    "0000 0001 00",
    "0000 0000 111",
    "0000 0000 110",
    "0000 0000 101",
    "0000 0000 100",
    "0000 0000 011",
    "0000 0000 010",
    "0000 0000 0011",
    "0000 0000 0010",
];

/// `dct_dc_size_luminance` 0..=12 (Table B-13).
pub(crate) const DC_SIZE_LUMA: [&str; 13] = [
    "011",
    "11",
    "10",
    "010",
    "001",
    "0001",
    "0000 1",
    "0000 01",
    "0000 001",
    "0000 0001",
    "0000 0000 1",
    "0000 0000 01",
    "0000 0000 001",
];

/// `dct_dc_size_chrominance` 0..=12 (Table B-14).
pub(crate) const DC_SIZE_CHROMA: [&str; 13] = [
    "11",
    "10",
    "01",
    "001",
    "0001",
    "0000 1",
    "0000 01",
    "0000 001",
    "0000 0001",
    "0000 0000 1",
    "0000 0000 01",
    "0000 0000 001",
    "0000 0000 0001",
];

/// The TCOEF escape code (both tables).
pub(crate) const TCOEF_ESCAPE: &str = "0000 011";

/// Inter TCOEF (Table B-17; H.263 Table 16): `(code, last, run, |level|)`,
/// sign bit after the code. Also the table the short video header uses for
/// intra blocks.
pub(crate) const TCOEF_INTER: &[(&str, u8, u8, u8)] = &[
    ("10", 0, 0, 1),
    ("1111", 0, 0, 2),
    ("0101 01", 0, 0, 3),
    ("0010 111", 0, 0, 4),
    ("0001 1111", 0, 0, 5),
    ("0001 0010 1", 0, 0, 6),
    ("0001 0010 0", 0, 0, 7),
    ("0000 1000 01", 0, 0, 8),
    ("0000 1000 00", 0, 0, 9),
    ("0000 0000 111", 0, 0, 10),
    ("0000 0000 110", 0, 0, 11),
    ("0000 0100 000", 0, 0, 12),
    ("110", 0, 1, 1),
    ("0101 00", 0, 1, 2),
    ("0001 1110", 0, 1, 3),
    ("0000 0011 11", 0, 1, 4),
    ("0000 0100 001", 0, 1, 5),
    ("0000 0101 0000", 0, 1, 6),
    ("1110", 0, 2, 1),
    ("0001 1101", 0, 2, 2),
    ("0000 0011 10", 0, 2, 3),
    ("0000 0101 0001", 0, 2, 4),
    ("0110 1", 0, 3, 1),
    ("0001 0001 1", 0, 3, 2),
    ("0000 0011 01", 0, 3, 3),
    ("0110 0", 0, 4, 1),
    ("0001 0001 0", 0, 4, 2),
    ("0000 0101 0010", 0, 4, 3),
    ("0101 1", 0, 5, 1),
    ("0000 0011 00", 0, 5, 2),
    ("0000 0101 0011", 0, 5, 3),
    ("0100 11", 0, 6, 1),
    ("0000 0010 11", 0, 6, 2),
    ("0000 0101 0100", 0, 6, 3),
    ("0100 10", 0, 7, 1),
    ("0000 0010 10", 0, 7, 2),
    ("0100 01", 0, 8, 1),
    ("0000 0010 01", 0, 8, 2),
    ("0100 00", 0, 9, 1),
    ("0000 0010 00", 0, 9, 2),
    ("0010 110", 0, 10, 1),
    ("0000 0101 0101", 0, 10, 2),
    ("0010 101", 0, 11, 1),
    ("0010 100", 0, 12, 1),
    ("0001 1100", 0, 13, 1),
    ("0001 1011", 0, 14, 1),
    ("0001 0000 1", 0, 15, 1),
    ("0001 0000 0", 0, 16, 1),
    ("0000 1111 1", 0, 17, 1),
    ("0000 1111 0", 0, 18, 1),
    ("0000 1110 1", 0, 19, 1),
    ("0000 1110 0", 0, 20, 1),
    ("0000 1101 1", 0, 21, 1),
    ("0000 1101 0", 0, 22, 1),
    ("0000 0100 010", 0, 23, 1),
    ("0000 0100 011", 0, 24, 1),
    ("0000 0101 0110", 0, 25, 1),
    ("0000 0101 0111", 0, 26, 1),
    ("0111", 1, 0, 1),
    ("0000 1100 1", 1, 0, 2),
    ("0000 0000 101", 1, 0, 3),
    ("0011 11", 1, 1, 1),
    ("0000 0000 100", 1, 1, 2),
    ("0011 10", 1, 2, 1),
    ("0011 01", 1, 3, 1),
    ("0011 00", 1, 4, 1),
    ("0010 011", 1, 5, 1),
    ("0010 010", 1, 6, 1),
    ("0010 001", 1, 7, 1),
    ("0010 000", 1, 8, 1),
    ("0001 1010", 1, 9, 1),
    ("0001 1001", 1, 10, 1),
    ("0001 1000", 1, 11, 1),
    ("0001 0111", 1, 12, 1),
    ("0001 0110", 1, 13, 1),
    ("0001 0101", 1, 14, 1),
    ("0001 0100", 1, 15, 1),
    ("0001 0011", 1, 16, 1),
    ("0000 1100 0", 1, 17, 1),
    ("0000 1011 1", 1, 18, 1),
    ("0000 1011 0", 1, 19, 1),
    ("0000 1010 1", 1, 20, 1),
    ("0000 1010 0", 1, 21, 1),
    ("0000 1001 1", 1, 22, 1),
    ("0000 1001 0", 1, 23, 1),
    ("0000 1000 1", 1, 24, 1),
    ("0000 0001 11", 1, 25, 1),
    ("0000 0001 10", 1, 26, 1),
    ("0000 0001 01", 1, 27, 1),
    ("0000 0001 00", 1, 28, 1),
    ("0000 0100 100", 1, 29, 1),
    ("0000 0100 101", 1, 30, 1),
    ("0000 0100 110", 1, 31, 1),
    ("0000 0100 111", 1, 32, 1),
    ("0000 0101 1000", 1, 33, 1),
    ("0000 0101 1001", 1, 34, 1),
    ("0000 0101 1010", 1, 35, 1),
    ("0000 0101 1011", 1, 36, 1),
    ("0000 0101 1100", 1, 37, 1),
    ("0000 0101 1101", 1, 38, 1),
    ("0000 0101 1110", 1, 39, 1),
    ("0000 0101 1111", 1, 40, 1),
];

/// Intra TCOEF (Table B-16): `(code, last, run, |level|)`, sign bit after
/// the code. The same 102 codewords as [`TCOEF_INTER`], assigned to the
/// statistics of intra blocks.
pub(crate) const TCOEF_INTRA: &[(&str, u8, u8, u8)] = &[
    ("10", 0, 0, 1),
    ("110", 0, 0, 2),
    ("1111", 0, 0, 3),
    ("0110 1", 0, 0, 4),
    ("0110 0", 0, 0, 5),
    ("0101 01", 0, 0, 6),
    ("0100 11", 0, 0, 7),
    ("0100 10", 0, 0, 8),
    ("0010 111", 0, 0, 9),
    ("0001 1111", 0, 0, 10),
    ("0001 1110", 0, 0, 11),
    ("0001 1101", 0, 0, 12),
    ("0001 0010 1", 0, 0, 13),
    ("0001 0010 0", 0, 0, 14),
    ("0001 0001 1", 0, 0, 15),
    ("0001 0000 1", 0, 0, 16),
    ("0000 1000 01", 0, 0, 17),
    ("0000 1000 00", 0, 0, 18),
    ("0000 0011 11", 0, 0, 19),
    ("0000 0011 10", 0, 0, 20),
    ("0000 0000 111", 0, 0, 21),
    ("0000 0000 110", 0, 0, 22),
    ("0000 0100 000", 0, 0, 23),
    ("0000 0100 001", 0, 0, 24),
    ("0000 0101 0000", 0, 0, 25),
    ("0000 0101 0001", 0, 0, 26),
    ("0000 0101 0010", 0, 0, 27),
    ("1110", 0, 1, 1),
    ("0101 00", 0, 1, 2),
    ("0010 110", 0, 1, 3),
    ("0001 1100", 0, 1, 4),
    ("0001 0000 0", 0, 1, 5),
    ("0000 1111 1", 0, 1, 6),
    ("0000 0011 01", 0, 1, 7),
    ("0000 0100 010", 0, 1, 8),
    ("0000 0101 0011", 0, 1, 9),
    ("0000 0101 0101", 0, 1, 10),
    ("0101 1", 0, 2, 1),
    ("0010 101", 0, 2, 2),
    ("0000 1111 0", 0, 2, 3),
    ("0000 0011 00", 0, 2, 4),
    ("0000 0101 0110", 0, 2, 5),
    ("0100 01", 0, 3, 1),
    ("0001 1011", 0, 3, 2),
    ("0000 1110 1", 0, 3, 3),
    ("0000 0010 11", 0, 3, 4),
    ("0100 00", 0, 4, 1),
    ("0001 0001 0", 0, 4, 2),
    ("0000 0010 10", 0, 4, 3),
    ("0011 01", 0, 5, 1),
    ("0000 1110 0", 0, 5, 2),
    ("0000 0010 00", 0, 5, 3),
    ("0010 010", 0, 6, 1),
    ("0000 1101 1", 0, 6, 2),
    ("0000 0101 0100", 0, 6, 3),
    ("0010 100", 0, 7, 1),
    ("0000 1101 0", 0, 7, 2),
    ("0000 0101 0111", 0, 7, 3),
    ("0001 1001", 0, 8, 1),
    ("0000 0010 01", 0, 8, 2),
    ("0001 1000", 0, 9, 1),
    ("0000 0100 011", 0, 9, 2),
    ("0001 0111", 0, 10, 1),
    ("0000 1100 1", 0, 11, 1),
    ("0000 1100 0", 0, 12, 1),
    ("0000 0001 11", 0, 13, 1),
    ("0000 0101 1000", 0, 14, 1),
    ("0111", 1, 0, 1),
    ("0011 00", 1, 0, 2),
    ("0001 0110", 1, 0, 3),
    ("0000 1011 1", 1, 0, 4),
    ("0000 0001 10", 1, 0, 5),
    ("0000 0000 101", 1, 0, 6),
    ("0000 0000 100", 1, 0, 7),
    ("0000 0101 1001", 1, 0, 8),
    ("0011 11", 1, 1, 1),
    ("0000 1011 0", 1, 1, 2),
    ("0000 0001 01", 1, 1, 3),
    ("0011 10", 1, 2, 1),
    ("0000 0001 00", 1, 2, 2),
    ("0010 001", 1, 3, 1),
    ("0000 0100 100", 1, 3, 2),
    ("0010 000", 1, 4, 1),
    ("0000 0100 101", 1, 4, 2),
    ("0010 011", 1, 5, 1),
    ("0000 0101 1010", 1, 5, 2),
    ("0001 0101", 1, 6, 1),
    ("0000 0101 1011", 1, 6, 2),
    ("0001 0100", 1, 7, 1),
    ("0001 0011", 1, 8, 1),
    ("0001 1010", 1, 9, 1),
    ("0000 1010 1", 1, 10, 1),
    ("0000 1010 0", 1, 11, 1),
    ("0000 1001 1", 1, 12, 1),
    ("0000 1001 0", 1, 13, 1),
    ("0000 1000 1", 1, 14, 1),
    ("0000 0100 110", 1, 15, 1),
    ("0000 0100 111", 1, 16, 1),
    ("0000 0101 1100", 1, 17, 1),
    ("0000 0101 1101", 1, 18, 1),
    ("0000 0101 1110", 1, 19, 1),
    ("0000 0101 1111", 1, 20, 1),
];

/// LMAX for escape mode 1 (Tables B-19 / B-20 of 14496-2): the largest
/// `|level|` the table codes directly, by `[last][run]`; runs beyond the
/// arrays have no codes.
pub(crate) const LMAX_INTRA: [&[u8]; 2] = [
    &[27, 10, 5, 4, 3, 3, 3, 3, 2, 2, 1, 1, 1, 1, 1],
    &[
        8, 3, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    ],
];
pub(crate) const LMAX_INTER: [&[u8]; 2] = [
    &[
        12, 6, 4, 3, 3, 3, 3, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    ],
    &[
        3, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    ],
];

/// LMAX(last, run): 0 where the table has no code for that run.
#[inline]
pub(crate) fn lmax(intra: bool, last: bool, run: usize) -> u32 {
    let t = if intra { LMAX_INTRA } else { LMAX_INTER }[last as usize];
    t.get(run).copied().unwrap_or(0) as u32
}

/// RMAX(last, level) for escape mode 2 (Tables B-21 / B-22): the longest
/// run the table codes directly with that `|level|`; `None` when it codes
/// no run at all for it.
pub(crate) fn rmax(intra: bool, last: bool, level: u32) -> Option<u32> {
    let t = if intra { LMAX_INTRA } else { LMAX_INTER }[last as usize];
    // The longest run whose LMAX reaches `level`: LMAX does not increase
    // with run, so that is the last index with lmax >= level.
    t.iter()
        .rposition(|&l| l as u32 >= level && level >= 1)
        .map(|r| r as u32)
}

/// Zigzag scan (Figure 7-2): `ZIGZAG[i]` is the raster index of the i-th
/// coefficient in scan order.
pub(crate) const ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// Alternate-horizontal scan (Figure 7-3), used for intra blocks with AC
/// prediction from the block above.
pub(crate) const ALT_HORIZONTAL: [u8; 64] = [
    0, 1, 2, 3, 8, 9, 16, 17, 10, 11, 4, 5, 6, 7, 15, 14, 13, 12, 19, 18, 24, 25, 32, 33, 26, 27,
    20, 21, 22, 23, 28, 29, 30, 31, 34, 35, 40, 41, 48, 49, 42, 43, 36, 37, 38, 39, 44, 45, 46, 47,
    50, 51, 56, 57, 58, 59, 52, 53, 54, 55, 60, 61, 62, 63,
];

/// Alternate-vertical scan (Figure 7-4), used for intra blocks with AC
/// prediction from the block to the left (and for interlaced material).
pub(crate) const ALT_VERTICAL: [u8; 64] = [
    0, 8, 16, 24, 1, 9, 2, 10, 17, 25, 32, 40, 48, 56, 57, 49, 41, 33, 26, 18, 3, 11, 4, 12, 19,
    27, 34, 42, 50, 58, 35, 43, 51, 59, 20, 28, 5, 13, 6, 14, 21, 29, 36, 44, 52, 60, 37, 45, 53,
    61, 22, 30, 7, 15, 23, 31, 38, 46, 54, 62, 39, 47, 55, 63,
];

/// Default intra quantiser matrix of 14496-2 (clause 6.3.3), raster order.
pub(crate) const DEFAULT_INTRA_MATRIX: [u8; 64] = [
    8, 17, 18, 19, 21, 23, 25, 27, //
    17, 18, 19, 21, 23, 25, 27, 28, //
    20, 21, 22, 23, 24, 26, 28, 30, //
    21, 22, 23, 24, 26, 28, 30, 32, //
    22, 23, 24, 26, 28, 30, 32, 35, //
    23, 24, 26, 28, 30, 32, 35, 38, //
    25, 26, 28, 30, 32, 35, 38, 41, //
    27, 28, 30, 32, 35, 38, 41, 45,
];

/// Default non-intra quantiser matrix of 14496-2, raster order.
pub(crate) const DEFAULT_INTER_MATRIX: [u8; 64] = [
    16, 17, 18, 19, 20, 21, 22, 23, //
    17, 18, 19, 20, 21, 22, 23, 24, //
    18, 19, 20, 21, 22, 23, 24, 25, //
    19, 20, 21, 22, 23, 24, 26, 27, //
    20, 21, 22, 23, 25, 26, 27, 28, //
    21, 22, 23, 24, 26, 27, 28, 30, //
    22, 23, 24, 26, 27, 28, 30, 31, //
    23, 24, 25, 27, 28, 30, 31, 33,
];

/// The DC scaler of Table 7-1 for 8-bit video: luminance blocks 0–3,
/// chrominance blocks 4–5.
#[inline]
pub(crate) fn dc_scaler(qp: u32, luma: bool) -> u32 {
    if luma {
        match qp {
            0..=4 => 8,
            5..=8 => 2 * qp,
            9..=24 => qp + 8,
            _ => 2 * qp - 16,
        }
    } else {
        match qp {
            0..=4 => 8,
            5..=24 => (qp + 13) / 2,
            _ => qp - 6,
        }
    }
}

/// Rounding table of Table 7-7 for the chrominance vector of a 4MV
/// macroblock: the sixteenths of the summed luminance vectors, rounded to a
/// half sample.
pub(crate) const CHROMA_ROUND_16: [i32; 16] = [0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2];

/// `dmv_length` codes for sprite / GMC warping vectors (Table B-33), 0..=14.
pub(crate) const DMV_LENGTH: [&str; 15] = [
    "00",
    "010",
    "011",
    "100",
    "101",
    "110",
    "1110",
    "1111 0",
    "1111 10",
    "1111 110",
    "1111 1110",
    "1111 1111 0",
    "1111 1111 10",
    "1111 1111 110",
    "1111 1111 1110",
];

/// Reversible TCOEF codes (Table B-23), used for the texture of I-, P- and
/// S-VOPs when the VOL sets `reversible_vlc` (with data partitioning):
/// `(code, intra (last, run, |level|), inter (last, run, |level|))`, the
/// sign bit after the code. Index order is the standard's, which is also
/// ascending code order within each length.
///
/// The codes are built so that they can be read in either direction: each
/// is a *core* followed by one free bit. A core that begins with `1` ends
/// at its second `1` (`1 0^k 1`, a palindrome); one that begins with `0`
/// ends at its third `0` (`0 1^a 0 1^b 0`). Neither rule depends on the
/// direction of reading, so a reader going backwards — sign, free bit, then
/// the core from its far end — finds the same boundaries.
pub(crate) const RVLC_TCOEF: &[(&str, (u8, u8, u8), (u8, u8, u8))] = &[
    ("110", (0, 0, 1), (0, 0, 1)),
    ("111", (0, 0, 2), (0, 1, 1)),
    ("0001", (0, 1, 1), (0, 0, 2)),
    ("1010", (0, 0, 3), (0, 2, 1)),
    ("1011", (1, 0, 1), (1, 0, 1)),
    ("00100", (0, 2, 1), (0, 0, 3)),
    ("00101", (0, 3, 1), (0, 3, 1)),
    ("01000", (0, 1, 2), (0, 4, 1)),
    ("01001", (0, 0, 4), (0, 5, 1)),
    ("10010", (1, 1, 1), (1, 1, 1)),
    ("10011", (1, 2, 1), (1, 2, 1)),
    ("001100", (0, 4, 1), (0, 1, 2)),
    ("001101", (0, 5, 1), (0, 6, 1)),
    ("010100", (0, 0, 5), (0, 7, 1)),
    ("010101", (0, 0, 6), (0, 8, 1)),
    ("011000", (1, 3, 1), (1, 3, 1)),
    ("011001", (1, 4, 1), (1, 4, 1)),
    ("100010", (1, 5, 1), (1, 5, 1)),
    ("100011", (1, 6, 1), (1, 6, 1)),
    ("0011100", (0, 6, 1), (0, 0, 4)),
    ("0011101", (0, 7, 1), (0, 2, 2)),
    ("0101100", (0, 2, 2), (0, 9, 1)),
    ("0101101", (0, 1, 3), (0, 10, 1)),
    ("0110100", (0, 0, 7), (0, 11, 1)),
    ("0110101", (1, 7, 1), (1, 7, 1)),
    ("0111000", (1, 8, 1), (1, 8, 1)),
    ("0111001", (1, 9, 1), (1, 9, 1)),
    ("1000010", (1, 10, 1), (1, 10, 1)),
    ("1000011", (1, 11, 1), (1, 11, 1)),
    ("00111100", (0, 8, 1), (0, 0, 5)),
    ("00111101", (0, 9, 1), (0, 0, 6)),
    ("01011100", (0, 3, 2), (0, 1, 3)),
    ("01011101", (0, 4, 2), (0, 3, 2)),
    ("01101100", (0, 1, 4), (0, 4, 2)),
    ("01101101", (0, 1, 5), (0, 12, 1)),
    ("01110100", (0, 0, 8), (0, 13, 1)),
    ("01110101", (0, 0, 9), (0, 14, 1)),
    ("01111000", (1, 0, 2), (1, 0, 2)),
    ("01111001", (1, 12, 1), (1, 12, 1)),
    ("10000010", (1, 13, 1), (1, 13, 1)),
    ("10000011", (1, 14, 1), (1, 14, 1)),
    ("001111100", (0, 10, 1), (0, 0, 7)),
    ("001111101", (0, 5, 2), (0, 1, 4)),
    ("010111100", (0, 2, 3), (0, 2, 3)),
    ("010111101", (0, 3, 3), (0, 5, 2)),
    ("011011100", (0, 1, 6), (0, 15, 1)),
    ("011011101", (0, 0, 10), (0, 16, 1)),
    ("011101100", (0, 0, 11), (0, 17, 1)),
    ("011101101", (1, 1, 2), (1, 1, 2)),
    ("011110100", (1, 15, 1), (1, 15, 1)),
    ("011110101", (1, 16, 1), (1, 16, 1)),
    ("011111000", (1, 17, 1), (1, 17, 1)),
    ("011111001", (1, 18, 1), (1, 18, 1)),
    ("100000010", (1, 19, 1), (1, 19, 1)),
    ("100000011", (1, 20, 1), (1, 20, 1)),
    ("0011111100", (0, 11, 1), (0, 0, 8)),
    ("0011111101", (0, 12, 1), (0, 0, 9)),
    ("0101111100", (0, 6, 2), (0, 1, 5)),
    ("0101111101", (0, 7, 2), (0, 3, 3)),
    ("0110111100", (0, 8, 2), (0, 6, 2)),
    ("0110111101", (0, 4, 3), (0, 7, 2)),
    ("0111011100", (0, 2, 4), (0, 8, 2)),
    ("0111011101", (0, 1, 7), (0, 9, 2)),
    ("0111101100", (0, 0, 12), (0, 18, 1)),
    ("0111101101", (0, 0, 13), (0, 19, 1)),
    ("0111110100", (0, 0, 14), (0, 20, 1)),
    ("0111110101", (1, 21, 1), (1, 21, 1)),
    ("0111111000", (1, 22, 1), (1, 22, 1)),
    ("0111111001", (1, 23, 1), (1, 23, 1)),
    ("1000000010", (1, 24, 1), (1, 24, 1)),
    ("1000000011", (1, 25, 1), (1, 25, 1)),
    ("00111111100", (0, 13, 1), (0, 0, 10)),
    ("00111111101", (0, 9, 2), (0, 0, 11)),
    ("01011111100", (0, 5, 3), (0, 1, 6)),
    ("01011111101", (0, 6, 3), (0, 2, 4)),
    ("01101111100", (0, 7, 3), (0, 4, 3)),
    ("01101111101", (0, 3, 4), (0, 5, 3)),
    ("01110111100", (0, 2, 5), (0, 10, 2)),
    ("01110111101", (0, 2, 6), (0, 21, 1)),
    ("01111011100", (0, 1, 8), (0, 22, 1)),
    ("01111011101", (0, 1, 9), (0, 23, 1)),
    ("01111101100", (0, 0, 15), (0, 24, 1)),
    ("01111101101", (0, 0, 16), (0, 25, 1)),
    ("01111110100", (0, 0, 17), (0, 26, 1)),
    ("01111110101", (1, 0, 3), (1, 0, 3)),
    ("01111111000", (1, 2, 2), (1, 2, 2)),
    ("01111111001", (1, 26, 1), (1, 26, 1)),
    ("10000000010", (1, 27, 1), (1, 27, 1)),
    ("10000000011", (1, 28, 1), (1, 28, 1)),
    ("001111111100", (0, 10, 2), (0, 0, 12)),
    ("001111111101", (0, 4, 4), (0, 1, 7)),
    ("010111111100", (0, 5, 4), (0, 2, 5)),
    ("010111111101", (0, 6, 4), (0, 3, 4)),
    ("011011111100", (0, 3, 5), (0, 6, 3)),
    ("011011111101", (0, 4, 5), (0, 7, 3)),
    ("011101111100", (0, 1, 10), (0, 11, 2)),
    ("011101111101", (0, 0, 18), (0, 27, 1)),
    ("011110111100", (0, 0, 19), (0, 28, 1)),
    ("011110111101", (0, 0, 22), (0, 29, 1)),
    ("011111011100", (1, 1, 3), (1, 1, 3)),
    ("011111011101", (1, 3, 2), (1, 3, 2)),
    ("011111101100", (1, 4, 2), (1, 4, 2)),
    ("011111101101", (1, 29, 1), (1, 29, 1)),
    ("011111110100", (1, 30, 1), (1, 30, 1)),
    ("011111110101", (1, 31, 1), (1, 31, 1)),
    ("011111111000", (1, 32, 1), (1, 32, 1)),
    ("011111111001", (1, 33, 1), (1, 33, 1)),
    ("100000000010", (1, 34, 1), (1, 34, 1)),
    ("100000000011", (1, 35, 1), (1, 35, 1)),
    ("0011111111100", (0, 14, 1), (0, 0, 13)),
    ("0011111111101", (0, 15, 1), (0, 0, 14)),
    ("0101111111100", (0, 11, 2), (0, 0, 15)),
    ("0101111111101", (0, 8, 3), (0, 0, 16)),
    ("0110111111100", (0, 9, 3), (0, 1, 8)),
    ("0110111111101", (0, 7, 4), (0, 3, 5)),
    ("0111011111100", (0, 3, 6), (0, 4, 4)),
    ("0111011111101", (0, 2, 7), (0, 5, 4)),
    ("0111101111100", (0, 2, 8), (0, 8, 3)),
    ("0111101111101", (0, 2, 9), (0, 12, 2)),
    ("0111110111100", (0, 1, 11), (0, 30, 1)),
    ("0111110111101", (0, 0, 20), (0, 31, 1)),
    ("0111111011100", (0, 0, 21), (0, 32, 1)),
    ("0111111011101", (0, 0, 23), (0, 33, 1)),
    ("0111111101100", (1, 0, 4), (1, 0, 4)),
    ("0111111101101", (1, 5, 2), (1, 5, 2)),
    ("0111111110100", (1, 6, 2), (1, 6, 2)),
    ("0111111110101", (1, 7, 2), (1, 7, 2)),
    ("0111111111000", (1, 8, 2), (1, 8, 2)),
    ("0111111111001", (1, 9, 2), (1, 9, 2)),
    ("1000000000010", (1, 36, 1), (1, 36, 1)),
    ("1000000000011", (1, 37, 1), (1, 37, 1)),
    ("00111111111100", (0, 16, 1), (0, 0, 17)),
    ("00111111111101", (0, 17, 1), (0, 0, 18)),
    ("01011111111100", (0, 18, 1), (0, 1, 9)),
    ("01011111111101", (0, 8, 4), (0, 1, 10)),
    ("01101111111100", (0, 5, 5), (0, 2, 6)),
    ("01101111111101", (0, 4, 6), (0, 2, 7)),
    ("01110111111100", (0, 5, 6), (0, 3, 6)),
    ("01110111111101", (0, 3, 7), (0, 6, 4)),
    ("01111011111100", (0, 3, 8), (0, 9, 3)),
    ("01111011111101", (0, 2, 10), (0, 13, 2)),
    ("01111101111100", (0, 2, 11), (0, 14, 2)),
    ("01111101111101", (0, 1, 12), (0, 15, 2)),
    ("01111110111100", (0, 1, 13), (0, 16, 2)),
    ("01111110111101", (0, 0, 24), (0, 34, 1)),
    ("01111111011100", (0, 0, 25), (0, 35, 1)),
    ("01111111011101", (0, 0, 26), (0, 36, 1)),
    ("01111111101100", (1, 0, 5), (1, 0, 5)),
    ("01111111101101", (1, 1, 4), (1, 1, 4)),
    ("01111111110100", (1, 10, 2), (1, 10, 2)),
    ("01111111110101", (1, 11, 2), (1, 11, 2)),
    ("01111111111000", (1, 12, 2), (1, 12, 2)),
    ("01111111111001", (1, 38, 1), (1, 38, 1)),
    ("10000000000010", (1, 39, 1), (1, 39, 1)),
    ("10000000000011", (1, 40, 1), (1, 40, 1)),
    ("001111111111100", (0, 0, 27), (0, 0, 19)),
    ("001111111111101", (0, 3, 9), (0, 3, 7)),
    ("010111111111100", (0, 6, 5), (0, 4, 5)),
    ("010111111111101", (0, 7, 5), (0, 7, 4)),
    ("011011111111100", (0, 9, 4), (0, 17, 2)),
    ("011011111111101", (0, 12, 2), (0, 37, 1)),
    ("011101111111100", (0, 19, 1), (0, 38, 1)),
    ("011101111111101", (1, 1, 5), (1, 1, 5)),
    ("011110111111100", (1, 2, 3), (1, 2, 3)),
    ("011110111111101", (1, 13, 2), (1, 13, 2)),
    ("011111011111100", (1, 41, 1), (1, 41, 1)),
    ("011111011111101", (1, 42, 1), (1, 42, 1)),
    ("011111101111100", (1, 43, 1), (1, 43, 1)),
    ("011111101111101", (1, 44, 1), (1, 44, 1)),
];

/// The reversible escape's code (the core `000` with free bit `0`), the
/// fifth bit after it being the sign position of Table B-23's `0000s`. An
/// escaped event is `0000 1`, `last`, a 6-bit `run`, a marker, an 11-bit
/// `|level|` (1..=2047), a marker and `0000 s`: the leading code always ends
/// in `1`, the trailing one carries the sign.
pub(crate) const RVLC_ESCAPE: &str = "0000";

/// Parses a codeword string into `(bits, length)`.
pub(crate) fn code(s: &str) -> (u32, u32) {
    let mut v = 0u32;
    let mut n = 0u32;
    for c in s.bytes() {
        match c {
            b'0' | b'1' => {
                v = (v << 1) | (c - b'0') as u32;
                n += 1;
            }
            b' ' => {}
            _ => panic!("bad codeword {s}"),
        }
    }
    (v, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checks a code is prefix-free and returns its Kraft sum (lengths
    /// include the sign bit where `signed` says so).
    fn kraft(codes: &[(u32, u32)]) -> f64 {
        for (i, &(a, la)) in codes.iter().enumerate() {
            for (j, &(b, lb)) in codes.iter().enumerate() {
                if i == j {
                    continue;
                }
                let l = la.min(lb);
                assert!(
                    (a >> (la - l)) != (b >> (lb - l)),
                    "codeword {a:0la$b} is a prefix of {b:0lb$b}",
                    la = la as usize,
                    lb = lb as usize
                );
            }
        }
        codes.iter().map(|&(_, l)| 0.5f64.powi(l as i32)).sum()
    }

    #[test]
    fn mcbpc_and_cbpy_are_prefix_codes() {
        let i: Vec<_> = MCBPC_I.iter().map(|e| code(e.0)).collect();
        // 0000 00xx is unused but for stuffing (start-code emulation).
        assert_eq!(kraft(&i), 1.0 - 7.0 / 512.0);
        let p: Vec<_> = MCBPC_P.iter().map(|e| code(e.0)).collect();
        // H.263's INTER4V+Q codes (0000 0000 01xx) are not MPEG-4 syntax.
        assert_eq!(kraft(&p), 1.0 - 1.0 / 512.0);
        let c: Vec<_> = CBPY.iter().map(|e| code(e.0)).collect();
        assert_eq!(kraft(&c), 1.0 - 1.0 / 32.0);
        for (k, e) in CBPY.iter().enumerate() {
            assert_eq!(e.1 as usize, k);
        }
    }

    #[test]
    fn mvd_is_a_prefix_code_leaving_only_eleven_zeros() {
        let mut v = vec![code(MVD[0])];
        for c in &MVD[1..] {
            let (b, l) = code(c);
            v.push((b << 1, l + 1));
            v.push((b << 1 | 1, l + 1));
        }
        assert_eq!(v.len(), 65);
        assert_eq!(kraft(&v), 1.0 - 1.0 / 2048.0);
    }

    #[test]
    fn dc_size_codes() {
        let l: Vec<_> = DC_SIZE_LUMA.iter().map(|c| code(c)).collect();
        assert_eq!(kraft(&l), 1.0 - 1.0 / 2048.0);
        let c: Vec<_> = DC_SIZE_CHROMA.iter().map(|c| code(c)).collect();
        assert_eq!(kraft(&c), 1.0 - 1.0 / 4096.0);
    }

    fn tcoef_codes(t: &[(&str, u8, u8, u8)]) -> Vec<(u32, u32)> {
        let mut v: Vec<_> = t
            .iter()
            .flat_map(|e| {
                let (b, l) = code(e.0);
                [(b << 1, l + 1), (b << 1 | 1, l + 1)]
            })
            .collect();
        v.push(code(TCOEF_ESCAPE));
        v
    }

    #[test]
    fn tcoef_tables_are_prefix_codes_over_the_same_codewords() {
        assert_eq!(TCOEF_INTER.len(), 102);
        assert_eq!(TCOEF_INTRA.len(), 102);
        let ki = kraft(&tcoef_codes(TCOEF_INTER));
        let kj = kraft(&tcoef_codes(TCOEF_INTRA));
        assert_eq!(ki, kj);
        // Only the all-zero region the start codes need is left over: the
        // 0000 0000 0xxx codes of 12 bits and 0000 0000 01xx are unused.
        assert!(ki > 0.99 && ki < 1.0, "{ki}");
        let mut a: Vec<_> = TCOEF_INTER.iter().map(|e| code(e.0)).collect();
        let mut b: Vec<_> = TCOEF_INTRA.iter().map(|e| code(e.0)).collect();
        a.sort();
        b.sort();
        assert_eq!(a, b, "the intra table reuses the inter codewords");
    }

    /// Every (last, run, level) event the tables code is exactly the set
    /// LMAX describes: run r codes levels 1..=LMAX(last, r).
    #[test]
    fn tcoef_events_match_lmax() {
        for (intra, t) in [(true, TCOEF_INTRA), (false, TCOEF_INTER)] {
            let mut seen = std::collections::HashSet::new();
            for e in t {
                assert!(seen.insert((e.1, e.2, e.3)), "duplicate event {e:?}");
                assert!(e.3 as u32 <= lmax(intra, e.1 == 1, e.2 as usize), "{e:?}");
            }
            let count: u32 = [false, true]
                .iter()
                .map(|&last| (0..64).map(|r| lmax(intra, last, r)).sum::<u32>())
                .sum();
            assert_eq!(count as usize, t.len());
        }
    }

    #[test]
    fn rmax_values() {
        // Spot values from Tables B-21 / B-22.
        assert_eq!(rmax(true, false, 1), Some(14));
        assert_eq!(rmax(true, false, 2), Some(9));
        assert_eq!(rmax(true, false, 3), Some(7));
        assert_eq!(rmax(true, false, 4), Some(3));
        assert_eq!(rmax(true, false, 5), Some(2));
        assert_eq!(rmax(true, false, 10), Some(1));
        assert_eq!(rmax(true, false, 27), Some(0));
        assert_eq!(rmax(true, false, 28), None);
        assert_eq!(rmax(true, true, 1), Some(20));
        assert_eq!(rmax(true, true, 2), Some(6));
        assert_eq!(rmax(true, true, 3), Some(1));
        assert_eq!(rmax(true, true, 8), Some(0));
        assert_eq!(rmax(false, false, 1), Some(26));
        assert_eq!(rmax(false, false, 2), Some(10));
        assert_eq!(rmax(false, false, 3), Some(6));
        assert_eq!(rmax(false, false, 4), Some(2));
        assert_eq!(rmax(false, false, 5), Some(1));
        assert_eq!(rmax(false, false, 6), Some(1));
        assert_eq!(rmax(false, false, 12), Some(0));
        assert_eq!(rmax(false, true, 1), Some(40));
        assert_eq!(rmax(false, true, 2), Some(1));
        assert_eq!(rmax(false, true, 3), Some(0));
    }

    fn is_permutation(s: &[u8; 64]) -> bool {
        let mut seen = [false; 64];
        for &i in s {
            if seen[i as usize] {
                return false;
            }
            seen[i as usize] = true;
        }
        true
    }

    #[test]
    fn scans() {
        assert!(is_permutation(&ZIGZAG));
        assert!(is_permutation(&ALT_HORIZONTAL));
        assert!(is_permutation(&ALT_VERTICAL));
        // The zigzag walks anti-diagonals.
        for w in ZIGZAG.windows(2) {
            let (r0, c0) = (w[0] / 8, w[0] % 8);
            let (r1, c1) = (w[1] / 8, w[1] % 8);
            assert!((r0 as i32 - r1 as i32).abs() <= 1 && (c0 as i32 - c1 as i32).abs() <= 1);
        }
        // Alternate-horizontal is alternate-vertical transposed.
        for i in 0..64 {
            let v = ALT_VERTICAL[i];
            let t = (v % 8) * 8 + v / 8;
            assert_eq!(ALT_HORIZONTAL[i], t);
        }
    }

    #[test]
    fn dc_scaler_table() {
        let l: Vec<u32> = (1..=31).map(|q| dc_scaler(q, true)).collect();
        assert_eq!(&l[..8], &[8, 8, 8, 8, 10, 12, 14, 16]);
        assert_eq!(l[8], 17);
        assert_eq!(l[23], 32);
        assert_eq!(l[24], 34);
        assert_eq!(l[30], 46);
        let c: Vec<u32> = (1..=31).map(|q| dc_scaler(q, false)).collect();
        assert_eq!(&c[..6], &[8, 8, 8, 8, 9, 9]);
        assert_eq!(c[23], 18);
        assert_eq!(c[24], 19);
        assert_eq!(c[30], 25);
    }

    /// The length of the RVLC core a bit string begins with: up to the
    /// second `1` when it starts with `1`, the third `0` when it starts
    /// with `0` (None when the string ends first).
    fn rvlc_core_len(bits: &[u8]) -> Option<usize> {
        let b = *bits.first()?;
        let need = if b == 1 { 2 } else { 3 };
        let mut seen = 0;
        for (i, &x) in bits.iter().enumerate() {
            if x == b {
                seen += 1;
                if seen == need {
                    return Some(i + 1);
                }
            }
        }
        None
    }

    fn bits_of(s: &str) -> Vec<u8> {
        s.bytes().filter(|&c| c != b' ').map(|c| c - b'0').collect()
    }

    /// Every core of a given length the construction allows, ascending.
    fn rvlc_cores(len: usize) -> Vec<Vec<u8>> {
        let mut v = Vec::new();
        for n in 0u32..1 << len {
            let bits: Vec<u8> = (0..len).rev().map(|i| (n >> i & 1) as u8).collect();
            if rvlc_core_len(&bits) == Some(len) {
                v.push(bits);
            }
        }
        v
    }

    fn prefix_free(codes: &[Vec<u8>]) -> bool {
        codes.iter().enumerate().all(|(i, a)| {
            codes
                .iter()
                .enumerate()
                .all(|(j, b)| i == j || !(b.len() >= a.len() && b[..a.len()] == a[..]))
        })
    }

    /// Table B-23 checked against the construction it was built by: each
    /// code is a core (the shortest prefix with two `1`s, for a leading
    /// `1`, or three `0`s, for a leading `0`) plus exactly one free bit;
    /// the `1` cores read the same both ways; within each length the codes
    /// run through every core in ascending order with both free bits — the
    /// escape `0000` taking `000` with free bit 0 — and only the longest
    /// length (16 bits with the sign) stops part way.
    #[test]
    fn rvlc_codes_follow_their_construction() {
        assert_eq!(RVLC_TCOEF.len(), 169);
        let codes: Vec<Vec<u8>> = RVLC_TCOEF.iter().map(|e| bits_of(e.0)).collect();
        for c in &codes {
            let core = rvlc_core_len(c).expect("a core");
            assert_eq!(core + 1, c.len(), "{c:?}: one free bit after the core");
            if c[0] == 1 {
                let k = &c[..core];
                assert!(k.iter().eq(k.iter().rev()), "{c:?}: 1-cores are palindromes");
            }
        }
        // The expected list, built from the construction alone.
        let mut built: Vec<Vec<u8>> = Vec::new();
        for core_len in 2..=14 {
            for core in rvlc_cores(core_len) {
                for x in [0u8, 1] {
                    let mut c = core.clone();
                    c.push(x);
                    if c != bits_of(RVLC_ESCAPE) {
                        built.push(c);
                    }
                }
            }
        }
        assert_eq!(built[..codes.len()], codes[..], "codes in construction order");
        // The last length is the only one cut short.
        let last = codes.last().unwrap().len();
        assert_eq!(last, 15);
        assert!(built[..codes.len()].iter().all(|c| c.len() <= last));
        assert!(built[codes.len()..].iter().all(|c| c.len() == last));
    }

    /// The code is instantaneous in both directions: the codes and escape
    /// with their sign bit form a prefix-free set, and so do the same
    /// codes reversed (what a reader going backwards sees).
    #[test]
    fn rvlc_is_prefix_free_both_ways() {
        let mut fwd: Vec<Vec<u8>> = Vec::new();
        for e in RVLC_TCOEF {
            for s in [0u8, 1] {
                let mut c = bits_of(e.0);
                c.push(s);
                fwd.push(c);
            }
        }
        for s in [0u8, 1] {
            let mut c = bits_of(RVLC_ESCAPE);
            c.push(s);
            fwd.push(c);
        }
        assert!(prefix_free(&fwd), "forward");
        let bwd: Vec<Vec<u8>> = fwd
            .iter()
            .map(|c| c.iter().rev().copied().collect())
            .collect();
        assert!(prefix_free(&bwd), "backward");
        // Without the sign the bodies are prefix-free and suffix-free too.
        let mut bodies: Vec<Vec<u8>> = RVLC_TCOEF.iter().map(|e| bits_of(e.0)).collect();
        bodies.push(bits_of(RVLC_ESCAPE));
        assert!(prefix_free(&bodies));
        let rev: Vec<Vec<u8>> = bodies
            .iter()
            .map(|c| c.iter().rev().copied().collect())
            .collect();
        assert!(prefix_free(&rev));
        let kraft: f64 = bodies.iter().map(|c| 0.5f64.powi(c.len() as i32)).sum();
        assert!(kraft < 1.0 && kraft > 0.99, "{kraft}");
    }

    /// Each column codes every event once; the intra and inter columns
    /// code the same `last = 1` events (runs 0..=44 at level 1, and the
    /// same higher levels), as Table B-23 prints them.
    #[test]
    fn rvlc_events() {
        for intra in [true, false] {
            let ev: Vec<(u8, u8, u8)> = RVLC_TCOEF
                .iter()
                .map(|e| if intra { e.1 } else { e.2 })
                .collect();
            let set: std::collections::HashSet<_> = ev.iter().copied().collect();
            assert_eq!(set.len(), 169, "intra {intra}: an event coded twice");
            assert!(ev.iter().all(|&(l, r, v)| l <= 1 && r < 64 && v >= 1));
            // For each (last, run) the levels coded are 1..=max, no gaps.
            for last in 0..=1u8 {
                for run in 0..64u8 {
                    let mut lv: Vec<u8> = ev
                        .iter()
                        .filter(|e| e.0 == last && e.1 == run)
                        .map(|e| e.2)
                        .collect();
                    lv.sort();
                    assert!(lv.iter().enumerate().all(|(i, &v)| v as usize == i + 1));
                }
            }
        }
        let l1 = |intra: bool| -> Vec<(u8, u8, u8)> {
            let mut v: Vec<_> = RVLC_TCOEF
                .iter()
                .map(|e| if intra { e.1 } else { e.2 })
                .filter(|e| e.0 == 1)
                .collect();
            v.sort();
            v
        };
        assert_eq!(l1(true), l1(false));
        assert_eq!(l1(true).iter().filter(|e| e.2 == 1).count(), 45);
        // Spot entries.
        assert_eq!(RVLC_TCOEF[0], ("110", (0, 0, 1), (0, 0, 1)));
        assert_eq!(RVLC_TCOEF[155].1, (0, 0, 27));
        assert_eq!(RVLC_TCOEF[168], ("011111101111101", (1, 44, 1), (1, 44, 1)));
    }
}
