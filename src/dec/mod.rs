//! The decoder: start-code units in, frames out in display order.

mod texture;
pub(crate) mod vop;

use std::collections::VecDeque;

use crate::bits::BitReader;
use crate::error::{Error, Result, invalid, unsupported};
use crate::frame::{Frame, VopType};
use crate::headers::{self, SpriteMode, VisualObject, VolHeader, VopHeader, sc};
use crate::mbstate::{MbKind, MbState};
use crate::picture::Pic;
use crate::quant::Quant;
use vop::{Motion, VopDec};

/// Splits a buffer into start-code units: `(start code value, payload
/// after the four start-code bytes)`. Bytes before the first start code
/// are ignored.
pub(crate) fn split_units(data: &[u8]) -> Vec<(u8, &[u8])> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i);
            // The value byte belongs to this start code, never to the next.
            i += 4;
        } else if data[i + 2] > 1 {
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (k, &s) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(data.len());
        out.push((data[s + 3], &data[s + 4..end]));
    }
    out
}

/// Positions of short video header pictures (`short_video_start_marker`,
/// 22 bits `0000 0000 0000 0000 1000 00`, byte aligned).
fn split_short(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 2 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] & 0xfc == 0x80 {
            starts.push(i);
            i += 3;
        } else {
            i += 1;
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(k, &s)| &data[s..starts.get(k + 1).copied().unwrap_or(data.len())])
        .collect()
}

/// Whether the first start pattern in `data` is a short video header
/// rather than an MPEG-4 start code.
fn starts_short(data: &[u8]) -> bool {
    for i in 0..data.len().saturating_sub(2) {
        if data[i] == 0 && data[i + 1] == 0 {
            if data[i + 2] == 1 {
                return false;
            }
            if data[i + 2] & 0xfc == 0x80 {
                return true;
            }
        }
    }
    false
}

/// A reconstructed I-, P- or S-VOP kept for prediction.
struct RefPic {
    pic: Pic,
    motion: Motion,
    time: i64,
    vop_type: VopType,
    decode_index: u64,
    concealed: bool,
}

/// An MPEG-4 Part 2 Visual decoder.
///
/// Feed it access units (or any buffers of whole start-code units, the
/// headers among them) with [`Decoder::decode`]; it returns the frames that
/// became displayable, in display order. B-VOPs reorder: an I- or P-VOP is
/// held until the next one arrives (or [`Decoder::flush`]), unless the VOL
/// says `low_delay`. The H.263 short video header is recognised by its
/// start marker. DivX-style packed bitstreams — a P-VOP and the B-VOP
/// before it in one access unit, then a not-coded VOP as a placeholder —
/// come out as one frame per access unit in display order.
pub struct Decoder {
    vo: VisualObject,
    profile_and_level: Option<u8>,
    vol: Option<VolHeader>,
    quant: Quant,
    short_header: bool,
    st: Option<MbState>,
    slice_counter: u32,
    past: Option<RefPic>,
    future: Option<RefPic>,
    future_pending: bool,
    spare: Option<Pic>,
    last_ref_sec: i64,
    prev_ref_sec: i64,
    gov_base: Option<i64>,
    sh_time: Option<(u32, i64)>,
    decode_index: u64,
    packed: bool,
    packed_debt: u32,
    ready: VecDeque<Frame>,
    last_error: Option<Error>,
    stats: DecoderStats,
    /// A `vop_time_increment` length found by [`Decoder::parse_vop`] to
    /// differ from the VOL's.
    time_bits: Option<u32>,
    /// The DivX version a user data string named (500 for DivX 5.00).
    divx_version: Option<u32>,
    /// Interlaced streams whose P-VOPs carry `dct_type` in every coded
    /// macroblock (see `VopDec::dct_type_always`).
    dct_type_always: bool,
}

/// Counts a [`Decoder`] keeps, for monitoring a stream's health.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecoderStats {
    /// Coded VOPs decoded (including concealed ones).
    pub vops: u64,
    /// VOPs in which damage was concealed.
    pub concealed_vops: u64,
    /// VOPs decoded without error whose macroblock data did not end exactly
    /// at the stuffing before the next start code: a stream that parses
    /// but not as this decoder reads it (or an encoder that appends data).
    pub misaligned_vops: u64,
    /// VOPs that produced no frame: not-coded placeholders after packed
    /// B-VOPs, B-VOPs or P-VOPs whose references were never decoded.
    pub dropped_vops: u64,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    /// A decoder that takes its configuration in-band (the VOL header in
    /// the stream, or a short video header).
    pub fn new() -> Decoder {
        Decoder {
            vo: VisualObject { verid: 1 },
            profile_and_level: None,
            vol: None,
            quant: Quant::h263(),
            short_header: false,
            st: None,
            slice_counter: 0,
            past: None,
            future: None,
            future_pending: false,
            spare: None,
            last_ref_sec: 0,
            prev_ref_sec: 0,
            gov_base: None,
            sh_time: None,
            decode_index: 0,
            packed: false,
            packed_debt: 0,
            ready: VecDeque::new(),
            last_error: None,
            stats: DecoderStats::default(),
            time_bits: None,
            divx_version: None,
            dct_type_always: false,
        }
    }

    /// A decoder configured from decoder specific info — the visual object
    /// sequence / visual object / VOL headers an MP4 `esds` (or Matroska
    /// `CodecPrivate`, or an AVI's `strf` extra data) carries.
    pub fn with_config(dsi: &[u8]) -> Result<Decoder> {
        let mut d = Decoder::new();
        d.configure(dsi)?;
        if d.vol.is_none() {
            return Err(invalid("decoder specific info without a video object layer header"));
        }
        Ok(d)
    }

    /// Applies configuration headers (VOS / VO / VOL) without decoding.
    pub fn configure(&mut self, dsi: &[u8]) -> Result<()> {
        let frames = self.decode(dsi)?;
        self.ready.extend(frames);
        Ok(())
    }

    /// The active VOL header, once one has been seen (for the short video
    /// header, the one it implies).
    pub fn vol(&self) -> Option<&VolHeader> {
        self.vol.as_ref()
    }

    /// What the decoder has counted so far.
    pub fn stats(&self) -> &DecoderStats {
        &self.stats
    }

    /// The first error concealed in the most recent damaged VOP, if any:
    /// frames with [`Frame::concealed`] set say which.
    pub fn last_error(&self) -> Option<&Error> {
        self.last_error.as_ref()
    }

    /// Decodes a buffer of whole start-code units (typically one access
    /// unit) and returns the frames that are now ready, in display order.
    ///
    /// Header errors and unsupported tools are returned as errors (frames
    /// decoded earlier in the same buffer are not lost: they come back
    /// from the next call). Damage inside a VOP's macroblocks is concealed
    /// instead: the frame comes back with [`Frame::concealed`] set and
    /// [`Decoder::last_error`] saying what was wrong.
    pub fn decode(&mut self, data: &[u8]) -> Result<Vec<Frame>> {
        let res = if self.short_header || (self.vol.is_none() && starts_short(data)) {
            self.decode_short(data)
        } else {
            self.decode_mpeg4(data)
        };
        let out: Vec<Frame> = self.ready.drain(..).collect();
        match res {
            Ok(()) => Ok(out),
            Err(e) => {
                self.ready.extend(out);
                Err(e)
            }
        }
    }

    /// Returns the frame still held for reordering, if any. The decoder
    /// can go on decoding afterwards (as after a seek: the next VOP should
    /// be an I-VOP).
    pub fn flush(&mut self) -> Vec<Frame> {
        if self.future_pending {
            self.future_pending = false;
            if let Some(f) = &self.future {
                let fr = self.ref_frame(f);
                self.ready.push_back(fr);
            }
        }
        self.ready.drain(..).collect()
    }

    fn ref_frame(&self, f: &RefPic) -> Frame {
        let res = self.vol.as_ref().map_or(1, |v| v.time_resolution);
        let mut fr = f.pic.to_frame(f.time, res, f.vop_type, f.decode_index);
        fr.concealed = f.concealed;
        fr
    }

    fn decode_mpeg4(&mut self, data: &[u8]) -> Result<()> {
        let units = split_units(data);
        let vops: Vec<u8> = units
            .iter()
            .filter(|u| u.0 == sc::VOP)
            .map(|u| u.1.first().map_or(0, |b| b >> 6))
            .collect();
        if vops.len() >= 2 {
            // A reference then a B-VOP in one buffer: a packed bitstream.
            if !self.packed && vops[0] != 2 && vops[1] == 2 {
                self.packed = true;
            }
            if self.packed {
                self.packed_debt += vops.len() as u32 - 1;
            }
        }
        for (code, body) in units {
            match code {
                sc::VOS => {
                    self.profile_and_level = body.first().copied();
                }
                sc::VISUAL_OBJECT => {
                    self.vo = headers::parse_visual_object(&mut BitReader::new(body))?;
                }
                sc::USER_DATA => self.user_data(body),
                sc::VOL_FIRST..=sc::VOL_LAST => {
                    let vol = headers::parse_vol(&mut BitReader::new(body), self.vo, self.profile_and_level)?;
                    self.set_vol(vol)?;
                }
                sc::GOV => {
                    self.gov_base = Some(headers::parse_gov(&mut BitReader::new(body))? as i64);
                }
                sc::VOP => self.vop(body)?,
                _ => {}
            }
        }
        Ok(())
    }

    fn user_data(&mut self, body: &[u8]) {
        // DivX writes "DivX<version>b<build>" (DivX 5.00: "DivX500Build413")
        // with a trailing 'p' when the stream packs B-VOPs with the
        // following P-VOP.
        if body.starts_with(b"DivX") {
            let digits: Vec<u8> = body[4..].iter().copied().take_while(u8::is_ascii_digit).collect();
            self.divx_version = std::str::from_utf8(&digits).ok().and_then(|s| s.parse().ok());
            let end = body.iter().rposition(|&b| b != 0).map_or(0, |p| p + 1);
            if end > 4 && body[end - 1] == b'p' {
                self.packed = true;
            }
        }
    }

    fn set_vol(&mut self, vol: VolHeader) -> Result<()> {
        vol.check_supported()?;
        let resize = self.vol.as_ref().is_none_or(|v| v.width != vol.width || v.height != vol.height);
        if resize {
            self.st = Some(MbState::new(vol.mb_width(), vol.mb_height()));
            self.past = None;
            self.future = None;
            self.future_pending = false;
            self.spare = None;
        }
        self.time_bits = None;
        self.dct_type_always = false;
        self.quant = Quant { mpeg: vol.mpeg_quant, intra_matrix: vol.intra_matrix, inter_matrix: vol.inter_matrix };
        self.vol = Some(vol);
        Ok(())
    }

    /// The display time of a VOP, in ticks, from `modulo_time_base` and
    /// `vop_time_increment` (6.3.5): I/P-VOPs count seconds from the
    /// previous I/P-VOP (or the GOV time code), B-VOPs from the I/P-VOP
    /// before them in display order.
    fn vop_time(&mut self, h: &VopHeader, res: u32) -> i64 {
        let res = res as i64;
        let inc = h.time_increment as i64;
        if h.vop_type == VopType::B {
            (self.prev_ref_sec + h.modulo_time_base as i64) * res + inc
        } else {
            let base = self.gov_base.take().unwrap_or(self.last_ref_sec);
            let sec = base + h.modulo_time_base as i64;
            self.prev_ref_sec = self.last_ref_sec;
            self.last_ref_sec = sec;
            sec * res + inc
        }
    }

    fn take_pic(&mut self, w: u32, h: u32) -> Pic {
        match self.spare.take() {
            Some(p) if p.w == w && p.h == h => p,
            _ => Pic::new(w, h),
        }
    }

    fn immediate_output(&self) -> bool {
        self.short_header || self.vol.as_ref().is_some_and(|v| v.low_delay == Some(true))
    }

    /// Makes a decoded I/P/S-VOP the newest reference, releasing the one it
    /// displaces for display.
    fn push_ref(&mut self, r: RefPic) {
        if self.future_pending
            && let Some(f) = &self.future
        {
            let fr = self.ref_frame(f);
            self.ready.push_back(fr);
        }
        if let Some(old) = self.past.take() {
            self.spare = Some(old.pic);
        }
        self.past = self.future.take();
        self.future = Some(r);
        self.future_pending = true;
        if self.immediate_output() {
            let f = self.future.as_ref().unwrap();
            let fr = self.ref_frame(f);
            self.ready.push_back(fr);
            self.future_pending = false;
        }
    }

    fn vop(&mut self, body: &[u8]) -> Result<()> {
        let vol = self.vol.clone().ok_or_else(|| invalid("a VOP before any video object layer header"))?;
        let mut r = BitReader::new(body);
        let h = self.parse_vop(&mut r, &vol)?;
        let time = self.vop_time(&h, vol.time_resolution);
        let index = self.decode_index;
        self.decode_index += 1;
        if !h.coded {
            return self.not_coded(&h, time, index);
        }
        self.decode_vop(&vol, &h, &mut r, time, index, false)
    }

    /// The VOP header, with the VOL's `vop_time_increment` length or, when
    /// the marker after it is missing, the first other length that parses
    /// (kept for the rest of the stream).
    fn parse_vop(&mut self, r: &mut BitReader, vol: &VolHeader) -> Result<VopHeader> {
        let bits = self.time_bits.unwrap_or(vol.time_increment_bits);
        let start = r.pos();
        if let Ok(h) = headers::parse_vop_with(r, vol, bits, true) {
            return Ok(h);
        }
        for n in (1..=16).filter(|&n| n != bits) {
            r.set_pos(start);
            if let Ok(h) = headers::parse_vop_with(r, vol, n, true)
                && (!h.coded || h.quant != 0)
            {
                self.time_bits = Some(n);
                return Ok(h);
            }
        }
        r.set_pos(start);
        headers::parse_vop_with(r, vol, bits, false)
    }

    /// `vop_coded == 0`: a placeholder after a packed B-VOP (dropped), or
    /// a VOP identical to the reference (repeated).
    fn not_coded(&mut self, h: &VopHeader, time: i64, index: u64) -> Result<()> {
        if self.packed_debt > 0 {
            self.packed_debt -= 1;
            self.stats.dropped_vops += 1;
            return Ok(());
        }
        let Some(f) = &self.future else {
            self.stats.dropped_vops += 1;
            return Ok(());
        };
        if h.vop_type == VopType::B {
            let src = self.past.as_ref().unwrap_or(f);
            let mut fr = self.ref_frame(src);
            fr.timestamp = time;
            fr.vop_type = VopType::B;
            fr.decode_index = index;
            self.ready.push_back(fr);
            return Ok(());
        }
        let mbw = f.pic.mbw;
        let mbh = f.pic.mbh;
        let r = RefPic {
            pic: f.pic.clone(),
            motion: Motion {
                mbw,
                kind: vec![MbKind::Skipped; mbw * mbh],
                mv: vec![[0, 0]; 4 * mbw * mbh],
                field: vec![false; mbw * mbh],
            },
            time,
            vop_type: h.vop_type,
            decode_index: index,
            concealed: false,
        };
        self.push_ref(r);
        Ok(())
    }

    fn decode_vop(
        &mut self,
        vol: &VolHeader,
        h: &VopHeader,
        r: &mut BitReader,
        time: i64,
        index: u64,
        sh: bool,
    ) -> Result<()> {
        let is_b = h.vop_type == VopType::B;
        if h.vop_type != VopType::I && self.future.is_none() {
            // A P-VOP with nothing to predict from (decoding started
            // mid-stream): nothing to show until an I-VOP.
            self.stats.dropped_vops += 1;
            return Ok(());
        }
        if is_b && self.past.is_none() {
            // A leading B-VOP of an open GOP, its past reference missing.
            self.stats.dropped_vops += 1;
            return Ok(());
        }
        let mut cur = self.take_pic(vol.width, vol.height);
        let (trb, trd) = match (&self.past, &self.future) {
            (Some(p), Some(f)) if is_b => ((time - p.time) as i32, (f.time - p.time) as i32),
            _ => (0, 0),
        };
        let st = self.st.as_mut().ok_or_else(|| invalid("no macroblock state"))?;
        let (fwd, bwd, col) = if is_b {
            let p = self.past.as_ref().unwrap();
            let f = self.future.as_ref().unwrap();
            (Some(&p.pic), Some(&f.pic), Some(&f.motion))
        } else {
            (self.future.as_ref().map(|f| &f.pic), None, None)
        };
        let divx500 = h.warping_divx500 || self.divx_version == Some(500);
        let gmc = (h.vop_type == VopType::S).then(|| crate::gmc::Gmc::new(vol, &h.warping, divx500));
        let start = r.pos();
        let mut d = VopDec {
            vol,
            hdr: h,
            quant: &self.quant,
            sh,
            cur: &mut cur,
            fwd,
            bwd,
            col,
            gmc: gmc.as_ref(),
            st,
            slice_counter: &mut self.slice_counter,
            trb,
            trd,
            slice: 0,
            qp: h.quant,
            first_coded: true,
            pmv: [[0, 0]; 2],
            error: None,
            tail_ok: false,
            dct_type_always: self.dct_type_always,
        };
        d.run(r);
        if d.error.is_some() && vol.interlaced && !self.dct_type_always {
            // Early Xvid codes dct_type in every coded macroblock of a P-VOP,
            // even those without coded blocks: try the VOP again that way,
            // and keep that reading for the stream if it decodes.
            r.set_pos(start);
            d.error = None;
            d.dct_type_always = true;
            d.slice = 0;
            d.qp = h.quant;
            d.first_coded = true;
            d.pmv = [[0, 0]; 2];
            d.run(r);
            if d.error.is_none() && d.tail_ok {
                self.dct_type_always = true;
            }
        }
        let error = d.error.take();
        let concealed = error.is_some();
        self.stats.vops += 1;
        if concealed {
            self.stats.concealed_vops += 1;
        } else if !d.tail_ok {
            self.stats.misaligned_vops += 1;
        }
        if let Some(e) = error {
            self.last_error = Some(e);
        }
        if is_b {
            let mut fr = cur.to_frame(time, vol.time_resolution, VopType::B, index);
            fr.concealed = concealed;
            self.ready.push_back(fr);
            self.spare = Some(cur);
            return Ok(());
        }
        let st = self.st.as_ref().unwrap();
        let motion = if h.vop_type == VopType::I {
            Motion::intra(st.mbw, st.mbh)
        } else {
            Motion { mbw: st.mbw, kind: st.kind.clone(), mv: st.mv.clone(), field: st.field.clone() }
        };
        self.push_ref(RefPic { pic: cur, motion, time, vop_type: h.vop_type, decode_index: index, concealed });
        Ok(())
    }

    fn decode_short(&mut self, data: &[u8]) -> Result<()> {
        self.short_header = true;
        for pic in split_short(data) {
            self.short_picture(pic)?;
        }
        Ok(())
    }

    /// `video_plane_with_short_header()` (6.2.7.1): the H.263 picture
    /// layer, then the GOBs.
    fn short_picture(&mut self, data: &[u8]) -> Result<()> {
        let mut r = BitReader::new(data);
        r.skip(22)?;
        let tr = r.read(8)?;
        r.marker("in the short video header")?;
        if r.read_bit()? {
            return Err(invalid("zero_bit set in the short video header"));
        }
        r.read(3)?; // split_screen_indicator, document_camera_indicator, full_picture_freeze_release
        let (w, h) = match r.read(3)? {
            1 => (128, 96),
            2 => (176, 144),
            3 => (352, 288),
            4 => (704, 576),
            5 => (1408, 1152),
            7 => return Err(unsupported("H.263 extended picture type (PLUSPTYPE, H.263 version 2)")),
            f => return Err(invalid(format!("source_format {f}"))),
        };
        let p = r.read_bit()?;
        if r.read(4)? != 0 {
            return Err(unsupported("H.263 optional modes (UMV, SAC, AP or PB-frames)"));
        }
        let quant = r.read(5)?;
        if quant == 0 {
            return Err(invalid("vop_quant is zero"));
        }
        if r.read_bit()? {
            return Err(unsupported("H.263 continuous presence multipoint"));
        }
        while r.read_bit()? {
            r.read(8)?; // PSUPP
        }
        if self.vol.as_ref().is_none_or(|v| v.width != w || v.height != h) {
            let vol = VolHeader::short_header(w, h);
            self.set_vol(vol)?;
        }
        let vol = self.vol.clone().unwrap();
        // Temporal reference: 8 bits at 29.97 Hz, unwrapped.
        let t = match self.sh_time {
            None => 0,
            Some((last, t)) => t + ((tr + 256 - last) % 256) as i64,
        };
        self.sh_time = Some((tr, t));
        let hdr = VopHeader {
            vop_type: if p { VopType::P } else { VopType::I },
            modulo_time_base: 0,
            time_increment: 0,
            coded: true,
            rounding: false,
            intra_dc_vlc_thr: 0,
            quant,
            fcode_forward: 1,
            fcode_backward: 1,
            warping: Vec::new(),
            warping_divx500: false,
            top_field_first: false,
            alternate_vertical_scan: false,
        };
        let index = self.decode_index;
        self.decode_index += 1;
        self.decode_vop(&vol, &hdr, &mut r, t * 1001, index, true)
    }
}

impl VolHeader {
    /// The VOL a short video header implies.
    pub(crate) fn short_header(width: u32, height: u32) -> VolHeader {
        VolHeader {
            profile_and_level: None,
            object_type: 1,
            verid: 1,
            width,
            height,
            pixel_aspect: (12, 11),
            time_resolution: 30000,
            fixed_vop_time_increment: Some(1001),
            low_delay: Some(true),
            interlaced: false,
            obmc_disable: true,
            sprite: SpriteMode::None,
            sprite_warping_points: 0,
            sprite_warping_accuracy: 0,
            sprite_brightness_change: false,
            mpeg_quant: false,
            intra_matrix: crate::tables::DEFAULT_INTRA_MATRIX,
            inter_matrix: crate::tables::DEFAULT_INTER_MATRIX,
            quarter_sample: false,
            resync_marker_disable: true,
            data_partitioned: false,
            reversible_vlc: false,
            newpred: false,
            reduced_resolution: false,
            scalability: false,
            time_increment_bits: 15,
            complexity: None,
        }
    }
}

#[cfg(test)]
pub(crate) mod texture_for_tests {
    pub(crate) use super::texture::{read_coeffs, read_dc_diff};
    pub(crate) use super::vop::read_mvd;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::BitWriter;

    /// A short-header picture header: PSC, TR, PTYPE (sub-QCIF), PQUANT,
    /// CPM, PEI.
    fn picture_header(w: &mut BitWriter, tr: u32, p: bool) {
        w.put(22, 0b0000_0000_0000_0000_1000_00);
        w.put(8, tr);
        w.put(1, 1); // marker
        w.put(4, 0); // zero, split screen, document camera, freeze release
        w.put(3, 1); // sub-QCIF
        w.put(1, p as u32);
        w.put(4, 0); // no optional modes
        w.put(5, 8); // PQUANT
        w.put(1, 0); // CPM
        w.put(1, 0); // PEI
    }

    fn gob_header(w: &mut BitWriter, gn: u32, stuff: bool) {
        if stuff {
            w.align_zero(); // GSTUF
        }
        w.put(17, 1);
        w.put(5, gn);
        w.put(2, 0); // GFID
        w.put(5, 8); // GQUANT
    }

    /// Sub-QCIF (8x6 macroblocks, one GOB per row): an intra picture whose
    /// macroblocks are flat (INTRADC only), then a P picture of skipped
    /// macroblocks but one moved a sample to the right.
    #[test]
    fn short_video_header_pictures() {
        let mut w = BitWriter::new();
        picture_header(&mut w, 0, false);
        for gob in 0..6u32 {
            if gob > 0 {
                gob_header(&mut w, gob, gob == 3);
            }
            for x in 0..8u32 {
                w.put(1, 1); // MCBPC: intra, no chroma blocks coded
                w.put(4, 0b0011); // CBPY: none
                for k in 0..6 {
                    let dc = if k < 4 { 16 + (gob * 8 + x) * 3 + k } else { 128 };
                    // INTRADC 128 is coded as 255 (1000 0000 is reserved).
                    w.put(8, if dc == 128 { 255 } else { dc });
                }
            }
        }
        w.align_zero();
        picture_header(&mut w, 1, true);
        for gob in 0..6u32 {
            for x in 0..8u32 {
                if gob == 1 && x == 1 {
                    w.put(1, 0); // COD: coded
                    w.put(1, 1); // MCBPC: inter, no chroma blocks
                    w.put(2, 0b11); // CBPY 15: inter, none coded
                    w.put(4, 0b0010); // MVD x = +2 half samples
                    w.put(1, 1); // MVD y = 0
                } else {
                    w.put(1, 1); // COD: skipped
                }
            }
        }
        w.align_zero();
        w.put(22, 0b0000_0000_0000_0000_1111_11); // EOS
        let bytes = w.into_bytes();
        let mut d = Decoder::new();
        let mut frames = d.decode(&bytes).unwrap();
        frames.extend(d.flush());
        assert_eq!(frames.len(), 2);
        assert_eq!(d.stats().vops, 2);
        assert_eq!(d.stats().concealed_vops, 0, "{:?}", d.last_error());
        assert_eq!(d.stats().misaligned_vops, 0);
        let (i, p) = (&frames[0], &frames[1]);
        assert_eq!((i.width, i.height, i.time_base), (128, 96, 30000));
        assert_eq!(p.timestamp - i.timestamp, 1001);
        let y = |f: &Frame, x: usize, r: usize| f.plane(0)[r * 128 + x];
        for gob in 0..6usize {
            for x in 0..8usize {
                let mb = (gob * 8 + x) as u8;
                // Block 0's DC, alone: 8 * dc / 8.
                assert_eq!(y(i, x * 16 + 2, gob * 16 + 2), 16 + mb * 3);
                assert_eq!(y(i, x * 16 + 13, gob * 16 + 13), 16 + mb * 3 + 3);
            }
        }
        // Skipped macroblocks copy; the moved one samples one to the right.
        assert_eq!(y(p, 40, 40), y(i, 40, 40));
        assert_eq!(y(p, 16 + 7, 16 + 3), y(i, 16 + 8, 16 + 3));
        assert_eq!(y(p, 16 + 15, 16), y(i, 32, 16));
        assert!(i.plane(1).iter().all(|&v| v == 128));
    }
}
