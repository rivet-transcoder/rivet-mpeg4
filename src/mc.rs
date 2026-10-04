//! Motion compensation (clause 7.6.2): half-sample bilinear interpolation,
//! quarter-sample interpolation (7.6.2.2: the 8-tap filter with block-edge
//! mirroring, horizontally and then vertically), the chrominance vector derivations, and unrestricted motion
//! vectors — every reference sample outside the reconstructed area takes
//! the value of the nearest sample on its boundary (the padding of 7.6.4;
//! see `Pic::ref_plane` for which boundary).

use crate::dsp::{self, Interp, Isa, QPEL_READ};
use crate::tables::CHROMA_ROUND_16;

/// A plane to predict from: `(samples, stride, valid width, valid height)`.
pub(crate) type Src<'a> = (&'a [u8], usize, i32, i32);

/// Copies the `ww` x `wh` window whose top-left sample is `(x, y)` into
/// `out` (rows `os` apart), clamping coordinates into the valid area.
#[inline]
fn fetch(src: Src, x: i32, y: i32, ww: usize, wh: usize, out: &mut [u8], os: usize) {
    let (plane, stride, pw, ph) = src;
    if x >= 0 && y >= 0 && x + ww as i32 <= pw && y + wh as i32 <= ph {
        for r in 0..wh {
            let s = (y as usize + r) * stride + x as usize;
            out[r * os..r * os + ww].copy_from_slice(&plane[s..s + ww]);
        }
        return;
    }
    // Columns left of the area take its first sample, columns right of
    // it its last, the rest are copied.
    let left = ((-x).max(0) as usize).min(ww);
    let right = ((x + ww as i32 - pw).max(0) as usize).min(ww - left);
    let mid = ww - left - right;
    for r in 0..wh {
        let row = &plane[(y + r as i32).clamp(0, ph - 1) as usize * stride..];
        let o = &mut out[r * os..r * os + ww];
        o[..left].fill(row[0]);
        if mid > 0 {
            let sx = (x + left as i32) as usize;
            o[left..left + mid].copy_from_slice(&row[sx..sx + mid]);
        }
        o[left + mid..].fill(row[pw as usize - 1]);
    }
}

/// `(samples, offset, stride)` of a window read in place.
type View<'a> = Option<(&'a [u8], usize, usize)>;

/// The window at `off` with rows `stride` apart, when its last row has
/// `read` readable bytes.
#[inline]
fn view_of(plane: &[u8], off: usize, stride: usize, wh: usize, read: usize) -> View<'_> {
    (off + (wh - 1) * stride + read <= plane.len()).then_some((plane, off, stride))
}

/// Where a prediction reads its reference samples from: a plane
/// ([`Src`]) or one field of one ([`Field`]).
pub(crate) trait Window<'a>: Copy {
    /// Copies the `ww` x `wh` window whose top-left sample is `(x, y)`
    /// into `out` (rows `os` apart), with the reference's padding outside
    /// it.
    fn fetch(self, x: i32, y: i32, ww: usize, wh: usize, out: &mut [u8], os: usize);

    /// The window in place, when it lies inside the reconstructed area
    /// (no padding needed) and its last row has `read` readable bytes
    /// (`read >= ww`).
    fn view(self, x: i32, y: i32, ww: usize, wh: usize, read: usize) -> View<'a>;
}

impl<'a> Window<'a> for Src<'a> {
    #[inline]
    fn fetch(self, x: i32, y: i32, ww: usize, wh: usize, out: &mut [u8], os: usize) {
        fetch(self, x, y, ww, wh, out, os)
    }

    #[inline]
    fn view(self, x: i32, y: i32, ww: usize, wh: usize, read: usize) -> View<'a> {
        let (plane, stride, pw, ph) = self;
        if x >= 0 && y >= 0 && x + ww as i32 <= pw && y + wh as i32 <= ph {
            view_of(plane, y as usize * stride + x as usize, stride, wh, read)
        } else {
            None
        }
    }
}

/// One field of a frame plane (`parity` 0 the top field's lines, 1 the
/// bottom's), for field prediction: row `r` of the field is line
/// `2r + parity` of the frame.
///
/// The padding is the frame's (7.6.4): a field line above or below the
/// reconstructed area takes the value of the frame's nearest line, so
/// below the picture the top field reads the frame's last line (a bottom
/// field line) and above it the bottom field reads the first (a top field
/// line) — not each field's own edge line.
#[derive(Clone, Copy)]
pub(crate) struct Field<'a> {
    pub frame: Src<'a>,
    pub parity: usize,
}

impl<'a> Window<'a> for Field<'a> {
    #[inline]
    fn fetch(self, x: i32, y: i32, ww: usize, wh: usize, out: &mut [u8], os: usize) {
        let (plane, stride, pw, ph) = self.frame;
        let par = self.parity as i32;
        for r in 0..wh {
            let line = (2 * (y + r as i32) + par).clamp(0, ph - 1) as usize;
            let row = &plane[line * stride..];
            let o = &mut out[r * os..r * os + ww];
            if x >= 0 && x + ww as i32 <= pw {
                o.copy_from_slice(&row[x as usize..x as usize + ww]);
            } else {
                for (c, v) in o.iter_mut().enumerate() {
                    *v = row[(x + c as i32).clamp(0, pw - 1) as usize];
                }
            }
        }
    }

    #[inline]
    fn view(self, x: i32, y: i32, ww: usize, wh: usize, read: usize) -> View<'a> {
        let (plane, stride, pw, ph) = self.frame;
        let top = 2 * y + self.parity as i32;
        if x >= 0 && x + ww as i32 <= pw && top >= 0 && top + 2 * (wh as i32 - 1) < ph {
            view_of(
                plane,
                top as usize * stride + x as usize,
                2 * stride,
                wh,
                read,
            )
        } else {
            None
        }
    }
}

/// Half-sample prediction of a `bw` x `bh` block at `(x, y)` displaced by
/// `(mvx, mvy)` half samples, with `rounding_control` (`vop_rounding_type`
/// in P- and S-VOPs, 0 in B-VOPs and the short video header).
#[allow(clippy::too_many_arguments)]
pub(crate) fn halfpel<'a>(
    src: impl Window<'a>,
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
    let a = Interp {
        bw,
        bh,
        fx: (mvx & 1) as usize,
        fy: (mvy & 1) as usize,
        rounding,
    };
    let isa = Isa::best();
    let (ww, wh) = (bw + 1, bh + 1);
    if let Some((p, off, s)) = src.view(ix, iy, ww, wh, ww) {
        dsp::halfpel_block(isa, p, off, s, a, out, out_stride);
    } else {
        let mut win = [0u8; 17 * 17];
        src.fetch(ix, iy, ww, wh, &mut win, ww);
        dsp::halfpel_block(isa, &win, 0, ww, a, out, out_stride);
    }
}

/// Quarter-sample luminance prediction (7.6.2.2) of a `bw` x `bh` block
/// (16x16 for one vector per macroblock, 8x8 for four, 16x8 for a field)
/// displaced by `(mvx, mvy)` quarter samples.
///
/// Separable, in two passes over the block's `(bw + 1)` x `(bh + 1)`
/// window of integer samples, each pass rounded (with `rounding_control`)
/// and clipped to eight bits before the next reads it:
///
/// 1. horizontally, every window row: the half-sample values from the
///    8-tap filter (taps mirrored at the window's edge), the quarter-sample
///    values the average of the integer and half-sample values either side
///    — giving each row at the vector's horizontal position;
/// 2. vertically, every column of those values the same way: the filter
///    for the half position, the average of the two nearest values for a
///    quarter position.
///
/// A horizontal quarter position combined with a vertical half or quarter
/// one is therefore the vertical filter (and average) of the horizontally
/// *quarter*-interpolated rows — not an average of half-sample values on a
/// two-dimensional grid. The two readings differ by one at about a third
/// of those samples; the two-pass one is what the standard describes and
/// what Xvid decodes (docs/CONFORMANCE.md).
#[allow(clippy::too_many_arguments)]
pub(crate) fn qpel<'a>(
    src: impl Window<'a>,
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
    let ix = x + (mvx >> 2);
    let iy = y + (mvy >> 2);
    let a = Interp {
        bw,
        bh,
        fx: (mvx & 3) as usize,
        fy: (mvy & 3) as usize,
        rounding,
    };
    let isa = Isa::best();
    let (ww, wh) = (bw + 1, bh + 1);
    if let Some((p, off, s)) = src.view(ix, iy, ww, wh, QPEL_READ) {
        dsp::qpel_block(isa, p, off, s, a, out, out_stride);
    } else {
        let mut win = [0u8; 17 * QPEL_READ];
        src.fetch(ix, iy, ww, wh, &mut win, QPEL_READ);
        dsp::qpel_block(isa, &win, 0, QPEL_READ, a, out, out_stride);
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
    /// The 8-tap half-sample filter of 7.6.2.2 over `p[0..=n]`, giving the
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

    /// Quarter-sample luminance prediction (7.6.2.2), sample by sample:
    /// each output sample's row is first interpolated horizontally (the
    /// 8-tap value at a half position, the average of the two nearest
    /// integer / half values at a quarter one), then those values of the
    /// window's rows are interpolated vertically the same way.
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
        fetch(src, ix, iy, nw, nh, &mut full, nw);
        let rc = rounding as i32;
        let avg = |a: u8, b: u8| ((a as i32 + b as i32 + 1 - rc) >> 1) as u8;
        // One dimension: the value at fraction `f` (quarters) after sample
        // `i` of the `n + 1` samples `p`.
        let interp = |p: &[u8], n: usize, i: usize, f: usize| -> u8 {
            match f {
                0 => p[i],
                1 => avg(p[i], tap8_ref(p, n, i, rc)),
                2 => tap8_ref(p, n, i, rc),
                _ => avg(tap8_ref(p, n, i, rc), p[i + 1]),
            }
        };
        let mut col = [0u8; 17];
        for c in 0..bw {
            for (r, v) in col[..nh].iter_mut().enumerate() {
                *v = interp(&full[r * nw..r * nw + nw], bw, c, fx);
            }
            for r in 0..bh {
                out[r * out_stride + c] = interp(&col, bh, r, fy);
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

    /// Field prediction pads like the frame (7.6.4): below the picture the
    /// top field reads the frame's last line, a bottom-field line; above
    /// it the bottom field reads the frame's first, a top-field line.
    #[test]
    fn field_windows_take_the_frame_padding() {
        // 4 x 8 frame, line r holding 10 * r + column.
        let p: Vec<u8> = (0..8u8)
            .flat_map(|r| (0..4u8).map(move |c| 10 * r + c))
            .collect();
        let frame: Src = (&p, 4, 4, 8);
        let mut out = [0u8; 4 * 6];
        // Top field rows 2..8: frame lines 4, 6, then 7 (the last) thrice.
        Field { frame, parity: 0 }.fetch(0, 2, 4, 6, &mut out, 4);
        let lines: Vec<u8> = out.chunks(4).map(|r| r[0] / 10).collect();
        assert_eq!(lines, [4, 6, 7, 7, 7, 7]);
        // Bottom field rows -2..4: frame line 0 (the first) twice, then 1, 3, 5, 7.
        Field { frame, parity: 1 }.fetch(0, -2, 4, 6, &mut out, 4);
        let lines: Vec<u8> = out.chunks(4).map(|r| r[0] / 10).collect();
        assert_eq!(lines, [0, 0, 1, 3, 5, 7]);
        // Columns clamp as in a frame.
        Field { frame, parity: 0 }.fetch(-2, 0, 4, 1, &mut out, 4);
        assert_eq!(&out[..4], &[0, 0, 0, 1]);
    }
}
