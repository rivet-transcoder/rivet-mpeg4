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
    let st = dec.stats();
    assert_eq!(
        (st.concealed_vops, st.misaligned_vops),
        (0, 0),
        "every VOP decodes cleanly to its stuffing: {:?}",
        dec.last_error()
    );
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

#[test]
fn data_partitioning() {
    for (rvlc, packets) in [
        (false, None),
        (false, Some(300)),
        (true, None),
        (true, Some(300)),
    ] {
        let mut cfg = EncoderConfig::new(352, 288, 25);
        cfg.data_partitioning = true;
        cfg.reversible_vlc = rvlc;
        cfg.packet_bytes = packets;
        cfg.four_mv = true;
        cfg.gop_size = 6;
        let r = run(cfg, 12);
        check(
            &r,
            33.0,
            &format!("data partitioned, RVLC {rvlc}, packets {packets:?}"),
        );
    }
}

#[test]
fn forced_keyframes() {
    for b in [0u32, 2] {
        let mut cfg = EncoderConfig::new(96, 64, 25);
        cfg.gop_size = 0;
        cfg.b_frames = b;
        cfg.keep_reconstructions = true;
        let mut enc = Encoder::new(cfg).unwrap();
        let mut dec = Decoder::new();
        let mut frames = Vec::new();
        let mut recon = Vec::new();
        for t in 0..12 {
            if t == 5 || t == 9 {
                enc.force_keyframe();
            }
            let au = enc.encode(&synth(96, 64, t)).unwrap();
            frames.extend(dec.decode(&au).unwrap());
            recon.extend(enc.take_reconstructions());
        }
        frames.extend(dec.decode(&enc.finish().unwrap()).unwrap());
        recon.extend(enc.take_reconstructions());
        frames.extend(dec.flush());
        recon.sort_by_key(|f| f.timestamp);
        assert_eq!(frames.len(), 12);
        let intra: Vec<i64> = frames
            .iter()
            .filter(|f| f.vop_type == VopType::I)
            .map(|f| f.timestamp)
            .collect();
        assert_eq!(intra, [0, 5, 9], "{b} B-VOPs");
        for (d, e) in frames.iter().zip(&recon) {
            assert_eq!(d.data, e.data);
        }
    }
}

#[test]
fn video_signal_type_is_signalled() {
    use mpeg4::{ColourDescription, VideoSignal};
    let mut cfg = EncoderConfig::new(64, 48, 25);
    let vs = VideoSignal {
        video_format: 5,
        full_range: true,
        colour: Some(ColourDescription::BT709),
    };
    cfg.video_signal = Some(vs);
    let enc = Encoder::new(cfg.clone()).unwrap();
    let dec = Decoder::with_config(enc.config()).unwrap();
    assert_eq!(dec.vol().unwrap().video_signal, Some(vs));
    cfg.video_signal = None;
    let enc = Encoder::new(cfg).unwrap();
    let dec = Decoder::with_config(enc.config()).unwrap();
    assert_eq!(dec.vol().unwrap().video_signal, None);
}

#[test]
fn quarter_sample_motion() {
    for b in [0u32, 2] {
        let mut cfg = EncoderConfig::new(176, 144, 25);
        cfg.quarter_sample = true;
        cfg.b_frames = b;
        cfg.four_mv = true;
        let r = run(cfg, 16);
        check(&r, 33.0, &format!("quarter-sample, {b} B-VOPs"));
    }
}

#[test]
fn mpeg_quantiser() {
    let mut flat = [16u8; 64];
    flat[0] = 8;
    let mut steep = mpeg4::Quantiser::mpeg_default();
    if let mpeg4::Quantiser::Mpeg { inter, .. } = &mut steep {
        for (i, v) in inter.iter_mut().enumerate() {
            *v = 12 + (i % 8 + i / 8) as u8 * 4;
        }
    }
    for (name, q) in [
        ("default matrices", mpeg4::Quantiser::mpeg_default()),
        (
            "flat matrices",
            mpeg4::Quantiser::Mpeg {
                intra: flat,
                inter: [16; 64],
            },
        ),
        ("steep inter matrix", steep),
    ] {
        for qp in [2u8, 5, 20] {
            let mut cfg = EncoderConfig::new(176, 144, 25);
            cfg.quantiser = q.clone();
            cfg.rate = RateControl::ConstantQuant(qp);
            cfg.gop_size = 8;
            let r = run(cfg, 12);
            let min = match qp {
                2 => 40.0,
                5 => 33.0,
                _ => 26.0,
            };
            check(&r, min, &format!("MPEG quantiser, {name}, q{qp}"));
        }
    }
}

#[test]
fn advanced_simple_tools_together() {
    let mut cfg = EncoderConfig::new(352, 288, 25);
    cfg.quarter_sample = true;
    cfg.quantiser = mpeg4::Quantiser::mpeg_default();
    cfg.b_frames = 2;
    cfg.four_mv = true;
    cfg.packet_bytes = Some(500);
    cfg.data_partitioning = true;
    cfg.reversible_vlc = true;
    cfg.gop_size = 9;
    let r = run(cfg, 18);
    check(
        &r,
        33.0,
        "quarter-sample, MPEG quantiser, B-VOPs, 4MV, packets, data partitioning, RVLC",
    );
}

#[test]
fn short_video_header() {
    for (w, h, packets) in [(128, 96, None), (176, 144, None), (352, 288, Some(400))] {
        let mut cfg = EncoderConfig::new(w, h, 30);
        cfg.short_header = true;
        cfg.packet_bytes = packets;
        cfg.gop_size = 10;
        cfg.search_range = 31; // capped at 15 for f_code 1
        let r = run(cfg.clone(), 14);
        check(
            &r,
            32.0,
            &format!("short header {w}x{h}, GOB headers {packets:?}"),
        );
        assert!(r.frames.iter().all(|f| f.time_base == 30000));
        // 30 frames a second: 1001 ticks apart, TR 1 apart.
        assert!(
            r.frames
                .windows(2)
                .all(|p| p[1].timestamp - p[0].timestamp == 1001)
        );
        let enc = Encoder::new(cfg).unwrap();
        assert!(enc.config().is_empty());
    }
    // Other sizes and the Advanced Simple tools are refused.
    let mut cfg = EncoderConfig::new(160, 120, 30);
    cfg.short_header = true;
    assert!(Encoder::new(cfg.clone()).is_err());
    cfg.width = 176;
    cfg.height = 144;
    cfg.b_frames = 1;
    assert!(Encoder::new(cfg).is_err());
}

/// Runs `n` interlaced frames through encoder and decoder; returns the
/// decoder's stats with the run.
fn run_interlaced(mut cfg: EncoderConfig, n: u32, speed: f64) -> (Run, mpeg4::DecoderStats) {
    cfg.keep_reconstructions = true;
    let (w, h, tff) = (cfg.width, cfg.height, cfg.top_field_first);
    let mut enc = Encoder::new(cfg).unwrap();
    let mut dec = Decoder::new();
    let (mut frames, mut recon, mut sources, mut bytes) = (Vec::new(), Vec::new(), Vec::new(), 0);
    for t in 0..n {
        let src = common::synth_interlaced(w, h, t, speed, tff);
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
    recon.sort_by_key(|f| f.timestamp);
    let st = dec.stats().clone();
    assert_eq!(
        (st.concealed_vops, st.misaligned_vops),
        (0, 0),
        "{:?}",
        dec.last_error()
    );
    (
        Run {
            frames,
            recon,
            sources,
            bytes,
        },
        st,
    )
}

#[test]
fn interlaced() {
    for (b, tff, qpel) in [
        (0u32, true, false),
        (2, true, false),
        (2, false, false),
        (1, true, true),
    ] {
        let mut cfg = EncoderConfig::new(352, 288, 25);
        cfg.interlaced = true;
        cfg.top_field_first = tff;
        cfg.b_frames = b;
        cfg.quarter_sample = qpel;
        cfg.gop_size = 12;
        let (r, st) = run_interlaced(cfg, 13, 6.0);
        let label = format!("interlaced, {b} B-VOPs, top field first {tff}, quarter-sample {qpel}");
        check(&r, 30.0, &label);
        println!("{label}: {} field direct macroblocks", st.field_direct_mbs);
        if b > 0 {
            assert!(
                st.field_direct_mbs > 0,
                "field direct mode was not exercised"
            );
        }
    }
}

#[test]
fn interlaced_compresses_interlaced_content() {
    let mut cfg = EncoderConfig::new(352, 288, 25);
    cfg.gop_size = 0;
    let (prog, _) = run_interlaced(cfg.clone(), 8, 6.0);
    cfg.interlaced = true;
    let (int, _) = run_interlaced(cfg, 8, 6.0);
    let p = check(&prog, 30.0, "interlaced content, progressive coding");
    let i = check(&int, 30.0, "interlaced content, interlaced coding");
    println!(
        "progressive {} bytes {p:.2} dB, interlaced {} bytes {i:.2} dB",
        prog.bytes, int.bytes
    );
    assert!(
        int.bytes < prog.bytes,
        "field tools should pay on interlaced content"
    );
}

#[test]
fn overlapped_block_motion_compensation() {
    for (four, qpel, packets) in [
        (false, false, None),
        (true, false, Some(300)),
        (true, true, None),
    ] {
        let mut cfg = EncoderConfig::new(176, 144, 25);
        cfg.obmc = true;
        cfg.four_mv = four;
        cfg.quarter_sample = qpel;
        cfg.packet_bytes = packets;
        let r = run(cfg, 14);
        check(
            &r,
            33.0,
            &format!("OBMC, 4MV {four}, quarter-sample {qpel}, packets {packets:?}"),
        );
    }
    // H.263 Advanced Prediction through the short header path.
    for four in [false, true] {
        let mut cfg = EncoderConfig::new(176, 144, 30);
        cfg.short_header = true;
        cfg.obmc = true;
        cfg.four_mv = four;
        let r = run(cfg, 14);
        check(&r, 32.0, &format!("H.263 Advanced Prediction, 4MV {four}"));
    }
}
