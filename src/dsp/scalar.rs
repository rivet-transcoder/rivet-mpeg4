//! The scalar kernels: the definitions the SIMD ones reproduce exactly.

use super::Interp;

/// `BASIS[k][n] = round(65536 * c(k) * cos((2n + 1) k pi / 16))`, with
/// `c(0) = sqrt(1/8)` and `c(k) = 1/2` otherwise: the orthonormal 8-point
/// DCT-II basis.
pub(crate) const BASIS: [[i32; 8]; 8] = [
    [23170, 23170, 23170, 23170, 23170, 23170, 23170, 23170],
    [32138, 27246, 18205, 6393, -6393, -18205, -27246, -32138],
    [30274, 12540, -12540, -30274, -30274, -12540, 12540, 30274],
    [27246, -6393, -32138, -18205, 18205, 32138, 6393, -27246],
    [23170, -23170, -23170, 23170, 23170, -23170, -23170, 23170],
    [18205, -32138, 6393, 27246, -27246, -6393, 32138, -18205],
    [12540, -30274, 30274, -12540, -12540, 30274, -30274, 12540],
    [6393, -18205, 27246, -32138, 32138, -27246, 18205, -6393],
];

#[inline(always)]
fn b(k: usize, n: usize) -> i64 {
    BASIS[k][n] as i64
}

/// One 8-point inverse transform: `out[n] = sum_k BASIS[k][n] * x[k]`, at
/// 16 fractional bits.
#[inline(always)]
fn idct8(x: [i64; 8]) -> [i64; 8] {
    let mut out = [0i64; 8];
    for n in 0..4 {
        let e = b(0, n) * x[0] + b(2, n) * x[2] + b(4, n) * x[4] + b(6, n) * x[6];
        let o = b(1, n) * x[1] + b(3, n) * x[3] + b(5, n) * x[5] + b(7, n) * x[7];
        out[n] = e + o;
        out[7 - n] = e - o;
    }
    out
}

/// One 8-point forward transform: `out[k] = sum_n BASIS[k][n] * x[n]`.
#[inline(always)]
fn fdct8(x: [i64; 8]) -> [i64; 8] {
    let s = [x[0] + x[7], x[1] + x[6], x[2] + x[5], x[3] + x[4]];
    let d = [x[0] - x[7], x[1] - x[6], x[2] - x[5], x[3] - x[4]];
    let mut out = [0i64; 8];
    for (k, o) in out.iter_mut().enumerate() {
        let v = if k & 1 == 0 { &s } else { &d };
        *o = b(k, 0) * v[0] + b(k, 1) * v[1] + b(k, 2) * v[2] + b(k, 3) * v[3];
    }
    out
}

/// Inverse DCT of a raster-order coefficient block, in place: rows, then
/// columns, eight fractional bits carried between the passes, the result
/// rounded to the nearest and saturated to 16 bits.
pub(crate) fn idct(block: &mut [i16; 64]) {
    let mut tmp = [0i64; 64];
    for r in 0..8 {
        let row = &block[r * 8..r * 8 + 8];
        if row[1..].iter().all(|&c| c == 0) {
            // DC only: every output of the row is BASIS[0][n] * dc.
            let v = (b(0, 0) * row[0] as i64 + 128) >> 8;
            tmp[r * 8..r * 8 + 8].fill(v);
            continue;
        }
        let x = std::array::from_fn(|k| row[k] as i64);
        let y = idct8(x);
        for n in 0..8 {
            tmp[r * 8 + n] = (y[n] + 128) >> 8;
        }
    }
    for c in 0..8 {
        let x = std::array::from_fn(|k| tmp[k * 8 + c]);
        let y = idct8(x);
        for n in 0..8 {
            let v = (y[n] + (1 << 23)) >> 24;
            block[n * 8 + c] = v.clamp(i16::MIN as i64, i16::MAX as i64) as i16;
        }
    }
}

/// Forward DCT of a raster-order block of samples or residuals, in place,
/// rounded to the nearest integer and saturated to [-2048, 2047].
pub(crate) fn fdct(block: &mut [i16; 64]) {
    let mut tmp = [0i64; 64];
    for r in 0..8 {
        let x = std::array::from_fn(|n| block[r * 8 + n] as i64);
        let y = fdct8(x);
        for k in 0..8 {
            tmp[r * 8 + k] = (y[k] + 128) >> 8;
        }
    }
    for c in 0..8 {
        let x = std::array::from_fn(|n| tmp[n * 8 + c]);
        let y = fdct8(x);
        for k in 0..8 {
            let v = (y[k] + (1 << 23)) >> 24;
            block[k * 8 + c] = v.clamp(-2048, 2047) as i16;
        }
    }
}

/// Where tap `s` of a row of `n + 1` samples reads, the block's edge
/// mirrored (`p[-1] = p[0]`, `p[-2] = p[1]`, `p[n + 1] = p[n]`, ...).
#[inline(always)]
pub(crate) const fn mirror(s: isize, n: isize) -> isize {
    if s < 0 {
        -s - 1
    } else if s > n {
        2 * n + 1 - s
    } else {
        s
    }
}

/// The 8-tap half-sample filter of 7.6.2.2 over the `n + 1` samples
/// `p(0..=n)`, writing the `n` values between neighbours to `out`, taps
/// beyond the samples mirrored back into them.
#[inline]
fn filter8(p: impl Fn(usize) -> u8, n: usize, rc: i32, out: &mut [u8]) {
    let mut e = [0i32; 17 + 6];
    for (k, v) in e[..n + 7].iter_mut().enumerate() {
        *v = p(mirror(k as isize - 3, n as isize) as usize) as i32;
    }
    for i in 0..n {
        let e = &e[i..i + 8];
        let v = 20 * (e[3] + e[4]) - 6 * (e[2] + e[5]) + 3 * (e[1] + e[6]) - (e[0] + e[7]);
        out[i] = ((v + 16 - rc) >> 5).clamp(0, 255) as u8;
    }
}

/// Quarter-sample interpolation: see [`super::qpel_block`] and
/// `crate::mc::qpel`.
pub(crate) fn qpel_block(win: &[u8], off: usize, ws: usize, a: Interp, out: &mut [u8], os: usize) {
    let Interp { bw, bh, fx, fy, .. } = a;
    let nh = bh + 1;
    let full = &win[off..];
    if fx == 0 && fy == 0 {
        for r in 0..bh {
            out[r * os..r * os + bw].copy_from_slice(&full[r * ws..r * ws + bw]);
        }
        return;
    }
    let rc = a.rounding as i32;
    let avg = |a: u8, b: u8| ((a as u32 + b as u32 + 1 - rc as u32) >> 1) as u8;
    // Pass 1: every window row at the horizontal position, `bw` wide.
    let mut hq = [0u8; 17 * 16];
    let mut half = [0u8; 16];
    for r in 0..nh {
        let row = &full[r * ws..r * ws + bw + 1];
        let o = &mut hq[r * bw..r * bw + bw];
        match fx {
            0 => o.copy_from_slice(&row[..bw]),
            _ => {
                filter8(|i| row[i], bw, rc, &mut half);
                for c in 0..bw {
                    o[c] = match fx {
                        1 => avg(row[c], half[c]),
                        2 => half[c],
                        _ => avg(half[c], row[c + 1]),
                    };
                }
            }
        }
    }
    // Pass 2: every column of those at the vertical position.
    for c in 0..bw {
        let col = |r: usize| hq[r * bw + c];
        if fy != 0 {
            filter8(col, bh, rc, &mut half);
        }
        for r in 0..bh {
            out[r * os + c] = match fy {
                0 => col(r),
                1 => avg(col(r), half[r]),
                2 => half[r],
                _ => avg(half[r], col(r + 1)),
            };
        }
    }
}

/// Half-sample interpolation: see [`super::halfpel_block`].
pub(crate) fn halfpel_block(
    win: &[u8],
    off: usize,
    ws: usize,
    a: Interp,
    out: &mut [u8],
    os: usize,
) {
    let Interp { bw, bh, fx, fy, .. } = a;
    let rc = a.rounding as u32;
    for r in 0..bh {
        let o = &mut out[r * os..r * os + bw];
        let p = &win[off + r * ws..];
        let q = &win[off + (r + 1).min(bh) * ws..];
        match (fx, fy) {
            (0, 0) => o.copy_from_slice(&p[..bw]),
            (1, 0) => {
                for c in 0..bw {
                    o[c] = ((p[c] as u32 + p[c + 1] as u32 + 1 - rc) >> 1) as u8;
                }
            }
            (0, _) => {
                for c in 0..bw {
                    o[c] = ((p[c] as u32 + q[c] as u32 + 1 - rc) >> 1) as u8;
                }
            }
            _ => {
                for c in 0..bw {
                    let s = p[c] as u32 + p[c + 1] as u32 + q[c] as u32 + q[c + 1] as u32;
                    o[c] = ((s + 2 - rc) >> 2) as u8;
                }
            }
        }
    }
}

/// Sum of absolute differences: see [`super::sad`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn sad(
    a: &[u8],
    ao: usize,
    as_: usize,
    b: &[u8],
    bo: usize,
    bs: usize,
    w: usize,
    h: usize,
) -> u32 {
    let mut sum = 0;
    for r in 0..h {
        let x = &a[ao + r * as_..ao + r * as_ + w];
        let y = &b[bo + r * bs..bo + r * bs + w];
        sum += x
            .iter()
            .zip(y)
            .map(|(&p, &q)| p.abs_diff(q) as u32)
            .sum::<u32>();
    }
    sum
}
