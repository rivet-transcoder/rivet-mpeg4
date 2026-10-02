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
            for (v, &w) in b.iter_mut().zip(&self.intra_matrix).skip(1) {
                let q = *v as i32;
                if q != 0 {
                    // F'' = (2 QF W QP) / 16, truncating.
                    let f = (2 * q * w as i32 * qp / 16).clamp(MIN, MAX);
                    *v = f as i16;
                    sum += f;
                }
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
                if q != 0 {
                    // F'' = ((2 QF + sign(QF)) W QP) / 16, truncating.
                    let f = ((2 * q + q.signum()) * w as i32 * qp / 16).clamp(MIN, MAX);
                    *v = f as i16;
                    sum += f;
                }
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
    for v in &mut b[from..] {
        let q = *v as i32;
        if q != 0 {
            let m = qp * (2 * q.abs() + 1) - odd_adj;
            *v = (if q < 0 { -m } else { m }).clamp(MIN, MAX) as i16;
        }
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

/// Forward quantisation for the second method (the encoder): intra AC
/// `|COF| / (2 QP)`, inter `(|COF| - QP / 2) / (2 QP)`, levels clipped to
/// what escape mode 3 can code. The intra DC (index 0) is left to the
/// caller.
pub(crate) fn quantise_h263(b: &mut [i16; 64], qp: u32, intra: bool) {
    let qp = qp as i32;
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
}
