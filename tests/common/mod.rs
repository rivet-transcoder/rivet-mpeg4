//! Shared helpers: synthetic moving pictures and quality measures.

#![allow(dead_code)]

use mpeg4::Frame;

/// A deterministic test picture at time `t`: textured gradients panning
/// with sub-sample speed, a bright square moving the other way, and
/// chroma that moves with them — content that exercises half-sample
/// motion, intra decisions and every block.
pub fn synth(w: u32, h: u32, t: u32) -> Frame {
    let mut f = Frame::new(w, h);
    let tf = t as f64;
    let (dx, dy) = (tf * 1.5, tf * 0.75);
    let (sx, sy) = (w as f64 * 0.6 - tf * 2.0, h as f64 * 0.3 + tf);
    let wy = w as usize;
    {
        let y = f.plane_mut(0);
        for r in 0..h as usize {
            for c in 0..wy {
                let (xf, yf) = (c as f64 + dx, r as f64 + dy);
                let mut v = 96.0
                    + 40.0 * (xf * 0.11).sin()
                    + 30.0 * (yf * 0.07).cos()
                    + 20.0 * ((xf + yf) * 0.31).sin();
                if (c as f64 - sx).abs() < 10.0 && (r as f64 - sy).abs() < 8.0 {
                    v = 220.0 - 3.0 * ((c + r) % 7) as f64;
                }
                y[r * wy + c] = v.clamp(0.0, 255.0) as u8;
            }
        }
    }
    let (cw, ch) = Frame::chroma_size(w, h);
    for (i, phase) in [(1usize, 0.0), (2, 1.3)] {
        let p = f.plane_mut(i);
        for r in 0..ch as usize {
            for c in 0..cw as usize {
                let (xf, yf) = (c as f64 * 2.0 + dx, r as f64 * 2.0 + dy);
                let v = 128.0 + 30.0 * (xf * 0.05 + phase).sin() + 20.0 * (yf * 0.04).cos();
                p[r * cw as usize + c] = v.clamp(0.0, 255.0) as u8;
            }
        }
    }
    f
}

/// PSNR of plane `i` of `b` against `a`.
pub fn psnr_plane(a: &Frame, b: &Frame, i: usize) -> f64 {
    let (pa, pb) = (a.plane(i), b.plane(i));
    assert_eq!(pa.len(), pb.len());
    let mse = pa.iter().zip(pb).map(|(&x, &y)| (x as f64 - y as f64).powi(2)).sum::<f64>() / pa.len() as f64;
    if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() }
}

/// Luma PSNR.
pub fn psnr(a: &Frame, b: &Frame) -> f64 {
    psnr_plane(a, b, 0)
}

pub mod media;
