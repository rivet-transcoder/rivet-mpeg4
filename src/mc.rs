//! Motion compensation (clause 7.6.2): half-sample bilinear interpolation,
//! quarter-sample interpolation (the 8-tap filter of 7.6.2.1 with block-edge
//! mirroring), the chrominance vector derivations, and unrestricted motion
//! vectors — every reference sample outside the reconstructed area takes
//! the value of the nearest sample on its boundary (the padding of 7.6.4;
//! see `Pic::ref_plane` for which boundary).

use crate::tables::CHROMA_ROUND_16;

/// A plane to predict from: `(samples, stride, valid width, valid height)`.
pub(crate) type Src<'a> = (&'a [u8], usize, i32, i32);

/// Copies the `ww` x `wh` window whose top-left sample is `(x, y)` into
/// `out` (stride `ww`), clamping coordinates into the valid area.
#[inline]
fn fetch(src: Src, x: i32, y: i32, ww: usize, wh: usize, out: &mut [u8]) {
    let (plane, stride, pw, ph) = src;
    if x >= 0 && y >= 0 && x + ww as i32 <= pw && y + wh as i32 <= ph {
        for r in 0..wh {
            let s = (y as usize + r) * stride + x as usize;
            out[r * ww..r * ww + ww].copy_from_slice(&plane[s..s + ww]);
        }
        return;
    }
    for r in 0..wh {
        let sy = (y + r as i32).clamp(0, ph - 1) as usize * stride;
        for c in 0..ww {
            let sx = (x + c as i32).clamp(0, pw - 1) as usize;
            out[r * ww + c] = plane[sy + sx];
        }
    }
}

/// Half-sample prediction of a `bw` x `bh` block at `(x, y)` displaced by
/// `(mvx, mvy)` half samples, with `rounding_control` (`vop_rounding_type`
/// in P- and S-VOPs, 0 in B-VOPs and the short video header).
#[allow(clippy::too_many_arguments)]
pub(crate) fn halfpel(
    src: Src,
    x: i32,
    y: i32,
    mvx: i32,
    mvy: i32,
    bw: usize,
    bh: usize,
    rounding: bool,
    out: &mut [u8],
    out_stride: usize,
) {
    let ix = x + (mvx >> 1);
    let iy = y + (mvy >> 1);
    let (fx, fy) = (mvx & 1, mvy & 1);
    let mut win = [0u8; 17 * 17];
    let ww = bw + 1;
    fetch(src, ix, iy, ww, bh + 1, &mut win);
    let rc = rounding as u32;
    for r in 0..bh {
        let o = &mut out[r * out_stride..r * out_stride + bw];
        let a = &win[r * ww..];
        let b = &win[(r + 1) * ww..];
        match (fx, fy) {
            (0, 0) => o.copy_from_slice(&a[..bw]),
            (1, 0) => {
                for c in 0..bw {
                    o[c] = ((a[c] as u32 + a[c + 1] as u32 + 1 - rc) >> 1) as u8;
                }
            }
            (0, _) => {
                for c in 0..bw {
                    o[c] = ((a[c] as u32 + b[c] as u32 + 1 - rc) >> 1) as u8;
                }
            }
            _ => {
                for c in 0..bw {
                    let s = a[c] as u32 + a[c + 1] as u32 + b[c] as u32 + b[c + 1] as u32;
                    o[c] = ((s + 2 - rc) >> 2) as u8;
                }
            }
        }
    }
}

/// The 8-tap half-sample filter of 7.6.2.1 over the `n + 1` samples
/// `p[0..=n]`, writing the `n` values between neighbours to `out`. Taps
/// beyond the block's samples are mirrored back into it, the edge sample
/// repeated (`p[-1] = p[0]`, `p[-2] = p[1]`, `p[n + 1] = p[n]`, ...).
#[inline]
fn filter8(p: &[u8], n: usize, rc: i32, out: &mut [u8]) {
    // The row extended by three mirrored samples each side.
    let mut e = [0i32; 17 + 6];
    for (k, v) in e[..n + 7].iter_mut().enumerate() {
        let i = k as isize - 3;
        let m = if i < 0 {
            -i - 1
        } else if i > n as isize {
            2 * n as isize + 1 - i
        } else {
            i
        };
        *v = p[m as usize] as i32;
    }
    for i in 0..n {
        let e = &e[i..i + 8];
        let v = 20 * (e[3] + e[4]) - 6 * (e[2] + e[5]) + 3 * (e[1] + e[6]) - (e[0] + e[7]);
        out[i] = ((v + 16 - rc) >> 5).clamp(0, 255) as u8;
    }
}

/// Quarter-sample luminance prediction (7.6.2.1) of a `bw` x `bh` block
/// (16x16 for one vector per macroblock, 8x8 for four, 16x8 for a field)
/// displaced by `(mvx, mvy)` quarter samples.
///
/// The half-sample values come from the 8-tap filter over the block's
/// `(bw + 1)` x `(bh + 1)` window of integer samples (horizontal first;
/// the centre position filters the horizontal half samples vertically);
/// the quarter-sample values are the bilinear average of the nearest
/// integer / half samples, with `rounding_control`. Only the half-sample
/// planes the position reads are computed.
#[allow(clippy::too_many_arguments)]
pub(crate) fn qpel(
    src: Src,
    x: i32,
    y: i32,
    mvx: i32,
    mvy: i32,
    bw: usize,
    bh: usize,
    rounding: bool,
    out: &mut [u8],
    out_stride: usize,
) {
    let (fx, fy) = ((mvx & 3) as usize, (mvy & 3) as usize);
    let ix = x + (mvx >> 2);
    let iy = y + (mvy >> 2);
    let (nw, nh) = (bw + 1, bh + 1);
    let mut full = [0u8; 17 * 17];
    fetch(src, ix, iy, nw, nh, &mut full);
    if fx == 0 && fy == 0 {
        for r in 0..bh {
            out[r * out_stride..r * out_stride + bw].copy_from_slice(&full[r * nw..r * nw + bw]);
        }
        return;
    }
    let rc = rounding as i32;
    // Planes, each `nw` wide: integer samples `full` (nh rows); horizontal
    // half samples `hp` (nh rows, bw used); vertical half samples `vp`
    // (bh rows, nw used); centre half samples `cp` (bh rows, bw used).
    let mut hp = [0u8; 17 * 17];
    let mut vp = [0u8; 17 * 17];
    let mut cp = [0u8; 17 * 17];
    let mut col = [0u8; 17];
    let mut res = [0u8; 16];
    if fx != 0 {
        for r in 0..nh {
            filter8(
                &full[r * nw..r * nw + nw],
                bw,
                rc,
                &mut hp[r * nw..r * nw + bw],
            );
        }
    }
    if fy != 0 && fx != 2 {
        for c in 0..nw {
            for r in 0..nh {
                col[r] = full[r * nw + c];
            }
            filter8(&col, bh, rc, &mut res);
            for r in 0..bh {
                vp[r * nw + c] = res[r];
            }
        }
    }
    if fx != 0 && fy != 0 {
        for c in 0..bw {
            for r in 0..nh {
                col[r] = hp[r * nw + c];
            }
            filter8(&col, bh, rc, &mut res);
            for r in 0..bh {
                cp[r * nw + c] = res[r];
            }
        }
    }
    // The half-sample grid position (gx, gy): integer, horizontal,
    // vertical or centre plane by parity.
    let at = |gx: usize, gy: usize| -> u32 {
        let (c, r) = (gx >> 1, gy >> 1);
        (match (gx & 1, gy & 1) {
            (0, 0) => full[r * nw + c],
            (1, 0) => hp[r * nw + c],
            (0, _) => vp[r * nw + c],
            _ => cp[r * nw + c],
        }) as u32
    };
    let rc = rc as u32;
    for r in 0..bh {
        let gy = 2 * r + fy / 2;
        let o = &mut out[r * out_stride..r * out_stride + bw];
        for (c, d) in o.iter_mut().enumerate() {
            let gx = 2 * c + fx / 2;
            let a = at(gx, gy);
            let v = match (fx & 1, fy & 1) {
                (0, 0) => a,
                (1, 0) => (a + at(gx + 1, gy) + 1 - rc) >> 1,
                (0, _) => (a + at(gx, gy + 1) + 1 - rc) >> 1,
                _ => (a + at(gx + 1, gy) + at(gx, gy + 1) + at(gx + 1, gy + 1) + 2 - rc) >> 2,
            };
            *d = v as u8;
        }
    }
}

/// The chrominance vector component of a macroblock with one luminance
/// vector `v` (half samples): `v / 2` with the quarter positions moved to
/// the half position between (Table 7-6), i.e. `(v >> 1) | (v & 1)`.
#[inline]
pub(crate) fn chroma_mv_1(v: i32) -> i32 {
    (v >> 1) | (v & 1)
}

/// The chrominance vector component of a macroblock with four luminance
/// vectors summing to `s` (half samples): `s / 8`, its sixteenths rounded
/// by Table 7-7.
#[inline]
pub(crate) fn chroma_mv_4(s: i32) -> i32 {
    let a = s.abs();
    let v = (a >> 4) * 2 + CHROMA_ROUND_16[(a & 15) as usize];
    if s < 0 { -v } else { v }
}

/// A luminance vector component in half samples: quarter-sample vectors
/// are halved (toward zero) before the chrominance derivation.
#[inline]
pub(crate) fn luma_to_halfpel(v: i32, quarter: bool) -> i32 {
    if quarter { v / 2 } else { v }
}

/// Averages a second prediction into `a` (B-VOP interpolated mode):
/// `(a + b + 1) >> 1`.
pub(crate) fn average(a: &mut [u8], b: &[u8]) {
    for (x, &y) in a.iter_mut().zip(b) {
        *x = ((*x as u32 + y as u32 + 1) >> 1) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The straightforward implementation the fast one must match: every
    // half-sample plane over the whole window, each tap mirrored on the fly.
    /// The 8-tap half-sample filter of 7.6.2.1 over `p[0..=n]`, giving the
    /// value between `p[i]` and `p[i + 1]`. Taps beyond the block's `n + 1`
    /// samples are mirrored back into it, the edge sample repeated
    /// (`p[-1] = p[0]`, `p[-2] = p[1]`, `p[n + 1] = p[n]`, ...).
    #[inline]
    fn tap8_ref(p: &[u8], n: usize, i: usize, rc: i32) -> u8 {
        let at = |k: isize| -> i32 {
            let k = if k < 0 {
                -k - 1
            } else if k > n as isize {
                2 * n as isize + 1 - k
            } else {
                k
            };
            p[k as usize] as i32
        };
        let i = i as isize;
        let v = 20 * (at(i) + at(i + 1)) - 6 * (at(i - 1) + at(i + 2))
            + 3 * (at(i - 2) + at(i + 3))
            - (at(i - 3) + at(i + 4));
        ((v + 16 - rc) >> 5).clamp(0, 255) as u8
    }

    /// Quarter-sample luminance prediction (7.6.2.1) of a `bw` x `bh` block
    /// (16x16 for one vector per macroblock, 8x8 for four, 16x8 for a field)
    /// displaced by `(mvx, mvy)` quarter samples.
    ///
    /// The half-sample values come from the 8-tap filter over the block's
    /// `(bw + 1)` x `(bh + 1)` window of integer samples (horizontal first;
    /// the centre position filters the horizontal half samples vertically);
    /// the quarter-sample values are the bilinear average of the nearest
    /// integer / half samples, with `rounding_control`.
    #[allow(clippy::too_many_arguments)]
    fn qpel_ref(
        src: Src,
        x: i32,
        y: i32,
        mvx: i32,
        mvy: i32,
        bw: usize,
        bh: usize,
        rounding: bool,
        out: &mut [u8],
        out_stride: usize,
    ) {
        let (fx, fy) = ((mvx & 3) as usize, (mvy & 3) as usize);
        let ix = x + (mvx >> 2);
        let iy = y + (mvy >> 2);
        let (nw, nh) = (bw + 1, bh + 1);
        let mut full = [0u8; 17 * 17];
        fetch(src, ix, iy, nw, nh, &mut full);
        if fx == 0 && fy == 0 {
            for r in 0..bh {
                out[r * out_stride..r * out_stride + bw]
                    .copy_from_slice(&full[r * nw..r * nw + bw]);
            }
            return;
        }
        let rc = rounding as i32;
        // The half-sample grid, (2bw + 1) x (2bh + 1): even/even integer
        // samples, odd columns horizontal half samples, odd rows vertical ones.
        let g = 2 * bw + 1;
        let mut grid = [0u8; 33 * 33];
        let mut col = [0u8; 17];
        // Horizontal half samples of every window row.
        let mut hrows = [0u8; 17 * 16];
        for r in 0..nh {
            let row = &full[r * nw..r * nw + nw];
            for c in 0..bw {
                hrows[r * bw + c] = tap8_ref(row, bw, c, rc);
            }
            for c in 0..nw {
                grid[2 * r * g + 2 * c] = row[c];
            }
            for c in 0..bw {
                grid[2 * r * g + 2 * c + 1] = hrows[r * bw + c];
            }
        }
        // Vertical half samples of every window column.
        for c in 0..nw {
            for r in 0..nh {
                col[r] = full[r * nw + c];
            }
            for r in 0..bh {
                grid[(2 * r + 1) * g + 2 * c] = tap8_ref(&col, bh, r, rc);
            }
        }
        // Centre half samples: the horizontal ones, filtered vertically.
        for c in 0..bw {
            for r in 0..nh {
                col[r] = hrows[r * bw + c];
            }
            for r in 0..bh {
                grid[(2 * r + 1) * g + 2 * c + 1] = tap8_ref(&col, bh, r, rc);
            }
        }
        let rc = rc as u32;
        for r in 0..bh {
            let gy = 2 * r + fy / 2;
            for c in 0..bw {
                let gx = 2 * c + fx / 2;
                let a = grid[gy * g + gx] as u32;
                let v = match (fx & 1, fy & 1) {
                    (0, 0) => a,
                    (1, 0) => (a + grid[gy * g + gx + 1] as u32 + 1 - rc) >> 1,
                    (0, _) => (a + grid[(gy + 1) * g + gx] as u32 + 1 - rc) >> 1,
                    _ => {
                        let s = a
                            + grid[gy * g + gx + 1] as u32
                            + grid[(gy + 1) * g + gx] as u32
                            + grid[(gy + 1) * g + gx + 1] as u32;
                        (s + 2 - rc) >> 2
                    }
                };
                out[r * out_stride + c] = v as u8;
            }
        }
    }

    #[test]
    fn qpel_matches_the_reference() {
        let mut seed = 12345u32;
        let mut p = vec![0u8; 48 * 48];
        for v in p.iter_mut() {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            *v = (seed >> 16) as u8;
        }
        let src: Src = (&p, 48, 48, 48);
        for (bw, bh) in [(16, 16), (8, 8), (16, 8)] {
            for mx in -9..9 {
                for my in -9..9 {
                    for rounding in [false, true] {
                        let mut a = [0u8; 256];
                        let mut b = [0u8; 256];
                        qpel(src, 13, 17, mx, my, bw, bh, rounding, &mut a, 16);
                        qpel_ref(src, 13, 17, mx, my, bw, bh, rounding, &mut b, 16);
                        assert_eq!(a, b, "{bw}x{bh} ({mx}, {my}) rounding {rounding}");
                    }
                }
            }
        }
    }

    fn plane() -> (Vec<u8>, usize) {
        let w = 32;
        let v: Vec<u8> = (0..w * w)
            .map(|i| ((i * 7 + i / w * 13) % 251) as u8)
            .collect();
        (v, w)
    }

    #[test]
    fn chroma_vectors() {
        // Table 7-6: luma half samples to chroma.
        let t: Vec<i32> = (-5..=5).map(chroma_mv_1).collect();
        assert_eq!(t, [-3, -2, -1, -1, -1, 0, 1, 1, 1, 2, 3]);
        // Four equal vectors give what one does.
        for v in -40..=40 {
            assert_eq!(chroma_mv_4(4 * v), chroma_mv_1(v), "{v}");
        }
        assert_eq!(chroma_mv_4(2), 0);
        assert_eq!(chroma_mv_4(3), 1);
        assert_eq!(chroma_mv_4(14), 2);
        assert_eq!(chroma_mv_4(-14), -2);
    }

    #[test]
    fn halfpel_positions() {
        let (p, w) = plane();
        let src: Src = (&p, w, w as i32, w as i32);
        let mut o = [0u8; 64];
        halfpel(src, 4, 4, 0, 0, 8, 8, false, &mut o, 8);
        assert_eq!(o[0], p[4 * w + 4]);
        halfpel(src, 4, 4, 1, 0, 8, 8, false, &mut o, 8);
        assert_eq!(
            o[0] as u32,
            (p[4 * w + 4] as u32 + p[4 * w + 5] as u32 + 1) >> 1
        );
        halfpel(src, 4, 4, 1, 1, 8, 8, true, &mut o, 8);
        let s =
            p[4 * w + 4] as u32 + p[4 * w + 5] as u32 + p[5 * w + 4] as u32 + p[5 * w + 5] as u32;
        assert_eq!(o[0] as u32, (s + 1) >> 2);
        // Far outside: every sample is the corner.
        halfpel(src, 0, 0, -200, -200, 8, 8, false, &mut o, 8);
        assert!(o.iter().all(|&v| v == p[0]));
    }

    #[test]
    fn qpel_flat_and_integer() {
        let flat = vec![77u8; 32 * 32];
        let src: Src = (&flat, 32, 32, 32);
        let mut o = [0u8; 256];
        for mx in 0..4 {
            for my in 0..4 {
                qpel(src, 8, 8, mx, my, 16, 16, false, &mut o, 16);
                assert!(o.iter().all(|&v| v == 77), "{mx},{my}");
            }
        }
        let (p, w) = plane();
        let src: Src = (&p, w, w as i32, w as i32);
        qpel(src, 8, 8, 8, -4, 8, 8, false, &mut o, 8);
        assert_eq!(o[0], p[7 * w + 10]);
    }
}
