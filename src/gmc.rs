//! Global motion compensation (clause 7.8, sprite warping applied to the
//! reference VOP): the warp the S-VOP's trajectory describes, the
//! prediction of a GMC macroblock, and the macroblock vector GMC
//! macroblocks contribute to vector prediction.
//!
//! The reference points are the VOP's corners `(0, 0)`, `(W, 0)` and
//! `(0, H)`; the trajectory moves them by `du`, `dv` (half samples); the
//! warp is computed at `1 / s` sample precision (`s = 2^(accuracy + 1)`)
//! through the "virtual" points at the powers of two `W' >= W`,
//! `H' >= H`, and samples are bilinear at that precision. With four
//! points the fourth corner `(W, H)` moves too, and the warp is the
//! perspective transform of 7.8.5, in 128-bit integers.

use crate::headers::VolHeader;
use crate::picture::Pic;

use crate::dec::vop::MbPix;

/// The coefficients of the four-point (perspective) warp of 7.8.5.
#[derive(Clone, Copy)]
struct Persp {
    a: i128,
    b: i128,
    c: i128,
    d: i128,
    e: i128,
    f: i128,
    g: i128,
    h: i128,
    /// `D W H`.
    dwh: i128,
}

/// `///` for any divisor: to the nearest integer, halves toward positive
/// infinity (3 /// 2 = 2, -3 /// 2 = -1).
#[inline]
fn div_round_up(n: i128, d: i128) -> i128 {
    let (n, d) = if d < 0 { (-n, -d) } else { (n, d) };
    (2 * n + d).div_euclid(2 * d)
}

/// One coordinate of a warp of at most three points, which is affine:
/// `base + div_up(ai i + aj j + c, 2^shift)` at sample `(i, j)` (`///` of a
/// power of two being `(n + d / 2) >> log2 d`). Along a row the numerator
/// grows by `ai` a sample, so it is kept as a running sum: the same
/// integers as evaluating the warp at each sample.
#[derive(Clone, Copy, Debug)]
struct Affine {
    ai: i64,
    aj: i64,
    c: i64,
    shift: u32,
    base: i64,
}

impl Affine {
    /// `n / d` with `d` a power of two (`div_up`'s divisor).
    fn new(ai: i64, aj: i64, c: i64, d: i64, base: i64) -> Affine {
        debug_assert!(d > 0 && d & (d - 1) == 0);
        Affine {
            ai,
            aj,
            c,
            shift: d.trailing_zeros(),
            base,
        }
    }

    /// The numerator, rounding offset included, at `(i, j)`.
    #[inline]
    fn start(&self, i: i64, j: i64) -> i64 {
        self.ai * i + self.aj * j + self.c + ((1i64 << self.shift) >> 1)
    }

    #[inline]
    fn at(&self, n: i64) -> i64 {
        self.base + (n >> self.shift)
    }
}

pub(crate) struct Gmc {
    points: usize,
    /// Without perspective: the luminance and chrominance warps as
    /// [`Affine`] coordinates `[f, g]`.
    lin: Option<([Affine; 2], [Affine; 2])>,
    persp: Option<Persp>,
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
/// positive infinity. Every divisor the warp uses is a power of two (`W'`,
/// `H'` and `r = 16 / s` are), so it is a shift.
#[inline]
fn div_up(n: i64, d: i64) -> i64 {
    debug_assert!(d > 0 && d & (d - 1) == 0);
    (n + d / 2) >> d.trailing_zeros()
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
        let (du3, dv3) = d(3);
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
        let i3p = s * w + k * (du3 + du2 + du1 + du0);
        let j3p = s * h + k * (dv3 + dv2 + dv1 + dv0);
        let persp = (warping.len() >= 4).then(|| {
            let (i0, j0, i1, j1) = (i0p as i128, j0p as i128, i1p as i128, j1p as i128);
            let (i2, j2, i3, j3) = (i2p as i128, j2p as i128, i3p as i128, j3p as i128);
            let (w, h) = (w as i128, h as i128);
            let g = ((i0 - i1 - i2 + i3) * (j2 - j3) - (i2 - i3) * (j0 - j1 - j2 + j3)) * h;
            let hh = ((i1 - i3) * (j0 - j1 - j2 + j3) - (i0 - i1 - i2 + i3) * (j1 - j3)) * w;
            let dd = (i1 - i3) * (j2 - j3) - (i2 - i3) * (j1 - j3);
            Persp {
                a: dd * (i1 - i0) * h + g * i1,
                b: dd * (i2 - i0) * w + hh * i2,
                c: dd * i0 * w * h,
                d: dd * (j1 - j0) * h + g * j1,
                e: dd * (j2 - j0) * w + hh * j2,
                f: dd * j0 * w * h,
                g,
                h: hh,
                dwh: dd * w * h,
            }
        });
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
        let points = warping.len().min(4);
        let lin = persp.is_none().then(|| {
            let (a2, b2, c2) = (-r * i0p + i1pp, r * j0p - j1pp, -r * j0p + j1pp);
            let (fi, fj) = ((-r * i0p + i1pp) * hp, (-r * i0p + i2pp) * wp);
            let (gi, gj) = ((-r * j0p + j1pp) * hp, (-r * j0p + j2pp) * wp);
            let a = |ai, aj, c, d, base| Affine::new(ai, aj, c, d, base);
            // Chrominance sample (ic, jc) is warped at x = 4 ic + 1,
            // y = 4 jc + 1: the coefficients times four, the ones added.
            match points {
                0 => (
                    [a(s, 0, 0, 1, 0), a(0, s, 0, 1, 0)],
                    [a(s, 0, 0, 1, 0), a(0, s, 0, 1, 0)],
                ),
                1 => (
                    [a(s, 0, 0, 1, i0p), a(0, s, 0, 1, j0p)],
                    [a(s, 0, 0, 1, div_up(i0p, 2)), a(0, s, 0, 1, div_up(j0p, 2))],
                ),
                2 => {
                    let d = wp * r;
                    (
                        [a(a2, b2, 0, d, i0p), a(c2, a2, 0, d, j0p)],
                        [
                            a(
                                4 * a2,
                                4 * b2,
                                a2 + b2 + 2 * wp * r * i0p - 16 * wp,
                                4 * d,
                                0,
                            ),
                            a(
                                4 * c2,
                                4 * a2,
                                c2 + a2 + 2 * wp * r * j0p - 16 * wp,
                                4 * d,
                                0,
                            ),
                        ],
                    )
                }
                _ => {
                    let d = wp * hp * r;
                    (
                        [a(fi, fj, 0, d, i0p), a(gi, gj, 0, d, j0p)],
                        [
                            a(
                                4 * fi,
                                4 * fj,
                                fi + fj + 2 * wp * hp * r * i0p - 16 * wp * hp,
                                4 * d,
                                0,
                            ),
                            a(
                                4 * gi,
                                4 * gj,
                                gi + gj + 2 * wp * hp * r * j0p - 16 * wp * hp,
                                4 * d,
                                0,
                            ),
                        ],
                    )
                }
            }
        });
        Gmc {
            points,
            lin,
            persp,
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
        if let Some(p) = &self.persp {
            let (ii, jj) = (i as i128, j as i128);
            let den = p.g * ii + p.h * jj + p.dwh;
            if den == 0 {
                // Disallowed by 7.8.5; a damaged trajectory gets no warp.
                return (s * i, s * j);
            }
            return (
                clamp_i64(div_round_up(p.a * ii + p.b * jj + p.c, den)),
                clamp_i64(div_round_up(p.d * ii + p.e * jj + p.f, den)),
            );
        }
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
        if let Some(p) = &self.persp {
            let (x, y) = ((4 * ic + 1) as i128, (4 * jc + 1) as i128);
            let gh = p.g * x + p.h * y;
            let den = 4 * gh + 8 * p.dwh;
            if den == 0 {
                return (s * ic, s * jc);
            }
            let t = (gh + 2 * p.dwh) * s as i128;
            return (
                clamp_i64(div_round_up(2 * p.a * x + 2 * p.b * y + 4 * p.c - t, den)),
                clamp_i64(div_round_up(2 * p.d * x + 2 * p.e * y + 4 * p.f - t, den)),
            );
        }
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

    /// Bilinear samples along a row: `out[c]` at `(f, g)` of `warp`'s
    /// sample `(i0 + c, j)`.
    #[inline]
    fn sample_row(
        &self,
        src: (&[u8], usize, i32, i32),
        warp: &[Affine; 2],
        (i0, j): (i64, i64),
        rounding: bool,
        out: &mut [u8],
    ) {
        let (p, stride, w, h) = src;
        let [wf, wg] = warp;
        let (mut nf, mut ng) = (wf.start(i0, j), wg.start(i0, j));
        let s = self.s;
        let round = s * s / 2 - rounding as i64;
        let sh = 2 * self.rho;
        // A translation (no more than one point): one fraction for the
        // whole row, whose samples are consecutive — a fixed-weight
        // bilinear filter over two rows when they lie inside.
        if wf.shift == 0 && wg.shift == 0 && wf.ai == s && wg.ai == 0 {
            let (f, g) = (wf.at(nf), wg.at(ng));
            let (x, y) = (f >> self.rho, g >> self.rho);
            let n = out.len() as i64;
            if x >= 0 && y >= 0 && x + n < w as i64 && y < h as i64 - 1 {
                let (ri, rj) = ((f & (s - 1)) as u32, (g & (s - 1)) as u32);
                let s = s as u32;
                let i = y as usize * stride + x as usize;
                let (a, b) = (
                    &p[i..i + out.len() + 1],
                    &p[i + stride..i + stride + out.len() + 1],
                );
                let (wa, wb) = ((s - ri) * (s - rj), ri * (s - rj));
                let (wc, wd) = ((s - ri) * rj, ri * rj);
                let round = round as u32;
                for (c, o) in out.iter_mut().enumerate() {
                    let v = wa * a[c] as u32
                        + wb * a[c + 1] as u32
                        + wc * b[c] as u32
                        + wd * b[c + 1] as u32;
                    *o = ((v + round) >> sh) as u8;
                }
                return;
            }
        }
        for o in out.iter_mut() {
            let (f, g) = (wf.at(nf), wg.at(ng));
            nf += wf.ai;
            ng += wg.ai;
            let (x, y) = (f >> self.rho, g >> self.rho);
            if x >= 0 && y >= 0 && x < w as i64 - 1 && y < h as i64 - 1 {
                let (ri, rj) = (f & (s - 1), g & (s - 1));
                let i = y as usize * stride + x as usize;
                let (a, b) = (p[i] as i64, p[i + 1] as i64);
                let (c, d) = (p[i + stride] as i64, p[i + stride + 1] as i64);
                let top = (s - ri) * a + ri * b;
                let bot = (s - ri) * c + ri * d;
                *o = (((s - rj) * top + rj * bot + round) >> sh).clamp(0, 255) as u8;
            } else {
                *o = self.sample(src, f, g, rounding);
            }
        }
    }

    /// The prediction of macroblock `(mbx, mby)` from `src`.
    pub fn predict_mb(&self, src: &Pic, mbx: usize, mby: usize, rounding: bool, out: &mut MbPix) {
        if let Some((lw, cw)) = &self.lin {
            let (x0, y0) = (mbx as i64 * 16, mby as i64 * 16);
            let luma = src.ref_plane(0);
            for (r, row) in out.y.as_chunks_mut::<16>().0.iter_mut().enumerate() {
                self.sample_row(luma, lw, (x0, y0 + r as i64), rounding, row);
            }
            let (x0, y0) = (mbx as i64 * 8, mby as i64 * 8);
            for (plane, o) in [(1, &mut out.cb), (2, &mut out.cr)] {
                let src = src.ref_plane(plane);
                for (r, row) in o.as_chunks_mut::<8>().0.iter_mut().enumerate() {
                    self.sample_row(src, cw, (x0, y0 + r as i64), rounding, row);
                }
            }
            return;
        }
        self.predict_mb_points(src, mbx, mby, rounding, out)
    }

    /// [`Gmc::predict_mb`] evaluating the warp at every sample (the
    /// definition; what a perspective warp uses).
    fn predict_mb_points(
        &self,
        src: &Pic,
        mbx: usize,
        mby: usize,
        rounding: bool,
        out: &mut MbPix,
    ) {
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
            let j = (mby * 16 + r) as i64;
            let i0 = (mbx * 16) as i64;
            if let Some(([wf, wg], _)) = &self.lin {
                let (mut nf, mut ng) = (wf.start(i0, j), wg.start(i0, j));
                for i in i0..i0 + 16 {
                    sx += wf.at(nf) - self.s * i;
                    sy += wg.at(ng) - self.s * j;
                    nf += wf.ai;
                    ng += wg.ai;
                }
                continue;
            }
            for i in i0..i0 + 16 {
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

/// A warped position kept within what later arithmetic handles (only a
/// damaged or degenerate trajectory comes near the limit).
#[inline]
fn clamp_i64(v: i128) -> i64 {
    v.clamp(-(1 << 40), 1 << 40) as i64
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

    /// `///` rounds halves toward positive infinity, for either sign of
    /// divisor.
    #[test]
    fn sign_dependent_rounding() {
        assert_eq!(div_round_up(3, 2), 2);
        assert_eq!(div_round_up(-3, 2), -1);
        assert_eq!(div_round_up(3, -2), -1);
        assert_eq!(div_round_up(7, 4), 2);
        assert_eq!(div_round_up(-7, 4), -2);
        assert_eq!(div_round_up(5, 4), 1);
        assert_eq!(div_round_up(-6, 4), -1);
    }

    /// Four points that only translate the VOP make the perspective warp
    /// a translation: ordinary motion compensation, sample for sample.
    #[test]
    fn four_point_translation_matches_halfpel_mc() {
        let mut pic = Pic::new(64, 48);
        for (i, v) in pic.y.iter_mut().enumerate() {
            *v = ((i * 37 + i / 64 * 11) % 251) as u8;
        }
        for acc in 0..4 {
            for (du, dv) in [(0, 0), (3, -5), (-7, 2)] {
                let g = Gmc::new(
                    &vol(64, 48, acc, false),
                    &[(du, dv), (0, 0), (0, 0), (0, 0)],
                    false,
                );
                let mut a = MbPix::new();
                let mut b = MbPix::new();
                for rounding in [false, true] {
                    g.predict_mb(&pic, 1, 1, rounding, &mut a);
                    predict_mb(&pic, 1, 1, &[[du, dv]; 4], false, rounding, false, &mut b);
                    assert_eq!(a.y, b.y, "acc {acc} du {du} dv {dv}");
                    assert_eq!(a.cb, b.cb, "acc {acc} du {du} dv {dv}");
                }
                assert_eq!(g.mb_vector(1, 1), [du, dv]);
            }
        }
    }

    /// Four points whose fourth corner moves as an affine map would move
    /// it make the perspective denominators constant: the warp is the
    /// three-point one, to within the one unit the three-point warp's
    /// virtual points can round differently.
    #[test]
    fn four_point_affine_matches_three_point() {
        let v = vol(176, 144, 3, false);
        // du[3] = -du[1] - du[2] - ... so that i3' = i1' + i2' - i0'.
        let three = [(4, -2), (10, 6), (-8, 12)];
        let four = [three[0], three[1], three[2], (0, 0)];
        let g3 = Gmc::new(&v, &three, false);
        let g4 = Gmc::new(&v, &four, false);
        let p = g4.persp.unwrap();
        assert_eq!((p.g, p.h), (0, 0));
        for j in (0..144).step_by(7) {
            for i in (0..176).step_by(5) {
                let (a, b) = (g3.luma(i, j), g4.luma(i, j));
                assert!(
                    (a.0 - b.0).abs() <= 1 && (a.1 - b.1).abs() <= 1,
                    "{i},{j}: {a:?} {b:?}"
                );
            }
        }
        for j in (0..72).step_by(5) {
            for i in (0..88).step_by(3) {
                let (a, b) = (g3.chroma(i, j), g4.chroma(i, j));
                assert!(
                    (a.0 - b.0).abs() <= 1 && (a.1 - b.1).abs() <= 1,
                    "{i},{j}: {a:?} {b:?}"
                );
            }
        }
    }

    /// A true perspective warp: the corners land where the trajectory puts
    /// them (in 1/s samples), and the warp is not affine in between.
    #[test]
    fn four_point_perspective_moves_the_corners() {
        let v = vol(64, 64, 1, false); // s = 4
        let t = [(0, 0), (0, 0), (0, 0), (-16, -16)];
        let g = Gmc::new(&v, &t, false);
        assert_eq!(g.luma(0, 0), (0, 0));
        assert_eq!(g.luma(64, 0), (4 * 64, 0));
        assert_eq!(g.luma(0, 64), (0, 4 * 64));
        // (W, H) moved by du = dv = -16 half samples: 8 samples in.
        assert_eq!(g.luma(64, 64), (4 * 56, 4 * 56));
        // The centre goes where the diagonals cross, as under any
        // projective map; the middle of the right edge stays on the moved
        // edge but, unlike an affine map, not at its middle.
        assert_eq!(g.luma(32, 32), (128, 128));
        let (x, y) = g.luma(64, 32);
        // On the line from (256, 0) to (224, 224): 7 x + y = 7 * 256.
        assert!((7 * x + y - 7 * 256).abs() <= 8, "{x},{y}");
        assert_ne!((x, y), (240, 112));
    }

    /// The running-sum warp predicts what evaluating the warp at every
    /// sample does, and gives the same macroblock vectors: every point
    /// count and accuracy, trajectories small and large (reaching outside
    /// the picture), odd sizes, both roundings.
    #[test]
    fn affine_rows_match_the_warp() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut rnd = move |n: i64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % (2 * n as u64 + 1)) as i64 - n
        };
        for (w, h) in [(64, 48), (176, 144), (200, 150)] {
            let mut pic = Pic::new(w, h);
            for (i, v) in pic.y.iter_mut().enumerate() {
                *v = ((i * 37 + i / 64 * 11) % 251) as u8;
            }
            for (i, v) in pic.cb.iter_mut().enumerate() {
                *v = ((i * 13 + 7) % 241) as u8;
            }
            for (i, v) in pic.cr.iter_mut().enumerate() {
                *v = ((i * 29 + 3) % 239) as u8;
            }
            for trial in 0..300 {
                let acc = (trial % 4) as u32;
                let points = trial % 4;
                let big = if trial % 3 == 0 { 400 } else { 24 };
                let traj: Vec<(i32, i32)> = (0..points)
                    .map(|_| (rnd(big) as i32, rnd(big) as i32))
                    .collect();
                let g = Gmc::new(&vol(w, h, acc, trial % 2 == 0), &traj, trial % 5 == 0);
                assert!(g.lin.is_some());
                let (mbw, mbh) = (w.div_ceil(16) as usize, h.div_ceil(16) as usize);
                for mby in 0..mbh {
                    for mbx in 0..mbw {
                        for rounding in [false, true] {
                            let mut a = MbPix::new();
                            let mut b = MbPix::new();
                            g.predict_mb(&pic, mbx, mby, rounding, &mut a);
                            g.predict_mb_points(&pic, mbx, mby, rounding, &mut b);
                            assert_eq!(a.y, b.y, "{traj:?} acc {acc} mb {mbx},{mby}");
                            assert_eq!(a.cb, b.cb, "{traj:?} acc {acc} mb {mbx},{mby}");
                            assert_eq!(a.cr, b.cr, "{traj:?} acc {acc} mb {mbx},{mby}");
                        }
                        let mut v = [0i64; 2];
                        for r in 0..16 {
                            for c in 0..16 {
                                let (i, j) = ((mbx * 16 + c) as i64, (mby * 16 + r) as i64);
                                let (f, gg) = g.luma(i, j);
                                v[0] += f - g.s * i;
                                v[1] += gg - g.s * j;
                            }
                        }
                        let unit = if g.quarter { 4 } else { 2 };
                        let d = 256 * g.s / unit;
                        let rd = |n: i64| {
                            if n >= 0 {
                                (n + d / 2) / d
                            } else {
                                -((-n + d / 2) / d)
                            }
                        };
                        assert_eq!(
                            g.mb_vector(mbx, mby),
                            [
                                rd(v[0]).clamp(-16384, 16383) as i32,
                                rd(v[1]).clamp(-16384, 16383) as i32
                            ]
                        );
                    }
                }
            }
        }
    }

    /// Prediction speed, the running-sum warp against the per-sample one:
    /// `cargo test --release --lib gmc_speed -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn gmc_speed() {
        let (w, h) = (1280u32, 720u32);
        let mut pic = Pic::new(w, h);
        for (i, v) in pic.y.iter_mut().enumerate() {
            *v = ((i * 37 + i / 64 * 11) % 251) as u8;
        }
        for (points, traj) in [
            (1, vec![(5, -3)]),
            (2, vec![(5, -3), (9, 4)]),
            (3, vec![(5, -3), (9, 4), (-6, 11)]),
        ] {
            let g = Gmc::new(&vol(w, h, 3, false), &traj, false);
            let mut px = MbPix::new();
            let time = |f: &mut dyn FnMut()| {
                let mut best = f64::MAX;
                for _ in 0..5 {
                    let t = std::time::Instant::now();
                    f();
                    best = best.min(t.elapsed().as_secs_f64());
                }
                best * 1e9 / (80.0 * 45.0)
            };
            let fast = time(&mut || {
                for mby in 0..45 {
                    for mbx in 0..80 {
                        g.predict_mb(&pic, mbx, mby, true, &mut px);
                        std::hint::black_box(&px);
                    }
                }
            });
            let slow = time(&mut || {
                for mby in 0..45 {
                    for mbx in 0..80 {
                        g.predict_mb_points(&pic, mbx, mby, true, &mut px);
                        std::hint::black_box(&px);
                    }
                }
            });
            println!(
                "GMC {points} point(s): {slow:.0} ns per macroblock per sample, {fast:.0} ns running sums ({:.1}x)",
                slow / fast
            );
        }
    }
}
