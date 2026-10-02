//! Global motion compensation (clause 7.8, sprite warping applied to the
//! reference VOP): the warp the S-VOP's trajectory describes, the
//! prediction of a GMC macroblock, and the macroblock vector GMC
//! macroblocks contribute to vector prediction.
//!
//! The reference points are the VOP's corners `(0, 0)`, `(W, 0)` and
//! `(0, H)`; the trajectory moves them by `du`, `dv` (half samples); the
//! warp is computed at `1 / s` sample precision (`s = 2^(accuracy + 1)`)
//! through the "virtual" points at the powers of two `W' >= W`,
//! `H' >= H`, and samples are bilinear at that precision.

use crate::headers::VolHeader;
use crate::picture::Pic;

use crate::dec::vop::MbPix;

pub(crate) struct Gmc {
    points: usize,
    /// `s`, and its log2.
    s: i64,
    rho: u32,
    /// `r = 16 / s`.
    r: i64,
    i0p: i64,
    j0p: i64,
    i1pp: i64,
    j1pp: i64,
    i2pp: i64,
    j2pp: i64,
    wp: i64,
    hp: i64,
    quarter: bool,
}

/// `///`: division rounding to the nearest integer, halves toward
/// positive infinity.
#[inline]
fn div_up(n: i64, d: i64) -> i64 {
    (n + d / 2).div_euclid(d)
}

impl Gmc {
    /// The warp of an S-VOP. `divx500`: the trajectory comes from DivX
    /// 5.00, which codes it in `1 / s` samples rather than half samples.
    pub fn new(vol: &VolHeader, warping: &[(i32, i32)], divx500: bool) -> Gmc {
        let s = 2i64 << vol.sprite_warping_accuracy;
        let r = 16 / s;
        let (w, h) = (vol.width as i64, vol.height as i64);
        let wp = (w as u64).next_power_of_two() as i64;
        let hp = (h as u64).next_power_of_two() as i64;
        let d = |k: usize| {
            warping
                .get(k)
                .map_or((0, 0), |&(u, v)| (u as i64, v as i64))
        };
        let (du0, dv0) = d(0);
        let (du1, dv1) = d(1);
        let (du2, dv2) = d(2);
        // Corners (i0, j0) = (0, 0), (i1, j1) = (W, 0), (i2, j2) = (0, H).
        // i0' = (s / 2)(2 i0 + du[0]), i1' = (s / 2)(2 i1 + du[1] + du[0]),
        // i2' = (s / 2)(2 i2 + du[2] + du[0]), and likewise for j.
        let k = if divx500 { 1 } else { s / 2 };
        let i0p = k * du0;
        let j0p = k * dv0;
        let i1p = s * w + k * (du1 + du0);
        let j1p = k * (dv1 + dv0);
        let i2p = k * (du2 + du0);
        let j2p = s * h + k * (dv2 + dv0);
        // `//`: to the nearest, halves away from zero.
        let rd = |n: i64, d: i64| {
            if n >= 0 {
                (n + d / 2) / d
            } else {
                -((-n + d / 2) / d)
            }
        };
        let i1pp = 16 * wp + rd((w - wp) * (r * i0p) + wp * (r * i1p - 16 * w), w);
        let j1pp = rd((w - wp) * (r * j0p) + wp * (r * j1p), w);
        let i2pp = rd((h - hp) * (r * i0p) + hp * (r * i2p), h);
        let j2pp = 16 * hp + rd((h - hp) * (r * j0p) + hp * (r * j2p - 16 * h), h);
        Gmc {
            points: warping.len().min(3),
            s,
            rho: s.trailing_zeros(),
            r,
            i0p,
            j0p,
            i1pp,
            j1pp,
            i2pp,
            j2pp,
            wp,
            hp,
            quarter: vol.quarter_sample,
        }
    }

    /// The warped position of luminance sample `(i, j)`, in `1 / s`
    /// samples.
    #[inline]
    fn luma(&self, i: i64, j: i64) -> (i64, i64) {
        let (s, r) = (self.s, self.r);
        match self.points {
            0 => (s * i, s * j),
            1 => (self.i0p + s * i, self.j0p + s * j),
            2 => {
                let a = -r * self.i0p + self.i1pp;
                let b = r * self.j0p - self.j1pp;
                let c = -r * self.j0p + self.j1pp;
                let d = self.wp * r;
                (
                    self.i0p + div_up(a * i + b * j, d),
                    self.j0p + div_up(c * i + a * j, d),
                )
            }
            _ => {
                let d = self.wp * self.hp * r;
                let f = (-r * self.i0p + self.i1pp) * self.hp * i
                    + (-r * self.i0p + self.i2pp) * self.wp * j;
                let g = (-r * self.j0p + self.j1pp) * self.hp * i
                    + (-r * self.j0p + self.j2pp) * self.wp * j;
                (self.i0p + div_up(f, d), self.j0p + div_up(g, d))
            }
        }
    }

    /// The warped position of chrominance sample `(ic, jc)`, in `1 / s`
    /// chrominance samples: the luminance warp at the centre of the 2x2
    /// luminance samples it covers, halved.
    #[inline]
    fn chroma(&self, ic: i64, jc: i64) -> (i64, i64) {
        let (s, r) = (self.s, self.r);
        match self.points {
            0 => (s * ic, s * jc),
            1 => (s * ic + div_up(self.i0p, 2), s * jc + div_up(self.j0p, 2)),
            2 => {
                let a = -r * self.i0p + self.i1pp;
                let b = r * self.j0p - self.j1pp;
                let c = -r * self.j0p + self.j1pp;
                let d = 4 * self.wp * r;
                let (x, y) = (4 * ic + 1, 4 * jc + 1);
                (
                    div_up(a * x + b * y + 2 * self.wp * r * self.i0p - 16 * self.wp, d),
                    div_up(c * x + a * y + 2 * self.wp * r * self.j0p - 16 * self.wp, d),
                )
            }
            _ => {
                let (wp, hp) = (self.wp, self.hp);
                let d = 4 * wp * hp * r;
                let (x, y) = (4 * ic + 1, 4 * jc + 1);
                let f = (-r * self.i0p + self.i1pp) * hp * x
                    + (-r * self.i0p + self.i2pp) * wp * y
                    + 2 * wp * hp * r * self.i0p
                    - 16 * wp * hp;
                let g = (-r * self.j0p + self.j1pp) * hp * x
                    + (-r * self.j0p + self.j2pp) * wp * y
                    + 2 * wp * hp * r * self.j0p
                    - 16 * wp * hp;
                (div_up(f, d), div_up(g, d))
            }
        }
    }

    /// Bilinear sample at `(f, g)` (`1 / s` units) of a plane.
    #[inline]
    fn sample(&self, src: (&[u8], usize, i32, i32), f: i64, g: i64, rounding: bool) -> u8 {
        let (p, stride, w, h) = src;
        let (x, y) = (f >> self.rho, g >> self.rho);
        let (ri, rj) = (f & (self.s - 1), g & (self.s - 1));
        let at = |xx: i64, yy: i64| -> i64 {
            let cx = xx.clamp(0, w as i64 - 1) as usize;
            let cy = yy.clamp(0, h as i64 - 1) as usize;
            p[cy * stride + cx] as i64
        };
        let s = self.s;
        let top = (s - ri) * at(x, y) + ri * at(x + 1, y);
        let bot = (s - ri) * at(x, y + 1) + ri * at(x + 1, y + 1);
        let v = ((s - rj) * top + rj * bot + s * s / 2 - rounding as i64) >> (2 * self.rho);
        v.clamp(0, 255) as u8
    }

    /// The prediction of macroblock `(mbx, mby)` from `src`.
    pub fn predict_mb(&self, src: &Pic, mbx: usize, mby: usize, rounding: bool, out: &mut MbPix) {
        let luma = src.ref_plane(0);
        for r in 0..16 {
            for c in 0..16 {
                let (f, g) = self.luma((mbx * 16 + c) as i64, (mby * 16 + r) as i64);
                out.y[r * 16 + c] = self.sample(luma, f, g, rounding);
            }
        }
        let (cb, cr) = (src.ref_plane(1), src.ref_plane(2));
        for r in 0..8 {
            for c in 0..8 {
                let (f, g) = self.chroma((mbx * 8 + c) as i64, (mby * 8 + r) as i64);
                out.cb[r * 8 + c] = self.sample(cb, f, g, rounding);
                out.cr[r * 8 + c] = self.sample(cr, f, g, rounding);
            }
        }
    }

    /// The vector a GMC macroblock stands for in vector prediction: the
    /// average displacement of its 256 luminance samples, in half (or,
    /// with quarter_sample, quarter) samples, rounded to the nearest.
    pub fn mb_vector(&self, mbx: usize, mby: usize) -> [i32; 2] {
        let (mut sx, mut sy) = (0i64, 0i64);
        for r in 0..16 {
            for c in 0..16 {
                let (i, j) = ((mbx * 16 + c) as i64, (mby * 16 + r) as i64);
                let (f, g) = self.luma(i, j);
                sx += f - self.s * i;
                sy += g - self.s * j;
            }
        }
        let unit = if self.quarter { 4 } else { 2 };
        let d = 256 * self.s / unit;
        let rd = |n: i64| {
            if n >= 0 {
                (n + d / 2) / d
            } else {
                -((-n + d / 2) / d)
            }
        };
        // Kept within what the vector store holds; only a damaged
        // trajectory gets near it.
        [
            rd(sx).clamp(-16384, 16383) as i32,
            rd(sy).clamp(-16384, 16383) as i32,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dec::vop::predict_mb;

    fn vol(w: u32, h: u32, acc: u32, quarter: bool) -> VolHeader {
        let mut v = VolHeader::short_header(w, h);
        v.sprite_warping_accuracy = acc;
        v.quarter_sample = quarter;
        v
    }

    /// A translation-only warp is ordinary motion compensation: with a
    /// half-sample trajectory the luminance prediction equals the
    /// half-sample prediction with that vector, and the macroblock vector
    /// is the trajectory itself.
    #[test]
    fn translation_matches_halfpel_mc() {
        let mut pic = Pic::new(64, 48);
        for (i, v) in pic.y.iter_mut().enumerate() {
            *v = ((i * 37 + i / 64 * 11) % 251) as u8;
        }
        for acc in 0..4 {
            for (du, dv) in [(0, 0), (3, -5), (-7, 2), (20, 9)] {
                for points in 1..=3 {
                    let mut w = vec![(du, dv)];
                    w.resize(points, (0, 0));
                    let g = Gmc::new(&vol(64, 48, acc, false), &w, false);
                    let mut a = MbPix::new();
                    let mut b = MbPix::new();
                    for rounding in [false, true] {
                        g.predict_mb(&pic, 1, 1, rounding, &mut a);
                        predict_mb(&pic, 1, 1, &[[du, dv]; 4], false, rounding, false, &mut b);
                        assert_eq!(a.y, b.y, "acc {acc} du {du} dv {dv} points {points}");
                    }
                    assert_eq!(g.mb_vector(1, 1), [du, dv]);
                    let gq = Gmc::new(&vol(64, 48, acc, true), &w, false);
                    assert_eq!(gq.mb_vector(2, 0), [2 * du, 2 * dv]);
                }
            }
        }
    }

    /// The two-point warp with the second point moved by the width is a
    /// zoom by two about the origin.
    #[test]
    fn two_point_zoom() {
        let g = Gmc::new(&vol(64, 64, 3, false), &[(0, 0), (128, 0)], false);
        // (W + 64 samples) / W: twice as far, in 1/16 samples.
        assert_eq!(g.luma(10, 0), (16 * 20, 0));
        assert_eq!(g.luma(0, 10), (0, 16 * 20));
        assert_eq!(g.luma(5, 7), (16 * 10, 16 * 14));
    }
}
