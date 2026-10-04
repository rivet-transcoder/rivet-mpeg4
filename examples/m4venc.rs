//! Writes this crate's encoder's output for a fixed set of configurations,
//! for another decoder to read: `tools/xvid-vectors.sh` decodes them with
//! Xvid's decoder and `tests/conformance.rs` compares its pictures with
//! this crate's.
//!
//! `cargo run --release --example m4venc -- OUT_DIR`
//!
//! For each configuration: `<name>.m4v` (the elementary stream) and
//! `<name>.size` (`W H`). The decoder reproduces the encoder's
//! reconstruction exactly (`tests/roundtrip.rs`), so another decoder's
//! pictures are compared with this crate's decode of the stream.

#[path = "../tests/common/mod.rs"]
mod common;

use mpeg4::{Encoder, EncoderConfig, Quantiser, RateControl};
use std::path::Path;

fn main() {
    let out = std::env::args().nth(1).expect("usage: m4venc OUT_DIR");
    let out = Path::new(&out);
    std::fs::create_dir_all(out).expect("create OUT_DIR");
    let base = |w, h| {
        let mut c = EncoderConfig::new(w, h, 25);
        c.gop_size = 12;
        c
    };
    let mut cases: Vec<(&str, EncoderConfig, u32)> = Vec::new();
    let mut interlaced: Vec<(&str, EncoderConfig, u32, f64)> = Vec::new();
    cases.push(("simple", base(320, 240), 30));
    let mut c = base(320, 240);
    c.four_mv = true;
    cases.push(("four_mv", c, 30));
    let mut c = base(320, 240);
    c.b_frames = 2;
    cases.push(("bframes", c, 30));
    let mut c = base(352, 288);
    c.packet_bytes = Some(300);
    cases.push(("packets", c, 30));
    let mut c = base(352, 288);
    c.b_frames = 2;
    c.four_mv = true;
    c.packet_bytes = Some(400);
    cases.push(("packets_b", c, 30));
    let mut c = base(320, 240);
    c.search_range = 100;
    cases.push(("wide_search", c, 20));
    let mut c = base(200, 150);
    c.b_frames = 1;
    cases.push(("oddsize", c, 30));
    let mut c = base(320, 240);
    c.rate = RateControl::ConstantQuant(1);
    cases.push(("fine", c, 12));
    let mut c = base(320, 240);
    c.rate = RateControl::ConstantQuant(31);
    c.b_frames = 3;
    cases.push(("coarse", c, 30));
    let mut c = base(352, 288);
    c.rate = RateControl::Bitrate(400_000);
    c.b_frames = 2;
    cases.push(("bitrate", c, 40));
    // Quarter-sample motion: every interpolation position (7.6.2.2) read
    // by another decoder, with one vector, four, and B-VOPs.
    let mut c = base(320, 240);
    c.quarter_sample = true;
    cases.push(("qpel", c, 30));
    let mut c = base(320, 240);
    c.quarter_sample = true;
    c.four_mv = true;
    c.b_frames = 2;
    cases.push(("qpel_4mv_b", c, 30));
    // Interlaced, field DCT and field prediction in P-VOPs (vectors from
    // either reference field, near the picture's edges too), from
    // interlaced content, either field first, the MPEG quantiser or
    // H.263's, with and without B-VOPs (frame-predicted: Xvid's decoder
    // misreads field-predicted B-VOP macroblocks, see
    // `EncoderConfig::b_field_prediction`). The content moves slowly enough
    // that no vector reaches past the border Xvid's decoder pads its
    // references with.
    for (name, tff, mpeg, b, w, h, speed) in [
        ("interlaced_p_bff_mpeg", false, true, 0, 352, 288, 6.0),
        ("interlaced_p_tff", true, false, 0, 320, 240, 6.0),
        ("interlaced_b_bff_mpeg", false, true, 1, 352, 288, 2.0),
        ("interlaced_b_tff", true, false, 2, 320, 240, 2.0),
    ] {
        let mut c = base(w, h);
        c.interlaced = true;
        c.top_field_first = tff;
        c.b_frames = b;
        if mpeg {
            c.quantiser = Quantiser::mpeg_default();
        }
        interlaced.push((name, c, 30, speed));
    }

    let all = cases
        .into_iter()
        .map(|(name, c, n)| (name, c, n, 0.0))
        .chain(interlaced);
    for (name, cfg, n, speed) in all {
        let (w, h) = (cfg.width, cfg.height);
        let (cfg_interlaced, tff) = (cfg.interlaced, cfg.top_field_first);
        let mut enc = Encoder::new(cfg).expect("config");
        let mut stream = Vec::new();
        for t in 0..n {
            let src = if cfg_interlaced {
                common::synth_interlaced(w, h, t, speed, tff)
            } else {
                common::synth(w, h, t)
            };
            stream.extend(enc.encode(&src).expect("encode"));
        }
        stream.extend(enc.finish().expect("finish"));
        std::fs::write(out.join(format!("{name}.m4v")), &stream).expect("write");
        std::fs::write(
            out.join(format!("{name}.size")),
            format!(
                "{w} {h}
"
            ),
        )
        .expect("write");
        println!("{name}: {} bytes, {n} pictures", stream.len());
    }
}
