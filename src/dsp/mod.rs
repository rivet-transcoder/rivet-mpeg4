//! The sample-processing kernels, with SIMD versions selected at run time.
//!
//! Every kernel has a scalar version, which is the definition, and SIMD
//! versions that compute exactly the same integers: x86-64 with SSE4.1 (and
//! SSSE3), the same code compiled for AVX2, and NEON on aarch64. The level
//! is detected once ([`Isa::best`]); `MPEG4_FORCE_SCALAR=1` in the
//! environment selects the scalar kernels instead. Because every level
//! computes the same values, decoded pictures do not depend on the CPU, and
//! the encoder's reconstruction stays the decoder's.
//!
//! The kernels:
//!
//! - [`idct`] / [`fdct`]: the 8x8 transforms of `crate::idct` (integer;
//!   the SIMD versions split the second pass's 64-bit products into two
//!   32-bit halves, so they stay exact).
//! - [`qpel_block`]: quarter-sample interpolation (7.6.2.2) of a window.
//! - [`halfpel_block`]: half-sample interpolation with rounding control.
//! - [`sad`]: sum of absolute differences, for the encoder's search.

#[cfg(target_arch = "aarch64")]
mod neon;
pub(crate) mod scalar;
#[cfg(test)]
mod tests;
#[cfg(target_arch = "x86_64")]
mod x86;

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Level {
    Scalar,
    #[cfg(target_arch = "x86_64")]
    Sse41,
    #[cfg(target_arch = "x86_64")]
    Avx2,
    #[cfg(target_arch = "aarch64")]
    Neon,
}

/// A kernel level the running CPU supports. Only [`Isa::best`] and
/// [`Isa::all`] make one, after checking the CPU, which is what makes
/// calling the SIMD kernels through it sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Isa(Level);

static BEST: AtomicU8 = AtomicU8::new(0);

impl Isa {
    /// The scalar kernels (always available).
    pub const SCALAR: Isa = Isa(Level::Scalar);

    /// The fastest level the CPU supports, or scalar when
    /// `MPEG4_FORCE_SCALAR` is set (to anything but `0` or empty).
    #[inline]
    pub fn best() -> Isa {
        match BEST.load(Ordering::Relaxed) {
            0 => {
                let isa = Self::detect();
                BEST.store(Self::code(isa), Ordering::Relaxed);
                isa
            }
            c => Self::decode(c),
        }
    }

    fn code(isa: Isa) -> u8 {
        match isa.0 {
            Level::Scalar => 1,
            #[cfg(target_arch = "x86_64")]
            Level::Sse41 => 2,
            #[cfg(target_arch = "x86_64")]
            Level::Avx2 => 3,
            #[cfg(target_arch = "aarch64")]
            Level::Neon => 4,
        }
    }

    #[inline]
    fn decode(c: u8) -> Isa {
        // Only `code` of a detected level is ever stored.
        match c {
            #[cfg(target_arch = "x86_64")]
            2 => Isa(Level::Sse41),
            #[cfg(target_arch = "x86_64")]
            3 => Isa(Level::Avx2),
            #[cfg(target_arch = "aarch64")]
            4 => Isa(Level::Neon),
            _ => Isa(Level::Scalar),
        }
    }

    fn forced_scalar() -> bool {
        std::env::var_os("MPEG4_FORCE_SCALAR").is_some_and(|v| !v.is_empty() && v != "0")
    }

    fn detect() -> Isa {
        if Self::forced_scalar() {
            return Isa::SCALAR;
        }
        *Self::all().last().unwrap()
    }

    /// Every level the CPU supports, slowest (scalar) first — what the
    /// tests compare with each other. Ignores `MPEG4_FORCE_SCALAR`.
    pub fn all() -> Vec<Isa> {
        #[allow(unused_mut)]
        let mut v = vec![Isa::SCALAR];
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("ssse3") && is_x86_feature_detected!("sse4.1") {
                v.push(Isa(Level::Sse41));
                if is_x86_feature_detected!("avx2") {
                    v.push(Isa(Level::Avx2));
                }
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            if std::arch::is_aarch64_feature_detected!("neon") {
                v.push(Isa(Level::Neon));
            }
        }
        v
    }

    /// The level's name, for reports.
    pub fn name(self) -> &'static str {
        match self.0 {
            Level::Scalar => "scalar",
            #[cfg(target_arch = "x86_64")]
            Level::Sse41 => "sse4.1",
            #[cfg(target_arch = "x86_64")]
            Level::Avx2 => "avx2",
            #[cfg(target_arch = "aarch64")]
            Level::Neon => "neon",
        }
    }
}

/// The name of the kernel level in use (`scalar`, `sse4.1`, `avx2` or
/// `neon`).
pub fn kernel_level() -> &'static str {
    Isa::best().name()
}

/// Inverse DCT of a raster-order block, in place (see `crate::idct`).
#[inline]
pub(crate) fn idct(b: &mut [i16; 64]) {
    idct_with(Isa::best(), b)
}

pub(crate) fn idct_with(isa: Isa, b: &mut [i16; 64]) {
    // A block with only a DC coefficient (common) transforms to a constant:
    // the scalar transform's two passes over it, exactly.
    if b[1..].iter().fold(0, |a, &c| a | c) == 0 {
        let row = (scalar::BASIS[0][0] * b[0] as i32 + 128) >> 8;
        let v = (scalar::BASIS[0][0] as i64 * row as i64 + (1 << 23)) >> 24;
        b.fill(v as i16);
        return;
    }
    match isa.0 {
        // SAFETY: an `Isa` of this level exists only when the CPU has it.
        #[cfg(target_arch = "x86_64")]
        Level::Avx2 => unsafe { x86::idct_avx2(b) },
        // SAFETY: as above.
        #[cfg(target_arch = "x86_64")]
        Level::Sse41 => unsafe { x86::idct_sse41(b) },
        // SAFETY: as above.
        #[cfg(target_arch = "aarch64")]
        Level::Neon => unsafe { neon::idct(b) },
        Level::Scalar => scalar::idct(b),
    }
}

/// Forward DCT of a raster-order block, in place (see `crate::idct`).
#[inline]
pub(crate) fn fdct(b: &mut [i16; 64]) {
    fdct_with(Isa::best(), b)
}

pub(crate) fn fdct_with(isa: Isa, b: &mut [i16; 64]) {
    match isa.0 {
        // SAFETY: an `Isa` of this level exists only when the CPU has it.
        #[cfg(target_arch = "x86_64")]
        Level::Avx2 => unsafe { x86::fdct_avx2(b) },
        // SAFETY: as above.
        #[cfg(target_arch = "x86_64")]
        Level::Sse41 => unsafe { x86::fdct_sse41(b) },
        // SAFETY: as above.
        #[cfg(target_arch = "aarch64")]
        Level::Neon => unsafe { neon::fdct(b) },
        Level::Scalar => scalar::fdct(b),
    }
}

/// Row stride of the windows [`qpel_block`] reads: the SIMD kernels read
/// up to [`QPEL_READ`] bytes from the start of each window row.
pub(crate) const QPEL_READ: usize = 32;

/// The arguments of one block interpolation.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Interp {
    /// Block width: 8 or 16 for the SIMD kernels (others are scalar), at
    /// least 4 for [`qpel_block`].
    pub bw: usize,
    /// Block height, 1 to 16 (4 to 16 for [`qpel_block`]).
    pub bh: usize,
    /// Horizontal and vertical fraction (quarters for [`qpel_block`],
    /// halves for [`halfpel_block`]).
    pub fx: usize,
    pub fy: usize,
    /// `rounding_control`.
    pub rounding: bool,
}

/// Quarter-sample interpolation (7.6.2.2) of a `bw` x `bh` block from the
/// `(bw + 1)` x `(bh + 1)` window of integer samples at `win[off..]`,
/// rows `ws` apart; every row of the window must have [`QPEL_READ`]
/// readable bytes (the window's own and padding). Writes `out` rows
/// `os` apart.
pub(crate) fn qpel_block(
    isa: Isa,
    win: &[u8],
    off: usize,
    ws: usize,
    a: Interp,
    out: &mut [u8],
    os: usize,
) {
    // The mirroring of the filter taps needs four samples each side.
    assert!(a.bh >= 4 && a.bh <= 16 && a.bw >= 4 && a.bw <= 16 && a.fx < 4 && a.fy < 4);
    assert!(off + a.bh * ws + QPEL_READ <= win.len() && ws > a.bw);
    assert!((a.bh - 1) * os + a.bw <= out.len() && os >= a.bw);
    let simd = a.bw == 8 || a.bw == 16;
    match isa.0 {
        // SAFETY: the CPU has the level (see `Isa`); the bounds of every
        // read and write are asserted above.
        #[cfg(target_arch = "x86_64")]
        Level::Avx2 if simd => unsafe { x86::qpel_avx2(win, off, ws, a, out, os) },
        // SAFETY: as above.
        #[cfg(target_arch = "x86_64")]
        Level::Sse41 if simd => unsafe { x86::qpel_sse41(win, off, ws, a, out, os) },
        // SAFETY: as above.
        #[cfg(target_arch = "aarch64")]
        Level::Neon if simd => unsafe { neon::qpel(win, off, ws, a, out, os) },
        _ => scalar::qpel_block(win, off, ws, a, out, os),
    }
}

/// Half-sample interpolation (7.6.2.1) of a `bw` x `bh` block from the
/// `(bw + 1)` x `(bh + 1)` window at `win[off..]`, rows `ws` apart (no
/// padding needed); `fx`, `fy` 0 or 1.
pub(crate) fn halfpel_block(
    isa: Isa,
    win: &[u8],
    off: usize,
    ws: usize,
    a: Interp,
    out: &mut [u8],
    os: usize,
) {
    assert!(a.bh >= 1 && a.bw >= 1 && a.fx < 2 && a.fy < 2);
    assert!(off + a.bh * ws + a.bw < win.len() && ws > a.bw);
    assert!((a.bh - 1) * os + a.bw <= out.len() && os >= a.bw);
    let simd = a.bw == 8 || a.bw == 16;
    match isa.0 {
        // SAFETY: the CPU has the level (see `Isa`); the bounds of every
        // read and write are asserted above.
        #[cfg(target_arch = "x86_64")]
        Level::Avx2 if simd => unsafe { x86::halfpel_avx2(win, off, ws, a, out, os) },
        // SAFETY: as above.
        #[cfg(target_arch = "x86_64")]
        Level::Sse41 if simd => unsafe { x86::halfpel_sse41(win, off, ws, a, out, os) },
        // SAFETY: as above.
        #[cfg(target_arch = "aarch64")]
        Level::Neon if simd => unsafe { neon::halfpel(win, off, ws, a, out, os) },
        _ => scalar::halfpel_block(win, off, ws, a, out, os),
    }
}

/// One coordinate of an affine (GMC) warp over a block of samples:
/// sample `(c, r)` (column, row) lies at
/// `base + ((n0 + c si + r sj) >> shift)`, in `1 / s` samples.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WarpBlock {
    pub n0: i64,
    pub si: i64,
    pub sj: i64,
    pub shift: u32,
    pub base: i64,
}

impl WarpBlock {
    #[inline]
    fn at(&self, c: i64, r: i64) -> Option<i64> {
        let n = self
            .n0
            .checked_add(self.si.checked_mul(c)?)?
            .checked_add(self.sj.checked_mul(r)?)?;
        self.base.checked_add(n >> self.shift)
    }
}

/// A [`WarpBlock`] for 32-bit lanes. Along a row the numerator is split as
/// `n = q 2^shift + rem` and the step as `si = qs 2^shift + rs`
/// (`0 <= rem, rs < 2^shift`), so sample `c` lies at
/// `base + q + c qs + ((rem + c rs) >> shift)` — exactly (the low parts'
/// carry is what the shift takes), with every term in 32 bits even when
/// the numerators are not.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub(crate) struct WarpLanes {
    pub w: WarpBlock,
    pub qs: i32,
    pub rs: i32,
}

impl WarpLanes {
    /// Row `r`'s `(base + q, rem)`: what lane 0 of the row starts from.
    #[inline]
    #[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
    pub fn row(&self, r: usize) -> (i32, i32) {
        let n = self.w.n0 + r as i64 * self.w.sj;
        let q = n >> self.w.shift;
        ((self.w.base + q) as i32, (n - (q << self.w.shift)) as i32)
    }
}

/// The lanes of a `cols` x `rows` block of the warp `(f, g)` on a plane,
/// when the vector kernel can take it: every number it forms fits 32 bits,
/// and every sample's 2x2 neighbourhood lies inside the plane with three
/// bytes to spare after its lower row (each gather reads four bytes). The
/// warp is the floor of an affine function, monotonic along rows and
/// columns, so the block's corners bound every sample.
#[allow(clippy::too_many_arguments)]
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
fn gmc_lanes(
    plane: &[u8],
    stride: usize,
    (w, h): (i32, i32),
    f: WarpBlock,
    g: WarpBlock,
    rho: u32,
    (cols, rows): (usize, usize),
) -> Option<(WarpLanes, WarpLanes)> {
    const LIM: i64 = 1 << 30;
    let small = |v: i64| v > -LIM && v < LIM;
    let (lc, lr) = (cols as i64 - 1, rows as i64 - 1);
    let lanes = |b: &WarpBlock| -> Option<(WarpLanes, i64, i64)> {
        if b.shift > 26 || !small(b.base) {
            return None;
        }
        let mut lo = i64::MAX;
        let mut hi = i64::MIN;
        for (c, r) in [(0, 0), (lc, 0), (0, lr), (lc, lr)] {
            let v = b.at(c, r)?;
            // The numerator too: `WarpLanes::row` forms it in 64 bits.
            b.n0.checked_add(b.sj.checked_mul(r)?)?;
            lo = lo.min(v);
            hi = hi.max(v);
        }
        let qs = b.si >> b.shift;
        let rs = b.si - (qs << b.shift);
        // base + q + c qs stays within a sample of the positions, and
        // rem + c rs < (1 + cols) 2^shift <= 17 * 2^26.
        (small(lo) && small(hi) && small(qs * lc)).then_some((
            WarpLanes {
                w: *b,
                qs: qs as i32,
                rs: rs as i32,
            },
            lo,
            hi,
        ))
    };
    let (fl, f0, f1) = lanes(&f)?;
    let (gl, g0, g1) = lanes(&g)?;
    let (x0, x1, y0, y1) = (f0 >> rho, f1 >> rho, g0 >> rho, g1 >> rho);
    (rho <= 4
        && plane.len() < i32::MAX as usize
        && x0 >= 0
        && y0 >= 0
        && x1 < w as i64 - 1
        && y1 < h as i64 - 1
        && (y1 as usize + 1) * stride + x1 as usize + 4 <= plane.len())
    .then_some((fl, gl))
}

/// GMC's bilinear samples (7.8.7) over a block: `out[r cols + c]` from the
/// plane at the warped position of sample `(c, r)`, `rho = log2 s`, with
/// `rounding_control`. Returns false, writing nothing, when this level has
/// no vector kernel (only AVX2 has gathers) or the block needs the
/// per-sample path (a position near or past the plane's edge, or numbers
/// beyond 32 bits); the caller then samples it the scalar way.
#[allow(clippy::too_many_arguments)]
pub(crate) fn gmc_block(
    isa: Isa,
    plane: &[u8],
    stride: usize,
    wh: (i32, i32),
    f: WarpBlock,
    g: WarpBlock,
    rho: u32,
    rounding: bool,
    out: &mut [u8],
    cols: usize,
) -> bool {
    match isa.0 {
        #[cfg(target_arch = "x86_64")]
        Level::Avx2 if cols > 0 && cols.is_multiple_of(8) && out.len().is_multiple_of(cols) => {
            let rows = out.len() / cols;
            let Some((fl, gl)) = gmc_lanes(plane, stride, wh, f, g, rho, (cols, rows)) else {
                return false;
            };
            // SAFETY: the CPU has AVX2 (see `Isa`); `gmc_lanes` bounds every
            // position and read.
            unsafe { x86::gmc_block_avx2(plane, stride, fl, gl, rho, rounding, out, cols) };
            true
        }
        _ => {
            let _ = (plane, stride, wh, f, g, rho, rounding, out, cols);
            false
        }
    }
}

/// Sum of absolute differences of the `w` x `h` blocks at `a[ao..]`
/// (rows `as_` apart) and `b[bo..]` (rows `bs` apart). `w` 8 or 16 for
/// the SIMD kernels.
#[allow(clippy::too_many_arguments)]
#[inline]
pub(crate) fn sad(
    isa: Isa,
    a: &[u8],
    ao: usize,
    as_: usize,
    b: &[u8],
    bo: usize,
    bs: usize,
    w: usize,
    h: usize,
) -> u32 {
    if h == 0 || w == 0 {
        return 0;
    }
    assert!(ao + (h - 1) * as_ + w <= a.len() && bo + (h - 1) * bs + w <= b.len());
    let simd = (w == 8 || w == 16) && h <= 64;
    match isa.0 {
        // SAFETY: the CPU has the level (see `Isa`); the bounds of every
        // read are asserted above.
        #[cfg(target_arch = "x86_64")]
        Level::Avx2 | Level::Sse41 if simd => unsafe { x86::sad_sse2(a, ao, as_, b, bo, bs, w, h) },
        // SAFETY: the CPU has NEON (see `Isa`).
        #[cfg(target_arch = "aarch64")]
        Level::Neon if simd => unsafe { neon::sad(a, ao, as_, b, bo, bs, w, h) },
        _ => scalar::sad(a, ao, as_, b, bo, bs, w, h),
    }
}
