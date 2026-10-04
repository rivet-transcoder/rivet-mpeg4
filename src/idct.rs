//! The 8x8 inverse DCT of clause 7.4.4 (Annex A), in integer arithmetic,
//! and the forward DCT the encoder uses. The transforms themselves are in
//! `crate::dsp` (`scalar.rs` defines them; the SIMD versions compute the
//! same integers).
//!
//! The standard defines the IDCT mathematically and requires an
//! implementation to meet the accuracy of IEEE Std 1180-1990; the test that
//! checks it is at the bottom of this file. This one is separable — rows,
//! then columns — with the basis scaled by 2^16 and eight fractional bits
//! carried between the passes, which leaves its error far inside the
//! IEEE 1180 limits, and an even/odd split that halves the multiplies. It
//! is integer throughout, so every platform reconstructs the same samples
//! (which encoder and decoder rely on to stay in step).

pub(crate) use crate::dsp::{fdct, idct};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::Isa;

    /// The pseudo-random generator IEEE Std 1180-1990 specifies for its
    /// test data: integers uniformly in [-l, h].
    struct Ieee1180Rand(u32);

    impl Ieee1180Rand {
        fn next(&mut self, l: i64, h: i64) -> i64 {
            self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let i = (self.0 & 0x7fff_fffe) as f64;
            let x = i / (0x7fff_ffff as f64) * (l + h + 1) as f64;
            x as i64 - l
        }
    }

    fn cosines() -> [[f64; 8]; 8] {
        let mut c = [[0.0; 8]; 8];
        for (k, row) in c.iter_mut().enumerate() {
            let ck = if k == 0 { (0.125f64).sqrt() } else { 0.5 };
            for (n, v) in row.iter_mut().enumerate() {
                *v = ck * (((2 * n + 1) * k) as f64 * std::f64::consts::PI / 16.0).cos();
            }
        }
        c
    }

    fn ref_fdct(x: &[f64; 64], c: &[[f64; 8]; 8]) -> [f64; 64] {
        let mut out = [0.0; 64];
        for v in 0..8 {
            for u in 0..8 {
                let mut s = 0.0;
                for y in 0..8 {
                    for xx in 0..8 {
                        s += c[v][y] * c[u][xx] * x[y * 8 + xx];
                    }
                }
                out[v * 8 + u] = s;
            }
        }
        out
    }

    fn ref_idct(f: &[f64; 64], c: &[[f64; 8]; 8]) -> [f64; 64] {
        let mut out = [0.0; 64];
        for y in 0..8 {
            for x in 0..8 {
                let mut s = 0.0;
                for v in 0..8 {
                    for u in 0..8 {
                        s += c[v][y] * c[u][x] * f[v * 8 + u];
                    }
                }
                out[y * 8 + x] = s;
            }
        }
        out
    }

    struct Stats {
        peak: i64,
        pmse: f64,
        omse: f64,
        pme: f64,
        ome: f64,
    }

    /// One IEEE 1180 run: `blocks` blocks of data in [-l, h] (negated when
    /// `sign` is -1), the tested IDCT against the double-precision one.
    fn ieee1180_run(isa: Isa, l: i64, h: i64, sign: i64, blocks: usize) -> Stats {
        let c = cosines();
        let mut rng = Ieee1180Rand(1);
        let mut err = [0i64; 64];
        let mut sq = [0i64; 64];
        let mut peak = 0;
        for _ in 0..blocks {
            let mut x = [0.0; 64];
            for v in x.iter_mut() {
                *v = (rng.next(l, h) * sign) as f64;
            }
            let f = ref_fdct(&x, &c);
            let mut fi = [0.0; 64];
            let mut blk = [0i16; 64];
            for i in 0..64 {
                let q = f[i].round().clamp(-2048.0, 2047.0);
                fi[i] = q;
                blk[i] = q as i16;
            }
            let r = ref_idct(&fi, &c);
            crate::dsp::idct_with(isa, &mut blk);
            for i in 0..64 {
                let refv = (r[i].round() as i64).clamp(-256, 255);
                let test = (blk[i] as i64).clamp(-256, 255);
                let e = test - refv;
                peak = peak.max(e.abs());
                err[i] += e;
                sq[i] += e * e;
            }
        }
        let n = blocks as f64;
        Stats {
            peak,
            pmse: sq.iter().map(|&s| s as f64 / n).fold(0.0, f64::max),
            omse: sq.iter().sum::<i64>() as f64 / (64.0 * n),
            pme: err
                .iter()
                .map(|&s| (s as f64 / n).abs())
                .fold(0.0, f64::max),
            ome: (err.iter().sum::<i64>() as f64 / (64.0 * n)).abs(),
        }
    }

    /// IEEE Std 1180-1990: for each range and sign, over 10 000 blocks,
    /// peak error <= 1, per-pixel mean square error <= 0.06, overall mean
    /// square error <= 0.02, per-pixel mean error <= 0.015, overall mean
    /// error <= 0.0015; and an all-zero block transforms to all zeros.
    /// Every kernel level the CPU has is measured (they compute the same
    /// integers, which `crate::dsp`'s tests check block for block).
    #[test]
    fn ieee_1180_accuracy() {
        let blocks = if cfg!(debug_assertions) {
            2_000
        } else {
            10_000
        };
        for isa in Isa::all() {
            for (l, h) in [(256, 255), (5, 5), (300, 300)] {
                for sign in [1, -1] {
                    let s = ieee1180_run(isa, l, h, sign, blocks);
                    let msg = format!(
                        "{} {blocks} blocks, L={l} H={h} sign={sign}: peak {} pmse {:.4} omse {:.4} pme {:.4} ome {:.5}",
                        isa.name(),
                        s.peak,
                        s.pmse,
                        s.omse,
                        s.pme,
                        s.ome
                    );
                    println!("{msg}");
                    assert!(s.peak <= 1, "{msg}");
                    assert!(s.pmse <= 0.06, "{msg}");
                    assert!(s.omse <= 0.02, "{msg}");
                    assert!(s.pme <= 0.015, "{msg}");
                    assert!(s.ome <= 0.0015, "{msg}");
                }
            }
        }
        let mut z = [0i16; 64];
        idct(&mut z);
        assert_eq!(z, [0; 64]);
    }

    #[test]
    fn forward_then_inverse_is_near_identity() {
        let mut rng = Ieee1180Rand(7);
        for _ in 0..500 {
            let mut b = [0i16; 64];
            for v in b.iter_mut() {
                *v = rng.next(255, 255) as i16;
            }
            let orig = b;
            fdct(&mut b);
            idct(&mut b);
            for i in 0..64 {
                assert!((b[i] - orig[i]).abs() <= 1, "{} vs {}", b[i], orig[i]);
            }
        }
    }

    #[test]
    fn dc_only_block_is_flat() {
        let mut b = [0i16; 64];
        b[0] = 8 * 100;
        idct(&mut b);
        assert!(b.iter().all(|&v| v == 100));
    }
}
