//! Real encoders' streams, as data: every sample `tools/fetch-samples.sh`
//! fetches is decoded and its counts compared with what was recorded when
//! it was checked (frames, VOPs, and — the strong part — VOPs concealed and
//! VOPs whose macroblock data did not end exactly at the VOP's stuffing),
//! then decoded again damaged, which must not panic.
//!
//! No reference output exists for these streams (no other decoder is run),
//! so the check is the parse: a wrong VLC table, syntax order or vector
//! rule desynchronises a VOP long before its end, and every VOP of every
//! stream below parses to the bit. The pictures were also inspected by eye
//! when the figures were recorded (docs/SAMPLES.md).
//!
//! Skipped, with a note, when the samples are absent: set `MPEG4_SAMPLES`
//! or run `tools/fetch-samples.sh` (CI does), and `MPEG4_REQUIRE_SAMPLES=1`
//! to make their absence a failure.

mod common;

use common::media;
use mpeg4::{Decoder, DecoderStats};

/// What a sample decodes to.
enum Expect {
    /// Frames out, VOPs decoded, VOPs concealed, VOPs misaligned, VOPs
    /// dropped (not-coded placeholders of packed streams).
    Decodes { frames: usize, vops: u64, concealed: u64, misaligned: u64, dropped: u64 },
    /// Refused: an error naming this.
    Refused(&'static str),
}

use Expect::*;

const SAMPLES: &[(&str, Expect)] = &[
    // libavcodec, 320x240, video packets; its VOL says 5 ticks a second but
    // its VOPs code 15-bit increments.
    ("demo.m4v", Decodes { frames: 42, vops: 42, concealed: 0, misaligned: 0, dropped: 0 }),
    // DivX-style packed B-VOPs.
    ("packed_bframes.avi", Decodes { frames: 16, vops: 16, concealed: 0, misaligned: 0, dropped: 4 }),
    // Xvid, 720x576, Advanced Simple, MPEG quantiser.
    ("xvid_vlc_trac7411.h263", Decodes { frames: 20, vops: 20, concealed: 0, misaligned: 0, dropped: 0 }),
    // 400x300: neither dimension a multiple of 16.
    ("resize_down-up.h263", Decodes { frames: 150, vops: 150, concealed: 0, misaligned: 0, dropped: 0 }),
    // Early Xvid, interlaced (field DCT); the last VOP is cut off.
    ("ttm1.avi", Decodes { frames: 234, vops: 234, concealed: 1, misaligned: 0, dropped: 0 }),
    // DivX 5.00, B-VOPs, 624x350.
    ("test.b-frames.divx5.avi", Decodes { frames: 800, vops: 800, concealed: 0, misaligned: 0, dropped: 0 }),
    // DivX 5.00, GMC with two warping points, quarter-sample, B-VOPs.
    ("01.avi", Decodes { frames: 239, vops: 239, concealed: 0, misaligned: 0, dropped: 0 }),
    // DivX 5.03 and 6.6.1, quarter-sample, packed B-VOPs.
    ("divx.5.0.5-qpel.avi", Decodes { frames: 281, vops: 281, concealed: 0, misaligned: 0, dropped: 138 }),
    ("divx.6.6.1-qpel.avi", Decodes { frames: 281, vops: 281, concealed: 0, misaligned: 0, dropped: 97 }),
    // 712x368, MPEG quantiser; pads VOPs with ones.
    ("color16.avi", Decodes { frames: 117, vops: 117, concealed: 0, misaligned: 0, dropped: 0 }),
    // OpenDivX: a verid 2 VOL without the verid 2 fields; cut off.
    ("10-short.avi", Decodes { frames: 152, vops: 152, concealed: 1, misaligned: 0, dropped: 0 }),
    // MS H.263 (the short video header) captured in fixed 32 KB records:
    // the record padding after each picture counts as misaligned, and two
    // pictures are damaged at a GOB start.
    ("messenger.h263", Decodes { frames: 96, vops: 96, concealed: 2, misaligned: 94, dropped: 0 }),
    // RealMagic, 640x480; omits stuffing when byte aligned.
    ("greenlines.rmp4.p.avi", Decodes { frames: 290, vops: 290, concealed: 0, misaligned: 0, dropped: 0 }),
    ("greenlines.rmp4.p.di.avi", Decodes { frames: 290, vops: 290, concealed: 0, misaligned: 0, dropped: 0 }),
    // UB Video, 640x480, Main object type, MPEG quantiser.
    ("clip10-640x480-550k.avi", Decodes { frames: 354, vops: 354, concealed: 0, misaligned: 0, dropped: 0 }),
    // DivX (DXGM), 1024x464.
    ("0x4D475844-wow.avi", Decodes { frames: 648, vops: 648, concealed: 0, misaligned: 0, dropped: 0 }),
    // Xvid, 856x472, GMC with three warping points, quarter-sample, B-VOPs.
    ("xvid_gmcqpel_artifact.avi", Decodes { frames: 743, vops: 743, concealed: 0, misaligned: 0, dropped: 0 }),
    // Data partitioning (no reversible VLCs), QCIF, stuffing before markers.
    ("ErrDec_mpeg4datapart-64_qcif.m4v", Decodes { frames: 281, vops: 281, concealed: 0, misaligned: 0, dropped: 0 }),
    // DivX 5.03 quarter-sample, 640x408.
    ("DivX51-Qpel.avi", Decodes { frames: 600, vops: 600, concealed: 0, misaligned: 0, dropped: 0 }),
    // DivX 5.01: GMC, B-VOPs, packed, 720x540.
    ("dx502_b_qpel.avi", Decodes { frames: 600, vops: 600, concealed: 0, misaligned: 0, dropped: 298 }),
    ("vdpart-bug.avi", Decodes { frames: 16, vops: 16, concealed: 0, misaligned: 0, dropped: 0 }),
    // Xvid, B-VOPs, 720x480.
    ("qprd_cmp_b-frames_naq1.avi", Decodes { frames: 255, vops: 255, concealed: 0, misaligned: 0, dropped: 0 }),
    ("prezentaciaXvid.avi", Decodes { frames: 228, vops: 228, concealed: 0, misaligned: 0, dropped: 0 }),
    ("mpeg4_sstp_dpcm.m4v", Refused("Studio")),
    ("mpeg4-from-nc4000-w10.cmp", Refused("NEWPRED")),
];

fn dir() -> Option<std::path::PathBuf> {
    let d = std::env::var_os("MPEG4_SAMPLES")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/samples"));
    d.is_dir().then_some(d)
}

struct Outcome {
    frames: usize,
    stats: DecoderStats,
    errors: Vec<String>,
}

fn decode(units: &[Vec<u8>], dsi: &[u8]) -> Outcome {
    let mut d = Decoder::new();
    let mut errors = Vec::new();
    if !dsi.is_empty()
        && let Err(e) = d.configure(dsi)
    {
        errors.push(e.to_string());
    }
    let mut frames = 0;
    for u in units {
        match d.decode(u) {
            Ok(f) => frames += f.len(),
            Err(e) => errors.push(e.to_string()),
        }
    }
    frames += d.flush().len();
    Outcome { frames, stats: d.stats().clone(), errors }
}

#[test]
fn sample_streams() {
    let Some(dir) = dir() else {
        assert!(
            std::env::var_os("MPEG4_REQUIRE_SAMPLES").is_none(),
            "MPEG4_REQUIRE_SAMPLES is set but there are no samples: run tools/fetch-samples.sh"
        );
        eprintln!("no sample streams (tools/fetch-samples.sh fetches them): skipped");
        return;
    };
    let mut missing = Vec::new();
    for (name, expect) in SAMPLES {
        let Ok(data) = std::fs::read(dir.join(name)) else {
            missing.push(*name);
            continue;
        };
        let s = media::load(&data);
        let o = decode(&s.units, &s.dsi);
        match expect {
            Decodes { frames, vops, concealed, misaligned, dropped } => {
                assert!(o.errors.is_empty(), "{name}: {:?}", o.errors);
                let got = (o.frames, o.stats.vops, o.stats.concealed_vops, o.stats.misaligned_vops, o.stats.dropped_vops);
                assert_eq!(got, (*frames, *vops, *concealed, *misaligned, *dropped), "{name}: (frames, vops, concealed, misaligned, dropped)");
                println!("{name}: {frames} frames, {vops} VOPs, {concealed} concealed, {misaligned} misaligned");
            }
            Refused(what) => {
                assert!(o.errors.iter().any(|e| e.contains(what)), "{name}: {:?}", o.errors);
                println!("{name}: refused ({what})");
            }
        }
    }
    assert!(
        missing.is_empty() || std::env::var_os("MPEG4_REQUIRE_SAMPLES").is_none(),
        "missing samples: {missing:?}"
    );
}

/// Each sample damaged a few dozen ways: never a panic.
#[test]
fn damaged_sample_streams() {
    let Some(dir) = dir() else { return };
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for (name, _) in SAMPLES {
        let Ok(data) = std::fs::read(dir.join(name)) else { continue };
        let s = media::load(&data);
        // The first 40 access units keep it quick.
        let units: Vec<Vec<u8>> = s.units.into_iter().take(40).collect();
        for _ in 0..12 {
            let mut u = units.clone();
            for _ in 0..1 + rnd() % 20 {
                let k = (rnd() as usize) % u.len().max(1);
                if let Some(b) = u.get_mut(k)
                    && !b.is_empty()
                {
                    let n = b.len();
                    let i = (rnd() as usize) % n;
                    match rnd() % 3 {
                        0 => b[i] ^= 1 << (rnd() % 8),
                        1 => b.truncate(i),
                        _ => b[i] = rnd() as u8,
                    }
                }
            }
            let _ = decode(&u, &s.dsi);
        }
    }
}
