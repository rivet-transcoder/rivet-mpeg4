//! aarch64 NEON kernels (NEON is part of the aarch64 base architecture).
//! Each computes exactly what its scalar counterpart in `super::scalar`
//! does. The callers in `super` assert the bounds of every read and write
//! these functions document.

use std::arch::aarch64::*;

use super::Interp;
use super::scalar::{self, BASIS, mirror};

// ---------------------------------------------------------------------------
// 8x8 transforms, in 32-bit lanes
// ---------------------------------------------------------------------------
//
// The first pass is exact in 32 bits. The second pass's products need up
// to 40 bits, so its input `t` is split as `t = 256 * th + tl` and the
// sums `H = sum B th`, `L = sum B tl` recombined as
// `(H + ((L + 2^23) >> 8)) >> 16`, which equals the scalar
// `(256 H + L + 2^23) >> 24` (see `x86.rs`).

type M8 = [[int32x4_t; 2]; 8];

#[target_feature(enable = "neon")]
fn tr4(a: int32x4_t, b: int32x4_t, c: int32x4_t, d: int32x4_t) -> [int32x4_t; 4] {
    let t0 = vtrn1q_s32(a, b);
    let t1 = vtrn2q_s32(a, b);
    let t2 = vtrn1q_s32(c, d);
    let t3 = vtrn2q_s32(c, d);
    let w = vreinterpretq_s64_s32;
    let n = vreinterpretq_s32_s64;
    [
        n(vtrn1q_s64(w(t0), w(t2))),
        n(vtrn1q_s64(w(t1), w(t3))),
        n(vtrn2q_s64(w(t0), w(t2))),
        n(vtrn2q_s64(w(t1), w(t3))),
    ]
}

/// Transposes an 8x8 matrix of 32-bit values (`m[i][h]` lane `j` is
/// element `(i, 4 h + j)`).
#[target_feature(enable = "neon")]
fn transpose(m: &M8) -> M8 {
    let mut o = [[vdupq_n_s32(0); 2]; 8];
    for h in 0..2 {
        for hp in 0..2 {
            let q = tr4(
                m[4 * hp][h],
                m[4 * hp + 1][h],
                m[4 * hp + 2][h],
                m[4 * hp + 3][h],
            );
            for (i, v) in q.into_iter().enumerate() {
                o[4 * h + i][hp] = v;
            }
        }
    }
    o
}

/// `y[n] = sum_k BASIS[k][n] v[k]`, even / odd split.
#[target_feature(enable = "neon")]
fn idct_pass(v: &M8) -> M8 {
    let mut y = [[vdupq_n_s32(0); 2]; 8];
    for n in 0..4 {
        for h in 0..2 {
            let mut e = vmulq_n_s32(v[0][h], BASIS[0][n]);
            e = vmlaq_n_s32(e, v[2][h], BASIS[2][n]);
            e = vmlaq_n_s32(e, v[4][h], BASIS[4][n]);
            e = vmlaq_n_s32(e, v[6][h], BASIS[6][n]);
            let mut o = vmulq_n_s32(v[1][h], BASIS[1][n]);
            o = vmlaq_n_s32(o, v[3][h], BASIS[3][n]);
            o = vmlaq_n_s32(o, v[5][h], BASIS[5][n]);
            o = vmlaq_n_s32(o, v[7][h], BASIS[7][n]);
            y[n][h] = vaddq_s32(e, o);
            y[7 - n][h] = vsubq_s32(e, o);
        }
    }
    y
}

/// `y[k] = sum_{n < 4} BASIS[k][n] (s or d)[n]`.
#[target_feature(enable = "neon")]
fn fdct_pass(s: &[[int32x4_t; 2]; 4], d: &[[int32x4_t; 2]; 4]) -> M8 {
    let mut y = [[vdupq_n_s32(0); 2]; 8];
    for (k, yk) in y.iter_mut().enumerate() {
        let v = if k & 1 == 0 { s } else { d };
        for h in 0..2 {
            let mut a = vmulq_n_s32(v[0][h], BASIS[k][0]);
            a = vmlaq_n_s32(a, v[1][h], BASIS[k][1]);
            a = vmlaq_n_s32(a, v[2][h], BASIS[k][2]);
            yk[h] = vmlaq_n_s32(a, v[3][h], BASIS[k][3]);
        }
    }
    y
}

#[target_feature(enable = "neon")]
fn combine(h: int32x4_t, l: int32x4_t) -> int32x4_t {
    let r = vshrq_n_s32::<8>(vaddq_s32(l, vdupq_n_s32(1 << 23)));
    vshrq_n_s32::<16>(vaddq_s32(h, r))
}

/// Loads the block as 32-bit lanes; `None` when a value is outside
/// [-2048, 2047].
#[target_feature(enable = "neon")]
fn load(b: &[i16; 64]) -> Option<M8> {
    // SAFETY: eight 16-byte loads covering the 128 bytes of `b`.
    let rows: [int16x8_t; 8] = std::array::from_fn(|r| unsafe { vld1q_s16(b.as_ptr().add(8 * r)) });
    let mut mx = rows[0];
    let mut mn = rows[0];
    for v in &rows[1..] {
        mx = vmaxq_s16(mx, *v);
        mn = vminq_s16(mn, *v);
    }
    if vmaxvq_s16(mx) > 2047 || vminvq_s16(mn) < -2048 {
        return None;
    }
    Some(std::array::from_fn(|r| {
        [vmovl_s16(vget_low_s16(rows[r])), vmovl_high_s16(rows[r])]
    }))
}

#[target_feature(enable = "neon")]
fn store(b: &mut [i16; 64], r: usize, v: int16x8_t) {
    // SAFETY: row `r` (< 8) of `b`: 16 bytes.
    unsafe { vst1q_s16(b.as_mut_ptr().add(8 * r), v) }
}

#[target_feature(enable = "neon")]
fn split(t: &M8) -> (M8, M8) {
    let m = vdupq_n_s32(255);
    let hi = std::array::from_fn(|i| [vshrq_n_s32::<8>(t[i][0]), vshrq_n_s32::<8>(t[i][1])]);
    let lo = std::array::from_fn(|i| [vandq_s32(t[i][0], m), vandq_s32(t[i][1], m)]);
    (hi, lo)
}

#[target_feature(enable = "neon")]
pub(super) fn idct(b: &mut [i16; 64]) {
    let Some(x) = load(b) else {
        return scalar::idct(b);
    };
    let y = idct_pass(&transpose(&x));
    let r = vdupq_n_s32(128);
    let t: M8 = std::array::from_fn(|n| {
        [
            vshrq_n_s32::<8>(vaddq_s32(y[n][0], r)),
            vshrq_n_s32::<8>(vaddq_s32(y[n][1], r)),
        ]
    });
    let (th, tl) = split(&transpose(&t));
    let hh = idct_pass(&th);
    let ll = idct_pass(&tl);
    for n in 0..8 {
        let lo = vqmovn_s32(combine(hh[n][0], ll[n][0]));
        let hi = vqmovn_s32(combine(hh[n][1], ll[n][1]));
        store(b, n, vcombine_s16(lo, hi));
    }
}

#[target_feature(enable = "neon")]
pub(super) fn fdct(b: &mut [i16; 64]) {
    let Some(x) = load(b) else {
        return scalar::fdct(b);
    };
    let xt = transpose(&x);
    let sd = |v: &M8| -> ([[int32x4_t; 2]; 4], [[int32x4_t; 2]; 4]) {
        (
            std::array::from_fn(|n| {
                [
                    vaddq_s32(v[n][0], v[7 - n][0]),
                    vaddq_s32(v[n][1], v[7 - n][1]),
                ]
            }),
            std::array::from_fn(|n| {
                [
                    vsubq_s32(v[n][0], v[7 - n][0]),
                    vsubq_s32(v[n][1], v[7 - n][1]),
                ]
            }),
        )
    };
    let (s, d) = sd(&xt);
    let y = fdct_pass(&s, &d);
    let r = vdupq_n_s32(128);
    let t: M8 = std::array::from_fn(|k| {
        [
            vshrq_n_s32::<8>(vaddq_s32(y[k][0], r)),
            vshrq_n_s32::<8>(vaddq_s32(y[k][1], r)),
        ]
    });
    let (s, d) = sd(&transpose(&t));
    let mut s8 = [[vdupq_n_s32(0); 2]; 8];
    let mut d8 = [[vdupq_n_s32(0); 2]; 8];
    s8[..4].copy_from_slice(&s);
    d8[..4].copy_from_slice(&d);
    let (sh, sl) = split(&s8);
    let (dh, dl) = split(&d8);
    let q = |m: &M8| -> [[int32x4_t; 2]; 4] { [m[0], m[1], m[2], m[3]] };
    let hh = fdct_pass(&q(&sh), &q(&dh));
    let ll = fdct_pass(&q(&sl), &q(&dl));
    let (lo, hi) = (vdupq_n_s16(-2048), vdupq_n_s16(2047));
    for k in 0..8 {
        let v = vcombine_s16(
            vqmovn_s32(combine(hh[k][0], ll[k][0])),
            vqmovn_s32(combine(hh[k][1], ll[k][1])),
        );
        store(b, k, vminq_s16(vmaxq_s16(v, lo), hi));
    }
}

// ---------------------------------------------------------------------------
// Interpolation
// ---------------------------------------------------------------------------

/// `vqtbl2q` indices of tap `t` for the 16 (or 8) outputs of a row of
/// `n + 1` samples: the mirrored sample each output's tap reads.
const fn vtab(n: isize, t: isize) -> [u8; 16] {
    let mut v = [0u8; 16];
    let mut j = 0;
    while j < n {
        v[j as usize] = mirror(j + t - 3, n) as u8;
        j += 1;
    }
    v
}

const fn vtabs(n: isize) -> [[u8; 16]; 8] {
    [
        vtab(n, 0),
        vtab(n, 1),
        vtab(n, 2),
        vtab(n, 3),
        vtab(n, 4),
        vtab(n, 5),
        vtab(n, 6),
        vtab(n, 7),
    ]
}

const T16: [[u8; 16]; 8] = vtabs(16);
const T8: [[u8; 16]; 8] = vtabs(8);

/// `(a + b + 1 - rc) >> 1`.
#[target_feature(enable = "neon")]
fn avg(a: uint8x16_t, b: uint8x16_t, rc: bool) -> uint8x16_t {
    if rc {
        vhaddq_u8(a, b)
    } else {
        vrhaddq_u8(a, b)
    }
}

/// The 8-tap filter over taps `v[0..8]` (16 lanes), rounded and clipped.
#[target_feature(enable = "neon")]
fn filt(v: &[uint8x16_t; 8], rnd: int16x8_t) -> uint8x16_t {
    let half = |lo: bool| -> uint8x8_t {
        let add = |a: uint8x16_t, b: uint8x16_t| {
            if lo {
                vaddl_u8(vget_low_u8(a), vget_low_u8(b))
            } else {
                vaddl_high_u8(a, b)
            }
        };
        // Wrapping 16-bit arithmetic: the true value fits in i16.
        let mut s = vmulq_n_u16(add(v[3], v[4]), 20);
        s = vmlsq_n_u16(s, add(v[2], v[5]), 6);
        s = vmlaq_n_u16(s, add(v[1], v[6]), 3);
        s = vsubq_u16(s, add(v[0], v[7]));
        let s = vaddq_s16(vreinterpretq_s16_u16(s), rnd);
        vqmovun_s16(vshrq_n_s16::<5>(s))
    };
    vcombine_u8(half(true), half(false))
}

#[target_feature(enable = "neon")]
fn ld16(p: &[u8], i: usize) -> uint8x16_t {
    assert!(i + 16 <= p.len());
    // SAFETY: 16 bytes at `i`, in bounds (asserted).
    unsafe { vld1q_u8(p.as_ptr().add(i)) }
}

/// Loads `bw` (8 or 16) bytes, the rest zero.
#[target_feature(enable = "neon")]
fn ldw(p: &[u8], i: usize, bw: usize) -> uint8x16_t {
    if bw == 16 {
        ld16(p, i)
    } else {
        assert!(i + 8 <= p.len());
        // SAFETY: 8 bytes at `i`, in bounds (asserted).
        vcombine_u8(unsafe { vld1_u8(p.as_ptr().add(i)) }, vdup_n_u8(0))
    }
}

#[target_feature(enable = "neon")]
fn stw(p: &mut [u8], i: usize, v: uint8x16_t, bw: usize) {
    if bw == 16 {
        assert!(i + 16 <= p.len());
        // SAFETY: 16 bytes at `i`, in bounds (asserted).
        unsafe { vst1q_u8(p.as_mut_ptr().add(i), v) }
    } else {
        assert!(i + 8 <= p.len());
        // SAFETY: 8 bytes at `i`, in bounds (asserted).
        unsafe { vst1_u8(p.as_mut_ptr().add(i), vget_low_u8(v)) }
    }
}

#[target_feature(enable = "neon")]
pub(super) fn qpel(win: &[u8], off: usize, ws: usize, a: Interp, out: &mut [u8], os: usize) {
    let Interp {
        bw,
        bh,
        fx,
        fy,
        rounding,
    } = a;
    if fx == 0 && fy == 0 {
        for r in 0..bh {
            stw(out, r * os, ldw(win, off + r * ws, bw), bw);
        }
        return;
    }
    let rnd = vdupq_n_s16(16 - rounding as i16);
    let tabs = if bw == 16 { &T16 } else { &T8 };
    let mut hq = [0u8; 17 * 16];
    for r in 0..=bh {
        let p = off + r * ws;
        let row = ld16(win, p);
        let v = if fx == 0 {
            row
        } else {
            let tbl = uint8x16x2_t(row, ld16(win, p + 16));
            let taps: [uint8x16_t; 8] = std::array::from_fn(|t| {
                // SAFETY: a 16-byte constant table.
                let idx = unsafe { vld1q_u8(tabs[t].as_ptr()) };
                vqtbl2q_u8(tbl, idx)
            });
            let half = filt(&taps, rnd);
            match fx {
                1 => avg(row, half, rounding),
                2 => half,
                _ => avg(half, ld16(win, p + 1), rounding),
            }
        };
        stw(&mut hq, r * 16, v, 16);
    }
    for r in 0..bh {
        let cur = ld16(&hq, r * 16);
        let v = if fy == 0 {
            cur
        } else {
            let taps: [uint8x16_t; 8] = std::array::from_fn(|t| {
                ld16(
                    &hq,
                    16 * mirror(r as isize + t as isize - 3, bh as isize) as usize,
                )
            });
            let half = filt(&taps, rnd);
            match fy {
                1 => avg(cur, half, rounding),
                2 => half,
                _ => avg(half, ld16(&hq, (r + 1) * 16), rounding),
            }
        };
        stw(out, r * os, v, bw);
    }
}

#[target_feature(enable = "neon")]
pub(super) fn halfpel(win: &[u8], off: usize, ws: usize, a: Interp, out: &mut [u8], os: usize) {
    let Interp {
        bw,
        bh,
        fx,
        fy,
        rounding,
    } = a;
    let row = |r: usize, d: usize| ldw(win, off + r * ws + d, bw);
    match (fx, fy) {
        (0, 0) => {
            for r in 0..bh {
                stw(out, r * os, row(r, 0), bw);
            }
        }
        (1, 0) => {
            for r in 0..bh {
                stw(out, r * os, avg(row(r, 0), row(r, 1), rounding), bw);
            }
        }
        (0, _) => {
            for r in 0..bh {
                stw(out, r * os, avg(row(r, 0), row(r + 1, 0), rounding), bw);
            }
        }
        _ => {
            let bias = vdupq_n_u16(2 - rounding as u16);
            let sums = |r: usize| {
                let (x, y) = (row(r, 0), row(r, 1));
                (
                    vaddl_u8(vget_low_u8(x), vget_low_u8(y)),
                    vaddl_high_u8(x, y),
                )
            };
            let mut prev = sums(0);
            for r in 0..bh {
                let next = sums(r + 1);
                let lo = vshrn_n_u16::<2>(vaddq_u16(vaddq_u16(prev.0, next.0), bias));
                let hi = vshrn_n_u16::<2>(vaddq_u16(vaddq_u16(prev.1, next.1), bias));
                stw(out, r * os, vcombine_u8(lo, hi), bw);
                prev = next;
            }
        }
    }
}

#[target_feature(enable = "neon")]
#[allow(clippy::too_many_arguments)]
pub(super) fn sad(
    a: &[u8],
    ao: usize,
    as_: usize,
    b: &[u8],
    bo: usize,
    bs: usize,
    w: usize,
    h: usize,
) -> u32 {
    // 16-bit lane sums: at most 64 rows of two differences of 255.
    let mut acc = vdupq_n_u16(0);
    for r in 0..h {
        let x = ldw(a, ao + r * as_, w);
        let y = ldw(b, bo + r * bs, w);
        acc = vpadalq_u8(acc, vabdq_u8(x, y));
    }
    vaddlvq_u16(acc)
}
