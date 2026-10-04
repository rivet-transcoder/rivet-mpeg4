//! Inverse quantisation (clause 7.4.2) — the first method (MPEG, weighting
//! matrices, mismatch control) and the second (H.263) — and the forward
//! quantiser the encoder pairs with the second.

/// Saturation of 7.4.2.3 for 8-bit video.
const MIN: i32 = -2048;
const MAX: i32 = 2047;

/// The quantisation a VOL selects.
#[derive(Clone)]
pub(crate) struct Quant {
    /// `quant_type == 1`: the first (MPEG) method.
    pub mpeg: bool,
    pub intra_matrix: [u8; 64],
    pub inter_matrix: [u8; 64],
}

impl Quant {
    pub fn h263() -> Quant {
        Quant {
            mpeg: false,
            intra_matrix: crate::tables::DEFAULT_INTRA_MATRIX,
            inter_matrix: crate::tables::DEFAULT_INTER_MATRIX,
        }
    }

    /// Dequantises an intra block in place: `QF[0][0] * dc_scaler` for the
    /// DC, the selected method for the rest.
    pub fn intra(&self, b: &mut [i16; 64], qp: u32, dc_scaler: u32) {
        let qp = qp as i32;
        let dc = (b[0] as i32 * dc_scaler as i32).clamp(MIN, MAX);
        if self.mpeg {
            let mut sum = dc;
            b[0] = dc as i16;
            // Branch-free (a zero QF gives zero), so it vectorises.
            for (v, &w) in b.iter_mut().zip(&self.intra_matrix).skip(1) {
                let q = *v as i32;
                // F'' = (2 QF W QP) / 16, truncating.
                let f = (2 * q * w as i32 * qp / 16).clamp(MIN, MAX);
                *v = f as i16;
                sum += f;
            }
            mismatch(b, sum);
        } else {
            b[0] = dc as i16;
            h263_ac(b, qp, 1);
        }
    }

    /// Dequantises a non-intra block in place.
    pub fn inter(&self, b: &mut [i16; 64], qp: u32) {
        let qp = qp as i32;
        if self.mpeg {
            let mut sum = 0;
            for (v, &w) in b.iter_mut().zip(&self.inter_matrix) {
                let q = *v as i32;
                // F'' = ((2 QF + sign(QF)) W QP) / 16, truncating.
                let f = ((2 * q + q.signum()) * w as i32 * qp / 16).clamp(MIN, MAX);
                *v = f as i16;
                sum += f;
            }
            mismatch(b, sum);
        } else {
            h263_ac(b, qp, 0);
        }
    }
}

/// The second method from coefficient `from` on:
/// `|F| = QP (2 |QF| + 1)`, less one when QP is even.
#[inline]
fn h263_ac(b: &mut [i16; 64], qp: i32, from: usize) {
    let odd_adj = if qp & 1 == 0 { 1 } else { 0 };
    // A select rather than a branch for zero, so it vectorises.
    for v in &mut b[from..] {
        let q = *v as i32;
        let m = qp * (2 * q.abs() + 1) - odd_adj;
        let f = (if q < 0 { -m } else { m }).clamp(MIN, MAX);
        *v = if q == 0 { 0 } else { f as i16 };
    }
}

/// Mismatch control of 7.4.2.4: when the sum of the coefficients is even,
/// the last one's least significant bit is toggled.
#[inline]
fn mismatch(b: &mut [i16; 64], sum: i32) {
    if sum & 1 == 0 {
        b[63] = if b[63] & 1 != 0 { b[63] - 1 } else { b[63] + 1 };
    }
}

/// `ceil(2^K / d)`, for [`rdiv`]; it fits 32 bits when `d >= 2^(K - 31)`.
pub(crate) const fn recip<const K: u32>(d: u32) -> u32 {
    (1u64 << K).div_ceil(d as u64) as u32
}

/// `a / d` as a multiplication by `m = recip::<K>(d)`, exact when
/// `a d < 2^K`: with `m = (2^K + e) / d`, `0 <= e < d`, and `a = q d + r`,
/// `a m / 2^K = q + (r + a e / 2^K) / d`, and `r + a e / 2^K < d` because
/// `r <= d - 1` and `a e < a d < 2^K`. (32-bit operands into a 64-bit
/// product: one vector multiply per lane.)
#[inline(always)]
pub(crate) fn rdiv<const K: u32>(a: u32, m: u32) -> u32 {
    ((a as u64 * m as u64) >> K) as u32
}

/// `recip::<32>(2 QP)` for QP 0..=31 (0 unused).
const H263_RECIP: [u32; 32] = {
    let mut t = [0u32; 32];
    let mut q = 1;
    while q < 32 {
        t[q] = recip::<32>(2 * q as u32);
        q += 1;
    }
    t
};

/// Forward quantisation for the second method (the encoder): intra AC
/// `|COF| / (2 QP)`, inter `(|COF| - QP / 2) / (2 QP)`, levels clipped to
/// what escape mode 3 can code. The intra DC (index 0) is left to the
/// caller.
pub(crate) fn quantise_h263(b: &mut [i16; 64], qp: u32, intra: bool) {
    let from = if intra { 1 } else { 0 };
    let Some(&m) = H263_RECIP.get(qp as usize).filter(|_| qp > 0) else {
        return quantise_h263_div(b, qp, intra);
    };
    // |COF| <= 2^15 and 2 QP < 64: the multiplication is exact.
    let dz = if intra { 0 } else { qp as i32 / 2 };
    for v in &mut b[from..] {
        let c = *v as i32;
        let a = (c.abs() - dz).max(0) as u32;
        let l = rdiv::<32>(a, m).min(2047) as i32;
        *v = (if c < 0 { -l } else { l }) as i16;
    }
}

/// [`quantise_h263`] by division (the definition), for any QP.
pub(crate) fn quantise_h263_div(b: &mut [i16; 64], qp: u32, intra: bool) {
    let qp = qp.max(1) as i32;
    let from = if intra { 1 } else { 0 };
    for v in &mut b[from..] {
        let c = *v as i32;
        let a = c.abs();
        let l = if intra {
            a / (2 * qp)
        } else {
            (a - qp / 2).max(0) / (2 * qp)
        };
        let l = l.min(2047);
        *v = (if c < 0 { -l } else { l }) as i16;
    }
}

/// The reciprocals of the first method's steps `W QP` for one pair of
/// weighting matrices, QP 1..=31: what [`quantise_mpeg`] multiplies by.
pub(crate) struct MpegRecips {
    /// `[qp][intra as usize][i]`: `recip::<31>(W QP)`.
    t: Vec<[[u32; 64]; 2]>,
    intra_matrix: [u8; 64],
    inter_matrix: [u8; 64],
}

impl MpegRecips {
    pub fn new(intra_matrix: &[u8; 64], inter_matrix: &[u8; 64]) -> MpegRecips {
        let t = (0..32u32)
            .map(|qp| {
                [inter_matrix, intra_matrix]
                    .map(|m| std::array::from_fn(|i| recip::<31>((m[i] as u32 * qp).max(1))))
            })
            .collect();
        MpegRecips {
            t,
            intra_matrix: *intra_matrix,
            inter_matrix: *inter_matrix,
        }
    }
}

/// Forward quantisation for the first (MPEG) method, the inverse of
/// [`Quant::intra`] / [`Quant::inter`]: with `step = W QP / 8` per
/// coefficient, intra AC levels are `|F| / step` rounded to the nearest,
/// inter levels `|F| / step` truncated (the inverse puts them back at the
/// middle of their interval), clipped to 2047. The intra DC is left to the
/// caller.
pub(crate) fn quantise_mpeg(b: &mut [i16; 64], qp: u32, intra: bool, r: &MpegRecips) {
    let matrix = if intra {
        &r.intra_matrix
    } else {
        &r.inter_matrix
    };
    let from = if intra { 1 } else { 0 };
    let Some(t) = r.t.get(qp as usize).filter(|_| qp > 0) else {
        return quantise_mpeg_div(b, qp, intra, matrix);
    };
    let t = &t[intra as usize];
    let qp = qp as i32;
    // a = 8 |F| (+ W QP / 2) <= 8 * 2^15 + 3952 and W QP <= 7905, so
    // a W QP < 2^31: the multiplications are exact.
    let half = if intra { 1 } else { 0 };
    for ((v, &w), &m) in b.iter_mut().zip(matrix).zip(t).skip(from) {
        let c = *v as i32;
        let d = w as i32 * qp;
        let a = 8 * c.abs() + ((d >> 1) & -half);
        let l = rdiv::<31>(a as u32, m).min(2047) as i32;
        *v = (if c < 0 { -l } else { l }) as i16;
    }
}

/// [`quantise_mpeg`] by division (the definition), for any QP.
pub(crate) fn quantise_mpeg_div(b: &mut [i16; 64], qp: u32, intra: bool, matrix: &[u8; 64]) {
    let qp = qp.max(1) as i32;
    let from = if intra { 1 } else { 0 };
    for (v, &w) in b.iter_mut().zip(matrix).skip(from) {
        let c = *v as i32;
        let d = (w as i32 * qp).max(1);
        let a = 8 * c.abs();
        let l = if intra { (a + d / 2) / d } else { a / d };
        let l = l.min(2047);
        *v = (if c < 0 { -l } else { l }) as i16;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h263_reconstruction_levels() {
        let q = Quant::h263();
        let mut b = [0i16; 64];
        b[1] = 1;
        b[2] = -2;
        b[3] = 3;
        q.inter(&mut b, 5);
        assert_eq!(&b[..4], &[0, 15, -25, 35]);
        let mut b = [0i16; 64];
        b[1] = 1;
        b[2] = -2;
        q.inter(&mut b, 4);
        assert_eq!(&b[..3], &[0, 11, -19]);
        // Intra: DC by the scaler, AC by the same rule.
        let mut b = [0i16; 64];
        b[0] = 100;
        b[1] = 1;
        q.intra(&mut b, 4, 8);
        assert_eq!(&b[..2], &[800, 11]);
        // Saturation.
        let mut b = [0i16; 64];
        b[5] = 2000;
        q.inter(&mut b, 31);
        assert_eq!(b[5], 2047);
    }

    #[test]
    fn mpeg_reconstruction_and_mismatch() {
        let q = Quant {
            mpeg: true,
            ..Quant::h263()
        };
        // One inter coefficient: ((2 + 1) * 16 * 2) / 16 = 6; the sum is
        // even, so F[7][7] (0) becomes 1.
        let mut b = [0i16; 64];
        b[0] = 1;
        q.inter(&mut b, 2);
        assert_eq!(b[0], 6);
        assert_eq!(b[63], 1);
        // Odd sum: untouched. ((2 * -1 - 1) * 16 * 3) / 16 = -9.
        let mut b = [0i16; 64];
        b[0] = -1;
        q.inter(&mut b, 3);
        assert_eq!(b[0], -9);
        assert_eq!(b[63], 0);
        // Intra: DC 10 * 8 = 80; F[0][1] = 2 * 3 * 17 * 5 / 16 = 31
        // (truncated from 31.875); sum 111 odd.
        let mut b = [0i16; 64];
        b[0] = 10;
        b[1] = 3;
        q.intra(&mut b, 5, 8);
        assert_eq!(&b[..2], &[80, 31]);
        assert_eq!(b[63], 0);
        // Even sum toggles an odd F[7][7] down: inter 1 at [63] with W 33,
        // QP 1: (3 * 33) / 16 = 6, even, so 6 -> 7.
        let mut b = [0i16; 64];
        b[63] = 1;
        q.inter(&mut b, 1);
        assert_eq!(b[63], 7);
        let mut b = [0i16; 64];
        b[63] = 2;
        // (5 * 33 * 1) / 16 = 10, even: 10 -> 11. Negative truncation:
        q.inter(&mut b, 1);
        assert_eq!(b[63], 11);
        let mut b = [0i16; 64];
        b[1] = -1;
        // (-3 * 17 * 1) / 16 = -3.1875 -> -3 (toward zero); odd sum.
        q.inter(&mut b, 1);
        assert_eq!(b[1], -3);
    }

    #[test]
    fn forward_quantiser_inverts() {
        let q = Quant::h263();
        for qp in 1..=31u32 {
            for c in -2000i16..2000 {
                let mut b = [0i16; 64];
                b[1] = c;
                quantise_h263(&mut b, qp, false);
                let mut r = b;
                q.inter(&mut r, qp);
                // Reconstruction is within one step of the input, or zero
                // inside the dead zone.
                let err = (r[1] as i32 - c as i32).abs();
                assert!(
                    err <= 2 * qp as i32 + qp as i32 / 2 + 1,
                    "qp {qp} c {c} -> {}",
                    r[1]
                );
            }
        }
    }

    /// The MPEG forward quantiser puts every coefficient back within half
    /// a step (intra) or within a step (inter) through the inverse, before
    /// mismatch control and saturation.
    #[test]
    fn mpeg_forward_quantiser_inverts() {
        let q = Quant {
            mpeg: true,
            intra_matrix: crate::tables::DEFAULT_INTRA_MATRIX,
            inter_matrix: crate::tables::DEFAULT_INTER_MATRIX,
        };
        for qp in [1u32, 4, 13, 31] {
            for f in (-2000i32..2000).step_by(37) {
                for intra in [true, false] {
                    let m = if intra {
                        &q.intra_matrix
                    } else {
                        &q.inter_matrix
                    };
                    let mut b = [0i16; 64];
                    b[9] = f as i16;
                    quantise_mpeg(
                        &mut b,
                        qp,
                        intra,
                        &MpegRecips::new(&q.intra_matrix, &q.inter_matrix),
                    );
                    let mut r = b;
                    // Undo mismatch control's toggle of the last coefficient.
                    if intra {
                        r[0] = 0;
                        q.intra(&mut r, qp, 8);
                    } else {
                        q.inter(&mut r, qp);
                    }
                    let step = m[9] as i32 * qp as i32 / 8;
                    let err = (r[9] as i32 - f).abs();
                    let bound = if intra { step / 2 + 1 } else { step + 1 };
                    assert!(
                        err <= bound.max(1) || b[9] == 0,
                        "qp {qp} f {f} intra {intra}: {} err {err}",
                        r[9]
                    );
                }
            }
        }
    }

    /// The multiplications compute the divisions they replace: every
    /// QP and every coefficient for the second method; every QP and
    /// weight, coefficients across the range, for the first.
    #[test]
    fn reciprocal_quantisers_divide_exactly() {
        for qp in 1..=31u32 {
            for c in i16::MIN..=i16::MAX {
                for intra in [false, true] {
                    let mut x = [c; 64];
                    let mut y = [c; 64];
                    quantise_h263(&mut x, qp, intra);
                    quantise_h263_div(&mut y, qp, intra);
                    assert_eq!(x[1], y[1], "h263 qp {qp} c {c} intra {intra}");
                }
            }
        }
        // Every weight 1..=255, in four matrices.
        let mats: Vec<[u8; 64]> = (0..4)
            .map(|k| std::array::from_fn(|i| (1 + (k * 64 + i) % 255) as u8))
            .collect();
        let mut seed = 1u32;
        for m in &mats {
            let r = MpegRecips::new(m, m);
            for qp in 1..=31u32 {
                for step in 0..2000 {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    let c = if step < 64 {
                        [i16::MAX, i16::MIN, 2047, -2048, 1, -1, 0, 4095][step % 8]
                    } else {
                        (seed >> 16) as i16
                    };
                    for intra in [false, true] {
                        let mut x = [c; 64];
                        let mut y = [c; 64];
                        quantise_mpeg(&mut x, qp, intra, &r);
                        quantise_mpeg_div(&mut y, qp, intra, m);
                        assert_eq!(x, y, "mpeg qp {qp} c {c} intra {intra}");
                    }
                }
            }
        }
    }
}
