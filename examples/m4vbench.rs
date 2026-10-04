//! End-to-end speed: encodes a clip with this crate's encoder, decodes the
//! stream with its decoder, and reports frames per second for each — the
//! fastest of `REPS` runs — after checking (in one more, untimed, encode)
//! that every decoded picture equals the encoder's reconstruction.
//!
//! `cargo run --release --example m4vbench -- Y4M WxH FRAMES PRESET [REPS] [THREADS]`
//!
//! - `Y4M`: an 8-bit 4:2:0 YUV4MPEG2 file. Its pictures are cropped (about
//!   the centre) or extended (mirrored) to `W` x `H`; with fewer than
//!   `FRAMES` pictures it plays forwards and backwards.
//! - `PRESET`: `sp` (Simple Profile: H.263 quantiser, half-sample motion,
//!   I- and P-VOPs), `asp` (two B-VOPs, quarter-sample motion, four
//!   vectors, the MPEG quantiser), or `decode:FILE` to time decoding an
//!   existing stream only.
//! - `THREADS`: the encoder's thread count (0: one per core; default 1).
//!
//! The last column is an FNV-1a hash of every decoded picture, to compare
//! builds for bit-exactness. `MPEG4_FORCE_SCALAR=1` selects the scalar
//! kernels.

use mpeg4::{Decoder, Encoder, EncoderConfig, Frame, Quantiser};
use std::time::Instant;

fn read_y4m(path: &str) -> (u32, u32, Vec<Vec<u8>>) {
    let data = std::fs::read(path).expect("read Y4M");
    let nl = data.iter().position(|&b| b == b'\n').expect("Y4M header");
    let header = std::str::from_utf8(&data[..nl]).expect("Y4M header");
    let (mut w, mut h) = (0u32, 0u32);
    for t in header.split_whitespace() {
        if let Some(v) = t.strip_prefix('W') {
            w = v.parse().unwrap();
        } else if let Some(v) = t.strip_prefix('H') {
            h = v.parse().unwrap();
        } else if let Some(v) = t.strip_prefix('C') {
            assert!(v.starts_with("420"), "4:2:0 only, not C{v}");
        }
    }
    let size = (w * h + 2 * w.div_ceil(2) * h.div_ceil(2)) as usize;
    let mut frames = Vec::new();
    let mut p = nl + 1;
    while p < data.len() {
        let e = p + data[p..].iter().position(|&b| b == b'\n').expect("FRAME");
        let f = &data[e + 1..e + 1 + size];
        frames.push(f.to_vec());
        p = e + 1 + size;
    }
    (w, h, frames)
}

/// Maps output coordinate `i` (of `n`) onto a source of `m` samples:
/// centred crop, or mirrored extension.
fn fit(i: usize, n: usize, m: usize) -> usize {
    let off = (m as isize - n as isize) / 2;
    let mut s = i as isize + off;
    let m = m as isize;
    while s < 0 || s >= m {
        s = if s < 0 { -s - 1 } else { 2 * m - 1 - s };
    }
    s as usize
}

fn fit_plane(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u8> {
    let mut out = vec![0u8; dw * dh];
    for r in 0..dh {
        let sr = fit(r, dh, sh);
        for c in 0..dw {
            out[r * dw + c] = src[sr * sw + fit(c, dw, sw)];
        }
    }
    out
}

fn load(path: &str, w: u32, h: u32, n: usize) -> Vec<Frame> {
    let (sw, sh, raw) = read_y4m(path);
    let (scw, sch) = Frame::chroma_size(sw, sh);
    let (cw, ch) = Frame::chroma_size(w, h);
    let pics: Vec<Frame> = raw
        .iter()
        .map(|f| {
            let ys = (sw * sh) as usize;
            let cs = (scw * sch) as usize;
            let y = fit_plane(&f[..ys], sw as usize, sh as usize, w as usize, h as usize);
            let cb = fit_plane(
                &f[ys..ys + cs],
                scw as usize,
                sch as usize,
                cw as usize,
                ch as usize,
            );
            let cr = fit_plane(
                &f[ys + cs..ys + 2 * cs],
                scw as usize,
                sch as usize,
                cw as usize,
                ch as usize,
            );
            Frame::from_planes(w, h, &y, &cb, &cr).unwrap()
        })
        .collect();
    let m = pics.len();
    (0..n)
        .map(|i| {
            let k = i % (2 * m);
            pics[if k < m { k } else { 2 * m - 1 - k }].clone()
        })
        .collect()
}

fn fnv(h: &mut u64, d: &[u8]) {
    for &b in d {
        *h ^= b as u64;
        *h = h.wrapping_mul(0x100_0000_01b3);
    }
}

fn decode(stream: &[u8]) -> Vec<Frame> {
    let mut d = Decoder::new();
    let mut out = d.decode(stream).expect("decode");
    out.extend(d.flush());
    out
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 4 {
        eprintln!("usage: m4vbench Y4M WxH FRAMES PRESET [REPS] [THREADS]");
        std::process::exit(2);
    }
    let (w, h) = a[1].split_once('x').expect("WxH");
    let (w, h): (u32, u32) = (w.parse().unwrap(), h.parse().unwrap());
    let n: usize = a[2].parse().unwrap();
    let reps: usize = a.get(4).map_or(5, |v| v.parse().unwrap());
    let threads: usize = a.get(5).map_or(1, |v| v.parse().unwrap());
    let preset = a[3].as_str();

    let stream;
    let mut enc_fps = 0.0;
    let mut recon = Vec::new();
    if let Some(file) = preset.strip_prefix("decode:") {
        stream = std::fs::read(file).expect("read stream");
    } else {
        let frames = load(&a[0], w, h, n);
        let mut cfg = EncoderConfig::new(w, h, 25);
        cfg.threads = threads;
        match preset {
            "sp" => {}
            "asp" => {
                cfg.b_frames = 2;
                cfg.quarter_sample = true;
                cfg.four_mv = true;
                cfg.quantiser = Quantiser::mpeg_default();
            }
            p => panic!("unknown preset {p}"),
        }
        let encode = |cfg: &EncoderConfig| {
            let mut enc = Encoder::new(cfg.clone()).expect("config");
            let mut s = Vec::new();
            for f in &frames {
                s.extend(enc.encode(f).expect("encode"));
            }
            s.extend(enc.finish().expect("finish"));
            (s, enc.take_reconstructions())
        };
        let mut best = f64::MAX;
        for _ in 0..reps {
            let t = Instant::now();
            std::hint::black_box(encode(&cfg));
            best = best.min(t.elapsed().as_secs_f64());
        }
        enc_fps = n as f64 / best;
        cfg.keep_reconstructions = true;
        let (s, r) = encode(&cfg);
        stream = s;
        recon = r;
    }
    let mut best = f64::MAX;
    let mut out = Vec::new();
    for _ in 0..reps {
        let t = Instant::now();
        out = decode(&stream);
        best = best.min(t.elapsed().as_secs_f64());
    }
    let dec_fps = out.len() as f64 / best;
    if !recon.is_empty() {
        // Reconstructions come in decode order, pictures in display order.
        recon.sort_by_key(|f| f.timestamp);
        assert_eq!(out.len(), recon.len(), "pictures");
        for (i, (f, r)) in out.iter().zip(&recon).enumerate() {
            assert!(
                r.data == f.data,
                "decoded picture {i} differs from the encoder's reconstruction"
            );
        }
    }
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for f in &out {
        fnv(&mut hash, &f.data);
    }
    if let Some(path) = std::env::var_os("M4VBENCH_SAVE") {
        std::fs::write(path, &stream).expect("save stream");
    }
    println!(
        "{preset} {w}x{h} {} frames, {} bytes: encode {enc_fps:.2} fps, decode {dec_fps:.1} fps, hash {hash:016x}",
        out.len(),
        stream.len()
    );
}
