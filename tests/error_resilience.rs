//! Error resilience with reversible VLCs: a data-partitioned stream from
//! this crate's encoder is damaged in the middle of a texture partition,
//! and the decoder must recover what Annex E.1.4.4 says it can — the
//! macroblocks before the damage by reading forwards, those after it by
//! reading backwards from the next resync marker (or the VOP's end) —
//! reproducing them exactly as the encoder reconstructed them, and conceal
//! only the macroblocks between.

mod common;

use common::synth;
use mpeg4::{Decoder, Encoder, EncoderConfig, Frame};

const W: u32 = 176;
const H: u32 = 144;

fn bit(d: &[u8], i: usize) -> u32 {
    (d[i >> 3] >> (7 - (i & 7)) & 1) as u32
}

fn bits(d: &[u8], i: usize, n: usize) -> u32 {
    (0..n).fold(0, |v, k| v << 1 | bit(d, i + k))
}

/// Encodes `n` frames, one access unit per frame (no B-VOPs).
fn encode(cfg: EncoderConfig, n: u32) -> (Vec<Vec<u8>>, Vec<Frame>) {
    let mut enc = Encoder::new(EncoderConfig {
        keep_reconstructions: true,
        ..cfg
    })
    .unwrap();
    let mut aus = Vec::new();
    let mut recon = Vec::new();
    for t in 0..n {
        aus.push(enc.encode(&synth(W, H, t)).unwrap());
        recon.extend(enc.take_reconstructions());
    }
    (aus, recon)
}

/// The macroblocks of `a` that differ from `b`, in raster order.
fn differing_mbs(a: &Frame, b: &Frame) -> Vec<usize> {
    let mbw = W.div_ceil(16) as usize;
    let mbh = H.div_ceil(16) as usize;
    let mut out = Vec::new();
    for mby in 0..mbh {
        for mbx in 0..mbw {
            let mut differs = false;
            for r in 0..16 {
                for c in 0..16 {
                    let (x, y) = (mbx * 16 + c, mby * 16 + r);
                    differs |= a.plane(0)[y * W as usize + x] != b.plane(0)[y * W as usize + x];
                }
            }
            for p in 1..3 {
                let cw = (W / 2) as usize;
                for r in 0..8 {
                    for c in 0..8 {
                        let i = (mby * 8 + r) * cw + mbx * 8 + c;
                        differs |= a.plane(p)[i] != b.plane(p)[i];
                    }
                }
            }
            if differs {
                out.push(mby * mbw + mbx);
            }
        }
    }
    out
}

/// The bit range of the first packet's second and texture partitions in a
/// VOP: from the end of
/// `marker` (`marker_len` bits, the first one found after the VOP start
/// code) to the stuffing before the next resync marker or the VOP's end.
fn texture_range(vop: &[u8], marker: u32, marker_len: usize, resync_len: usize) -> (usize, usize) {
    let start = (0..vop.len() * 8 - marker_len)
        .find(|&i| i > 32 && bits(vop, i, marker_len) == marker)
        .expect("partition marker")
        + marker_len;
    let mut end = vop.len() * 8;
    let mut p = start.div_ceil(8) * 8;
    while p + resync_len <= vop.len() * 8 {
        if bits(vop, p, resync_len) == 1 {
            end = p;
            break;
        }
        p += 8;
    }
    // Back over the stuffing: ones, then the zero.
    while bit(vop, end - 1) == 1 {
        end -= 1;
    }
    (start, end - 1)
}

struct Damage {
    differing: Vec<usize>,
    backward: u64,
    discarded: u64,
    concealed: u64,
}

/// Decodes the stream with access unit `au` damaged by clearing 16 bits at
/// the middle of its first packet's second and texture partitions (which
/// lands in the texture), and compares that
/// frame with the encoder's reconstruction.
fn damage(
    aus: &[Vec<u8>],
    recon: &[Frame],
    au: usize,
    marker: u32,
    marker_len: usize,
    resync_len: usize,
) -> Damage {
    let mut aus = aus.to_vec();
    // The VOP is the last start-code unit of the access unit.
    let vop_at = (0..aus[au].len() - 3)
        .rev()
        .find(|&i| aus[au][i..i + 4] == [0, 0, 1, 0xb6])
        .unwrap();
    let vop = &mut aus[au][vop_at..];
    let (t0, e) = texture_range(vop, marker, marker_len, resync_len);
    let mid = (t0 + e) / 2;
    println!(
        "partitions 2 and 3 at bits {t0}..{e} ({} bits), damaged at {mid}",
        e - t0
    );
    // Sixteen zero bits: no reversible code or escape reads through them
    // in either direction.
    for k in mid..mid + 16 {
        vop[k >> 3] &= !(0x80 >> (k & 7));
    }
    let mut dec = Decoder::new();
    let mut frames = Vec::new();
    for a in &aus[..=au] {
        frames.extend(dec.decode(a).unwrap());
    }
    frames.extend(dec.flush());
    let f = &frames[au];
    for (i, g) in frames.iter().enumerate().take(au) {
        assert_eq!(g.data, recon[i].data, "frame {i}, before the damage");
    }
    assert!(f.concealed, "the damaged frame is marked");
    let s = dec.stats();
    Damage {
        differing: differing_mbs(f, &recon[au]),
        backward: s.rvlc_backward_mbs,
        discarded: s.rvlc_discarded_mbs,
        concealed: s.concealed_vops,
    }
}

/// A P-VOP coded as one data-partitioned packet with reversible VLCs,
/// damaged mid-texture: the macroblocks before and after the damage come
/// back exactly; only a run of macroblocks around it (the T = 90 bits of
/// Annex E.1.4.4.2 each side, and the macroblocks those bits touch) is
/// concealed — with its intact motion — and the backward decode accounts
/// for the macroblocks after it.
#[test]
fn p_vop_damage_recovered_both_ways() {
    let mut cfg = EncoderConfig::new(W, H, 25);
    cfg.data_partitioning = true;
    cfg.reversible_vlc = true;
    cfg.gop_size = 0;
    let (aus, recon) = encode(cfg, 6);
    let motion_marker = 0b1_1111_0000_0000_0001;
    let d = damage(&aus, &recon, 4, motion_marker, 17, 17);
    println!(
        "P-VOP: {} macroblocks differ ({:?}), {} recovered backwards, {} discarded",
        d.differing.len(),
        d.differing,
        d.backward,
        d.discarded
    );
    assert_eq!(d.concealed, 1);
    assert!(d.backward > 0, "backward decoding recovered nothing");
    assert!(d.discarded > 0);
    // What differs is one run of macroblocks in the middle of the VOP, no
    // more than were discarded.
    let (first, last) = (d.differing[0], *d.differing.last().unwrap());
    assert!(first > 0 && last < 98, "{first}..={last}");
    assert!(d.differing.len() as u64 <= d.discarded);
    assert!(last - first < 99 / 2, "{first}..={last}");
}

/// The same in an I-VOP: the DC of every macroblock is in the first
/// partition, so discarded macroblocks keep it; recovered ones are exact
/// unless they predict AC coefficients from a discarded neighbour.
#[test]
fn i_vop_damage_recovered_both_ways() {
    let mut cfg = EncoderConfig::new(W, H, 25);
    cfg.data_partitioning = true;
    cfg.reversible_vlc = true;
    let (aus, recon) = encode(cfg, 1);
    let dc_marker = 0b110_1011_0000_0000_0001;
    let d = damage(&aus, &recon, 0, dc_marker, 19, 17);
    println!(
        "I-VOP: {} macroblocks differ, {} recovered backwards, {} discarded",
        d.differing.len(),
        d.backward,
        d.discarded
    );
    assert!(d.backward > 0);
    assert!(d.discarded > 0);
    // The first macroblocks (forward) and the last row (backward) are exact.
    assert!(d.differing[0] > 0);
    assert!(*d.differing.last().unwrap() < 99 - 11);
}

/// With video packets, the damage stays in its packet: every other
/// packet's macroblocks decode exactly, and backward decoding starts from
/// the packet's own end (the next resync marker).
#[test]
fn damage_stays_in_its_packet() {
    let mut cfg = EncoderConfig::new(W, H, 25);
    cfg.data_partitioning = true;
    cfg.reversible_vlc = true;
    cfg.packet_bytes = Some(200);
    cfg.gop_size = 0;
    let (aus, recon) = encode(cfg, 4);
    let motion_marker = 0b1_1111_0000_0000_0001;
    // P-VOPs here use vop_fcode 1: 17-bit resync markers.
    let d = damage(&aus, &recon, 2, motion_marker, 17, 17);
    println!(
        "packets: {} macroblocks differ ({:?}), {} recovered backwards, {} discarded",
        d.differing.len(),
        d.differing,
        d.backward,
        d.discarded
    );
    assert!(!d.differing.is_empty());
    assert!(d.differing.len() as u64 <= d.discarded);
    // The first packet was damaged: nothing beyond it differs.
    assert!(*d.differing.last().unwrap() < 60);
}
