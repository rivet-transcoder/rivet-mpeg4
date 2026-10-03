//! Streams with reference pictures from outside this crate.
//!
//! - **ITU-T H.263** (`itu_h263`): the baseline stream of the bitstream set
//!   published on ITU-T's own server for the H.263 (version 2) work —
//!   `base_fmnq.263`, Foreman QCIF with every annex off — with the decoded
//!   sequence its producer published beside it. Baseline H.263 is what
//!   14496-2 decodes as the short video header, so every picture must
//!   match.
//! - **Xvid** (`xvid`): streams Xvid's encoder makes from a synthetic
//!   source, one per tool (B-VOPs, packed B-VOPs, quarter-sample motion,
//!   GMC, interlace, the MPEG quantiser, and their combinations), with
//!   Xvid's own decoder's pictures as the reference. Xvid is run as a
//!   black box (`tools/xvid-vectors.sh` builds it and makes the set); none
//!   of its source is read.
//!
//! `tools/fetch-conformance.sh` fetches the ITU-T set and
//! `tools/xvid-vectors.sh` makes the Xvid one, both into
//! `tests/conformance/` (or `$MPEG4_CONFORMANCE`). Each test skips, with a
//! note, when its set is absent, unless `MPEG4_REQUIRE_CONFORMANCE=1` (as
//! in CI), which makes the absence a failure.

mod common;

use common::media;
use mpeg4::{Decoder, Frame};
use std::path::PathBuf;

fn dir(sub: &str) -> Option<PathBuf> {
    let d = std::env::var_os("MPEG4_CONFORMANCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/conformance"))
        .join(sub);
    if d.is_dir() {
        return Some(d);
    }
    assert!(
        std::env::var_os("MPEG4_REQUIRE_CONFORMANCE").is_none(),
        "MPEG4_REQUIRE_CONFORMANCE is set but {} is missing",
        d.display()
    );
    eprintln!("{} is absent: skipped", d.display());
    None
}

/// Decodes every access unit of a stream; fails on any error.
fn decode_all(name: &str, data: &[u8]) -> (Vec<Frame>, mpeg4::DecoderStats) {
    let s = media::load(data);
    let mut d = Decoder::new();
    if !s.dsi.is_empty() {
        d.configure(&s.dsi)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
    }
    let mut out = Vec::new();
    for u in &s.units {
        out.extend(d.decode(u).unwrap_or_else(|e| panic!("{name}: {e}")));
    }
    out.extend(d.flush());
    (out, d.stats().clone())
}

/// Planar 4:2:0 pictures of `w` x `h`, back to back.
fn yuv_frames(data: &[u8], w: u32, h: u32) -> Vec<&[u8]> {
    let (cw, ch) = Frame::chroma_size(w, h);
    let size = (w * h + 2 * cw * ch) as usize;
    assert_eq!(
        data.len() % size,
        0,
        "reference is not whole {w}x{h} pictures"
    );
    data.chunks(size).collect()
}

/// Largest absolute difference and the luma PSNR of a frame against a
/// packed planar reference picture.
fn compare(f: &Frame, reference: &[u8]) -> (u8, f64) {
    let mut ours = Vec::with_capacity(reference.len());
    for i in 0..3 {
        ours.extend_from_slice(f.plane(i));
    }
    assert_eq!(ours.len(), reference.len(), "picture size");
    let max = ours
        .iter()
        .zip(reference)
        .map(|(&a, &b)| a.abs_diff(b))
        .max()
        .unwrap_or(0);
    let n = (f.width * f.height) as usize;
    let mse = ours[..n]
        .iter()
        .zip(&reference[..n])
        .map(|(&a, &b)| (a as f64 - b as f64).powi(2))
        .sum::<f64>()
        / n as f64;
    let psnr = if mse == 0.0 {
        99.0
    } else {
        10.0 * (255.0f64 * 255.0 / mse).log10()
    };
    (max, psnr)
}

/// Baseline H.263 against its producer's decoded pictures: every picture
/// within one sample of the reference (the inverse DCTs' rounding, which
/// H.263 leaves to the IEEE 1180 accuracy test), with no drift.
#[test]
fn itu_h263() {
    let Some(dir) = dir("itu-h263") else { return };
    let bits = std::fs::read(dir.join("base_fmnq.263")).expect("base_fmnq.263");
    let reference = std::fs::read(dir.join("base_fmnq.dec")).expect("base_fmnq.dec");
    let (frames, stats) = decode_all("base_fmnq.263", &bits);
    let refs = yuv_frames(&reference, 176, 144);
    assert_eq!(frames.len(), 68, "pictures decoded");
    assert_eq!(refs.len(), 68, "reference pictures");
    assert_eq!((stats.concealed_vops, stats.misaligned_vops), (0, 0));
    let mut worst = 99.0f64;
    for (i, (f, r)) in frames.iter().zip(&refs).enumerate() {
        assert_eq!((f.width, f.height), (176, 144));
        let (max, psnr) = compare(f, r);
        assert!(
            max <= 1 && psnr > 60.0,
            "picture {i}: largest difference {max}, PSNR {psnr:.2} dB"
        );
        worst = worst.min(psnr);
    }
    println!(
        "base_fmnq.263: 68 pictures, each within 1 of the reference, worst luma PSNR {worst:.2} dB"
    );
}

/// Streams in tools the crate does not decode are refused, naming the tool:
/// H.263 version 2's extended picture type (annexes D, F, I, J, S, T) and
/// the Studio profiles (an ISO/IEC 14496-4 Simple Studio stream).
#[test]
fn refused_conformance_streams() {
    for (sub, name, what) in [
        ("itu-h263", "dfijst_fmnq.263", "PLUSPTYPE"),
        ("iso-14496-4", "vcon-stp12L2.bits", "Studio"),
    ] {
        let Some(dir) = dir(sub) else { continue };
        let data = std::fs::read(dir.join(name)).unwrap_or_else(|_| panic!("{sub}/{name}"));
        let s = media::load(&data);
        let mut d = Decoder::new();
        let mut frames = 0;
        let mut errors = Vec::new();
        for u in &s.units {
            match d.decode(u) {
                Ok(f) => frames += f.len(),
                Err(e) => errors.push(e.to_string()),
            }
        }
        frames += d.flush().len();
        assert_eq!(frames, 0, "{name}: decoded pictures");
        assert!(
            errors.iter().any(|e| e.contains(what)),
            "{name}: {:?}",
            errors.first()
        );
        println!("{name}: refused ({what})");
    }
}

/// A stream set made by `tools/xvid-vectors.sh`: `<name>.m4v`, the
/// reference pictures `<name>.<ext>` (I420, display order) and
/// `<name>.size`. Every case in `limits` must be present; each must decode
/// without error, concealment or misalignment to as many pictures as the
/// reference, every picture within `(largest sample difference, lowest luma
/// PSNR)` of it.
fn check_set(sub: &str, ext: &str, limits: &[(&str, u8, f64)]) {
    let Some(dir) = dir(sub) else { return };
    let verbose = std::env::var_os("MPEG4_VERBOSE").is_some();
    let mut failures = Vec::new();
    for &(name, max_diff, min_psnr) in limits {
        let read = |e: &str| {
            std::fs::read(dir.join(format!("{name}.{e}")))
                .unwrap_or_else(|_| panic!("{sub}/{name}.{e} is missing"))
        };
        let (bits, reference) = (read("m4v"), read(ext));
        let size = String::from_utf8(read("size")).unwrap();
        let mut wh = size.split_whitespace().map(|v| v.parse::<u32>().unwrap());
        let (w, h) = (wh.next().unwrap(), wh.next().unwrap());
        let (frames, stats) = decode_all(name, &bits);
        let refs = yuv_frames(&reference, w, h);
        let mut worst = (0u8, 99.0f64);
        for (i, (f, r)) in frames.iter().zip(&refs).enumerate() {
            let (max, psnr) = compare(f, r);
            worst = (worst.0.max(max), worst.1.min(psnr));
            if verbose {
                println!("  {name} {i}: {:?} max {max} psnr {psnr:.2}", f.vop_type);
            }
        }
        println!(
            "{sub}/{name}: {} pictures (reference {}), {} concealed, {} misaligned, largest difference {}, worst luma PSNR {:.2} dB",
            frames.len(),
            refs.len(),
            stats.concealed_vops,
            stats.misaligned_vops,
            worst.0,
            worst.1
        );
        if frames.len() != refs.len()
            || stats.concealed_vops != 0
            || stats.misaligned_vops != 0
            || worst.0 > max_diff
            || worst.1 < min_psnr
        {
            failures.push(name);
        }
    }
    assert!(failures.is_empty(), "{sub}: {failures:?}");
}

/// Xvid's streams against Xvid's decoder.
///
/// The two inverse DCTs differ by the rounding 14496-2 allows (IEEE 1180),
/// so pictures agree to a sample or two, drifting a little over each
/// 12-picture GOP. The quarter-sample cases drift further: Xvid computes
/// the quarter positions that lie between two half-sample rows or
/// columns (horizontal quarter with vertical half or quarter) by filtering
/// the horizontally interpolated row, where this decoder averages the
/// neighbouring half-sample values as 7.6.2.1 describes them; the
/// predictions differ by one at about a third of those samples
/// (docs/CONFORMANCE.md).
#[test]
fn xvid() {
    check_set(
        "xvid",
        "yuv",
        &[
            ("simple", 4, 52.0),
            ("bframes", 4, 52.0),
            ("packed", 4, 52.0),
            ("mpegquant", 4, 52.0),
            ("gmc", 4, 52.0),
            ("interlaced", 4, 52.0),
            ("interlaced_b", 4, 52.0),
            ("slices", 4, 52.0),
            ("fine", 4, 52.0),
            ("coarse", 4, 52.0),
            ("qpel", 8, 44.0),
            ("qpel_b", 8, 44.0),
            ("gmc_qpel_b", 8, 44.0),
            ("oddsize", 8, 44.0),
        ],
    );
}

/// This crate's encoder's streams, decoded by Xvid's decoder, against the
/// encoder's own reconstruction: another decoder reads what the encoder
/// writes, to the IDCT's rounding.
#[test]
fn rivet_encoder_in_xvid() {
    check_set(
        "rivet-enc",
        "yuv",
        &[
            ("simple", 4, 52.0),
            ("four_mv", 4, 52.0),
            ("bframes", 4, 52.0),
            ("packets", 4, 52.0),
            ("packets_b", 4, 52.0),
            ("wide_search", 4, 52.0),
            ("oddsize", 4, 52.0),
            ("fine", 4, 52.0),
            ("coarse", 4, 52.0),
            ("bitrate", 4, 52.0),
        ],
    );
}

/// Every stream of every set, damaged a few dozen ways: never a panic.
#[test]
fn damaged_conformance_streams() {
    let root = std::env::var_os("MPEG4_CONFORMANCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/conformance"));
    let mut files = Vec::new();
    for sub in ["itu-h263", "iso-14496-4", "xvid", "rivet-enc"] {
        if let Ok(rd) = std::fs::read_dir(root.join(sub)) {
            for e in rd.flatten() {
                let p = e.path();
                if matches!(
                    p.extension().and_then(|x| x.to_str()),
                    Some("m4v" | "263" | "bits")
                ) {
                    files.push(p);
                }
            }
        }
    }
    files.sort();
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for path in &files {
        let data = std::fs::read(path).unwrap();
        // The first 40 access units keep it quick.
        let units: Vec<Vec<u8>> = media::load(&data).units.into_iter().take(40).collect();
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
            let mut d = Decoder::new();
            for b in &u {
                let _ = d.decode(b);
            }
            let _ = d.flush();
        }
    }
    println!("{} streams damaged", files.len());
}
