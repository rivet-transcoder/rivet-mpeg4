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

use mpeg4::{Encoder, EncoderConfig, RateControl};
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

    for (name, cfg, n) in cases {
        let (w, h) = (cfg.width, cfg.height);
        let mut enc = Encoder::new(cfg).expect("config");
        let mut stream = Vec::new();
        for t in 0..n {
            stream.extend(enc.encode(&common::synth(w, h, t)).expect("encode"));
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
