//! Round trips through this crate's encoder and decoder.
//!
//! Two properties: the decoder reproduces the encoder's reconstruction
//! exactly (the encoder predicts from it, so any difference is a desync
//! between the two sides — a bug, not a quality question), and the
//! reconstruction is close to the source (PSNR, gated loosely and
//! printed).

mod common;

use common::{psnr, psnr_plane, synth};
use mpeg4::{Decoder, Encoder, EncoderConfig, Frame, RateControl, VopType};

struct Run {
    frames: Vec<Frame>,
    recon: Vec<Frame>,
    sources: Vec<Frame>,
    bytes: usize,
}

/// Encodes `n` synthetic frames, decoding each access unit as it comes.
fn run(mut cfg: EncoderConfig, n: u32) -> Run {
    cfg.keep_reconstructions = true;
    let (w, h) = (cfg.width, cfg.height);
    let mut enc = Encoder::new(cfg).unwrap();
    let mut dec = Decoder::new();
    let mut frames = Vec::new();
    let mut recon = Vec::new();
    let mut sources = Vec::new();
    let mut bytes = 0;
    for t in 0..n {
        let src = synth(w, h, t);
        let au = enc.encode(&src).unwrap();
        bytes += au.len();
        recon.extend(enc.take_reconstructions());
        frames.extend(dec.decode(&au).unwrap());
        sources.push(src);
    }
    let tail = enc.finish().unwrap();
    bytes += tail.len();
    recon.extend(enc.take_reconstructions());
    frames.extend(dec.decode(&tail).unwrap());
    frames.extend(dec.flush());
    // Reconstructions come in decode order, frames in display order.
    recon.sort_by_key(|f| f.timestamp);
    Run {
        frames,
        recon,
        sources,
        bytes,
    }
}

fn check(r: &Run, min_psnr: f64, label: &str) -> f64 {
    assert_eq!(r.frames.len(), r.recon.len(), "{label}: frame count");
    let mut worst = f64::MAX;
    let mut sum = 0.0;
    for (i, ((d, e), s)) in r.frames.iter().zip(&r.recon).zip(&r.sources).enumerate() {
        assert!(!d.concealed, "{label}: frame {i} concealed");
        assert_eq!(
            d.data, e.data,
            "{label}: frame {i} differs from the encoder's reconstruction"
        );
        let p = psnr(s, d);
        worst = worst.min(p);
        sum += p;
        assert!(
            psnr_plane(s, d, 1) > min_psnr - 3.0,
            "{label}: Cb of frame {i}"
        );
    }
    let avg = sum / r.frames.len() as f64;
    println!(
        "{label}: {} frames, {} bytes, luma PSNR avg {avg:.2} dB, worst {worst:.2} dB",
        r.frames.len(),
        r.bytes
    );
    assert!(worst > min_psnr, "{label}: worst PSNR {worst:.2} dB");
    avg
}

#[test]
fn intra_only() {
    let mut cfg = EncoderConfig::new(176, 144, 25);
    cfg.gop_size = 1;
    for q in [2u8, 5, 12, 31] {
        cfg.rate = RateControl::ConstantQuant(q);
        let r = run(cfg.clone(), 4);
        let min = match q {
            2 => 42.0,
            5 => 36.0,
            12 => 30.0,
            _ => 24.0,
        };
        check(&r, min, &format!("intra q{q}"));
        assert!(r.frames.iter().all(|f| f.vop_type == VopType::I));
    }
}

#[test]
fn intra_and_predicted() {
    let cfg = EncoderConfig::new(176, 144, 25);
    let r = run(cfg, 30);
    check(&r, 33.0, "IPPP q5");
    assert_eq!(
        r.frames.iter().filter(|f| f.vop_type == VopType::I).count(),
        3
    );
    // Display order and timing.
    for (i, f) in r.frames.iter().enumerate() {
        assert_eq!(f.timestamp, i as i64);
        assert_eq!(f.time_base, 25);
    }
}

#[test]
fn predicted_compresses() {
    let mut cfg = EncoderConfig::new(176, 144, 25);
    cfg.gop_size = 1;
    let intra = run(cfg.clone(), 12);
    cfg.gop_size = 0;
    let inter = run(cfg, 12);
    check(&inter, 33.0, "P q5");
    println!(
        "all-intra {} bytes, IPPP {} bytes",
        intra.bytes, inter.bytes
    );
    assert!(
        inter.bytes * 2 < intra.bytes,
        "P-VOPs should at least halve the size"
    );
}

#[test]
fn odd_sizes_crop_and_pad() {
    for (w, h) in [(50, 34), (17, 15), (1, 1), (33, 64), (200, 9)] {
        let mut cfg = EncoderConfig::new(w, h, 30);
        cfg.gop_size = 5;
        let r = run(cfg, 8);
        check(&r, 30.0, &format!("{w}x{h}"));
        assert!(r.frames.iter().all(|f| f.width == w && f.height == h));
    }
}

#[test]
fn four_vectors() {
    let mut cfg = EncoderConfig::new(176, 144, 25);
    cfg.four_mv = true;
    let r = run(cfg, 15);
    check(&r, 33.0, "4MV");
}

#[test]
fn video_packets() {
    let mut cfg = EncoderConfig::new(352, 288, 25);
    cfg.packet_bytes = Some(300);
    cfg.four_mv = true;
    let r = run(cfg, 10);
    check(&r, 33.0, "packets of 300 bytes");
}

#[test]
fn wide_search_needs_a_larger_fcode() {
    let mut cfg = EncoderConfig::new(320, 240, 25);
    cfg.search_range = 100;
    let r = run(cfg, 6);
    check(&r, 33.0, "search range 100 (fcode 4)");
}

#[test]
fn bit_rate_control() {
    for kbps in [100u32, 300, 700] {
        let mut cfg = EncoderConfig::new(352, 288, 25);
        cfg.rate = RateControl::Bitrate(kbps * 1000);
        cfg.gop_size = 25;
        let n = 50;
        let r = run(cfg, n);
        check(&r, 22.0, &format!("{kbps} kb/s"));
        let got = r.bytes as f64 * 8.0 * 25.0 / n as f64 / 1000.0;
        println!("{kbps} kb/s target: {got:.0} kb/s");
        assert!(
            got > kbps as f64 * 0.6 && got < kbps as f64 * 1.5,
            "{kbps} kb/s target, {got:.0} kb/s"
        );
    }
}

#[test]
fn config_from_esds_then_bare_vops() {
    let cfg = EncoderConfig::new(96, 64, 30);
    let mut enc = Encoder::new(cfg).unwrap();
    let dsi = enc.config().to_vec();
    let mut aus = Vec::new();
    for t in 0..5 {
        aus.push(enc.encode(&synth(96, 64, t)).unwrap());
    }
    // Strip the in-band headers from the first access unit, as an MP4
    // muxer that moves them into the esds would.
    aus[0].drain(..dsi.len());
    let mut dec = Decoder::with_config(&dsi).unwrap();
    assert_eq!(dec.vol().unwrap().width, 96);
    let mut frames = Vec::new();
    for au in &aus {
        frames.extend(dec.decode(au).unwrap());
    }
    frames.extend(dec.flush());
    assert_eq!(frames.len(), 5);
    // A decoder without the configuration refuses the VOPs.
    assert!(Decoder::new().decode(&aus[1]).is_err());
}

#[test]
fn whole_stream_in_one_buffer() {
    let cfg = EncoderConfig::new(64, 64, 30);
    let mut enc = Encoder::new(cfg).unwrap();
    let mut stream = Vec::new();
    for t in 0..7 {
        stream.extend(enc.encode(&synth(64, 64, t)).unwrap());
    }
    let mut dec = Decoder::new();
    let mut frames = dec.decode(&stream).unwrap();
    frames.extend(dec.flush());
    assert_eq!(frames.len(), 7);
    assert!(frames.windows(2).all(|w| w[0].timestamp < w[1].timestamp));
}

#[test]
fn b_frames() {
    for b in [1u32, 2, 3] {
        let mut cfg = EncoderConfig::new(176, 144, 25);
        cfg.b_frames = b;
        let r = run(cfg, 31);
        check(&r, 31.0, &format!("{b} B-VOPs"));
        assert!(r.frames.iter().any(|f| f.vop_type == VopType::B));
        for (i, f) in r.frames.iter().enumerate() {
            assert_eq!(f.timestamp, i as i64, "display order");
        }
    }
}

#[test]
fn b_frames_with_packets_four_vectors_and_rate_control() {
    let mut cfg = EncoderConfig::new(352, 288, 25);
    cfg.b_frames = 2;
    cfg.four_mv = true;
    cfg.packet_bytes = Some(400);
    cfg.rate = RateControl::Bitrate(500_000);
    cfg.gop_size = 10;
    let r = run(cfg, 25);
    check(&r, 28.0, "B-VOPs, 4MV, packets, 500 kb/s");
}

#[test]
fn b_frames_compress() {
    let mut cfg = EncoderConfig::new(176, 144, 25);
    cfg.gop_size = 0;
    let p = run(cfg.clone(), 24);
    cfg.b_frames = 2;
    let b = run(cfg, 24);
    println!("IPPP {} bytes, IBBP {} bytes", p.bytes, b.bytes);
    assert!(b.bytes < p.bytes * 11 / 10);
}
