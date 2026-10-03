//! The encoder: I- and P-VOPs (Simple Profile) and, on request, B-VOPs
//! (Advanced Simple Profile), with half-sample motion search, optional
//! four-vector macroblocks and video packets, intra AC / DC prediction, the
//! H.263 quantiser and a constant-quantiser or bit-rate control.
//!
//! The encoder reconstructs every VOP with the decoder's own functions
//! (prediction, inverse quantisation, IDCT, motion compensation), so its
//! reference pictures are the ones any conforming decoder builds.

mod syntax;
mod write;

use std::collections::VecDeque;

use crate::bits::BitWriter;
use crate::dec::vop::{
    MbPix, Motion, add_block, direct_vectors, field_direct_distances, field_direct_predict,
    field_direct_vectors, field_to_frame, predict_fields, predict_mb, put_block, write_mb,
};
use crate::error::{Result, config};
use crate::frame::{Frame, VopType};
use crate::headers::{self, VideoSignal, VolParams, VopHeader, time_increment_bits};
use crate::idct::{fdct, idct};
use crate::mbstate::{Dir, IntraPred, MbKind, MbState, ac_pred_value, round_div};
use crate::mc;
use crate::picture::Pic;
use crate::quant::{Quant, quantise_h263, quantise_mpeg};
use crate::tables::{ALT_HORIZONTAL, ALT_VERTICAL, ZIGZAG, code, dc_scaler};
use syntax::{MbSyntax, write_partitioned};
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

/// The inverse quantisation the encoder declares (`quant_type`, 6.3.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Quantiser {
    /// The second (H.263) method: one step size per macroblock.
    H263,
    /// The first (MPEG) method, with weighting matrices in raster order:
    /// each 1..=255, the intra matrix's first entry 8 (it is not used: the
    /// intra DC has its own scaler). An Advanced Simple Profile tool.
    Mpeg {
        /// Intra weighting matrix.
        intra: [u8; 64],
        /// Non-intra weighting matrix.
        inter: [u8; 64],
    },
}

impl Quantiser {
    /// The MPEG method with the default matrices of 6.3.3 (which the VOL
    /// then need not carry).
    pub fn mpeg_default() -> Quantiser {
        Quantiser::Mpeg {
            intra: crate::tables::DEFAULT_INTRA_MATRIX,
            inter: crate::tables::DEFAULT_INTER_MATRIX,
        }
    }
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
    /// The short video header: H.263 baseline pictures (sub-QCIF, QCIF,
    /// CIF, 4CIF or 16CIF only) that any H.263 decoder, and any 14496-2
    /// decoder, takes: no configuration headers ([`Encoder::config`] is
    /// empty), one vector per macroblock pointing inside the picture, no
    /// AC / DC prediction, H.263's quantiser, timing in 1001/30000 s
    /// ticks; with `packet_bytes`, GOB headers instead of video packets.
    /// Incompatible with B-VOPs, quarter-sample motion, four vectors, the
    /// MPEG quantiser and data partitioning.
    pub short_header: bool,
    /// Interlaced coding (an Advanced Simple tool): each macroblock picks
    /// frame or field DCT, P-VOP macroblocks frame or field prediction (a
    /// vector per field, from either reference field), and B-VOP direct
    /// mode over a field-predicted macroblock uses field direct mode.
    /// Not with data partitioning (which 14496-2's profiles exclude) or
    /// the short video header.
    pub interlaced: bool,
    /// Interlaced: `top_field_first` (the top field is the earlier one).
    pub top_field_first: bool,
    /// Quarter-sample motion vectors (7.6.2.2's 8-tap interpolation), an
    /// Advanced Simple Profile tool: the search refines to quarter samples.
    pub quarter_sample: bool,
    /// The quantiser: H.263 (the default) or MPEG with matrices.
    pub quantiser: Quantiser,
    /// Start a new video packet (a resync marker) once the current one
    /// passes this many bytes; `None` codes each VOP as one packet with
    /// resync markers disabled in the VOL.
    pub packet_bytes: Option<u32>,
    /// Data partitioning (6.2.5.2): each video packet's macroblock headers
    /// and vectors (or, in I-VOPs, DC coefficients) ahead of a marker, then
    /// the rest of their syntax, then their texture — so damage to the
    /// texture leaves the motion usable. Applies to I- and P-VOPs; B-VOPs
    /// are not partitioned.
    pub data_partitioning: bool,
    /// Reversible VLCs (Table B-23) for the texture of data-partitioned
    /// VOPs, which a decoder can read backwards from the next resync
    /// marker after damage. Requires `data_partitioning`.
    pub reversible_vlc: bool,
    /// `video_signal_type()` for the visual object header: the video
    /// format, range and colour description a decoder (or a player) is
    /// told the pictures use; `None` writes none (unspecified).
    pub video_signal: Option<VideoSignal>,
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
            short_header: false,
            interlaced: false,
            top_field_first: true,
            quarter_sample: false,
            quantiser: Quantiser::H263,
            packet_bytes: None,
            data_partitioning: false,
            reversible_vlc: false,
            video_signal: None,
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
    /// The next frame is to be an I-VOP.
    force_intra: bool,
    /// `Tframe` of field direct mode: the first B-VOP's distance from its
    /// past reference, as the decoder takes it.
    tframe: Option<i64>,
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
        if let Some(v) = cfg.video_signal
            && v.video_format > 7
        {
            return Err(config(format!("video_format {} (0..=7)", v.video_format)));
        }
        if let Quantiser::Mpeg { intra, inter } = &cfg.quantiser
            && (intra[0] != 8 || intra.contains(&0) || inter.contains(&0))
        {
            return Err(config(
                "quantiser matrices: every entry 1..=255, the intra matrix's first 8",
            ));
        }
        if cfg.short_header {
            if headers::short_header_format(cfg.width, cfg.height).is_none() {
                return Err(config(format!(
                    "a {}x{} short-header picture (128x96, 176x144, 352x288, 704x576 or 1408x1152)",
                    cfg.width, cfg.height
                )));
            }
            if cfg.b_frames > 0
                || cfg.quarter_sample
                || cfg.four_mv
                || cfg.data_partitioning
                || cfg.quantiser != Quantiser::H263
            {
                return Err(config(
                    "the short video header takes no B-VOPs, quarter-sample motion, four vectors, \
                     MPEG quantiser or data partitioning",
                ));
            }
        }
        if cfg.interlaced && (cfg.data_partitioning || cfg.short_header) {
            return Err(config(
                "interlaced coding with data partitioning or the short video header",
            ));
        }
        if cfg.reversible_vlc && !cfg.data_partitioning {
            return Err(config("reversible VLCs without data partitioning"));
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
        let unit = if cfg.quarter_sample { 4 } else { 2 };
        // The short header's vectors have f_code 1: [-32, 31] half samples.
        let mut cfg = cfg;
        if cfg.short_header {
            cfg.search_range = cfg.search_range.min(15);
        }
        let need = unit * cfg.search_range as i32 + unit - 1;
        let fcode = (1..=7u32)
            .find(|&f| 32 * (1 << (f - 1)) > need)
            .unwrap_or(7);
        // With video packets in B-VOPs, at least 2: a B-VOP's resync marker
        // is 16 + vop_fcode bits by 6.3.5.2, but with both codes 1 Xvid
        // writes and expects 18. From 2 up the two agree.
        let fcode = if cfg.b_frames > 0 && cfg.packet_bytes.is_some() {
            fcode.max(2)
        } else {
            fcode
        };
        let mbw = cfg.width.div_ceil(16) as usize;
        let mbh = cfg.height.div_ceil(16) as usize;
        let mpeg = matches!(cfg.quantiser, Quantiser::Mpeg { .. });
        let advanced = cfg.b_frames > 0 || cfg.quarter_sample || mpeg || cfg.interlaced;
        let (intra_matrix, inter_matrix) = match &cfg.quantiser {
            Quantiser::Mpeg { intra, inter } => (*intra, *inter),
            Quantiser::H263 => (
                crate::tables::DEFAULT_INTRA_MATRIX,
                crate::tables::DEFAULT_INTER_MATRIX,
            ),
        };
        let config_bytes = if cfg.short_header {
            Vec::new()
        } else {
            headers::write_config(&VolParams {
                profile_and_level: profile_level(mbw * mbh, advanced),
                advanced_simple: advanced,
                width: cfg.width,
                height: cfg.height,
                time_resolution: cfg.time_base,
                fixed_increment: (cfg.frame_duration < cfg.time_base).then_some(cfg.frame_duration),
                resync_markers: cfg.packet_bytes.is_some(),
                data_partitioned: cfg.data_partitioning,
                reversible_vlc: cfg.reversible_vlc,
                video_signal: cfg.video_signal,
                quarter_sample: cfg.quarter_sample,
                interlaced: cfg.interlaced,
                mpeg_quant: mpeg.then_some((intra_matrix, inter_matrix)),
            })
        };
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
            quant: Quant {
                mpeg,
                intra_matrix,
                inter_matrix,
            },
            rc,
            recons: Vec::new(),
            force_intra: false,
            tframe: None,
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
        let intra =
            self.force_intra || self.future.is_none() || (gop > 0 && index.is_multiple_of(gop));
        self.force_intra = false;
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

    /// Makes the next frame given to [`Encoder::encode`] an I-VOP (a
    /// random access point), coded at once: B-VOPs still waiting are coded
    /// after it, predicted from it. The I-VOP period
    /// ([`EncoderConfig::gop_size`]) carries on counting frames as before.
    pub fn force_keyframe(&mut self) {
        self.force_intra = true;
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
        if !intra && !self.cfg.short_header {
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
            top_field_first: self.cfg.top_field_first,
            alternate_vertical_scan: false,
        };
        self.prev_ref_sec = self.last_ref_sec;
        self.last_ref_sec = sec;
        let start = w.len_bits();
        if self.cfg.short_header {
            // TR: the frame's time in 1001/30000 s units (rounded), modulo
            // 256.
            let d = res as i128 * 1001;
            let tr = ((t as i128 * 30000 * 2 + d) / (2 * d)) as u32;
            let format = headers::short_header_format(self.cfg.width, self.cfg.height).unwrap();
            headers::write_short_header(w, tr, format, !intra, qp);
        } else {
            headers::write_vop_header(w, self.time_bits, &hdr, self.cfg.interlaced);
        }
        let mut recon = Pic::new(self.cfg.width, self.cfg.height);
        self.code_ip(w, src, &mut recon, &hdr);
        if self.cfg.short_header {
            w.align_zero();
        } else {
            w.stuff();
        }
        self.rate_update(w.len_bits() - start, intra);
        self.prev_mv.clone_from(&self.st.mv);
        let motion = if intra {
            Motion::intra(self.st.mbw, self.st.mbh)
        } else {
            Motion::of(&self.st)
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
        if !self.packet_due(mb, w.len_bits() - *packet_start) {
            return false;
        }
        self.put_packet_header(w, mb, h);
        *packet_start = w.len_bits();
        true
    }

    /// Whether a packet of `bits` so far should end before macroblock `mb`.
    fn packet_due(&self, mb: usize, bits: usize) -> bool {
        self.cfg
            .packet_bytes
            .is_some_and(|limit| mb > 0 && bits > limit as usize * 8)
    }

    /// Stuffing, a resync marker and `video_packet_header()` for a packet
    /// starting at macroblock `mb`, which opens a new slice.
    fn put_packet_header(&mut self, w: &mut BitWriter, mb: usize, h: &VopHeader) {
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
        self.new_slice();
    }

    fn code_ip(&mut self, w: &mut BitWriter, src: &Pic, recon: &mut Pic, h: &VopHeader) {
        let (mbw, mbh) = (self.st.mbw, self.st.mbh);
        self.new_slice();
        let dp = self.cfg.data_partitioning;
        let i_vop = h.vop_type == VopType::I;
        let reference = self.future.take();
        let mut packet: Vec<MbSyntax> = Vec::new();
        let mut packet_bits = 0;
        let sh = self.cfg.short_header;
        // GOBs of the short video header: one macroblock row up to CIF,
        // two at 4CIF, four at 16CIF.
        let gob_rows = match self.cfg.height {
            0..=288 => 1,
            289..=576 => 2,
            _ => 4,
        };
        for mb in 0..mbw * mbh {
            if sh {
                if mb > 0 && mb % (mbw * gob_rows) == 0 && self.packet_due(mb, packet_bits) {
                    // GOB header: GBSC, GN, GFID, GQUANT.
                    w.put(17, 1);
                    w.put(5, (mb / (mbw * gob_rows)) as u32);
                    w.put(2, 0);
                    w.put(5, h.quant);
                    self.new_slice();
                    packet_bits = 0;
                }
            } else if self.packet_due(mb, packet_bits) {
                if dp {
                    write_partitioned(w, &packet, i_vop, self.cfg.reversible_vlc);
                    packet.clear();
                }
                self.put_packet_header(w, mb, h);
                packet_bits = 0;
            }
            let (mbx, mby) = (mb % mbw, mb / mbw);
            self.st.slice[mb] = self.slice;
            self.st.qp[mb] = h.quant as u8;
            let syn = match (&reference, h.vop_type) {
                (Some(rf), VopType::P) => self.code_p_mb(src, &rf.pic, recon, mbx, mby, h),
                _ if sh => self.code_intra_mb_short(src, recon, mbx, mby, h.quant, !i_vop),
                _ => self.code_intra_mb(src, recon, mbx, mby, h.quant, !i_vop),
            };
            let mut t = BitWriter::new();
            syn.write(&mut t);
            packet_bits += t.len_bits();
            if dp {
                packet.push(syn);
            } else {
                w.append(&t);
            }
        }
        if dp {
            write_partitioned(w, &packet, i_vop, self.cfg.reversible_vlc);
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
            top_field_first: self.cfg.top_field_first,
            alternate_vertical_scan: false,
        };
        let start = w.len_bits();
        headers::write_vop_header(w, self.time_bits, &hdr, self.cfg.interlaced);
        let trb = (t - past.time) as i32;
        let trd = (future.time - past.time) as i32;
        let tframe = *self.tframe.get_or_insert(t - past.time);
        let field_times = [past.time, t, future.time, tframe];
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
                field_times,
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
        field_times: [i64; 4],
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
        let qpel = self.cfg.quarter_sample;
        predict_mb(&past.pic, mbx, mby, &[mvf; 4], false, false, qpel, &mut pf);
        predict_mb(
            &future.pic,
            mbx,
            mby,
            &[mvb; 4],
            false,
            false,
            qpel,
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
        let fm = &future.motion;
        let mut pd = MbPix::new();
        if fm.field[mb] && fm.kind[mb] == MbKind::Inter {
            // Field direct mode (7.7.2.3).
            let refs = fm.field_ref[mb];
            let (tb, td) = field_direct_distances(field_times, self.cfg.top_field_first, refs);
            let (f, b) = field_direct_vectors(fm.field_mv[mb], [0, 0], tb, td);
            field_direct_predict(
                &past.pic,
                &future.pic,
                mbx,
                mby,
                &f,
                &b,
                refs,
                qpel,
                &mut pd,
            );
        } else {
            let (dmf, dmb) = direct_vectors(fm, mbx, mby, [0, 0], trb, trd);
            let mut pdb = MbPix::new();
            predict_mb(&past.pic, mbx, mby, &dmf, true, false, qpel, &mut pd);
            predict_mb(&future.pic, mbx, mby, &dmb, true, false, qpel, &mut pdb);
            mc::average(&mut pd.y, &pdb.y);
            mc::average(&mut pd.cb, &pdb.cb);
            mc::average(&mut pd.cr, &pdb.cr);
        }
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
        let field_dct = self.cfg.interlaced && prefer_field_dct(&luma_residual(src, mbx, mby, &px));
        let mut levels = [[0i16; 64]; 6];
        let mut cbp = 0u8;
        for (k, lv) in levels.iter_mut().enumerate() {
            let s = source_block_f(src, mbx, mby, k, field_dct);
            let p = pred_block_f(&px, k, field_dct);
            for i in 0..64 {
                lv[i] = s[i] - p[i];
            }
            fdct(lv);
            self.quantise(lv, qp, false);
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
            if self.cfg.interlaced {
                // interlaced_information(): frame prediction only.
                if cbp != 0 {
                    w.put(1, field_dct as u32);
                }
                if mode != BMode::Direct {
                    w.put(1, 0); // field_prediction
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
            add_block(recon, mbx, mby, k, &rec, field_dct);
        }
    }

    /// An intra macroblock (in an I-VOP, or `in_p` a P-VOP).
    #[allow(clippy::too_many_arguments)]
    fn code_intra_mb(
        &mut self,
        src: &Pic,
        recon: &mut Pic,
        mbx: usize,
        mby: usize,
        qp: u32,
        in_p: bool,
    ) -> MbSyntax {
        let mb = mby * self.st.mbw + mbx;
        self.st.kind[mb] = MbKind::Intra;
        self.st.field[mb] = false;
        self.st.set_mb_mv(mbx, mby, [0, 0]);
        let field_dct = self.cfg.interlaced && {
            let mut ys = [0u8; 256];
            luma_mb(src, mbx, mby, &mut ys);
            prefer_field_dct(&ys.map(|v| v as i16))
        };
        let mut levels = [[0i16; 64]; 6];
        let mut preds: [Option<IntraPred>; 6] = [None; 6];
        for k in 0..6 {
            let mut b = source_block_f(src, mbx, mby, k, field_dct);
            fdct(&mut b);
            let scaler = dc_scaler(qp, k < 4) as i32;
            let dc = round_div(b[0] as i32, scaler).clamp(0, 2047 / scaler);
            self.quantise(&mut b, qp, true);
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
        for (k, lv) in levels.iter().enumerate() {
            let mut rec = *lv;
            self.quant.intra(&mut rec, qp, dc_scaler(qp, k < 4));
            idct(&mut rec);
            put_block(recon, mbx, mby, k, &rec, field_dct);
        }
        MbSyntax {
            interlaced: self.cfg.interlaced,
            field_dct,
            mb_type: 3,
            cbp,
            ac_pred,
            dc: dc_diff,
            blocks: coded,
            scans,
            ..MbSyntax::new(in_p, self.fcode)
        }
    }

    /// An intra macroblock of a short-header picture: INTRADC (the DC over
    /// 8, 1..=254, 128 coded as 255) and AC levels (H.263's quantiser,
    /// clipped to what its escape codes) with no prediction.
    fn code_intra_mb_short(
        &mut self,
        src: &Pic,
        recon: &mut Pic,
        mbx: usize,
        mby: usize,
        qp: u32,
        in_p: bool,
    ) -> MbSyntax {
        let mb = mby * self.st.mbw + mbx;
        self.st.kind[mb] = MbKind::Intra;
        self.st.set_mb_mv(mbx, mby, [0, 0]);
        let mut syn = MbSyntax::new(in_p, self.fcode);
        syn.sh = true;
        syn.mb_type = 3;
        for k in 0..6 {
            let mut b = source_block(src, mbx, mby, k);
            fdct(&mut b);
            let dc = round_div(b[0] as i32, 8).clamp(1, 254);
            quantise_h263(&mut b, qp, true);
            for v in &mut b[1..] {
                *v = (*v).clamp(-127, 127);
            }
            syn.dc[k] = if dc == 128 { 255 } else { dc };
            b[0] = 0;
            if b.iter().any(|&v| v != 0) {
                syn.cbp |= 1 << (5 - k);
            }
            let mut rec = b;
            rec[0] = dc as i16;
            self.quant.intra(&mut rec, qp, 8);
            idct(&mut rec);
            put_block(recon, mbx, mby, k, &rec, false);
            syn.blocks[k] = b;
        }
        syn
    }

    /// A P-VOP macroblock: motion search, then intra, skipped, one- or
    /// four-vector inter coding.
    #[allow(clippy::too_many_arguments)]
    fn code_p_mb(
        &mut self,
        src: &Pic,
        rf: &Pic,
        recon: &mut Pic,
        mbx: usize,
        mby: usize,
        h: &VopHeader,
    ) -> MbSyntax {
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
            if self.cfg.short_header {
                return self.code_intra_mb_short(src, recon, mbx, mby, qp, true);
            }
            return self.code_intra_mb(src, recon, mbx, mby, qp, true);
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
        let qpel = self.cfg.quarter_sample;
        // Interlaced: field prediction when its two vectors predict better
        // than the frame vector, rate included.
        let mut field: Option<([[i32; 2]; 2], [bool; 2])> = None;
        if self.cfg.interlaced && !four {
            let fp = [pred0[0], pred0[1] >> 1];
            let start = [mv[0], mv[1] >> 1];
            let mut fsad = 0;
            let mut fmv = [[0; 2]; 2];
            let mut fref = [false; 2];
            for f in 0..2 {
                let mut best = (u32::MAX, [0, 0], false);
                for parity in [f == 1, f != 1] {
                    let (v, sad) =
                        self.search_field(src, rf, mbx, mby, f, parity, start, h.rounding);
                    let c = sad + qp * self.mv_bits(v, fp);
                    if c < best.0 {
                        best = (c, v, parity);
                    }
                }
                fsad += best.0;
                fmv[f] = best.1;
                fref[f] = best.2;
            }
            if fsad + 2 * qp < sad16 + qp * self.mv_bits(mv, pred0) {
                field = Some((fmv, fref));
            }
        }
        match field {
            Some((fmv, fref)) => {
                predict_fields(rf, mbx, mby, &fmv, fref, h.rounding, qpel, &mut px)
            }
            None => predict_mb(rf, mbx, mby, &mvs, four, h.rounding, qpel, &mut px),
        }
        let field_dct = self.cfg.interlaced && prefer_field_dct(&luma_residual(src, mbx, mby, &px));
        let mut levels = [[0i16; 64]; 6];
        let mut cbp = 0u8;
        for (k, lv) in levels.iter_mut().enumerate() {
            let s = source_block_f(src, mbx, mby, k, field_dct);
            let p = pred_block_f(&px, k, field_dct);
            for i in 0..64 {
                lv[i] = s[i] - p[i];
            }
            fdct(lv);
            self.quantise(lv, qp, false);
            if lv.iter().any(|&v| v != 0) {
                cbp |= 1 << (5 - k);
            }
        }
        write_mb(recon, mbx, mby, &px);
        let mut syn = MbSyntax::new(true, self.fcode);
        syn.sh = self.cfg.short_header;
        syn.interlaced = self.cfg.interlaced;
        self.st.field[mb] = false;
        if syn.sh {
            // H.263's escape codes levels up to 127; reconstruct what is
            // coded.
            for lv in levels.iter_mut() {
                for v in lv.iter_mut() {
                    *v = (*v).clamp(-127, 127);
                }
            }
        }
        if !four && field.is_none() && mv == [0, 0] && cbp == 0 {
            self.st.kind[mb] = MbKind::Skipped;
            self.st.set_mb_mv(mbx, mby, [0, 0]);
            syn.not_coded = true;
            return syn;
        }
        self.st.kind[mb] = MbKind::Inter;
        syn.mb_type = if four { 2 } else { 0 };
        syn.cbp = cbp;
        syn.field_dct = field_dct && cbp != 0;
        let field_dct = syn.field_dct;
        if let Some((fmv, fref)) = field {
            let fp = [pred0[0], pred0[1] >> 1];
            syn.field_pred = Some(fref);
            syn.mvd.push(self.mv_diff(fmv[0], fp));
            syn.mvd.push(self.mv_diff(fmv[1], fp));
            self.st.set_mb_mv(mbx, mby, field_to_frame(fmv[0], fmv[1]));
            self.st.field[mb] = true;
            self.st.field_mv[mb] = fmv.map(|v| [v[0] as i16, v[1] as i16]);
            self.st.field_ref[mb] = fref;
        } else if four {
            for (k, v) in mvs.iter().enumerate() {
                let p = self.st.mv_pred(mbx, mby, k, self.slice);
                syn.mvd.push(self.mv_diff(*v, p));
                self.st.set_mv(mbx, mby, k, *v);
            }
        } else {
            syn.mvd.push(self.mv_diff(mv, pred0));
            self.st.set_mb_mv(mbx, mby, mv);
        }
        for (k, lv) in levels.iter().enumerate() {
            if cbp >> (5 - k) & 1 == 0 {
                continue;
            }
            let mut rec = *lv;
            self.quant.inter(&mut rec, qp);
            idct(&mut rec);
            add_block(recon, mbx, mby, k, &rec, field_dct);
        }
        syn.blocks = levels;
        syn
    }

    /// A vector's difference from its predictor, wrapped into the range
    /// `vop_fcode` codes.
    fn mv_diff(&self, mv: [i32; 2], pred: [i32; 2]) -> [i32; 2] {
        [
            wrap_diff(mv[0] - pred[0], self.fcode),
            wrap_diff(mv[1] - pred[1], self.fcode),
        ]
    }

    /// Forward quantisation with the VOL's method.
    fn quantise(&self, b: &mut [i16; 64], qp: u32, intra: bool) {
        if self.quant.mpeg {
            let m = if intra {
                &self.quant.intra_matrix
            } else {
                &self.quant.inter_matrix
            };
            quantise_mpeg(b, qp, intra, m);
        } else {
            quantise_h263(b, qp, intra);
        }
    }

    /// Vector units per sample: 2 (half samples) or 4 (quarter samples).
    fn unit(&self) -> i32 {
        if self.cfg.quarter_sample { 4 } else { 2 }
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
    /// against its prediction at `mv` (half or quarter samples), over block `k`
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
        if self.cfg.quarter_sample {
            mc::qpel(
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
        } else {
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
        }
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

    /// Clamps a vector into the search window.
    fn clamp_mv(&self, v: [i32; 2]) -> [i32; 2] {
        let r = self.unit() * self.cfg.search_range as i32;
        [v[0].clamp(-r, r), v[1].clamp(-r, r)]
    }

    /// Clamps a half-sample vector of macroblock `(mbx, mby)` so that
    /// every sample it predicts from lies inside the picture, as H.263's
    /// baseline (no unrestricted vectors) requires of the short header.
    fn inside(&self, mbx: usize, mby: usize, v: [i32; 2]) -> [i32; 2] {
        if !self.cfg.short_header {
            return v;
        }
        let lim = |pos: usize, size: u32, m: i32| -> i32 {
            // Integer position pos + (m >> 1) >= 0, and the 16 samples (17
            // with a half-sample offset) end inside.
            let lo = -2 * pos as i32;
            let hi = 2 * (size as i32 - 16 - pos as i32);
            m.clamp(lo, hi)
        };
        [
            lim(mbx * 16, self.cfg.width, v[0]),
            lim(mby * 16, self.cfg.height, v[1]),
        ]
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
        let u = self.unit();
        let even = |v: [i32; 2]| [v[0] & !(u - 1), v[1] & !(u - 1)];
        let mut best = [0, 0];
        let mut best_cost = u32::MAX;
        let mut best_sad = u32::MAX;
        for &c in [[0, 0], pred].iter().chain(extra) {
            let c = self.inside(mbx, mby, even(self.clamp_mv(c)));
            let s = self.sad(src, rf, mbx, mby, None, c, rounding);
            let k = cost(c, s);
            if k < best_cost {
                (best, best_cost, best_sad) = (c, k, s);
            }
        }
        // Small diamond in whole samples.
        for _ in 0..64 {
            let mut moved = false;
            for d in [[u, 0], [-u, 0], [0, u], [0, -u]] {
                let c = self.inside(mbx, mby, self.clamp_mv([best[0] + d[0], best[1] + d[1]]));
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
        // Half-sample, then (quarter-sample motion) quarter-sample
        // refinement.
        let mut step = u / 2;
        while step > 0 {
            let centre = best;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let c = self.inside(
                        mbx,
                        mby,
                        self.clamp_mv([centre[0] + dx * step, centre[1] + dy * step]),
                    );
                    let s = self.sad(src, rf, mbx, mby, None, c, rounding);
                    let k = cost(c, s);
                    if k < best_cost {
                        (best, best_cost, best_sad) = (c, k, s);
                    }
                }
            }
            step /= 2;
        }
        (best, best_sad)
    }

    /// SAD of field `f` (0 top, 1 bottom: every other line, 16x8) of
    /// macroblock `(mbx, mby)` against reference field `parity` (true:
    /// bottom) at vector `mv`, whose vertical component counts field lines.
    #[allow(clippy::too_many_arguments)]
    fn sad_field(
        &self,
        src: &Pic,
        rf: &Pic,
        mbx: usize,
        mby: usize,
        f: usize,
        parity: bool,
        mv: [i32; 2],
        rounding: bool,
    ) -> u32 {
        let (p, stride, w, h) = rf.ref_plane(0);
        let fld: mc::Src = (&p[parity as usize * stride..], 2 * stride, w, h / 2);
        let (x, y) = (mbx as i32 * 16, mby as i32 * 8);
        let mut out = [0u8; 128];
        if self.cfg.quarter_sample {
            mc::qpel(fld, x, y, mv[0], mv[1], 16, 8, rounding, &mut out, 16);
        } else {
            mc::halfpel(fld, x, y, mv[0], mv[1], 16, 8, rounding, &mut out, 16);
        }
        let s = src.ystride();
        let mut sum = 0;
        for r in 0..8 {
            let row = &src.y[(mby * 16 + f + 2 * r) * s + mbx * 16..];
            for c in 0..16 {
                sum += (row[c] as i32 - out[r * 16 + c] as i32).unsigned_abs();
            }
        }
        sum
    }

    /// A field vector for field `f` from reference field `parity`: a
    /// whole-sample diamond from `start`, then finer steps.
    #[allow(clippy::too_many_arguments)]
    fn search_field(
        &self,
        src: &Pic,
        rf: &Pic,
        mbx: usize,
        mby: usize,
        f: usize,
        parity: bool,
        start: [i32; 2],
        rounding: bool,
    ) -> ([i32; 2], u32) {
        let u = self.unit();
        let mut best = self.clamp_mv([start[0] & !(u - 1), start[1] & !(u - 1)]);
        let mut best_sad = self.sad_field(src, rf, mbx, mby, f, parity, best, rounding);
        for step in [u, u / 2, u / 4].into_iter().filter(|&s| s > 0) {
            for _ in 0..16 {
                let mut moved = false;
                for d in [[step, 0], [-step, 0], [0, step], [0, -step]] {
                    let c = self.clamp_mv([best[0] + d[0], best[1] + d[1]]);
                    let s = self.sad_field(src, rf, mbx, mby, f, parity, c, rounding);
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
        let u = self.unit();
        for step in [u, u / 2, u / 4].into_iter().filter(|&s| s > 0) {
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

/// Block `k` of the source; with `field_dct`, luminance blocks 0 and 1
/// are the top field's lines and 2 and 3 the bottom field's.
fn source_block_f(src: &Pic, mbx: usize, mby: usize, k: usize, field_dct: bool) -> [i16; 64] {
    if !field_dct || k > 3 {
        return source_block(src, mbx, mby, k);
    }
    let s = src.ystride();
    let o = (mby * 16 + (k >> 1)) * s + mbx * 16 + (k & 1) * 8;
    let mut b = [0i16; 64];
    for r in 0..8 {
        for c in 0..8 {
            b[r * 8 + c] = src.y[o + 2 * r * s + c] as i16;
        }
    }
    b
}

/// Block `k` of a prediction, laid out as [`source_block_f`].
fn pred_block_f(px: &MbPix, k: usize, field_dct: bool) -> [i16; 64] {
    if !field_dct || k > 3 {
        return pred_block(px, k);
    }
    let mut b = [0i16; 64];
    for r in 0..8 {
        for c in 0..8 {
            b[r * 8 + c] = px.y[((k >> 1) + 2 * r) * 16 + (k & 1) * 8 + c] as i16;
        }
    }
    b
}

/// The luminance residual of a macroblock against a prediction.
fn luma_residual(src: &Pic, mbx: usize, mby: usize, px: &MbPix) -> [i16; 256] {
    let mut ys = [0u8; 256];
    luma_mb(src, mbx, mby, &mut ys);
    let mut d = [0i16; 256];
    for i in 0..256 {
        d[i] = ys[i] as i16 - px.y[i] as i16;
    }
    d
}

/// Whether field DCT suits 16x16 luminance (samples or residual): its
/// lines differ less from the next line of the same field than from the
/// next line of the frame.
fn prefer_field_dct(y: &[i16; 256]) -> bool {
    let diff = |gap: usize| -> i32 {
        (0..16 - gap)
            .map(|r| {
                (0..16)
                    .map(|c| (y[r * 16 + c] - y[(r + gap) * 16 + c]).unsigned_abs() as i32)
                    .sum::<i32>()
            })
            .sum()
    };
    // The frame sum has 15 line pairs, the field sum 14.
    diff(2) * 15 < diff(1) * 14
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
