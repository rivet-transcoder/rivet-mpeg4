//! Malformed input: arbitrary bytes, and valid streams damaged — bits
//! flipped, bytes cut, garbage spliced — fed to every entry point. The
//! decoder must return errors or concealed frames, never panic. Run it in
//! a debug build too (`cargo test --test fuzz`), where integer overflow is
//! a panic.

mod common;

use mpeg4::{Decoder, Encoder, EncoderConfig, RateControl};
use proptest::prelude::*;

/// A short valid stream: configuration headers, then the VOPs one access
/// unit each.
fn stream(b_frames: u32, packets: bool, four_mv: bool) -> Vec<Vec<u8>> {
    stream_with(b_frames, packets, four_mv, |_| {})
}

/// [`stream`] with further settings.
fn stream_with(
    b_frames: u32,
    packets: bool,
    four_mv: bool,
    f: impl Fn(&mut EncoderConfig),
) -> Vec<Vec<u8>> {
    let mut cfg = EncoderConfig::new(48, 32, 25);
    cfg.b_frames = b_frames;
    cfg.four_mv = four_mv;
    cfg.gop_size = 6;
    cfg.rate = RateControl::ConstantQuant(4);
    if packets {
        cfg.packet_bytes = Some(40);
    }
    f(&mut cfg);
    let mut enc = Encoder::new(cfg).unwrap();
    let mut aus = Vec::new();
    for t in 0..8 {
        let au = enc.encode(&common::synth(48, 32, t)).unwrap();
        if !au.is_empty() {
            aus.push(au);
        }
    }
    let tail = enc.finish().unwrap();
    if !tail.is_empty() {
        aus.push(tail);
    }
    aus
}

/// Every access unit through one decoder, then a flush.
fn decode_all(aus: &[Vec<u8>]) {
    let mut d = Decoder::new();
    for au in aus {
        let _ = d.decode(au);
    }
    let _ = d.flush();
}

/// Configuration headers, for feeding damaged VOPs to a decoder that has
/// a configuration.
fn config() -> Vec<u8> {
    Encoder::new(EncoderConfig::new(48, 32, 25))
        .unwrap()
        .config()
        .to_vec()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn arbitrary_bytes(data in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let _ = Decoder::new().decode(&data);
        let _ = Decoder::with_config(&data);
        // The same bytes behind a valid configuration, and behind a VOP
        // start code, so they reach the macroblock layer.
        let mut d = Decoder::with_config(&config()).unwrap();
        let mut v = vec![0, 0, 1, 0xb6];
        v.extend(&data);
        let _ = d.decode(&v);
        let _ = d.decode(&data);
        let _ = d.flush();
        // And as a short video header picture.
        let mut h = vec![0, 0, 0x80, 0x02];
        h.extend(&data);
        let _ = Decoder::new().decode(&h);
    }

    #[test]
    fn damaged_streams(
        kind in 0usize..6,
        flips in proptest::collection::vec((any::<usize>(), 0u8..8), 1..12),
        cut in any::<usize>(),
        splice in proptest::collection::vec(any::<u8>(), 0..32),
    ) {
        let mut aus = match kind {
            0 => stream(0, false, false),
            1 => stream(2, false, true),
            2 => stream(0, true, true),
            3 => stream(1, true, false),
            4 => stream_with(0, true, true, |c| {
                c.data_partitioning = true;
                c.quarter_sample = true;
            }),
            _ => stream_with(1, true, false, |c| {
                c.data_partitioning = true;
                c.reversible_vlc = true;
            }),
        };
        let count = aus.len();
        for (pos, bit) in &flips {
            let k = pos % count;
            let n = aus[k].len();
            if n > 0 {
                aus[k][(pos / count) % n] ^= 1 << bit;
            }
        }
        decode_all(&aus);
        let k = cut % aus.len();
        let n = aus[k].len();
        aus[k].truncate(cut % (n + 1));
        let at = cut % (aus[k].len() + 1);
        aus[k].splice(at..at, splice.iter().copied());
        decode_all(&aus);
        // All of it in one buffer.
        decode_all(&[aus.concat()]);
    }

    #[test]
    fn encoder_configurations(
        w in 1u32..80, h in 1u32..80,
        b in 0u32..4, four in any::<bool>(), q in 1u8..32,
        packets in proptest::option::of(8u32..200),
        range in 1u32..64, gop in 0u32..5,
        dp in 0u8..3, qpel in any::<bool>(), mpeg in any::<bool>(),
    ) {
        let mut cfg = EncoderConfig::new(w, h, 30);
        cfg.b_frames = b;
        cfg.four_mv = four;
        cfg.rate = RateControl::ConstantQuant(q);
        cfg.packet_bytes = packets;
        cfg.search_range = range;
        cfg.gop_size = gop;
        cfg.data_partitioning = dp > 0;
        cfg.reversible_vlc = dp > 1;
        cfg.quarter_sample = qpel;
        if mpeg {
            cfg.quantiser = mpeg4::Quantiser::mpeg_default();
        }
        cfg.keep_reconstructions = true;
        let mut enc = Encoder::new(cfg).unwrap();
        let mut dec = Decoder::new();
        let mut frames = Vec::new();
        let mut recon = Vec::new();
        for t in 0..5 {
            let au = enc.encode(&common::synth(w, h, t)).unwrap();
            frames.extend(dec.decode(&au).unwrap());
            recon.extend(enc.take_reconstructions());
        }
        frames.extend(dec.decode(&enc.finish().unwrap()).unwrap());
        recon.extend(enc.take_reconstructions());
        frames.extend(dec.flush());
        recon.sort_by_key(|f| f.timestamp);
        prop_assert_eq!(frames.len(), 5);
        for (d, e) in frames.iter().zip(&recon) {
            prop_assert!(!d.concealed);
            prop_assert_eq!(&d.data, &e.data);
        }
    }
}
