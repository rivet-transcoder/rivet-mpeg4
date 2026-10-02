//! The encoder: I- and P-VOPs (Simple Profile) and, on request, B-VOPs
//! (Advanced Simple Profile), with half-sample motion search, optional
//! four-vector macroblocks and video packets, intra AC / DC prediction, the
//! H.263 quantiser and a constant-quantiser or bit-rate control.
//!
//! The encoder reconstructs every VOP with the decoder's own functions
//! (prediction, inverse quantisation, IDCT, motion compensation), so its
//! reference pictures are the ones any conforming decoder builds.

mod write;

use std::collections::VecDeque;

use crate::bits::BitWriter;
use crate::dec::vop::{MbPix, Motion, add_block, direct_vectors, predict_mb, put_block, write_mb};
use crate::error::{Result, config};
use crate::frame::{Frame, VopType};
use crate::headers::{self, VolParams, VopHeader, time_increment_bits};
use crate::idct::{fdct, idct};
use crate::mbstate::{Dir, IntraPred, MbKind, MbState, ac_pred_value, round_div};
use crate::mc;
use crate::picture::Pic;
use crate::quant::{Quant, quantise_h263};
use crate::tables::{ALT_HORIZONTAL, ALT_VERTICAL, ZIGZAG, code, dc_scaler};
use write::*;

/// How the encoder picks its quantiser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateControl {
    /// The same `vop_quant` (1..=31) for every I- and P-VOP; B-VOPs use
    /// a quarter more.
    ConstantQuant(u8),
    /// A target bit rate in bits per second: one quantiser per VOP, raised
    /// and lowered to keep the running total near the target.
    Bitrate(u32),
}

/// Encoder settings. [`EncoderConfig::new`] fills in the defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderConfig {
    /// Luma width (1..=8191).
    pub width: u32,
    /// Luma height (1..=8191).
    pub height: u32,
    /// Ticks per second (`vop_time_increment_resolution`, 1..=65535).
    pub time_base: u32,
    /// Ticks per frame; frame `n` is stamped `n * frame_duration`.
    pub frame_duration: u32,
    /// An I-VOP every this many frames (0: only the first).
    pub gop_size: u32,
    /// Consecutive B-VOPs between reference VOPs (0..=8). Anything but 0
    /// makes the stream Advanced Simple Profile and delays output: frames
    /// are coded in decode order, B-VOPs after the reference that follows
    /// them.
    pub b_frames: u32,
    /// Quantiser control.
    pub rate: RateControl,
    /// Motion search range in whole samples (1..=1023); `vop_fcode` is the
    /// smallest that codes it.
    pub search_range: u32,
    /// Let P-VOP macroblocks use four vectors where that predicts better.
    pub four_mv: bool,
    /// Start a new video packet (a resync marker) once the current one
    /// passes this many bytes; `None` codes each VOP as one packet with
    /// resync markers disabled in the VOL.
    pub packet_bytes: Option<u32>,
    /// Keep the reconstruction of every coded VOP for
    /// [`Encoder::take_reconstructions`] (for measuring quality; costs a
    /// copy of every frame).
    pub keep_reconstructions: bool,
}

impl EncoderConfig {
    /// `width` x `height` at `fps` frames per second: an I-VOP every 12
    /// frames, no B-VOPs, constant quantiser 5, a 15-sample search, one
    /// vector per macroblock, no video packets.
    pub fn new(width: u32, height: u32, fps: u32) -> EncoderConfig {
        EncoderConfig {
            width,
            height,
            time_base: fps.max(1),
            frame_duration: 1,
            gop_size: 12,
            b_frames: 0,
            rate: RateControl::ConstantQuant(5),
            search_range: 15,
            four_mv: false,
            packet_bytes: None,
            keep_reconstructions: false,
        }
    }
}

/// The profile_and_level_indication for a frame size: Simple Profile (or,
/// with B-VOPs, Advanced Simple) at the lowest level whose macroblock count
/// covers it.
fn profile_level(mbs: usize, advanced: bool) -> u8 {
    if advanced {
        match mbs {
            0..=99 => 0xf1,    // ASP L1, QCIF
            100..=396 => 0xf3, // L3, CIF
            397..=792 => 0xf4, // L4, 352x576
            _ => 0xf5,         // L5, 720x576 (and beyond)
        }
    } else {
        match mbs {
            0..=99 => 0x01,      // SP L1, QCIF
            100..=396 => 0x03,   // L3, CIF
            397..=1200 => 0x04,  // L4a, VGA
            1201..=1620 => 0x05, // L5, D1
            _ => 0x06,           // L6, 720p (and beyond)
        }
    }
}

struct Rc {
    target: f64,
    base: f64,
    err: f64,
}

/// A coded I- or P-VOP kept for prediction.
struct Ref {
    pic: Pic,
    motion: Motion,
    /// Display time in ticks.
    time: i64,
}

/// B-VOP macroblock modes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BMode {
    Direct,
    Interpolate,
    Backward,
    Forward,
}

/// An MPEG-4 Part 2 encoder: Simple Profile, or Advanced Simple with
/// B-VOPs.
///
/// Each [`Encoder::encode`] call takes one frame and returns the bytes of
/// the VOPs it allowed to be coded, in decode order — one VOP per call
/// without B-VOPs; with them, nothing for a frame that waits to be a
/// B-VOP, then the next reference VOP and the B-VOPs before it in one
/// buffer (the decoder takes such a buffer whole). [`Encoder::finish`]
/// codes what is still waiting. The first bytes begin with the
/// configuration headers ([`Encoder::config`]: visual object sequence,
/// visual object, video object layer), so the concatenated output is a
/// complete elementary stream.
pub struct Encoder {
    cfg: EncoderConfig,
    config_bytes: Vec<u8>,
    headers_written: bool,
    /// Frames received.
    n: u64,
    /// Index of the last frame coded as a reference.
    last_ref_index: Option<u64>,
    /// Frames waiting to be coded as B-VOPs: (index, picture).
    pending: VecDeque<(u64, Pic)>,
    past: Option<Ref>,
    future: Option<Ref>,
    st: MbState,
    prev_mv: Vec<[i16; 2]>,
    slice_counter: u32,
    slice: u32,
    fcode: u32,
    time_bits: u32,
    last_ref_sec: i64,
    prev_ref_sec: i64,
    rounding: bool,
    quant: Quant,
    rc: Option<Rc>,
    recons: Vec<Frame>,
}

impl Encoder {
    /// An encoder for `cfg`, or [`crate::Error::Config`] naming what is out
    /// of range.
    pub fn new(cfg: EncoderConfig) -> Result<Encoder> {
        if cfg.width == 0 || cfg.height == 0 || cfg.width > 8191 || cfg.height > 8191 {
            return Err(config(format!(
                "frame size {}x{} (1..=8191 each way)",
                cfg.width, cfg.height
            )));
        }
        if cfg.time_base == 0 || cfg.time_base > 65535 {
            return Err(config(format!("time base {} (1..=65535)", cfg.time_base)));
        }
        if cfg.frame_duration == 0 {
            return Err(config("frame duration 0"));
        }
        if !(1..=1023).contains(&cfg.search_range) {
            return Err(config(format!(
                "search range {} (1..=1023)",
                cfg.search_range
            )));
        }
        if cfg.b_frames > 8 {
            return Err(config(format!(
                "{} consecutive B-VOPs (0..=8)",
                cfg.b_frames
            )));
        }
        match cfg.rate {
            RateControl::ConstantQuant(q) if !(1..=31).contains(&q) => {
                return Err(config(format!("quantiser {q} (1..=31)")));
            }
            RateControl::Bitrate(0) => return Err(config("bit rate 0")),
            _ => {}
        }
        // The smallest f_code whose range [-32 f, 32 f - 1] half samples
        // holds every vector the search can return.
        let need = 2 * cfg.search_range as i32 + 1;
        let fcode = (1..=7u32)
            .find(|&f| 32 * (1 << (f - 1)) > need)
            .unwrap_or(7);
        let mbw = cfg.width.div_ceil(16) as usize;
        let mbh = cfg.height.div_ceil(16) as usize;
        let advanced = cfg.b_frames > 0;
        let config_bytes = headers::write_config(&VolParams {
            profile_and_level: profile_level(mbw * mbh, advanced),
            advanced_simple: advanced,
            width: cfg.width,
            height: cfg.height,
            time_resolution: cfg.time_base,
            fixed_increment: (cfg.frame_duration < cfg.time_base).then_some(cfg.frame_duration),
            resync_markers: cfg.packet_bytes.is_some(),
        });
        let rc = match cfg.rate {
            RateControl::Bitrate(bps) => {
                let target = bps as f64 * cfg.frame_duration as f64 / cfg.time_base as f64;
                let bpp = target / (cfg.width as f64 * cfg.height as f64);
                Some(Rc {
                    target,
                    base: (1.2 / bpp.max(1e-3)).clamp(2.0, 31.0),
                    err: 0.0,
                })
            }
            RateControl::ConstantQuant(_) => None,
        };
        Ok(Encoder {
            time_bits: time_increment_bits(cfg.time_base),
            config_bytes,
            headers_written: false,
            n: 0,
            last_ref_index: None,
            pending: VecDeque::new(),
            past: None,
            future: None,
            st: MbState::new(mbw, mbh),
            prev_mv: vec![[0, 0]; 4 * mbw * mbh],
            slice_counter: 0,
            slice: 0,
            fcode,
            last_ref_sec: 0,
            prev_ref_sec: 0,
            rounding: false,
            quant: Quant::h263(),
            rc,
            recons: Vec::new(),
            cfg,
        })
    }

    /// The configuration headers (visual object sequence, visual object,
    /// video object layer): the decoder specific info for an MP4 `esds`.
    pub fn config(&self) -> &[u8] {
        &self.config_bytes
    }

    /// Takes one frame, which must be `width` x `height`, and returns the
    /// VOPs that could be coded (see [`Encoder`] for when that is none).
    pub fn encode(&mut self, frame: &Frame) -> Result<Vec<u8>> {
        frame.validate()?;
        if frame.width != self.cfg.width || frame.height != self.cfg.height {
            return Err(config(format!(
                "a {}x{} frame for a {}x{} encoder",
                frame.width, frame.height, self.cfg.width, self.cfg.height
            )));
        }
        let pic = Pic::from_frame(frame);
        let index = self.n;
        self.n += 1;
        let gop = self.cfg.gop_size as u64;
        let intra = self.future.is_none() || (gop > 0 && index.is_multiple_of(gop));
        let due = self
            .last_ref_index
            .is_none_or(|l| index - l > self.cfg.b_frames as u64);
        let mut w = BitWriter::new();
        self.put_headers(&mut w);
        if intra || due {
            self.code_ref(&mut w, &pic, index, intra);
            self.code_pending(&mut w);
        } else {
            self.pending.push_back((index, pic));
        }
        Ok(w.into_bytes())
    }

    /// Codes the frames still waiting: the last as a P-VOP, the others as
    /// B-VOPs before it. Returns their bytes (empty when nothing waits).
    pub fn finish(&mut self) -> Result<Vec<u8>> {
        let mut w = BitWriter::new();
        if let Some((index, pic)) = self.pending.pop_back() {
            self.put_headers(&mut w);
            self.code_ref(&mut w, &pic, index, false);
            self.code_pending(&mut w);
        }
        Ok(w.into_bytes())
    }

    /// The reconstructions of the VOPs coded since the last call, in
    /// decode order, when [`EncoderConfig::keep_reconstructions`] is set:
    /// the pictures every conforming decoder builds from the bytes.
    pub fn take_reconstructions(&mut self) -> Vec<Frame> {
        std::mem::take(&mut self.recons)
    }

    fn put_headers(&mut self, w: &mut BitWriter) {
        if !self.headers_written {
            w.put_bytes(&self.config_bytes);
            self.headers_written = true;
        }
    }

    fn code_pending(&mut self, w: &mut BitWriter) {
        while let Some((index, pic)) = self.pending.pop_front() {
            self.code_b(w, &pic, index);
        }
    }

    fn ticks(&self, index: u64) -> i64 {
        index as i64 * self.cfg.frame_duration as i64
    }

    fn keep(&mut self, pic: &Pic, index: u64, t: VopType) {
        if self.cfg.keep_reconstructions {
            self.recons
                .push(pic.to_frame(self.ticks(index), self.cfg.time_base, t, index));
        }
    }

    /// Codes frame `index` as an I- or P-VOP and makes it the newest
    /// reference.
    fn code_ref(&mut self, w: &mut BitWriter, src: &Pic, index: u64, intra: bool) {
        let intra = intra || self.future.is_none();
        let qp = self.frame_qp(false);
        let t = self.ticks(index);
        let res = self.cfg.time_base as i64;
        let sec = t / res;
        if !intra {
            self.rounding = !self.rounding;
        }
        let hdr = VopHeader {
            vop_type: if intra { VopType::I } else { VopType::P },
            modulo_time_base: (sec - self.last_ref_sec) as u32,
            time_increment: (t % res) as u32,
            coded: true,
            rounding: self.rounding,
            intra_dc_vlc_thr: 0,
            quant: qp,
            fcode_forward: self.fcode,
            fcode_backward: 1,
            warping: Vec::new(),
            warping_divx500: false,
            top_field_first: false,
            alternate_vertical_scan: false,
        };
        self.prev_ref_sec = self.last_ref_sec;
        self.last_ref_sec = sec;
        let start = w.len_bits();
        headers::write_vop_header(w, self.time_bits, &hdr);
        let mut recon = Pic::new(self.cfg.width, self.cfg.height);
        self.code_ip(w, src, &mut recon, &hdr);
        w.stuff();
        self.rate_update(w.len_bits() - start, intra);
        self.prev_mv.clone_from(&self.st.mv);
        let motion = if intra {
            Motion::intra(self.st.mbw, self.st.mbh)
        } else {
            Motion {
                mbw: self.st.mbw,
                kind: self.st.kind.clone(),
                mv: self.st.mv.clone(),
                field: vec![false; self.st.mbw * self.st.mbh],
            }
        };
        self.keep(&recon, index, hdr.vop_type);
        self.past = self.future.take();
        self.future = Some(Ref {
            pic: recon,
            motion,
            time: t,
        });
        self.last_ref_index = Some(index);
    }

    fn frame_qp(&self, b: bool) -> u32 {
        let q = match (&self.rc, self.cfg.rate) {
            (_, RateControl::ConstantQuant(q)) => q as u32,
            (Some(rc), _) => {
                let q = rc.base * 2f64.powf((rc.err / (6.0 * rc.target)).clamp(-2.0, 2.0));
                q.round().clamp(1.0, 31.0) as u32
            }
            (None, _) => 5,
        };
        if b { ((q * 5 + 2) / 4).min(31) } else { q }
    }

    fn rate_update(&mut self, bits: usize, intra: bool) {
        if let Some(rc) = &mut self.rc {
            rc.err += bits as f64 - rc.target;
            if !intra {
                // Drift the base quantiser toward the one that hits the
                // target on inter VOPs.
                let r = (bits as f64 / rc.target).clamp(0.25, 4.0);
                rc.base = (rc.base * r.powf(0.2)).clamp(1.0, 31.0);
            }
        }
    }

    fn new_slice(&mut self) {
        self.slice_counter = self.slice_counter.wrapping_add(1).max(1);
        self.slice = self.slice_counter;
    }

    /// Starts a new video packet at macroblock `mb` when the current one
    /// has grown past the configured size.
    fn maybe_packet(
        &mut self,
        w: &mut BitWriter,
        packet_start: &mut usize,
        mb: usize,
        h: &VopHeader,
    ) -> bool {
        let Some(limit) = self.cfg.packet_bytes else {
            return false;
        };
        if mb == 0 || w.len_bits() - *packet_start <= limit as usize * 8 {
            return false;
        }
        let total = self.st.mbw * self.st.mbh;
        let mb_bits = (usize::BITS - (total - 1).leading_zeros()).max(1);
        w.stuff();
        let len = match h.vop_type {
            VopType::I => 17,
            VopType::B => 16 + h.fcode_forward.max(h.fcode_backward),
            _ => 16 + h.fcode_forward,
        };
        w.put(len, 1);
        w.put(mb_bits, mb as u32);
        w.put(5, h.quant);
        w.put(1, 0); // header_extension_code
        *packet_start = w.len_bits();
        self.new_slice();
        true
    }

    fn code_ip(&mut self, w: &mut BitWriter, src: &Pic, recon: &mut Pic, h: &VopHeader) {
        let (mbw, mbh) = (self.st.mbw, self.st.mbh);
        self.new_slice();
        let mut packet_start = w.len_bits();
        let reference = self.future.take();
        for mb in 0..mbw * mbh {
            self.maybe_packet(w, &mut packet_start, mb, h);
            let (mbx, mby) = (mb % mbw, mb / mbw);
            self.st.slice[mb] = self.slice;
            self.st.qp[mb] = h.quant as u8;
            match (&reference, h.vop_type) {
                (Some(rf), VopType::P) => self.code_p_mb(w, src, &rf.pic, recon, mbx, mby, h),
                _ => self.code_intra_mb(w, src, recon, mbx, mby, h.quant, false),
            }
        }
        self.future = reference;
    }

    /// Codes frame `index`, which lies between the two references, as a
    /// B-VOP.
    fn code_b(&mut self, w: &mut BitWriter, src: &Pic, index: u64) {
        let (Some(past), Some(future)) = (self.past.take(), self.future.take()) else {
            unreachable!("B-VOPs are coded after two references");
        };
        let qp = self.frame_qp(true);
        let t = self.ticks(index);
        let res = self.cfg.time_base as i64;
        let hdr = VopHeader {
            vop_type: VopType::B,
            modulo_time_base: (t / res - self.prev_ref_sec) as u32,
            time_increment: (t % res) as u32,
            coded: true,
            rounding: false,
            intra_dc_vlc_thr: 0,
            quant: qp,
            fcode_forward: self.fcode,
            fcode_backward: self.fcode,
            warping: Vec::new(),
            warping_divx500: false,
            top_field_first: false,
            alternate_vertical_scan: false,
        };
        let start = w.len_bits();
        headers::write_vop_header(w, self.time_bits, &hdr);
        let trb = (t - past.time) as i32;
        let trd = (future.time - past.time) as i32;
        let mut recon = Pic::new(self.cfg.width, self.cfg.height);
        let (mbw, mbh) = (self.st.mbw, self.st.mbh);
        self.new_slice();
        let mut packet_start = w.len_bits();
        let mut pmv = [[0i32; 2]; 2];
        for mb in 0..mbw * mbh {
            if self.maybe_packet(w, &mut packet_start, mb, &hdr) {
                pmv = [[0, 0]; 2];
            }
            let (mbx, mby) = (mb % mbw, mb / mbw);
            if mbx == 0 {
                pmv = [[0, 0]; 2];
            }
            self.code_b_mb(
                w,
                src,
                &past,
                &future,
                &mut recon,
                mbx,
                mby,
                qp,
                (trb, trd),
                &mut pmv,
            );
        }
        w.stuff();
        self.rate_update(w.len_bits() - start, false);
        self.keep(&recon, index, VopType::B);
        self.past = Some(past);
        self.future = Some(future);
    }

    #[allow(clippy::too_many_arguments)]
    fn code_b_mb(
        &self,
        w: &mut BitWriter,
        src: &Pic,
        past: &Ref,
        future: &Ref,
        recon: &mut Pic,
        mbx: usize,
        mby: usize,
        qp: u32,
        (trb, trd): (i32, i32),
        pmv: &mut [[i32; 2]; 2],
    ) {
        let mb = mby * self.st.mbw + mbx;
        let mut px = MbPix::new();
        if future.motion.kind[mb] == MbKind::Skipped {
            // Not coded: the decoder copies the past reference.
            predict_mb(
                &past.pic,
                mbx,
                mby,
                &[[0, 0]; 4],
                false,
                false,
                false,
                &mut px,
            );
            write_mb(recon, mbx, mby, &px);
            return;
        }
        let col = {
            let v = future.motion.mv[(2 * mby) * 2 * self.st.mbw + 2 * mbx];
            [v[0] as i32, v[1] as i32]
        };
        let scale = |n: i32| {
            if trd != 0 {
                [col[0] * n / trd, col[1] * n / trd]
            } else {
                [0, 0]
            }
        };
        let (mvf, _) = self.search(
            src,
            &past.pic,
            mbx,
            mby,
            pmv[0],
            &[pmv[0], scale(trb)],
            qp,
            false,
        );
        let (mvb, _) = self.search(
            src,
            &future.pic,
            mbx,
            mby,
            pmv[1],
            &[pmv[1], scale(trb - trd)],
            qp,
            false,
        );
        // Compare the four modes on the luma prediction error plus a rate
        // estimate.
        let mut pf = MbPix::new();
        let mut pb = MbPix::new();
        predict_mb(&past.pic, mbx, mby, &[mvf; 4], false, false, false, &mut pf);
        predict_mb(
            &future.pic,
            mbx,
            mby,
            &[mvb; 4],
            false,
            false,
            false,
            &mut pb,
        );
        let mut pi = MbPix {
            y: pf.y,
            cb: pf.cb,
            cr: pf.cr,
        };
        mc::average(&mut pi.y, &pb.y);
        mc::average(&mut pi.cb, &pb.cb);
        mc::average(&mut pi.cr, &pb.cr);
        let (dmf, dmb) = direct_vectors(&future.motion, mbx, mby, [0, 0], trb, trd);
        let mut pd = MbPix::new();
        let mut pdb = MbPix::new();
        predict_mb(&past.pic, mbx, mby, &dmf, true, false, false, &mut pd);
        predict_mb(&future.pic, mbx, mby, &dmb, true, false, false, &mut pdb);
        mc::average(&mut pd.y, &pdb.y);
        mc::average(&mut pd.cb, &pdb.cb);
        mc::average(&mut pd.cr, &pdb.cr);
        let mut ys = [0u8; 256];
        luma_mb(src, mbx, mby, &mut ys);
        let sad = |p: &[u8; 256]| -> u32 {
            ys.iter()
                .zip(p)
                .map(|(&a, &b)| (a as i32 - b as i32).unsigned_abs())
                .sum()
        };
        let lambda = qp;
        let costs = [
            (BMode::Direct, sad(&pd.y)),
            (
                BMode::Forward,
                sad(&pf.y) + lambda * (self.mv_bits(mvf, pmv[0]) + 4),
            ),
            (
                BMode::Backward,
                sad(&pb.y) + lambda * (self.mv_bits(mvb, pmv[1]) + 3),
            ),
            (
                BMode::Interpolate,
                sad(&pi.y) + lambda * (self.mv_bits(mvf, pmv[0]) + self.mv_bits(mvb, pmv[1]) + 2),
            ),
        ];
        let mode = costs.iter().min_by_key(|c| c.1).unwrap().0;
        let px = match mode {
            BMode::Direct => pd,
            BMode::Forward => pf,
            BMode::Backward => pb,
            BMode::Interpolate => pi,
        };
        let mut levels = [[0i16; 64]; 6];
        let mut cbp = 0u8;
        for (k, lv) in levels.iter_mut().enumerate() {
            let s = source_block(src, mbx, mby, k);
            let p = pred_block(&px, k);
            for i in 0..64 {
                lv[i] = s[i] - p[i];
            }
            fdct(lv);
            quantise_h263(lv, qp, false);
            if lv.iter().any(|&v| v != 0) {
                cbp |= 1 << (5 - k);
            }
        }
        // modb: 1 (direct, no data), 01 (mb_type, no cbpb), 00 (both).
        if mode == BMode::Direct && cbp == 0 {
            w.put(1, 1);
        } else {
            w.put(2, if cbp == 0 { 0b01 } else { 0b00 });
            match mode {
                BMode::Direct => w.put(1, 1),
                BMode::Interpolate => w.put(2, 0b01),
                BMode::Backward => w.put(3, 0b001),
                BMode::Forward => w.put(4, 0b0001),
            }
            if cbp != 0 {
                w.put(6, cbp as u32);
                if mode != BMode::Direct {
                    w.put(1, 0); // dbquant: no change
                }
            }
            if matches!(mode, BMode::Forward | BMode::Interpolate) {
                self.put_mv(w, mvf, pmv[0]);
                pmv[0] = mvf;
            }
            if matches!(mode, BMode::Backward | BMode::Interpolate) {
                self.put_mv(w, mvb, pmv[1]);
                pmv[1] = mvb;
            }
            if mode == BMode::Direct {
                // MVDB (0, 0), f_code 1.
                put_mvd(w, 0, 1);
                put_mvd(w, 0, 1);
            }
        }
        write_mb(recon, mbx, mby, &px);
        for (k, lv) in levels.iter().enumerate() {
            if cbp >> (5 - k) & 1 == 0 {
                continue;
            }
            put_coeffs(w, lv, &ZIGZAG, 0, false);
            let mut rec = *lv;
            self.quant.inter(&mut rec, qp);
            idct(&mut rec);
            add_block(recon, mbx, mby, k, &rec, false);
        }
    }

    /// An intra macroblock (in an I-VOP, or `in_p` a P-VOP).
    #[allow(clippy::too_many_arguments)]
    fn code_intra_mb(
        &mut self,
        w: &mut BitWriter,
        src: &Pic,
        recon: &mut Pic,
        mbx: usize,
        mby: usize,
        qp: u32,
        in_p: bool,
    ) {
        let mb = mby * self.st.mbw + mbx;
        self.st.kind[mb] = MbKind::Intra;
        self.st.set_mb_mv(mbx, mby, [0, 0]);
        let mut levels = [[0i16; 64]; 6];
        let mut preds: [Option<IntraPred>; 6] = [None; 6];
        for k in 0..6 {
            let mut b = source_block(src, mbx, mby, k);
            fdct(&mut b);
            let scaler = dc_scaler(qp, k < 4) as i32;
            let dc = round_div(b[0] as i32, scaler).clamp(0, 2047 / scaler);
            quantise_h263(&mut b, qp, true);
            b[0] = dc as i16;
            preds[k] = Some(self.st.intra_pred(mbx, mby, k, self.slice));
            self.st.store_intra(mb, k, dc * scaler, &b);
            levels[k] = b;
        }
        // AC prediction when it shrinks the first rows / columns overall
        // and every predicted difference stays codable.
        let mut gain = 0i32;
        let mut ok = true;
        for k in 0..6 {
            let p = preds[k].unwrap();
            if let Some((v, qpn)) = p.ac {
                for i in 1..8 {
                    let idx = if p.dir == Dir::Up { i } else { i * 8 };
                    let o = levels[k][idx] as i32;
                    let d = o - ac_pred_value(v[i - 1], qpn, qp);
                    gain += o.abs() - d.abs();
                    ok &= d.abs() <= 2047;
                }
            }
        }
        let ac_pred = ok && gain > 0;
        let mut coded = levels;
        let mut scans = [&ZIGZAG; 6];
        let mut cbp = 0u8;
        let mut dc_diff = [0i32; 6];
        for k in 0..6 {
            let p = preds[k].unwrap();
            let scaler = dc_scaler(qp, k < 4) as i32;
            dc_diff[k] = levels[k][0] as i32 - round_div(p.dc, scaler);
            if ac_pred {
                scans[k] = if p.dir == Dir::Up {
                    &ALT_HORIZONTAL
                } else {
                    &ALT_VERTICAL
                };
                if let Some((v, qpn)) = p.ac {
                    for i in 1..8 {
                        let idx = if p.dir == Dir::Up { i } else { i * 8 };
                        coded[k][idx] =
                            (levels[k][idx] as i32 - ac_pred_value(v[i - 1], qpn, qp)) as i16;
                    }
                }
            }
            coded[k][0] = 0;
            if coded[k].iter().any(|&v| v != 0) {
                cbp |= 1 << (5 - k);
            }
        }
        if in_p {
            w.put(1, 0); // not_coded
            put_mcbpc_p(w, 3, cbp & 3);
        } else {
            put_mcbpc_i(w, 3, cbp & 3);
        }
        w.put(1, ac_pred as u32);
        put_cbpy(w, cbp >> 2);
        for k in 0..6 {
            put_dc_diff(w, dc_diff[k], k < 4);
            if cbp >> (5 - k) & 1 != 0 {
                put_coeffs(w, &coded[k], scans[k], 1, true);
            }
        }
        for (k, lv) in levels.iter().enumerate() {
            let mut rec = *lv;
            self.quant.intra(&mut rec, qp, dc_scaler(qp, k < 4));
            idct(&mut rec);
            put_block(recon, mbx, mby, k, &rec, false);
        }
    }

    /// A P-VOP macroblock: motion search, then intra, skipped, one- or
    /// four-vector inter coding.
    #[allow(clippy::too_many_arguments)]
    fn code_p_mb(
        &mut self,
        w: &mut BitWriter,
        src: &Pic,
        rf: &Pic,
        recon: &mut Pic,
        mbx: usize,
        mby: usize,
        h: &VopHeader,
    ) {
        let qp = h.quant;
        let mb = mby * self.st.mbw + mbx;
        let pred0 = self.st.mv_pred(mbx, mby, 0, self.slice);
        let mbw = self.st.mbw;
        let prev = self.prev_mv[(2 * mby) * 2 * mbw + 2 * mbx];
        let mut cands = vec![[prev[0] as i32, prev[1] as i32]];
        if mbx > 0 {
            cands.push(self.st.get_mv(mbx - 1, mby, 1));
        }
        if mby > 0 {
            cands.push(self.st.get_mv(mbx, mby - 1, 2));
            if mbx + 1 < mbw {
                cands.push(self.st.get_mv(mbx + 1, mby - 1, 2));
            }
        }
        let (mv, sad16) = self.search(src, rf, mbx, mby, pred0, &cands, qp, h.rounding);
        // TMN's intra decision: intra when the macroblock's deviation from
        // its own mean is clearly below the best prediction error.
        let mut ys = [0u8; 256];
        luma_mb(src, mbx, mby, &mut ys);
        let mean = ys.iter().map(|&v| v as u32).sum::<u32>() / 256;
        let dev: u32 = ys
            .iter()
            .map(|&v| (v as i32 - mean as i32).unsigned_abs())
            .sum();
        if dev + 500 < sad16 {
            self.code_intra_mb(w, src, recon, mbx, mby, qp, true);
            return;
        }
        let mut mvs = [mv; 4];
        let mut four = false;
        if self.cfg.four_mv {
            let mut sad4 = 0;
            for (k, v) in mvs.iter_mut().enumerate() {
                let (b, s) = self.search8(src, rf, mbx, mby, k, mv, h.rounding);
                *v = b;
                sad4 += s;
            }
            if sad4 + 16 * qp * 3 < sad16 && mvs.iter().any(|&v| v != mv) {
                four = true;
            } else {
                mvs = [mv; 4];
            }
        }
        let mut px = MbPix::new();
        predict_mb(rf, mbx, mby, &mvs, four, h.rounding, false, &mut px);
        let mut levels = [[0i16; 64]; 6];
        let mut cbp = 0u8;
        for (k, lv) in levels.iter_mut().enumerate() {
            let s = source_block(src, mbx, mby, k);
            let p = pred_block(&px, k);
            for i in 0..64 {
                lv[i] = s[i] - p[i];
            }
            fdct(lv);
            quantise_h263(lv, qp, false);
            if lv.iter().any(|&v| v != 0) {
                cbp |= 1 << (5 - k);
            }
        }
        write_mb(recon, mbx, mby, &px);
        if !four && mv == [0, 0] && cbp == 0 {
            w.put(1, 1); // not_coded
            self.st.kind[mb] = MbKind::Skipped;
            self.st.set_mb_mv(mbx, mby, [0, 0]);
            return;
        }
        self.st.kind[mb] = MbKind::Inter;
        w.put(1, 0);
        put_mcbpc_p(w, if four { 2 } else { 0 }, cbp & 3);
        put_cbpy(w, 15 - (cbp >> 2));
        if four {
            for (k, v) in mvs.iter().enumerate() {
                let p = self.st.mv_pred(mbx, mby, k, self.slice);
                self.put_mv(w, *v, p);
                self.st.set_mv(mbx, mby, k, *v);
            }
        } else {
            self.put_mv(w, mv, pred0);
            self.st.set_mb_mv(mbx, mby, mv);
        }
        for (k, lv) in levels.iter().enumerate() {
            if cbp >> (5 - k) & 1 == 0 {
                continue;
            }
            put_coeffs(w, lv, &ZIGZAG, 0, false);
            let mut rec = *lv;
            self.quant.inter(&mut rec, qp);
            idct(&mut rec);
            add_block(recon, mbx, mby, k, &rec, false);
        }
    }

    fn put_mv(&self, w: &mut BitWriter, mv: [i32; 2], pred: [i32; 2]) {
        for j in 0..2 {
            put_mvd(w, wrap_diff(mv[j] - pred[j], self.fcode), self.fcode);
        }
    }

    /// Bits of a vector difference.
    fn mv_bits(&self, mv: [i32; 2], pred: [i32; 2]) -> u32 {
        let rs = self.fcode - 1;
        (0..2)
            .map(|j| {
                let d = wrap_diff(mv[j] - pred[j], self.fcode);
                if d == 0 {
                    return 1;
                }
                let a = d.unsigned_abs();
                let m = if rs == 0 { a } else { ((a - 1) >> rs) + 1 };
                code(crate::tables::MVD[m as usize]).1 + 1 + rs
            })
            .sum()
    }

    /// Sum of absolute differences of the luma of macroblock `(mbx, mby)`
    /// against its prediction at `mv` (half samples), over block `k`
    /// (an 8x8 quarter) or the whole macroblock (`None`).
    #[allow(clippy::too_many_arguments)]
    fn sad(
        &self,
        src: &Pic,
        rf: &Pic,
        mbx: usize,
        mby: usize,
        k: Option<usize>,
        mv: [i32; 2],
        rounding: bool,
    ) -> u32 {
        let (bx, by, n) = match k {
            None => (0, 0, 16),
            Some(k) => ((k & 1) * 8, (k >> 1) * 8, 8),
        };
        let x = (mbx * 16 + bx) as i32;
        let y = (mby * 16 + by) as i32;
        let mut p = [0u8; 256];
        mc::halfpel(
            rf.ref_plane(0),
            x,
            y,
            mv[0],
            mv[1],
            n,
            n,
            rounding,
            &mut p,
            16,
        );
        let s = src.ystride();
        let mut sum = 0;
        for r in 0..n {
            let row = &src.y[(y as usize + r) * s + x as usize..];
            for c in 0..n {
                sum += (row[c] as i32 - p[r * 16 + c] as i32).unsigned_abs();
            }
        }
        sum
    }

    /// Clamps a half-sample vector into the search window.
    fn clamp_mv(&self, v: [i32; 2]) -> [i32; 2] {
        let r = 2 * self.cfg.search_range as i32;
        [v[0].clamp(-r, r), v[1].clamp(-r, r)]
    }

    /// Predictive diamond search over whole samples from the best of the
    /// zero vector, the predictor and `extra` candidates, then the eight
    /// half-sample neighbours: the vector with the lowest SAD plus a rate
    /// term, and its SAD.
    #[allow(clippy::too_many_arguments)]
    fn search(
        &self,
        src: &Pic,
        rf: &Pic,
        mbx: usize,
        mby: usize,
        pred: [i32; 2],
        extra: &[[i32; 2]],
        qp: u32,
        rounding: bool,
    ) -> ([i32; 2], u32) {
        let lambda = qp;
        let cost = |v: [i32; 2], sad: u32| sad + lambda * self.mv_bits(v, pred);
        let even = |v: [i32; 2]| [v[0] & !1, v[1] & !1];
        let mut best = [0, 0];
        let mut best_cost = u32::MAX;
        let mut best_sad = u32::MAX;
        for &c in [[0, 0], pred].iter().chain(extra) {
            let c = even(self.clamp_mv(c));
            let s = self.sad(src, rf, mbx, mby, None, c, rounding);
            let k = cost(c, s);
            if k < best_cost {
                (best, best_cost, best_sad) = (c, k, s);
            }
        }
        // Small diamond in whole samples.
        for _ in 0..64 {
            let mut moved = false;
            for d in [[2, 0], [-2, 0], [0, 2], [0, -2]] {
                let c = self.clamp_mv([best[0] + d[0], best[1] + d[1]]);
                if c == best {
                    continue;
                }
                let s = self.sad(src, rf, mbx, mby, None, c, rounding);
                let k = cost(c, s);
                if k < best_cost {
                    (best, best_cost, best_sad) = (c, k, s);
                    moved = true;
                }
            }
            if !moved {
                break;
            }
        }
        // Half-sample refinement.
        let centre = best;
        for dy in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let c = self.clamp_mv([centre[0] + dx, centre[1] + dy]);
                let s = self.sad(src, rf, mbx, mby, None, c, rounding);
                let k = cost(c, s);
                if k < best_cost {
                    (best, best_cost, best_sad) = (c, k, s);
                }
            }
        }
        (best, best_sad)
    }

    /// Refines one 8x8 block's vector around the macroblock's.
    #[allow(clippy::too_many_arguments)]
    fn search8(
        &self,
        src: &Pic,
        rf: &Pic,
        mbx: usize,
        mby: usize,
        k: usize,
        start: [i32; 2],
        rounding: bool,
    ) -> ([i32; 2], u32) {
        let mut best = start;
        let mut best_sad = self.sad(src, rf, mbx, mby, Some(k), start, rounding);
        for step in [2, 1] {
            for _ in 0..8 {
                let mut moved = false;
                for d in [[step, 0], [-step, 0], [0, step], [0, -step]] {
                    let c = self.clamp_mv([best[0] + d[0], best[1] + d[1]]);
                    let s = self.sad(src, rf, mbx, mby, Some(k), c, rounding);
                    if s < best_sad {
                        (best, best_sad) = (c, s);
                        moved = true;
                    }
                }
                if !moved {
                    break;
                }
            }
        }
        (best, best_sad)
    }
}

/// The 16x16 luma of a macroblock.
fn luma_mb(src: &Pic, mbx: usize, mby: usize, out: &mut [u8; 256]) {
    let s = src.ystride();
    for r in 0..16 {
        let o = (mby * 16 + r) * s + mbx * 16;
        out[r * 16..r * 16 + 16].copy_from_slice(&src.y[o..o + 16]);
    }
}

/// Block `k` of a macroblock of the source, as signed samples.
fn source_block(src: &Pic, mbx: usize, mby: usize, k: usize) -> [i16; 64] {
    let (p, s, o) = match k {
        0..=3 => (
            &src.y,
            src.ystride(),
            (mby * 16 + (k >> 1) * 8) * src.ystride() + mbx * 16 + (k & 1) * 8,
        ),
        4 => (&src.cb, src.cstride(), mby * 8 * src.cstride() + mbx * 8),
        _ => (&src.cr, src.cstride(), mby * 8 * src.cstride() + mbx * 8),
    };
    let mut b = [0i16; 64];
    for r in 0..8 {
        for c in 0..8 {
            b[r * 8 + c] = p[o + r * s + c] as i16;
        }
    }
    b
}

/// Block `k` of a macroblock prediction.
fn pred_block(px: &MbPix, k: usize) -> [i16; 64] {
    let mut b = [0i16; 64];
    for r in 0..8 {
        for c in 0..8 {
            b[r * 8 + c] = match k {
                0..=3 => px.y[((k >> 1) * 8 + r) * 16 + (k & 1) * 8 + c],
                4 => px.cb[r * 8 + c],
                _ => px.cr[r * 8 + c],
            } as i16;
        }
    }
    b
}
