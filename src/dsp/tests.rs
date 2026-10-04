//! Every SIMD kernel against the scalar one: random and edge inputs, every
//! level the CPU has. With `MPEG4_REQUIRE_SIMD=1` (CI) a host without the
//! SIMD level expected of its architecture fails instead of testing the
//! scalar kernels alone.

use super::*;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next() % (hi - lo + 1) as u64) as i32
    }

    fn byte(&mut self) -> u8 {
        self.next() as u8
    }
}

/// The SIMD levels to compare with scalar.
fn simd_levels() -> Vec<Isa> {
    let all: Vec<Isa> = Isa::all().into_iter().skip(1).collect();
    if std::env::var_os("MPEG4_REQUIRE_SIMD").is_some_and(|v| !v.is_empty() && v != "0") {
        let want: &[&str] = if cfg!(target_arch = "x86_64") {
            &["sse4.1", "avx2"]
        } else if cfg!(target_arch = "aarch64") {
            &["neon"]
        } else {
            &[]
        };
        for w in want {
            assert!(
                all.iter().any(|i| i.name() == *w),
                "MPEG4_REQUIRE_SIMD: this CPU lacks {w}"
            );
        }
    }
    all
}

#[test]
fn level_names() {
    let all = Isa::all();
    assert_eq!(all[0], Isa::SCALAR);
    println!(
        "kernel levels: {:?}, in use: {}",
        all.iter().map(|i| i.name()).collect::<Vec<_>>(),
        kernel_level()
    );
    let _ = simd_levels();
}

/// Coefficient blocks: random at several densities and ranges, the
/// extremes, and out-of-range values (which the SIMD kernels hand to the
/// scalar one).
fn blocks(rng: &mut Rng, lo: i32, hi: i32) -> Vec<[i16; 64]> {
    let mut v = Vec::new();
    v.push([0; 64]);
    v.push([hi as i16; 64]);
    v.push([lo as i16; 64]);
    let mut alt = [0i16; 64];
    for (i, c) in alt.iter_mut().enumerate() {
        *c = if (i + i / 8) % 2 == 0 {
            hi as i16
        } else {
            lo as i16
        };
    }
    v.push(alt);
    for i in 0..64 {
        let mut b = [0i16; 64];
        b[i] = hi as i16;
        v.push(b);
        b[i] = lo as i16;
        v.push(b);
    }
    for n in 0..20_000 {
        let mut b = [0i16; 64];
        let density = [1, 3, 10, 64][n % 4];
        let (l, h) = [(lo, hi), (-300, 300), (-20, 20), (lo, hi)][n / 4 % 4];
        for _ in 0..density {
            let i = (rng.next() % 64) as usize;
            b[i] = rng.range(l, h) as i16;
        }
        v.push(b);
    }
    for _ in 0..200 {
        let mut b = [0i16; 64];
        for c in b.iter_mut() {
            *c = rng.range(-32768, 32767) as i16;
        }
        v.push(b);
    }
    v
}

#[test]
fn idct_matches_scalar() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let input = blocks(&mut rng, -2048, 2047);
    for isa in simd_levels() {
        for b in &input {
            let mut x = *b;
            let mut y = *b;
            scalar::idct(&mut x);
            idct_with(isa, &mut y);
            assert_eq!(x, y, "{} idct of {b:?}", isa.name());
        }
    }
}

#[test]
fn fdct_matches_scalar() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut input = blocks(&mut rng, -2048, 2047);
    // Samples and residuals, the encoder's inputs.
    for _ in 0..20_000 {
        let mut b = [0i16; 64];
        let (l, h) = if rng.next() & 1 == 0 {
            (0, 255)
        } else {
            (-255, 255)
        };
        for c in b.iter_mut() {
            *c = rng.range(l, h) as i16;
        }
        input.push(b);
    }
    for isa in simd_levels() {
        for b in &input {
            let mut x = *b;
            let mut y = *b;
            scalar::fdct(&mut x);
            fdct_with(isa, &mut y);
            assert_eq!(x, y, "{} fdct of {b:?}", isa.name());
        }
    }
}

/// A window of random samples, or of extremes (0 and 255), with
/// `QPEL_READ`-byte rows.
fn window(rng: &mut Rng, kind: usize) -> Vec<u8> {
    (0..17 * QPEL_READ + 64)
        .map(|_| match kind {
            0 => rng.byte(),
            1 => [0, 255][(rng.next() & 1) as usize],
            2 => 255,
            _ => 0,
        })
        .collect()
}

#[test]
fn qpel_matches_scalar() {
    let mut rng = Rng(77);
    let levels = simd_levels();
    for trial in 0..400usize {
        let win = window(&mut rng, trial % 4);
        let off = (rng.next() % 8) as usize;
        let ws = QPEL_READ + (rng.next() % 3) as usize;
        for (bw, bh) in [(16, 16), (8, 8), (16, 8), (8, 16), (16, 4), (8, 5)] {
            if off + bh * ws + QPEL_READ > win.len() {
                continue;
            }
            for fx in 0..4 {
                for fy in 0..4 {
                    for rounding in [false, true] {
                        let a = Interp {
                            bw,
                            bh,
                            fx,
                            fy,
                            rounding,
                        };
                        let os = 16 + (trial % 3) * 8;
                        let mut want = vec![0xa5u8; os * 16];
                        scalar::qpel_block(&win, off, ws, a, &mut want, os);
                        for &isa in &levels {
                            let mut got = vec![0xa5u8; os * 16];
                            qpel_block(isa, &win, off, ws, a, &mut got, os);
                            assert_eq!(want, got, "{} qpel {a:?}", isa.name());
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn halfpel_matches_scalar() {
    let mut rng = Rng(99);
    let levels = simd_levels();
    for trial in 0..400usize {
        let win = window(&mut rng, trial % 4);
        let off = (rng.next() % 8) as usize;
        let ws = 17 + (rng.next() % 20) as usize;
        for (bw, bh) in [(16, 16), (8, 8), (16, 8), (8, 4), (8, 1)] {
            if off + bh * ws + bw + 1 > win.len() {
                continue;
            }
            for fx in 0..2 {
                for fy in 0..2 {
                    for rounding in [false, true] {
                        let a = Interp {
                            bw,
                            bh,
                            fx,
                            fy,
                            rounding,
                        };
                        let os = 16 + (trial % 2) * 16;
                        let mut want = vec![0x5au8; os * 16];
                        scalar::halfpel_block(&win, off, ws, a, &mut want, os);
                        for &isa in &levels {
                            let mut got = vec![0x5au8; os * 16];
                            halfpel_block(isa, &win, off, ws, a, &mut got, os);
                            assert_eq!(want, got, "{} halfpel {a:?}", isa.name());
                        }
                    }
                }
            }
        }
    }
}

/// Exactly `bw + 1` samples per row and `bh + 1` rows: the half-sample
/// kernels read nothing past the window.
#[test]
fn halfpel_reads_only_the_window() {
    let mut rng = Rng(5);
    for (bw, bh) in [(16, 16), (8, 8), (8, 4)] {
        let ws = bw + 1;
        let win: Vec<u8> = (0..ws * (bh + 1)).map(|_| rng.byte()).collect();
        for fx in 0..2 {
            for fy in 0..2 {
                let a = Interp {
                    bw,
                    bh,
                    fx,
                    fy,
                    rounding: true,
                };
                let mut want = vec![0u8; bw * bh];
                scalar::halfpel_block(&win, 0, ws, a, &mut want, bw);
                for isa in simd_levels() {
                    let mut got = vec![0u8; bw * bh];
                    halfpel_block(isa, &win, 0, ws, a, &mut got, bw);
                    assert_eq!(want, got, "{}", isa.name());
                }
            }
        }
    }
}

#[test]
fn sad_matches_scalar() {
    let mut rng = Rng(3);
    let levels = simd_levels();
    for trial in 0..2000 {
        let kind = trial % 3;
        let gen_ = |rng: &mut Rng| -> Vec<u8> {
            (0..64 * 40)
                .map(|_| match kind {
                    0 => rng.byte(),
                    1 => 255,
                    _ => 0,
                })
                .collect()
        };
        let a = gen_(&mut rng);
        let b = if kind == 0 {
            gen_(&mut rng)
        } else {
            vec![255 - a[0]; 64 * 40]
        };
        for (w, h) in [
            (16, 16),
            (8, 8),
            (16, 8),
            (8, 16),
            (16, 32),
            (16, 1),
            (16, 64),
            (4, 4),
        ] {
            let (ao, bo) = ((rng.next() % 16) as usize, (rng.next() % 16) as usize);
            let (as_, bs) = (
                16 + (rng.next() % 24) as usize,
                16 + (rng.next() % 24) as usize,
            );
            if ao + (h - 1) * as_ + w > a.len() || bo + (h - 1) * bs + w > b.len() {
                continue;
            }
            let want = scalar::sad(&a, ao, as_, &b, bo, bs, w, h);
            for &isa in &levels {
                assert_eq!(
                    want,
                    sad(isa, &a, ao, as_, &b, bo, bs, w, h),
                    "{} sad {w}x{h}",
                    isa.name()
                );
            }
        }
    }
}

/// Kernel speed, each level against scalar (the code the kernels
/// replaced): `cargo test --release --lib kernel_speed -- --ignored
/// --nocapture`. Nanoseconds per call, the fastest of seven runs.
#[test]
#[ignore]
fn kernel_speed() {
    use std::hint::black_box;
    use std::time::Instant;
    fn time(f: &mut dyn FnMut()) -> f64 {
        let n = 20_000;
        let mut best = f64::MAX;
        for _ in 0..7 {
            let t = Instant::now();
            for _ in 0..n {
                f();
            }
            best = best.min(t.elapsed().as_nanos() as f64 / n as f64);
        }
        best
    }
    let mut rng = Rng(42);
    let mut coefs = [0i16; 64];
    for c in coefs.iter_mut().take(20) {
        *c = rng.range(-300, 300) as i16;
    }
    let mut samples = [0i16; 64];
    for c in samples.iter_mut() {
        *c = rng.range(-255, 255) as i16;
    }
    let win = window(&mut rng, 0);
    let mut out = [0u8; 16 * 16];
    let levels = Isa::all();
    let mut rows: Vec<(String, Vec<f64>)> = Vec::new();
    let mut row = |name: &str, f: &mut dyn FnMut(Isa)| {
        let t = levels.iter().map(|&isa| time(&mut || f(isa))).collect();
        rows.push((name.to_string(), t));
    };
    row("idct 8x8", &mut |isa| {
        let mut b = black_box(coefs);
        idct_with(isa, &mut b);
        black_box(b);
    });
    row("fdct 8x8", &mut |isa| {
        let mut b = black_box(samples);
        fdct_with(isa, &mut b);
        black_box(b);
    });
    for (bw, bh) in [(16, 16), (8, 8)] {
        for (fx, fy, what) in [
            (0, 0, "full"),
            (2, 0, "h half"),
            (1, 3, "h+v quarter"),
            (2, 2, "h+v half"),
        ] {
            row(&format!("qpel {bw}x{bh} {what}"), &mut |isa| {
                let a = Interp {
                    bw,
                    bh,
                    fx,
                    fy,
                    rounding: true,
                };
                qpel_block(isa, black_box(&win), 0, QPEL_READ, a, &mut out, 16);
                black_box(&out);
            });
        }
        for (fx, fy, what) in [(1, 0, "h"), (0, 1, "v"), (1, 1, "hv")] {
            row(&format!("halfpel {bw}x{bh} {what}"), &mut |isa| {
                let a = Interp {
                    bw,
                    bh,
                    fx,
                    fy,
                    rounding: true,
                };
                halfpel_block(isa, black_box(&win), 0, QPEL_READ, a, &mut out, 16);
                black_box(&out);
            });
        }
        row(&format!("sad {bw}x{bh}"), &mut |isa| {
            black_box(sad(
                isa,
                black_box(&win),
                0,
                QPEL_READ,
                &win,
                7,
                QPEL_READ,
                bw,
                bh,
            ));
        });
    }
    // The forward quantisers: by division (what the encoder did) against
    // the reciprocal multiplications, shown in the scalar and last columns.
    {
        use crate::quant::{
            MpegRecips, quantise_h263, quantise_h263_div, quantise_mpeg, quantise_mpeg_div,
        };
        let m = crate::tables::DEFAULT_INTER_MATRIX;
        let r = MpegRecips::new(&crate::tables::DEFAULT_INTRA_MATRIX, &m);
        let n = levels.len();
        let mut q = |name: &str, old: &mut dyn FnMut(), new: &mut dyn FnMut()| {
            let (a, b) = (time(&mut *old), time(&mut *new));
            let mut t = vec![f64::NAN; n];
            t[0] = a;
            t[n - 1] = b;
            rows.push((name.to_string(), t));
        };
        q(
            "quant h263 (div / mul)",
            &mut || {
                let mut b = black_box(samples);
                quantise_h263_div(&mut b, black_box(7), false);
                black_box(b);
            },
            &mut || {
                let mut b = black_box(samples);
                quantise_h263(&mut b, black_box(7), false);
                black_box(b);
            },
        );
        q(
            "quant mpeg (div / mul)",
            &mut || {
                let mut b = black_box(samples);
                quantise_mpeg_div(&mut b, black_box(7), false, &m);
                black_box(b);
            },
            &mut || {
                let mut b = black_box(samples);
                quantise_mpeg(&mut b, black_box(7), false, &r);
                black_box(b);
            },
        );
    }
    print!("{:<24}", "kernel (ns/call)");
    for isa in &levels {
        print!("{:>10}", isa.name());
    }
    println!("{:>10}", "speed-up");
    for (name, t) in rows {
        print!("{name:<24}");
        for v in &t {
            print!("{v:>10.1}");
        }
        println!("{:>9.1}x", t[0] / t.last().unwrap());
    }
}

/// The vector GMC blocks against per-sample bilinear sampling: random
/// affine warps at every accuracy (`s` 2 to 16), divisors up to 2^28 and
/// numerators far beyond 32 bits, steps from a fraction of a sample to
/// many, positions up to and past the plane's edges (where the kernel
/// must decline), blocks of 8 or 16 columns and 1 to 16 rows, both
/// roundings.
#[test]
fn gmc_blocks_match_scalar() {
    let mut rng = Rng(0xfeed_beef);
    let (w, h, stride) = (96i32, 64i32, 96usize);
    let plane: Vec<u8> = (0..stride * h as usize).map(|_| rng.byte()).collect();
    let levels = simd_levels();
    let mut taken = 0;
    for trial in 0..100_000u32 {
        let rho = 1 + trial % 4;
        let s = 1i64 << rho;
        let cols = if trial % 2 == 0 { 16 } else { 8 };
        let rows = 1 + (rng.next() % 16) as usize;
        let rounding = trial % 3 == 0;
        let shift = rng.range(0, 28) as u32;
        let coord = |rng: &mut Rng, size: i32| {
            let span = size as i64 * s;
            let low = |rng: &mut Rng| (rng.next() as i64) & ((1i64 << shift) - 1);
            let pos = rng.range(-(span as i32) / 8, span as i32 + span as i32 / 8) as i64;
            let step = |rng: &mut Rng| match rng.next() % 5 {
                0 => (rng.range(-2 * s as i32, 2 * s as i32) as i64) << shift,
                1 => ((rng.range(-2 * s as i32, 2 * s as i32) as i64) << shift) + low(rng),
                2 => rng.range(-8, 8) as i64,
                3 => (rng.next() as i32) as i64 * 4096,
                _ => 0,
            };
            WarpBlock {
                n0: (pos << shift) + low(rng),
                si: step(rng),
                sj: step(rng),
                shift,
                base: rng.range(-200, 200) as i64,
            }
        };
        let (f, g) = (coord(&mut rng, w), coord(&mut rng, h));
        let mut want = vec![0u8; cols * rows];
        let mut inside = true;
        'all: for r in 0..rows {
            for c in 0..cols {
                let (Some(fp), Some(gp)) = (f.at(c as i64, r as i64), g.at(c as i64, r as i64))
                else {
                    inside = false;
                    break 'all;
                };
                let (x, y) = (fp >> rho, gp >> rho);
                if x < 0 || y < 0 || x >= w as i64 - 1 || y >= h as i64 - 1 {
                    inside = false;
                    break 'all;
                }
                let (ri, rj) = (fp & (s - 1), gp & (s - 1));
                let i = y as usize * stride + x as usize;
                let (a, b) = (plane[i] as i64, plane[i + 1] as i64);
                let (cc, d) = (plane[i + stride] as i64, plane[i + stride + 1] as i64);
                let top = (s - ri) * a + ri * b;
                let bot = (s - ri) * cc + ri * d;
                want[r * cols + c] = (((s - rj) * top + rj * bot + s * s / 2 - rounding as i64)
                    >> (2 * rho))
                    .clamp(0, 255) as u8;
            }
        }
        for &isa in &levels {
            let mut got = vec![0xa5u8; cols * rows];
            if gmc_block(
                isa,
                &plane,
                stride,
                (w, h),
                f,
                g,
                rho,
                rounding,
                &mut got,
                cols,
            ) {
                assert!(
                    inside,
                    "{} took a block outside the plane: {f:?} {g:?}",
                    isa.name()
                );
                assert_eq!(want, got, "{} rho {rho} {f:?} {g:?}", isa.name());
                taken += 1;
            }
        }
    }
    if levels.iter().any(|i| i.name() == "avx2") {
        assert!(taken > 5_000, "only {taken} blocks took the vector kernel");
    }
}
