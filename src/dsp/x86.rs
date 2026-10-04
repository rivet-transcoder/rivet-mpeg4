//! x86-64 kernels: SSE2 / SSSE3 / SSE4.1 intrinsics, compiled once for
//! SSE4.1 and once for AVX2 (the same instructions, VEX-encoded). Each
//! computes exactly what its scalar counterpart in `super::scalar` does.
//!
//! The entry points are `unsafe`: the caller guarantees the CPU has the
//! features they are compiled for, and that the reads and writes their
//! documentation describes are in bounds (`super` asserts both before
//! calling).

use std::arch::x86_64::*;

use super::Interp;
use super::scalar::{self, BASIS, mirror};

// ---------------------------------------------------------------------------
// 8x8 transforms
// ---------------------------------------------------------------------------
//
// Both passes are computed with `pmaddwd` on pairs of 16-bit inputs. The
// first pass is exact in 32 bits. The second pass's products need up to 40
// bits, so its input `t` is split as `t = 256 * th + tl` (`th = t >> 8`,
// `tl = t & 255`, both 16-bit) and the two sums `H = sum B th`,
// `L = sum B tl` (each within 32 bits for the coefficient ranges checked
// on entry) recombined: the scalar `(256 H + L + 2^23) >> 24` equals
// `(H + ((L + 2^23) >> 8)) >> 16` exactly, because the low eight bits of
// `256 H` are zero.

/// `(BASIS[k0][n], BASIS[k1][n])` as one 32-bit lane of 16-bit pairs.
#[inline(always)]
fn pair_i(k0: usize, k1: usize, n: usize) -> i32 {
    (BASIS[k1][n] << 16) | (BASIS[k0][n] & 0xffff)
}

/// `(BASIS[k][n0], BASIS[k][n1])`, for the forward transform.
#[inline(always)]
fn pair_f(k: usize, n0: usize, n1: usize) -> i32 {
    (BASIS[k][n1] << 16) | (BASIS[k][n0] & 0xffff)
}

/// Transposes an 8x8 matrix of 16-bit values (`r[i]` lane `j` is element
/// `(i, j)`).
#[inline(always)]
unsafe fn transpose8(r: [__m128i; 8]) -> [__m128i; 8] {
    // SAFETY: SSE2 only, which every x86-64 CPU has.
    unsafe {
        let a0 = _mm_unpacklo_epi16(r[0], r[1]);
        let a1 = _mm_unpackhi_epi16(r[0], r[1]);
        let a2 = _mm_unpacklo_epi16(r[2], r[3]);
        let a3 = _mm_unpackhi_epi16(r[2], r[3]);
        let a4 = _mm_unpacklo_epi16(r[4], r[5]);
        let a5 = _mm_unpackhi_epi16(r[4], r[5]);
        let a6 = _mm_unpacklo_epi16(r[6], r[7]);
        let a7 = _mm_unpackhi_epi16(r[6], r[7]);
        let b0 = _mm_unpacklo_epi32(a0, a2);
        let b1 = _mm_unpackhi_epi32(a0, a2);
        let b2 = _mm_unpacklo_epi32(a1, a3);
        let b3 = _mm_unpackhi_epi32(a1, a3);
        let b4 = _mm_unpacklo_epi32(a4, a6);
        let b5 = _mm_unpackhi_epi32(a4, a6);
        let b6 = _mm_unpacklo_epi32(a5, a7);
        let b7 = _mm_unpackhi_epi32(a5, a7);
        [
            _mm_unpacklo_epi64(b0, b4),
            _mm_unpackhi_epi64(b0, b4),
            _mm_unpacklo_epi64(b1, b5),
            _mm_unpackhi_epi64(b1, b5),
            _mm_unpacklo_epi64(b2, b6),
            _mm_unpackhi_epi64(b2, b6),
            _mm_unpacklo_epi64(b3, b7),
            _mm_unpackhi_epi64(b3, b7),
        ]
    }
}

/// Transposes four rows of four 32-bit values.
#[inline(always)]
unsafe fn transpose4(r0: __m128i, r1: __m128i, r2: __m128i, r3: __m128i) -> [__m128i; 4] {
    // SAFETY: SSE2 only.
    unsafe {
        let t0 = _mm_unpacklo_epi32(r0, r1);
        let t1 = _mm_unpacklo_epi32(r2, r3);
        let t2 = _mm_unpackhi_epi32(r0, r1);
        let t3 = _mm_unpackhi_epi32(r2, r3);
        [
            _mm_unpacklo_epi64(t0, t1),
            _mm_unpackhi_epi64(t0, t1),
            _mm_unpacklo_epi64(t2, t3),
            _mm_unpackhi_epi64(t2, t3),
        ]
    }
}

/// One inverse pass over eight vectors `v[k]` (16-bit lanes): `y[n] =
/// sum_k BASIS[k][n] v[k]`, as 32-bit lanes 0-3 (`[n][0]`) and 4-7
/// (`[n][1]`).
#[inline(always)]
unsafe fn idct_pass(v: &[__m128i; 8]) -> [[__m128i; 2]; 8] {
    // SAFETY: SSE2 only.
    unsafe {
        let p02 = [
            _mm_unpacklo_epi16(v[0], v[2]),
            _mm_unpackhi_epi16(v[0], v[2]),
        ];
        let p46 = [
            _mm_unpacklo_epi16(v[4], v[6]),
            _mm_unpackhi_epi16(v[4], v[6]),
        ];
        let p13 = [
            _mm_unpacklo_epi16(v[1], v[3]),
            _mm_unpackhi_epi16(v[1], v[3]),
        ];
        let p57 = [
            _mm_unpacklo_epi16(v[5], v[7]),
            _mm_unpackhi_epi16(v[5], v[7]),
        ];
        let mut y = [[_mm_setzero_si128(); 2]; 8];
        for n in 0..4 {
            let c02 = _mm_set1_epi32(pair_i(0, 2, n));
            let c46 = _mm_set1_epi32(pair_i(4, 6, n));
            let c13 = _mm_set1_epi32(pair_i(1, 3, n));
            let c57 = _mm_set1_epi32(pair_i(5, 7, n));
            for h in 0..2 {
                let e = _mm_add_epi32(_mm_madd_epi16(p02[h], c02), _mm_madd_epi16(p46[h], c46));
                let o = _mm_add_epi32(_mm_madd_epi16(p13[h], c13), _mm_madd_epi16(p57[h], c57));
                y[n][h] = _mm_add_epi32(e, o);
                y[7 - n][h] = _mm_sub_epi32(e, o);
            }
        }
        y
    }
}

/// Whether every lane of the eight rows lies in [-2048, 2047], the range
/// within which the SIMD transforms are exact.
#[inline(always)]
unsafe fn in_range(x: &[__m128i; 8]) -> bool {
    // SAFETY: SSE2 only.
    unsafe {
        let mut mx = x[0];
        let mut mn = x[0];
        for v in &x[1..] {
            mx = _mm_max_epi16(mx, *v);
            mn = _mm_min_epi16(mn, *v);
        }
        let over = _mm_or_si128(
            _mm_cmpgt_epi16(mx, _mm_set1_epi16(2047)),
            _mm_cmplt_epi16(mn, _mm_set1_epi16(-2048)),
        );
        _mm_movemask_epi8(over) == 0
    }
}

/// Splits 32-bit `t` (two halves) into `t >> 8` and `t & 255`, each packed
/// to eight 16-bit lanes.
#[inline(always)]
unsafe fn split(t: [__m128i; 2]) -> (__m128i, __m128i) {
    // SAFETY: SSE2 only.
    unsafe {
        let m = _mm_set1_epi32(255);
        (
            _mm_packs_epi32(_mm_srai_epi32(t[0], 8), _mm_srai_epi32(t[1], 8)),
            _mm_packs_epi32(_mm_and_si128(t[0], m), _mm_and_si128(t[1], m)),
        )
    }
}

/// `(H + ((L + 2^23) >> 8)) >> 16`: the second pass's result.
#[inline(always)]
unsafe fn combine(h: __m128i, l: __m128i) -> __m128i {
    // SAFETY: SSE2 only.
    unsafe {
        let r = _mm_srai_epi32(_mm_add_epi32(l, _mm_set1_epi32(1 << 23)), 8);
        _mm_srai_epi32(_mm_add_epi32(h, r), 16)
    }
}

/// The inverse transform; false (leaving the block untouched) when a
/// coefficient is outside [-2048, 2047].
#[inline(always)]
unsafe fn idct_body(b: &mut [i16; 64]) -> bool {
    // SAFETY: SSE2 only; the eight 16-byte loads and stores are the 128
    // bytes of `b`.
    unsafe {
        let p = b.as_mut_ptr() as *mut __m128i;
        let x: [__m128i; 8] = std::array::from_fn(|r| _mm_loadu_si128(p.add(r)));
        if !in_range(&x) {
            return false;
        }
        // Rows: lanes are the block's rows after the transpose.
        let y = idct_pass(&transpose8(x));
        let r128 = _mm_set1_epi32(128);
        let mut th = [_mm_setzero_si128(); 8];
        let mut tl = [_mm_setzero_si128(); 8];
        for n in 0..8 {
            let t = [
                _mm_srai_epi32(_mm_add_epi32(y[n][0], r128), 8),
                _mm_srai_epi32(_mm_add_epi32(y[n][1], r128), 8),
            ];
            (th[n], tl[n]) = split(t);
        }
        // Columns: lanes are the block's columns.
        let hh = idct_pass(&transpose8(th));
        let ll = idct_pass(&transpose8(tl));
        for n in 0..8 {
            let lo = combine(hh[n][0], ll[n][0]);
            let hi = combine(hh[n][1], ll[n][1]);
            _mm_storeu_si128(p.add(n), _mm_packs_epi32(lo, hi));
        }
        true
    }
}

/// One forward pass over eight vectors `x[n]` (16-bit lanes): `y[k] =
/// sum_n BASIS[k][n] x[n]` with the even / odd split.
#[inline(always)]
unsafe fn fdct_pass_s(s: &[__m128i; 4], d: &[__m128i; 4]) -> [[__m128i; 2]; 8] {
    // SAFETY: SSE2 only.
    unsafe {
        let s01 = [
            _mm_unpacklo_epi16(s[0], s[1]),
            _mm_unpackhi_epi16(s[0], s[1]),
        ];
        let s23 = [
            _mm_unpacklo_epi16(s[2], s[3]),
            _mm_unpackhi_epi16(s[2], s[3]),
        ];
        let d01 = [
            _mm_unpacklo_epi16(d[0], d[1]),
            _mm_unpackhi_epi16(d[0], d[1]),
        ];
        let d23 = [
            _mm_unpacklo_epi16(d[2], d[3]),
            _mm_unpackhi_epi16(d[2], d[3]),
        ];
        let mut y = [[_mm_setzero_si128(); 2]; 8];
        for (k, yk) in y.iter_mut().enumerate() {
            let (v01, v23) = if k & 1 == 0 {
                (&s01, &s23)
            } else {
                (&d01, &d23)
            };
            let c01 = _mm_set1_epi32(pair_f(k, 0, 1));
            let c23 = _mm_set1_epi32(pair_f(k, 2, 3));
            for h in 0..2 {
                yk[h] = _mm_add_epi32(_mm_madd_epi16(v01[h], c01), _mm_madd_epi16(v23[h], c23));
            }
        }
        y
    }
}

/// The forward transform; false (leaving the block untouched) when a
/// sample is outside [-2048, 2047].
#[inline(always)]
unsafe fn fdct_body(b: &mut [i16; 64]) -> bool {
    // SAFETY: SSE2 only; the loads and stores are the 128 bytes of `b`.
    unsafe {
        let p = b.as_mut_ptr() as *mut __m128i;
        let x: [__m128i; 8] = std::array::from_fn(|r| _mm_loadu_si128(p.add(r)));
        if !in_range(&x) {
            return false;
        }
        let xt = transpose8(x);
        let s: [__m128i; 4] = std::array::from_fn(|n| _mm_add_epi16(xt[n], xt[7 - n]));
        let d: [__m128i; 4] = std::array::from_fn(|n| _mm_sub_epi16(xt[n], xt[7 - n]));
        let y = fdct_pass_s(&s, &d);
        // t[k] (lanes: the block's rows) = (y + 128) >> 8.
        let r128 = _mm_set1_epi32(128);
        let t: [[__m128i; 2]; 8] = std::array::from_fn(|k| {
            [
                _mm_srai_epi32(_mm_add_epi32(y[k][0], r128), 8),
                _mm_srai_epi32(_mm_add_epi32(y[k][1], r128), 8),
            ]
        });
        // Transpose to rows of the intermediate block (lanes: columns).
        let mut tr = [[_mm_setzero_si128(); 2]; 8];
        for (h_in, rows) in [(0usize, 0usize), (1, 4)] {
            for (h_out, ks) in [(0usize, 0usize), (1, 4)] {
                let q = transpose4(
                    t[ks][h_in],
                    t[ks + 1][h_in],
                    t[ks + 2][h_in],
                    t[ks + 3][h_in],
                );
                for i in 0..4 {
                    tr[rows + i][h_out] = q[i];
                }
            }
        }
        let mut sh = [_mm_setzero_si128(); 4];
        let mut sl = [_mm_setzero_si128(); 4];
        let mut dh = [_mm_setzero_si128(); 4];
        let mut dl = [_mm_setzero_si128(); 4];
        for n in 0..4 {
            let sv = [
                _mm_add_epi32(tr[n][0], tr[7 - n][0]),
                _mm_add_epi32(tr[n][1], tr[7 - n][1]),
            ];
            let dv = [
                _mm_sub_epi32(tr[n][0], tr[7 - n][0]),
                _mm_sub_epi32(tr[n][1], tr[7 - n][1]),
            ];
            (sh[n], sl[n]) = split(sv);
            (dh[n], dl[n]) = split(dv);
        }
        let hh = fdct_pass_s(&sh, &dh);
        let ll = fdct_pass_s(&sl, &dl);
        let (lo, hi) = (_mm_set1_epi16(-2048), _mm_set1_epi16(2047));
        for k in 0..8 {
            let v = _mm_packs_epi32(combine(hh[k][0], ll[k][0]), combine(hh[k][1], ll[k][1]));
            _mm_storeu_si128(p.add(k), _mm_min_epi16(_mm_max_epi16(v, lo), hi));
        }
        true
    }
}

/// # Safety
/// The CPU has SSE4.1 and SSSE3.
#[target_feature(enable = "sse4.1,ssse3")]
pub(super) unsafe fn idct_sse41(b: &mut [i16; 64]) {
    // SAFETY: the features are enabled here.
    if !unsafe { idct_body(b) } {
        scalar::idct(b)
    }
}

/// # Safety
/// The CPU has AVX2.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn idct_avx2(b: &mut [i16; 64]) {
    // SAFETY: the features are enabled here.
    if !unsafe { idct_body(b) } {
        scalar::idct(b)
    }
}

/// # Safety
/// The CPU has SSE4.1 and SSSE3.
#[target_feature(enable = "sse4.1,ssse3")]
pub(super) unsafe fn fdct_sse41(b: &mut [i16; 64]) {
    // SAFETY: the features are enabled here.
    if !unsafe { fdct_body(b) } {
        scalar::fdct(b)
    }
}

/// # Safety
/// The CPU has AVX2.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn fdct_avx2(b: &mut [i16; 64]) {
    // SAFETY: the features are enabled here.
    if !unsafe { fdct_body(b) } {
        scalar::fdct(b)
    }
}

// ---------------------------------------------------------------------------
// Interpolation
// ---------------------------------------------------------------------------

/// The 8-tap filter's coefficients in pairs of taps (`pmaddubsw`).
const TAPS: [[i8; 2]; 4] = [[-1, 3], [-6, 20], [20, -6], [3, -1]];

/// `pshufb` indices for eight outputs `8 g .. 8 g + 8` of the horizontal
/// filter over a row of `n + 1` samples, tap pair `pair`: bytes `2 j` and
/// `2 j + 1` are the (mirrored) samples taps `2 pair` and `2 pair + 1` of
/// output `8 g + j` read, relative to `base`.
const fn htab(n: isize, g: isize, pair: isize, base: isize) -> [i8; 16] {
    let mut t = [0i8; 16];
    let mut j = 0;
    while j < 8 {
        let i = 8 * g + j;
        t[2 * j as usize] = (mirror(i + 2 * pair - 3, n) - base) as i8;
        t[2 * j as usize + 1] = (mirror(i + 2 * pair - 2, n) - base) as i8;
        j += 1;
    }
    t
}

const fn htabs(n: isize, g: isize, base: isize) -> [[i8; 16]; 4] {
    [
        htab(n, g, 0, base),
        htab(n, g, 1, base),
        htab(n, g, 2, base),
        htab(n, g, 3, base),
    ]
}

/// Rows of 16: outputs 0-7 read samples 0-11, outputs 8-15 samples 5-16.
const H16: [[[i8; 16]; 4]; 2] = [htabs(16, 0, 0), htabs(16, 1, 5)];
/// Rows of 8: samples 0-8.
const H8: [[i8; 16]; 4] = htabs(8, 0, 0);

#[inline(always)]
unsafe fn ld(p: *const u8) -> __m128i {
    // SAFETY: the caller's pointer has 16 readable bytes.
    unsafe { _mm_loadu_si128(p as *const __m128i) }
}

/// `(a + b + 1 - rc) >> 1` per byte (`rcm` all `rc`).
#[inline(always)]
unsafe fn avg(a: __m128i, b: __m128i, rcm: __m128i) -> __m128i {
    // SAFETY: SSE2 only.
    unsafe { _mm_sub_epi8(_mm_avg_epu8(a, b), _mm_and_si128(_mm_xor_si128(a, b), rcm)) }
}

/// Eight horizontal filter outputs from the 16 samples `a`, with the
/// tables `t`: `(sum + 16 - rc) >> 5` as 16-bit lanes.
#[inline(always)]
unsafe fn hfilt(a: __m128i, t: &[[i8; 16]; 4], coef: &[__m128i; 4], rnd: __m128i) -> __m128i {
    // SAFETY: SSSE3 (the caller's target features); the table loads are
    // 16-byte arrays.
    unsafe {
        let mut s = _mm_setzero_si128();
        for p in 0..4 {
            let idx = _mm_loadu_si128(t[p].as_ptr() as *const __m128i);
            s = _mm_add_epi16(s, _mm_maddubs_epi16(_mm_shuffle_epi8(a, idx), coef[p]));
        }
        _mm_srai_epi16(_mm_add_epi16(s, rnd), 5)
    }
}

/// Stores the first `bw` (8 or 16) bytes of `v` at `p`.
#[inline(always)]
unsafe fn st(p: *mut u8, v: __m128i, bw: usize) {
    // SAFETY: the caller's pointer has `bw` writable bytes.
    unsafe {
        if bw == 16 {
            _mm_storeu_si128(p as *mut __m128i, v)
        } else {
            _mm_storel_epi64(p as *mut __m128i, v)
        }
    }
}

/// Loads `bw` (8 or 16) bytes from `p`.
#[inline(always)]
unsafe fn ldw(p: *const u8, bw: usize) -> __m128i {
    // SAFETY: the caller's pointer has `bw` readable bytes.
    unsafe {
        if bw == 16 {
            _mm_loadu_si128(p as *const __m128i)
        } else {
            _mm_loadl_epi64(p as *const __m128i)
        }
    }
}

#[inline(always)]
unsafe fn qpel_body(win: &[u8], off: usize, ws: usize, a: Interp, out: &mut [u8], os: usize) {
    let Interp { bw, bh, fx, fy, .. } = a;
    let rc = a.rounding as i32;
    // SAFETY: SSSE3 / SSE4.1 (the callers' target features). The window
    // reads are within `QPEL_READ` bytes of each of its `bh + 1` rows and
    // the writes within `bw` bytes of each of `bh` output rows, which
    // `super::qpel_block` asserts; `hq` holds 17 rows of 16.
    unsafe {
        let src = win.as_ptr().add(off);
        let dst = out.as_mut_ptr();
        if fx == 0 && fy == 0 {
            for r in 0..bh {
                st(dst.add(r * os), ldw(src.add(r * ws), bw), bw);
            }
            return;
        }
        let rnd = _mm_set1_epi16((16 - rc) as i16);
        let rcm = _mm_set1_epi8(rc as i8);
        let coef: [__m128i; 4] = std::array::from_fn(|p| {
            _mm_set1_epi16(((TAPS[p][1] as i16) << 8) | (TAPS[p][0] as u8 as i16))
        });
        let mut hq = [0u8; 17 * 16];
        let h = hq.as_mut_ptr();
        // Pass 1: the window's rows at the horizontal position.
        for r in 0..=bh {
            let p = src.add(r * ws);
            let row = ld(p);
            let v = if fx == 0 {
                row
            } else {
                let half = if bw == 16 {
                    _mm_packus_epi16(
                        hfilt(row, &H16[0], &coef, rnd),
                        hfilt(ld(p.add(5)), &H16[1], &coef, rnd),
                    )
                } else {
                    let f = hfilt(row, &H8, &coef, rnd);
                    _mm_packus_epi16(f, f)
                };
                match fx {
                    1 => avg(row, half, rcm),
                    2 => half,
                    _ => avg(half, ld(p.add(1)), rcm),
                }
            };
            _mm_storeu_si128(h.add(r * 16) as *mut __m128i, v);
        }
        // Pass 2: down the columns. The mirrored rows E[i] = hq[m(i - 3)]
        // interleaved in pairs P[s] = (E[s], E[s + 1]) byte by byte
        // (columns 0-7, `lo`, and 8-15, `hi`); output row r is
        // sum_p TAPS[p] . P[r + 2 p].
        let h = hq.as_ptr();
        if fy == 0 {
            for r in 0..bh {
                st(dst.add(r * os), ld(h.add(r * 16)), bw);
            }
            return;
        }
        let e = |i: usize| ld(h.add(16 * mirror(i as isize - 3, bh as isize) as usize));
        let mut lo = [_mm_setzero_si128(); 16 + 6];
        let mut hi = [_mm_setzero_si128(); 16 + 6];
        let mut prev = e(0);
        for s in 0..bh + 6 {
            let next = e(s + 1);
            lo[s] = _mm_unpacklo_epi8(prev, next);
            hi[s] = _mm_unpackhi_epi8(prev, next);
            prev = next;
        }
        for r in 0..bh {
            let mut sl = _mm_setzero_si128();
            let mut sh = _mm_setzero_si128();
            for p in 0..4 {
                sl = _mm_add_epi16(sl, _mm_maddubs_epi16(lo[r + 2 * p], coef[p]));
                if bw == 16 {
                    sh = _mm_add_epi16(sh, _mm_maddubs_epi16(hi[r + 2 * p], coef[p]));
                }
            }
            let half = _mm_packus_epi16(
                _mm_srai_epi16(_mm_add_epi16(sl, rnd), 5),
                _mm_srai_epi16(_mm_add_epi16(sh, rnd), 5),
            );
            let v = match fy {
                1 => avg(ld(h.add(r * 16)), half, rcm),
                2 => half,
                _ => avg(half, ld(h.add((r + 1) * 16)), rcm),
            };
            st(dst.add(r * os), v, bw);
        }
    }
}

/// # Safety
/// The CPU has SSE4.1 and SSSE3; the bounds `super::qpel_block` asserts
/// hold and `bw` is 8 or 16.
#[target_feature(enable = "sse4.1,ssse3")]
pub(super) unsafe fn qpel_sse41(
    win: &[u8],
    off: usize,
    ws: usize,
    a: Interp,
    out: &mut [u8],
    os: usize,
) {
    // SAFETY: as documented.
    unsafe { qpel_body(win, off, ws, a, out, os) }
}

/// # Safety
/// The CPU has AVX2; as [`qpel_sse41`] otherwise.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn qpel_avx2(
    win: &[u8],
    off: usize,
    ws: usize,
    a: Interp,
    out: &mut [u8],
    os: usize,
) {
    // SAFETY: as documented.
    unsafe {
        if a.bw == 16 && (a.fx | a.fy) != 0 {
            qpel16_avx2(win, off, ws, a, out, os)
        } else {
            qpel_body(win, off, ws, a, out, os)
        }
    }
}

/// The two halves of an AVX2 register of 16 bytes per lane, the sixteen
/// 16-bit filter sums `s` (lane 0 outputs 0-7, lane 1 outputs 8-15)
/// rounded, clipped and packed to 16 bytes in order.
#[inline(always)]
unsafe fn round_pack16(s: __m256i, rnd: __m256i) -> __m128i {
    // SAFETY: AVX2 (the caller's target features).
    unsafe {
        let v = _mm256_srai_epi16(_mm256_add_epi16(s, rnd), 5);
        let p = _mm256_packus_epi16(v, v);
        _mm256_castsi256_si128(_mm256_permute4x64_epi64::<0b1000>(p))
    }
}

/// Quarter-sample interpolation of a 16-wide block with both halves of
/// each row (and each pair of rows) in one AVX2 register.
#[target_feature(enable = "avx2")]
unsafe fn qpel16_avx2(win: &[u8], off: usize, ws: usize, a: Interp, out: &mut [u8], os: usize) {
    let Interp { bh, fx, fy, .. } = a;
    let rc = a.rounding as i32;
    // SAFETY: as `qpel_body`: the window reads are within `QPEL_READ`
    // bytes of each of its `bh + 1` rows, the writes 16 bytes of each of
    // `bh` output rows (asserted by `super::qpel_block`); `hq` holds 17
    // rows of 16 and every row index read is mirrored into 0..=bh.
    unsafe {
        let src = win.as_ptr().add(off);
        let dst = out.as_mut_ptr();
        let rnd = _mm256_set1_epi16((16 - rc) as i16);
        let rcm = _mm_set1_epi8(rc as i8);
        let coef: [__m256i; 4] = std::array::from_fn(|p| {
            _mm256_set1_epi16(((TAPS[p][1] as i16) << 8) | (TAPS[p][0] as u8 as i16))
        });
        let tabs: [__m256i; 4] = std::array::from_fn(|p| {
            _mm256_loadu2_m128i(
                H16[1][p].as_ptr() as *const __m128i,
                H16[0][p].as_ptr() as *const __m128i,
            )
        });
        let mut hq = [0u8; 17 * 16];
        let h = hq.as_mut_ptr();
        // Pass 1, the rows: with no vertical fraction the last window
        // row is not needed.
        let rows = if fy == 0 { bh } else { bh + 1 };
        for r in 0..rows {
            let p = src.add(r * ws);
            let row = ld(p);
            let v = if fx == 0 {
                row
            } else {
                // Outputs 0-7 from samples 0-15, 8-15 from samples 5-20.
                let w = _mm256_inserti128_si256::<1>(_mm256_castsi128_si256(row), ld(p.add(5)));
                let mut s = _mm256_setzero_si256();
                for q in 0..4 {
                    s = _mm256_add_epi16(
                        s,
                        _mm256_maddubs_epi16(_mm256_shuffle_epi8(w, tabs[q]), coef[q]),
                    );
                }
                let half = round_pack16(s, rnd);
                match fx {
                    1 => avg(row, half, rcm),
                    2 => half,
                    _ => avg(half, ld(p.add(1)), rcm),
                }
            };
            if fy == 0 {
                _mm_storeu_si128(dst.add(r * os) as *mut __m128i, v);
            } else {
                _mm_storeu_si128(h.add(r * 16) as *mut __m128i, v);
            }
        }
        if fy == 0 {
            return;
        }
        // Pass 2, the columns. The mirrored rows E[i] = hq[m(i - 3)]
        // interleaved in pairs P[s] = (E[s], E[s + 1]), byte by byte:
        // columns 0-7 in lane 0, 8-15 in lane 1. Output row r is
        // sum_q TAPS[q] . P[r + 2 q].
        let h = hq.as_ptr();
        let e = |i: usize| ld(h.add(16 * mirror(i as isize - 3, bh as isize) as usize));
        let mut pairs = [_mm256_setzero_si256(); 16 + 6];
        let mut prev = e(0);
        for (s, pr) in pairs.iter_mut().enumerate().take(bh + 6) {
            let next = e(s + 1);
            *pr = _mm256_set_m128i(_mm_unpackhi_epi8(prev, next), _mm_unpacklo_epi8(prev, next));
            prev = next;
        }
        for r in 0..bh {
            let mut s = _mm256_setzero_si256();
            for q in 0..4 {
                s = _mm256_add_epi16(s, _mm256_maddubs_epi16(pairs[r + 2 * q], coef[q]));
            }
            let half = round_pack16(s, rnd);
            let v = match fy {
                1 => avg(ld(h.add(r * 16)), half, rcm),
                2 => half,
                _ => avg(half, ld(h.add((r + 1) * 16)), rcm),
            };
            _mm_storeu_si128(dst.add(r * os) as *mut __m128i, v);
        }
    }
}

#[inline(always)]
unsafe fn halfpel_body(win: &[u8], off: usize, ws: usize, a: Interp, out: &mut [u8], os: usize) {
    let Interp { bw, bh, fx, fy, .. } = a;
    let rc = a.rounding as i32;
    // SAFETY: SSE2 / SSE4.1. Each read is within the `bw + 1` samples of
    // one of the window's `bh + 1` rows, each write within `bw` bytes of
    // one of `bh` output rows, as `super::halfpel_block` asserts.
    unsafe {
        let src = win.as_ptr().add(off);
        let dst = out.as_mut_ptr();
        let rcm = _mm_set1_epi8(rc as i8);
        match (fx, fy) {
            (0, 0) => {
                for r in 0..bh {
                    st(dst.add(r * os), ldw(src.add(r * ws), bw), bw);
                }
            }
            (1, 0) => {
                for r in 0..bh {
                    let p = src.add(r * ws);
                    st(dst.add(r * os), avg(ldw(p, bw), ldw(p.add(1), bw), rcm), bw);
                }
            }
            (0, _) => {
                let mut prev = ldw(src, bw);
                for r in 0..bh {
                    let next = ldw(src.add((r + 1) * ws), bw);
                    st(dst.add(r * os), avg(prev, next, rcm), bw);
                    prev = next;
                }
            }
            _ => {
                let z = _mm_setzero_si128();
                let bias = _mm_set1_epi16((2 - rc) as i16);
                // Horizontal pair sums of a row, low and high eight samples.
                let sums = |p: *const u8| -> (__m128i, __m128i) {
                    let x = ldw(p, bw);
                    let y = ldw(p.add(1), bw);
                    (
                        _mm_add_epi16(_mm_unpacklo_epi8(x, z), _mm_unpacklo_epi8(y, z)),
                        _mm_add_epi16(_mm_unpackhi_epi8(x, z), _mm_unpackhi_epi8(y, z)),
                    )
                };
                let mut prev = sums(src);
                for r in 0..bh {
                    let next = sums(src.add((r + 1) * ws));
                    let lo = _mm_srli_epi16(_mm_add_epi16(_mm_add_epi16(prev.0, next.0), bias), 2);
                    let hi = _mm_srli_epi16(_mm_add_epi16(_mm_add_epi16(prev.1, next.1), bias), 2);
                    st(dst.add(r * os), _mm_packus_epi16(lo, hi), bw);
                    prev = next;
                }
            }
        }
    }
}

/// # Safety
/// The CPU has SSE4.1; the bounds `super::halfpel_block` asserts hold
/// and `bw` is 8 or 16.
#[target_feature(enable = "sse4.1,ssse3")]
pub(super) unsafe fn halfpel_sse41(
    win: &[u8],
    off: usize,
    ws: usize,
    a: Interp,
    out: &mut [u8],
    os: usize,
) {
    // SAFETY: as documented.
    unsafe { halfpel_body(win, off, ws, a, out, os) }
}

/// # Safety
/// The CPU has AVX2; as [`halfpel_sse41`] otherwise.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn halfpel_avx2(
    win: &[u8],
    off: usize,
    ws: usize,
    a: Interp,
    out: &mut [u8],
    os: usize,
) {
    // SAFETY: as documented.
    unsafe { halfpel_body(win, off, ws, a, out, os) }
}

/// # Safety
/// The bounds `super::sad` asserts hold and `w` is 8 or 16.
#[allow(clippy::too_many_arguments)]
#[inline]
pub(super) unsafe fn sad_sse2(
    a: &[u8],
    ao: usize,
    as_: usize,
    b: &[u8],
    bo: usize,
    bs: usize,
    w: usize,
    h: usize,
) -> u32 {
    // SAFETY: SSE2 only; each read is `w` bytes of one of `h` rows of
    // either block, as `super::sad` asserts.
    unsafe {
        let pa = a.as_ptr().add(ao);
        let pb = b.as_ptr().add(bo);
        let mut acc = _mm_setzero_si128();
        for r in 0..h {
            let x = ldw(pa.add(r * as_), w);
            let y = ldw(pb.add(r * bs), w);
            acc = _mm_add_epi64(acc, _mm_sad_epu8(x, y));
        }
        (_mm_cvtsi128_si32(acc) as u32)
            .wrapping_add(_mm_cvtsi128_si32(_mm_unpackhi_epi64(acc, acc)) as u32)
    }
}

/// GMC's bilinear samples over a block, eight lanes at a time: the warped
/// positions from `super::WarpLanes` (lane `k` of a row's block `b` at
/// column `8 b + k`), the two rows of each 2x2 neighbourhood gathered, and
/// the scalar bilinear sum multiplied out:
/// `((s - ri)(s - rj) a + ri (s - rj) b + (s - ri) rj c + ri rj d + s^2/2
/// - rounding) >> 2 rho`, the same integers.
///
/// # Safety
/// The CPU has AVX2; `super::gmc_lanes` accepted the block (every value
/// fits 32 bits, every gather is inside `plane`); `cols` is a multiple of 8
/// dividing `out.len()`.
#[allow(clippy::too_many_arguments)]
#[target_feature(enable = "avx2")]
pub(super) unsafe fn gmc_block_avx2(
    plane: &[u8],
    stride: usize,
    f: super::WarpLanes,
    g: super::WarpLanes,
    rho: u32,
    rounding: bool,
    out: &mut [u8],
    cols: usize,
) {
    let s = 1i32 << rho;
    let round = _mm256_set1_epi32(s * s / 2 - rounding as i32);
    let lanes = _mm256_setr_epi32(0, 1, 2, 3, 4, 5, 6, 7);
    let mask = _mm256_set1_epi32(s - 1);
    let sv = _mm256_set1_epi32(s);
    let byte = _mm256_set1_epi32(255);
    let stride_v = _mm256_set1_epi32(stride as i32);
    let rho_c = _mm_cvtsi32_si128(rho as i32);
    let sh_c = _mm_cvtsi32_si128(2 * rho as i32);
    let (fqs, frs) = (
        _mm256_mullo_epi32(lanes, _mm256_set1_epi32(f.qs)),
        _mm256_mullo_epi32(lanes, _mm256_set1_epi32(f.rs)),
    );
    let (gqs, grs) = (
        _mm256_mullo_epi32(lanes, _mm256_set1_epi32(g.qs)),
        _mm256_mullo_epi32(lanes, _mm256_set1_epi32(g.rs)),
    );
    let (fsh, gsh) = (
        _mm_cvtsi32_si128(f.w.shift as i32),
        _mm_cvtsi32_si128(g.w.shift as i32),
    );
    for (r, row) in out.chunks_exact_mut(cols).enumerate() {
        let ((fq, frem), (gq, grem)) = (f.row(r), g.row(r));
        for (b, o) in row.as_chunks_mut::<8>().0.iter_mut().enumerate() {
            let c0 = 8 * b as i32;
            // The position of each lane: high part plus the low part's carry.
            let pos = |q: i32, rem: i32, qs: i32, rs: i32, vq, vr, sh| {
                let hi = _mm256_add_epi32(_mm256_set1_epi32(q + c0 * qs), vq);
                let lo = _mm256_add_epi32(_mm256_set1_epi32(rem + c0 * rs), vr);
                _mm256_add_epi32(hi, _mm256_srl_epi32(lo, sh))
            };
            let fv = pos(fq, frem, f.qs, f.rs, fqs, frs, fsh);
            let gv = pos(gq, grem, g.qs, g.rs, gqs, grs, gsh);
            let (x, ri) = (_mm256_sra_epi32(fv, rho_c), _mm256_and_si256(fv, mask));
            let (y, rj) = (_mm256_sra_epi32(gv, rho_c), _mm256_and_si256(gv, mask));
            let idx = _mm256_add_epi32(_mm256_mullo_epi32(y, stride_v), x);
            // SAFETY: every index is a sample of the plane with its right
            // neighbour, the row below, and the four bytes each gather
            // reads inside `plane` (`gmc_lanes`).
            let (top, bot) = unsafe {
                let p = plane.as_ptr();
                (
                    _mm256_i32gather_epi32::<1>(p as *const i32, idx),
                    _mm256_i32gather_epi32::<1>(p.add(stride) as *const i32, idx),
                )
            };
            let a = _mm256_and_si256(top, byte);
            let bb = _mm256_and_si256(_mm256_srli_epi32::<8>(top), byte);
            let c = _mm256_and_si256(bot, byte);
            let d = _mm256_and_si256(_mm256_srli_epi32::<8>(bot), byte);
            let (si, sj) = (_mm256_sub_epi32(sv, ri), _mm256_sub_epi32(sv, rj));
            let mut v = _mm256_mullo_epi32(_mm256_mullo_epi32(si, sj), a);
            v = _mm256_add_epi32(v, _mm256_mullo_epi32(_mm256_mullo_epi32(ri, sj), bb));
            v = _mm256_add_epi32(v, _mm256_mullo_epi32(_mm256_mullo_epi32(si, rj), c));
            v = _mm256_add_epi32(v, _mm256_mullo_epi32(_mm256_mullo_epi32(ri, rj), d));
            v = _mm256_srl_epi32(_mm256_add_epi32(v, round), sh_c);
            // Eight values 0..=255: byte 0 of each 32-bit lane.
            let p16 = _mm256_packus_epi32(v, v);
            let p8 = _mm256_packus_epi16(p16, p16);
            let res = _mm_unpacklo_epi32(
                _mm256_castsi256_si128(p8),
                _mm256_extracti128_si256::<1>(p8),
            );
            // SAFETY: `o` is eight bytes.
            unsafe { _mm_storel_epi64(o.as_mut_ptr() as *mut __m128i, res) };
        }
    }
}
