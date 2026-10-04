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
