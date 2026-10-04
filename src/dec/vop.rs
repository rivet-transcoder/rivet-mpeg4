//! VOP data: video packets, the macroblock layer of I-, P- and B-VOPs
//! (6.2.6 / 6.3.6), data partitioning, the short video header's GOB layer,
//! and reconstruction (clause 7.4 texture, 7.6 motion compensation).

use crate::bits::BitReader;
use crate::error::{Error, Result, invalid};
use crate::frame::VopType;
use crate::headers::{VolHeader, VopHeader};
use crate::idct::idct;
use crate::mbstate::{Dir, MbKind, MbState, ac_pred_value, round_div};
use crate::mc::{self, average, chroma_mv_1, chroma_mv_4, luma_to_halfpel};
use crate::picture::Pic;
use crate::quant::Quant;
use crate::tables::{ALT_HORIZONTAL, ALT_VERTICAL, MB_STUFFING, ZIGZAG, dc_scaler};
use crate::vlc;

use super::texture::{read_coeffs, read_coeffs_rvlc, read_dc_diff, read_rvlc_event_back};
use crate::bits::BackReader;

/// What a B-VOP needs of its backward reference: how each macroblock was
/// coded and its vectors (direct mode, 7.6.9.5; skipping, 6.3.6.2).
#[derive(Clone)]
pub(crate) struct Motion {
    pub mbw: usize,
    pub kind: Vec<MbKind>,
    pub mv: Vec<[i16; 2]>,
    /// Interlaced: field-predicted macroblocks.
    pub field: Vec<bool>,
    /// Interlaced: their field vectors and reference fields.
    pub field_mv: Vec<[[i16; 2]; 2]>,
    pub field_ref: Vec<[bool; 2]>,
}

impl Motion {
    /// The motion of an I-VOP: everything intra.
    pub fn intra(mbw: usize, mbh: usize) -> Motion {
        Motion {
            mbw,
            kind: vec![MbKind::Intra; mbw * mbh],
            mv: vec![[0, 0]; 4 * mbw * mbh],
            field: vec![false; mbw * mbh],
            field_mv: vec![[[0, 0]; 2]; mbw * mbh],
            field_ref: vec![[false; 2]; mbw * mbh],
        }
    }

    /// The motion a P- or S-VOP left in `st`.
    pub fn of(st: &MbState) -> Motion {
        Motion {
            mbw: st.mbw,
            kind: st.kind.clone(),
            mv: st.mv.clone(),
            field: st.field.clone(),
            field_mv: st.field_mv.clone(),
            field_ref: st.field_ref.clone(),
        }
    }
}

/// A macroblock's prediction, before the residual.
pub(crate) struct MbPix {
    pub y: [u8; 256],
    pub cb: [u8; 64],
    pub cr: [u8; 64],
}

impl MbPix {
    pub fn new() -> MbPix {
        MbPix {
            y: [0; 256],
            cb: [0; 64],
            cr: [0; 64],
        }
    }

    fn average(&mut self, o: &MbPix) {
        average(&mut self.y, &o.y);
        average(&mut self.cb, &o.cb);
        average(&mut self.cr, &o.cr);
    }
}

/// Motion-compensated prediction of macroblock `(mbx, mby)` from `src`:
/// one vector (`mvs[0]`, a 16x16 luminance block) or four (8x8 each), in
/// half or quarter samples; the chrominance vector derived per 7.6.2.
#[allow(clippy::too_many_arguments)]
pub(crate) fn predict_mb(
    src: &Pic,
    mbx: usize,
    mby: usize,
    mvs: &[[i32; 2]; 4],
    four: bool,
    rounding: bool,
    qpel: bool,
    out: &mut MbPix,
) {
    let (x, y) = (mbx as i32 * 16, mby as i32 * 16);
    let luma = src.ref_plane(0);
    if !four {
        let [mx, my] = mvs[0];
        if qpel {
            mc::qpel(luma, x, y, mx, my, 16, 16, rounding, &mut out.y, 16);
        } else {
            mc::halfpel(luma, x, y, mx, my, 16, 16, rounding, &mut out.y, 16);
        }
    } else {
        for (k, &[mx, my]) in mvs.iter().enumerate() {
            let (bx, by) = ((k & 1) as i32 * 8, (k >> 1) as i32 * 8);
            let o = &mut out.y[(by as usize) * 16 + bx as usize..];
            if qpel {
                mc::qpel(luma, x + bx, y + by, mx, my, 8, 8, rounding, o, 16);
            } else {
                mc::halfpel(luma, x + bx, y + by, mx, my, 8, 8, rounding, o, 16);
            }
        }
    }
    let (cx, cy) = if four {
        let sx: i32 = mvs.iter().map(|v| luma_to_halfpel(v[0], qpel)).sum();
        let sy: i32 = mvs.iter().map(|v| luma_to_halfpel(v[1], qpel)).sum();
        (chroma_mv_4(sx), chroma_mv_4(sy))
    } else {
        (
            chroma_mv_1(luma_to_halfpel(mvs[0][0], qpel)),
            chroma_mv_1(luma_to_halfpel(mvs[0][1], qpel)),
        )
    };
    let (px, py) = (mbx as i32 * 8, mby as i32 * 8);
    mc::halfpel(
        src.ref_plane(1),
        px,
        py,
        cx,
        cy,
        8,
        8,
        rounding,
        &mut out.cb,
        8,
    );
    mc::halfpel(
        src.ref_plane(2),
        px,
        py,
        cx,
        cy,
        8,
        8,
        rounding,
        &mut out.cr,
        8,
    );
}

/// Field-based motion-compensated prediction of macroblock `(mbx, mby)`
/// (7.6.2, interlaced): each field of the macroblock (16x8 luminance, 8x4
/// chrominance) from the field of `src` that `refs` selects (false top,
/// true bottom) with its own vector, whose vertical component counts field
/// lines.
#[allow(clippy::too_many_arguments)]
pub(crate) fn predict_fields(
    src: &Pic,
    mbx: usize,
    mby: usize,
    mvs: &[[i32; 2]; 2],
    refs: [bool; 2],
    rounding: bool,
    qpel: bool,
    out: &mut MbPix,
) {
    for f in 0..2 {
        let parity = refs[f] as usize;
        let [mx, my] = mvs[f];
        let field = mc::Field {
            frame: src.ref_plane(0),
            parity,
        };
        let (x, y) = (mbx as i32 * 16, mby as i32 * 8);
        let o = &mut out.y[f * 16..];
        if qpel {
            mc::qpel(field, x, y, mx, my, 16, 8, rounding, o, 32);
        } else {
            mc::halfpel(field, x, y, mx, my, 16, 8, rounding, o, 32);
        }
        let cx = chroma_mv_1(luma_to_halfpel(mx, qpel));
        let cy = chroma_mv_1(luma_to_halfpel(my, qpel));
        for (plane, dst) in [(1, &mut out.cb), (2, &mut out.cr)] {
            let field = mc::Field {
                frame: src.ref_plane(plane),
                parity,
            };
            mc::halfpel(
                field,
                mbx as i32 * 8,
                mby as i32 * 4,
                cx,
                cy,
                8,
                4,
                rounding,
                &mut dst[f * 8..],
                16,
            );
        }
    }
}

/// Copies a prediction into the picture.
pub(crate) fn write_mb(cur: &mut Pic, mbx: usize, mby: usize, px: &MbPix) {
    let ys = cur.ystride();
    let cs = cur.cstride();
    for r in 0..16 {
        let o = (mby * 16 + r) * ys + mbx * 16;
        cur.y[o..o + 16].copy_from_slice(&px.y[r * 16..r * 16 + 16]);
    }
    for r in 0..8 {
        let o = (mby * 8 + r) * cs + mbx * 8;
        cur.cb[o..o + 8].copy_from_slice(&px.cb[r * 8..r * 8 + 8]);
        cur.cr[o..o + 8].copy_from_slice(&px.cr[r * 8..r * 8 + 8]);
    }
}

/// The plane, stride and top-left offset of block `k` of a macroblock;
/// with `field_dct`, luminance blocks 0 and 1 are the top field's lines
/// and 2 and 3 the bottom field's (6.3.6.5, `dct_type`).
#[inline]
pub(crate) fn block_pos(
    cur: &mut Pic,
    mbx: usize,
    mby: usize,
    k: usize,
    field_dct: bool,
) -> (&mut [u8], usize, usize) {
    match k {
        0..=3 if field_dct => {
            let s = cur.ystride();
            let o = (mby * 16 + (k >> 1)) * s + mbx * 16 + (k & 1) * 8;
            (&mut cur.y, 2 * s, o)
        }
        0..=3 => {
            let s = cur.ystride();
            let o = (mby * 16 + (k >> 1) * 8) * s + mbx * 16 + (k & 1) * 8;
            (&mut cur.y, s, o)
        }
        4 => {
            let s = cur.cstride();
            (&mut cur.cb, s, mby * 8 * s + mbx * 8)
        }
        _ => {
            let s = cur.cstride();
            (&mut cur.cr, s, mby * 8 * s + mbx * 8)
        }
    }
}

/// Writes an intra block, clipped to 0..=255.
pub(crate) fn put_block(
    cur: &mut Pic,
    mbx: usize,
    mby: usize,
    k: usize,
    blk: &[i16; 64],
    field_dct: bool,
) {
    let (p, s, o) = block_pos(cur, mbx, mby, k, field_dct);
    for (r, b) in blk.as_chunks::<8>().0.iter().enumerate() {
        let row = &mut p[o + r * s..o + r * s + 8];
        for (d, &v) in row.iter_mut().zip(b) {
            *d = v.clamp(0, 255) as u8;
        }
    }
}

/// Adds a residual block to the prediction already in the picture.
pub(crate) fn add_block(
    cur: &mut Pic,
    mbx: usize,
    mby: usize,
    k: usize,
    blk: &[i16; 64],
    field_dct: bool,
) {
    let (p, s, o) = block_pos(cur, mbx, mby, k, field_dct);
    for (r, b) in blk.as_chunks::<8>().0.iter().enumerate() {
        let row = &mut p[o + r * s..o + r * s + 8];
        for (d, &v) in row.iter_mut().zip(b) {
            *d = (*d as i16 + v.clamp(-512, 512)).clamp(0, 255) as u8;
        }
    }
}

/// `intra_dc_vlc_thr` (Table 6-21): whether the intra DC has its own VLC
/// at this running quantiser.
#[inline]
pub(crate) fn use_intra_dc_vlc(thr: u32, running_qp: u32) -> bool {
    thr == 0 || (thr < 7 && running_qp < 11 + 2 * thr)
}

/// `dquant` (Table 6-22).
const DQUANT: [i32; 4] = [-1, -2, 1, 2];

/// Wraps a decoded vector component into the range `vop_fcode` allows
/// (7.6.3).
#[inline]
pub(crate) fn wrap_mv(v: i32, fcode: u32) -> i32 {
    let f = 1i32 << (fcode - 1);
    let (low, high, range) = (-32 * f, 32 * f - 1, 64 * f);
    if v < low {
        v + range
    } else if v > high {
        v - range
    } else {
        v
    }
}

/// A motion vector difference component: `motion_code` and, when
/// `fcode > 1`, `motion_residual`.
pub(crate) fn read_mvd(r: &mut BitReader, fcode: u32) -> Result<i32> {
    let m = vlc::mvd().decode(r)? as i32;
    if m == 0 {
        return Ok(0);
    }
    let neg = r.read_bit()?;
    let rs = fcode - 1;
    let v = if rs == 0 {
        m
    } else {
        ((m - 1) << rs) + r.read(rs)? as i32 + 1
    };
    Ok(if neg { -v } else { v })
}

fn read_mv(r: &mut BitReader, pred: [i32; 2], fcode: u32) -> Result<[i32; 2]> {
    let x = wrap_mv(pred[0] + read_mvd(r, fcode)?, fcode);
    let y = wrap_mv(pred[1] + read_mvd(r, fcode)?, fcode);
    Ok([x, y])
}

/// A macroblock's syntax, parsed ahead of its texture.
#[derive(Clone, Copy, Default)]
struct MbHdr {
    kind: MbKind,
    mb_type: u8,
    cbp: u8,
    ac_pred: bool,
    qp: u32,
    four: bool,
    mvs: [[i32; 2]; 4],
    use_dc_vlc: bool,
    /// DC differentials read in an earlier partition (data partitioning).
    dc: Option<[i32; 6]>,
    /// Predicted by global motion compensation (`mcsel`, or not coded in
    /// an S-VOP).
    gmc: bool,
    /// Interlaced: `dct_type` (field DCT).
    field_dct: bool,
    /// Interlaced: `field_prediction`, the reference field of each of the
    /// macroblock's fields (true: bottom) and their vectors.
    field_pred: bool,
    field_ref: [bool; 2],
    field_mvs: [[i32; 2]; 2],
}

/// A macroblock's six blocks of coefficients in scan order (index `i` is
/// the `i`-th coefficient of the block's scan), as the reversible-VLC
/// path reads them ahead of reconstruction.
pub(crate) type Coefs = [[i16; 64]; 6];

/// Places scan-order coefficients into a raster block.
#[inline]
fn unscan(c: &[i16; 64], scan: &[u8; 64], blk: &mut [i16; 64]) {
    for (i, &v) in c.iter().enumerate() {
        if v != 0 {
            blk[scan[i] as usize] = v;
        }
    }
}

/// B-VOP macroblock types (Table 6-26).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BType {
    Direct,
    Interpolate,
    Backward,
    Forward,
}

pub(crate) const DC_MARKER: u32 = 0b110_1011_0000_0000_0001;
pub(crate) const MOTION_MARKER: u32 = 0b1_1111_0000_0000_0001;

/// Decodes the data of one VOP into `cur`.
pub(crate) struct VopDec<'a> {
    pub vol: &'a VolHeader,
    pub hdr: &'a VopHeader,
    pub quant: &'a Quant,
    /// The short video header (H.263 baseline syntax).
    pub sh: bool,
    pub cur: &'a mut Pic,
    /// P: the reference. B: the past reference.
    pub fwd: Option<&'a Pic>,
    /// B: the future reference.
    pub bwd: Option<&'a Pic>,
    /// B: the future reference's motion.
    pub col: Option<&'a Motion>,
    /// S-VOPs: the global motion.
    pub gmc: Option<&'a crate::gmc::Gmc>,
    pub st: &'a mut MbState,
    pub slice_counter: &'a mut u32,
    /// B: temporal distances (7.6.9.5), in ticks.
    pub trb: i32,
    pub trd: i32,
    pub slice: u32,
    pub qp: u32,
    pub first_coded: bool,
    /// B-VOP vector predictors, `[forward, backward][top field, bottom
    /// field]` in frame units: a frame vector sets both of its direction's,
    /// a field-predicted macroblock each field's (7.7.2.2), and frame
    /// vectors are predicted from the top field's.
    pub pmv: [[[i32; 2]; 2]; 2],
    /// Set when part of the VOP was concealed; the first error is kept.
    pub error: Option<Error>,
    /// Whether the macroblock data ended exactly where the VOP's stuffing
    /// begins (set by [`Self::run`] when nothing was concealed).
    pub tail_ok: bool,
    /// Interlaced: read `dct_type` for every coded P/S-VOP macroblock, not
    /// only for intra ones and those with coded blocks (early Xvid).
    pub dct_type_always: bool,
    /// Field direct mode as Xvid and libavcodec code and decode it: the
    /// co-located macroblock's field vectors taken as zero, so each field
    /// is predicted with `MVD` forward and backward from the reference
    /// fields 7.7.2.2 names. 7.7.2.2 scales the field vectors by the field
    /// distances (the default); both encoders write their streams for
    /// their own reading (docs/CONFORMANCE.md).
    pub zero_field_direct: bool,
    /// B-VOPs: the display times (ticks) of the past reference, this VOP
    /// and the future reference, and the frame period `Tframe` (7.7.2.3),
    /// for field direct mode.
    pub field_times: [i64; 4],
    /// Reversible VLCs: macroblocks whose texture was recovered by
    /// decoding backwards, and macroblocks whose texture was discarded.
    pub rvlc_backward_mbs: u64,
    pub rvlc_discarded_mbs: u64,
    /// B-VOP macroblocks predicted in field direct mode.
    pub field_direct_mbs: u64,
    /// B-VOP macroblocks with field prediction.
    pub field_b_mbs: u64,
    /// Overlapped block motion compensation in this P- or S-VOP
    /// (`obmc_disable` 0, or H.263's Advanced Prediction mode).
    pub obmc: bool,
    /// OBMC: the inter macroblocks whose prediction waits for their right
    /// neighbour's vectors, with their residual.
    pub obmc_pending: Vec<Option<Box<ObmcMb>>>,
    /// S-VOPs: which macroblocks GMC predicts (OBMC does not cross the
    /// boundary between those and the others).
    pub mcsel: Vec<bool>,
}

/// An inter macroblock waiting for its overlapped prediction.
pub(crate) struct ObmcMb {
    /// The residual of each block, after the IDCT (zero where not coded).
    res: [[i16; 64]; 6],
    cbp: u8,
    field_dct: bool,
    four: bool,
}

impl VopDec<'_> {
    fn total(&self) -> usize {
        self.st.mbw * self.st.mbh
    }

    fn new_packet(&mut self, qp: u32) {
        *self.slice_counter = self.slice_counter.wrapping_add(1).max(1);
        self.slice = *self.slice_counter;
        self.qp = qp;
        self.first_coded = true;
        self.pmv = [[[0, 0]; 2]; 2];
    }

    fn mb_xy(&self, mb: usize) -> (usize, usize) {
        (mb % self.st.mbw, mb / self.st.mbw)
    }

    /// Replaces macroblocks `from..to` with the co-located ones of the
    /// reference (or leaves them grey with none), after an error.
    fn conceal(&mut self, from: usize, to: usize) {
        let src = self.fwd.or(self.bwd);
        let mut px = MbPix::new();
        for mb in from..to.min(self.total()) {
            let (mbx, mby) = self.mb_xy(mb);
            self.st.slice[mb] = 0;
            self.st.kind[mb] = MbKind::Skipped;
            self.st.set_mb_mv(mbx, mby, [0, 0]);
            if let Some(p) = self.obmc_pending.get_mut(mb) {
                *p = None;
            }
            if let Some(src) = src {
                predict_mb(src, mbx, mby, &[[0, 0]; 4], false, false, false, &mut px);
                write_mb(self.cur, mbx, mby, &px);
            }
        }
    }

    fn record(&mut self, e: Error) {
        if self.error.is_none() {
            self.error = Some(e);
        }
    }

    fn resync_enabled(&self) -> bool {
        !self.sh && !self.vol.resync_marker_disable
    }

    /// The resync marker's length for this VOP (6.3.5.2).
    fn resync_len(&self) -> usize {
        match self.hdr.vop_type {
            VopType::I => 17,
            VopType::B => 16 + self.hdr.fcode_forward.max(self.hdr.fcode_backward) as usize,
            _ => 16 + self.hdr.fcode_forward as usize,
        }
    }

    /// The length of the resync marker at the reader, if one is there.
    ///
    /// A B-VOP whose larger `vop_fcode` is 1 has a 17-bit marker by
    /// 6.3.5.2, but Xvid writes 18 bits there (as though the `vop_fcode`
    /// were at least 2). A one after the 17th zero-or-one position can
    /// only be a marker, so both are accepted.
    fn marker_len_at(&self, r: &BitReader) -> Option<usize> {
        let len = self.resync_len();
        if is_marker(r, len) {
            Some(len)
        } else if self.hdr.vop_type == VopType::B && len == 17 && is_marker(r, 18) {
            Some(18)
        } else {
            None
        }
    }

    /// When stuffing then a resync marker follow, the marker's position.
    fn at_resync(&self, r: &BitReader) -> Option<usize> {
        let p = r.stuffing_end()?;
        let mut t = r.clone();
        t.set_pos(p);
        self.marker_len_at(&t).map(|_| p)
    }

    /// `video_packet_header()` from the resync marker on: the number of
    /// the packet's first macroblock.
    fn packet_header(&mut self, r: &mut BitReader) -> Result<usize> {
        let len = self.marker_len_at(r).unwrap_or_else(|| self.resync_len());
        r.skip(len)?;
        let total = self.total();
        let bits = usize::BITS - (total - 1).leading_zeros();
        let mb = r.read(bits.max(1))? as usize;
        let qp = r.read(5)?;
        if qp == 0 {
            return Err(invalid("quant_scale is zero"));
        }
        if r.read_bit()? {
            // header_extension_code: a copy of the VOP header's fields.
            while r.read_bit()? {}
            r.lenient_marker()?;
            r.read(self.vol.time_increment_bits)?;
            r.lenient_marker()?;
            r.read(2)?; // vop_coding_type
            r.read(3)?; // intra_dc_vlc_thr
            if self.hdr.vop_type == VopType::S && self.vol.sprite_warping_points > 0 {
                crate::headers::parse_sprite_trajectory(r, self.vol.sprite_warping_points)?;
            }
            if self.hdr.vop_type != VopType::I {
                r.read(3)?;
            }
            if self.hdr.vop_type == VopType::B {
                r.read(3)?;
            }
        }
        if mb >= total {
            return Err(invalid(format!(
                "video packet starts at macroblock {mb} of {total}"
            )));
        }
        self.new_packet(qp);
        Ok(mb)
    }

    /// Searches forward, byte by byte, for the next resync marker, leaving
    /// the reader on it.
    fn find_resync(&self, r: &mut BitReader) -> bool {
        let len = self.resync_len();
        r.align();
        while r.left() >= len + 8 {
            if self.marker_len_at(r).is_some() {
                return true;
            }
            r.set_pos(r.pos() + 8);
        }
        false
    }

    /// Decodes the whole VOP. Damage is concealed, not returned: the
    /// macroblocks from the failure to the next video packet that can be
    /// found are copied from the reference, and the error is kept in
    /// `self.error`.
    pub fn run(&mut self, r: &mut BitReader) {
        if self.obmc {
            self.obmc_pending.clear();
            self.obmc_pending.resize_with(self.total(), || None);
            self.mcsel.clear();
            self.mcsel.resize(self.total(), false);
        }
        self.run_inner(r);
        if self.obmc {
            self.obmc_finish();
        }
        if self.error.is_none() {
            self.tail_ok = tail_ok(r, self.sh);
        }
    }

    fn run_inner(&mut self, r: &mut BitReader) {
        let total = self.total();
        self.new_packet(self.hdr.quant);
        if self.sh {
            return self.run_short(r);
        }
        let mut mb = 0;
        // Whether the reader sits on a resync marker found after an error.
        let mut on_marker = false;
        while mb < total {
            let step = self.step(r, mb, on_marker);
            on_marker = false;
            match step {
                Ok(n) => mb = n,
                Err(e) => {
                    self.record(e);
                    loop {
                        if !(self.resync_enabled() && self.find_resync(r)) {
                            self.conceal(mb, total);
                            return;
                        }
                        match self.peek_packet_mb(r) {
                            Some(n) if n > mb && n < total => {
                                self.conceal(mb, n);
                                mb = n;
                                on_marker = true;
                                break;
                            }
                            _ => r.set_pos(r.pos() + 8),
                        }
                    }
                }
            }
        }
    }

    /// The `macroblock_number` of the video packet header at the reader.
    fn peek_packet_mb(&self, r: &BitReader) -> Option<usize> {
        let bits = (usize::BITS - (self.total() - 1).leading_zeros()).max(1);
        let len = self.marker_len_at(r).unwrap_or_else(|| self.resync_len());
        (r.left() >= len + bits as usize).then(|| r.peek_at(len, bits) as usize)
    }

    /// A video packet header if one is due, then one macroblock (or, data
    /// partitioned, one packet): returns the next macroblock number.
    fn step(&mut self, r: &mut BitReader, mut mb: usize, on_marker: bool) -> Result<usize> {
        if self.resync_enabled() {
            let marker = if on_marker {
                Some(r.pos())
            } else if mb > 0 {
                self.at_resync(r)
            } else {
                None
            };
            if let Some(p) = marker {
                r.set_pos(p);
                let next = self.packet_header(r)?;
                if next < mb {
                    return Err(invalid("video packets out of order"));
                }
                if next > mb {
                    if self.uncoded_b_run(mb, next) {
                        // A B-VOP macroblock whose co-located macroblock
                        // was not coded has no bits, so a packet that
                        // starts after a run of them may have its marker
                        // before them.
                        for m in mb..next {
                            self.mb(r, m)?;
                        }
                    } else {
                        self.record(invalid("macroblocks missing between video packets"));
                        self.conceal(mb, next);
                    }
                }
                mb = next;
            }
        }
        if self.vol.data_partitioned && self.hdr.vop_type != VopType::B {
            self.dp_packet(r, mb)
        } else if self.mb(r, mb)? {
            Ok(mb + 1)
        } else {
            Ok(mb)
        }
    }

    /// Whether macroblocks `from..to` are all B-VOP macroblocks whose
    /// co-located macroblock was not coded: macroblocks without any bits.
    fn uncoded_b_run(&self, from: usize, to: usize) -> bool {
        self.hdr.vop_type == VopType::B
            && self
                .col
                .is_some_and(|c| (from..to).all(|m| c.kind.get(m) == Some(&MbKind::Skipped)))
    }

    /// One macroblock, not data partitioned.
    /// Returns false when what was read was macroblock stuffing.
    fn mb(&mut self, r: &mut BitReader, mb: usize) -> Result<bool> {
        let (mbx, mby) = self.mb_xy(mb);
        if self.hdr.vop_type == VopType::B {
            if mbx == 0 {
                self.pmv = [[[0, 0]; 2]; 2];
            }
            self.mb_b(r, mbx, mby)?;
            return Ok(true);
        }
        let Some(h) = self.mb_header(r, mbx, mby, false)? else {
            return Ok(false);
        };
        self.mb_texture(r, mbx, mby, &h, None)?;
        Ok(true)
    }

    /// The header of an I-, P- or S-VOP macroblock (`partition`: the first
    /// partition of a data-partitioned VOP, where only the syntax up to the
    /// vectors is read; the rest comes from the second partition). `None`:
    /// a stuffing code was read instead, and the caller looks again for a
    /// marker before the macroblock.
    fn mb_header(
        &mut self,
        r: &mut BitReader,
        mbx: usize,
        mby: usize,
        partition: bool,
    ) -> Result<Option<MbHdr>> {
        let mb = mby * self.st.mbw + mbx;
        let p = self.hdr.vop_type != VopType::I;
        self.st.slice[mb] = self.slice;
        if p && r.read_bit()? {
            // not_coded
            if let Some(g) = self.gmc {
                // In an S-VOP, a GMC macroblock without texture.
                if let Some(m) = self.mcsel.get_mut(mb) {
                    *m = true;
                }
                let v = self.gmc_vector(g, mbx, mby);
                self.st.kind[mb] = MbKind::Inter;
                self.st.field[mb] = false;
                self.st.qp[mb] = self.qp as u8;
                self.st.set_mb_mv(mbx, mby, v);
                return Ok(Some(MbHdr {
                    kind: MbKind::Inter,
                    qp: self.qp,
                    gmc: true,
                    mvs: [v; 4],
                    ..Default::default()
                }));
            }
            self.st.kind[mb] = MbKind::Skipped;
            self.st.qp[mb] = self.qp as u8;
            self.st.field[mb] = false;
            self.st.set_mb_mv(mbx, mby, [0, 0]);
            return Ok(Some(MbHdr {
                kind: MbKind::Skipped,
                qp: self.qp,
                ..Default::default()
            }));
        }
        let v = if p { vlc::mcbpc_p() } else { vlc::mcbpc_i() }.decode(r)?;
        let mb_type = (v >> 2) as u8;
        if mb_type == MB_STUFFING {
            return Ok(None);
        }
        let intra = mb_type >= 3;
        if self.sh && mb_type == 2 && !self.obmc {
            return Err(invalid(
                "INTER4V in a short-header picture without Advanced Prediction",
            ));
        }
        let mcsel = self.gmc.is_some() && mb_type < 2 && r.read_bit()?;
        if let Some(m) = self.mcsel.get_mut(mb) {
            *m = mcsel;
        }
        let mut h = MbHdr {
            kind: if intra { MbKind::Intra } else { MbKind::Inter },
            mb_type,
            cbp: (v & 3) as u8,
            four: mb_type == 2,
            gmc: mcsel,
            ..Default::default()
        };
        self.st.kind[mb] = h.kind;
        let i_dp = partition && !p;
        if !partition || i_dp {
            if !partition {
                h.ac_pred = intra && !self.sh && r.read_bit()?;
                let cbpy = vlc::cbpy().decode(r)? as u8;
                h.cbp |= (if intra { cbpy } else { 15 - cbpy }) << 2;
            }
            self.dquant_and_dc_mode(r, &mut h)?;
            if i_dp && h.use_dc_vlc {
                h.dc = Some(read_dcs(r)?);
            }
        }
        if self.vol.interlaced {
            // interlaced_information()
            if intra || h.cbp != 0 || self.dct_type_always {
                h.field_dct = r.read_bit()?;
            }
            if !intra && mb_type < 2 && !mcsel {
                h.field_pred = r.read_bit()?;
                if h.field_pred {
                    h.field_ref = [r.read_bit()?, r.read_bit()?];
                }
            }
        }
        self.st.field[mb] = h.field_pred;
        if h.field_pred {
            self.st.field_ref[mb] = h.field_ref;
        }
        self.st.qp[mb] = self.qp as u8;
        if intra {
            self.st.set_mb_mv(mbx, mby, [0, 0]);
        } else if let Some(g) = self.gmc.filter(|_| mcsel) {
            let v = self.gmc_vector(g, mbx, mby);
            self.st.set_mb_mv(mbx, mby, v);
            h.mvs = [v; 4];
        } else if h.field_pred {
            let pred = self.st.mv_pred(mbx, mby, 0, self.slice);
            let mvs = read_field_mvs(r, [pred; 2], self.hdr.fcode_forward)?;
            let frame = field_to_frame(mvs[0], mvs[1]);
            h.field_mvs = mvs;
            self.st.field_mv[mb] = mvs.map(|v| [v[0] as i16, v[1] as i16]);
            self.st.set_mb_mv(mbx, mby, frame);
            h.mvs = [frame; 4];
        } else if h.four {
            for k in 0..4 {
                let pred = self.st.mv_pred(mbx, mby, k, self.slice);
                let mv = read_mv(r, pred, self.hdr.fcode_forward)?;
                self.st.set_mv(mbx, mby, k, mv);
                h.mvs[k] = mv;
            }
        } else {
            let pred = self.st.mv_pred(mbx, mby, 0, self.slice);
            let mv = read_mv(r, pred, self.hdr.fcode_forward)?;
            self.st.set_mb_mv(mbx, mby, mv);
            h.mvs = [mv; 4];
        }
        Ok(Some(h))
    }

    /// The vector a GMC macroblock stands for in vector prediction (and as
    /// a co-located vector in direct mode): the mean warp displacement,
    /// clipped to the range `vop_fcode_forward` gives vectors. Unclipped,
    /// a zoom whose warp outruns that range makes its neighbours' vectors
    /// wrap — Xvid's GMC streams show it as misplaced blocks.
    fn gmc_vector(&self, g: &crate::gmc::Gmc, mbx: usize, mby: usize) -> [i32; 2] {
        let v = g.mb_vector(mbx, mby);
        let f = 1i32 << (self.hdr.fcode_forward - 1);
        [
            v[0].clamp(-32 * f, 32 * f - 1),
            v[1].clamp(-32 * f, 32 * f - 1),
        ]
    }

    /// `dquant` when the type has one, then whether the intra DC uses its
    /// own VLC at the running quantiser.
    fn dquant_and_dc_mode(&mut self, r: &mut BitReader, h: &mut MbHdr) -> Result<()> {
        let old = self.qp;
        if h.mb_type == 1 || h.mb_type == 4 {
            self.qp = (self.qp as i32 + DQUANT[r.read(2)? as usize]).clamp(1, 31) as u32;
        }
        let running = if self.first_coded { self.qp } else { old };
        self.first_coded = false;
        h.qp = self.qp;
        h.use_dc_vlc = !self.sh && use_intra_dc_vlc(self.hdr.intra_dc_vlc_thr, running);
        Ok(())
    }

    /// A data-partitioned video packet from macroblock `first`: returns
    /// the macroblock after its last.
    fn dp_packet(&mut self, r: &mut BitReader, first: usize) -> Result<usize> {
        let total = self.total();
        let i_vop = self.hdr.vop_type == VopType::I;
        let mut hdrs: Vec<(usize, MbHdr)> = Vec::new();
        let mut mb = first;
        loop {
            if i_vop && r.peek(19) == DC_MARKER {
                r.skip(19)?;
                break;
            }
            if !i_vop && r.peek(17) == MOTION_MARKER {
                r.skip(17)?;
                break;
            }
            if mb >= total {
                return Err(invalid(
                    "a video packet's first partition runs past the last macroblock",
                ));
            }
            let (mbx, mby) = self.mb_xy(mb);
            let Some(h) = self.mb_header(r, mbx, mby, true)? else {
                continue;
            };
            hdrs.push((mb, h));
            mb += 1;
        }
        // Second partition.
        for (mb, h) in hdrs.iter_mut() {
            if i_vop {
                h.ac_pred = r.read_bit()?;
                h.cbp |= (vlc::cbpy().decode(r)? as u8) << 2;
                continue;
            }
            if h.kind == MbKind::Skipped {
                continue;
            }
            let intra = h.kind == MbKind::Intra;
            if intra {
                h.ac_pred = r.read_bit()?;
            }
            let cbpy = vlc::cbpy().decode(r)? as u8;
            h.cbp |= (if intra { cbpy } else { 15 - cbpy }) << 2;
            self.dquant_and_dc_mode(r, h)?;
            self.st.qp[*mb] = h.qp as u8;
            if intra && h.use_dc_vlc {
                h.dc = Some(read_dcs(r)?);
            }
        }
        // Third partition.
        if self.vol.reversible_vlc {
            self.rvlc_texture(r, &hdrs);
            return Ok(mb);
        }
        for (mb, h) in &hdrs {
            let (mbx, mby) = self.mb_xy(*mb);
            self.mb_texture(r, mbx, mby, h, None)?;
        }
        Ok(mb)
    }

    /// The texture partition of a data-partitioned packet coded with
    /// reversible VLCs (Annex E.1.4.4). It is read forwards first; when
    /// that fails — an illegal code, an illegal escape, more than 64
    /// coefficients, or a partition that does not end at the stuffing
    /// before the next resync marker (or the VOP's end) — it is read again
    /// backwards from that end, and the macroblocks each direction
    /// recovered are kept as the strategies of E.1.4.4.2.1 decide. The
    /// rest are concealed: inter macroblocks with their (intact) motion and
    /// no residual; intra macroblocks of an I-VOP from their DC (in the
    /// first partition when `intra_dc_vlc` codes it) with no AC; and, as
    /// E.1.4.4.2.2 says, every intra macroblock of a damaged packet in a
    /// P- or S-VOP is copied from the reference rather than shown.
    fn rvlc_texture(&mut self, r: &mut BitReader, hdrs: &[(usize, MbHdr)]) {
        let n = hdrs.len();
        let t0 = r.pos();
        // (macroblock in the packet, block, first coefficient, intra table)
        let mut blocks: Vec<(usize, usize, usize, bool)> = Vec::new();
        for (j, (_, h)) in hdrs.iter().enumerate() {
            for k in 0..6 {
                if h.kind == MbKind::Skipped || h.cbp >> (5 - k) & 1 == 0 {
                    continue;
                }
                let intra = h.kind == MbKind::Intra;
                blocks.push((j, k, (intra && h.use_dc_vlc) as usize, intra));
            }
        }
        let has_bits = |j: usize| blocks.iter().any(|b| b.0 == j);
        let mut fwd = vec![[[0i16; 64]; 6]; n];
        let mut fstart = Vec::with_capacity(n);
        let mut n1 = 0;
        let mut err = None;
        let mut bi = 0;
        'fwd: for (j, c) in fwd.iter_mut().enumerate() {
            fstart.push(r.pos() - t0);
            while bi < blocks.len() && blocks[bi].0 == j {
                let (_, k, start, intra) = blocks[bi];
                if let Err(e) = read_coeffs_rvlc(r, &mut c[k], &IDENTITY_SCAN, start, intra) {
                    err = Some(e);
                    break 'fwd;
                }
                bi += 1;
            }
            n1 = j + 1;
        }
        let fend = r.pos();
        let end = self.texture_end(r, t0);
        if err.is_none() {
            match end {
                Some(e) if e != fend => {
                    err = Some(invalid(
                        "a reversible-VLC texture partition does not end at the stuffing",
                    ));
                }
                _ => {
                    for (j, (mb, h)) in hdrs.iter().enumerate() {
                        let (mbx, mby) = self.mb_xy(*mb);
                        // Cannot fail: the coefficients are all read.
                        let _ = self.mb_texture(r, mbx, mby, h, Some(&fwd[j]));
                    }
                    return;
                }
            }
        }
        // Two-way decoding.
        let (l, l1) = match end {
            Some(e) => (e - t0, fend.min(e) - t0),
            None => (fend - t0 + 1, fend - t0),
        };
        let mut bwd = vec![[[0i16; 64]; 6]; n];
        let mut bend = vec![None; n];
        let mut n2 = 0;
        let mut l2 = 0;
        if let Some(e) = end {
            let mut br = BackReader::new(r.data(), e, t0);
            let mut bi = blocks.len();
            let mut ok = true;
            'bwd: for j in (0..n).rev() {
                bend[j] = Some(e - br.pos());
                while bi > 0 && blocks[bi - 1].0 == j {
                    let (_, k, start, intra) = blocks[bi - 1];
                    match read_block_back(&mut br, start, intra, bi == 1) {
                        Ok(c) => bwd[j][k] = c,
                        Err(_) => {
                            ok = false;
                            break 'bwd;
                        }
                    }
                    bi -= 1;
                }
                n2 += 1;
            }
            if ok && br.left() != 0 {
                n2 = n2.min(n.saturating_sub(1));
            }
            l2 = e - br.pos();
        }
        let f_mb = |sb: i64| -> usize {
            fstart
                .iter()
                .enumerate()
                .filter(|&(j, &st)| (st as i64) < sb || (!has_bits(j) && st as i64 <= sb))
                .count()
        };
        let b_mb = |sb: i64| -> usize {
            bend.iter()
                .enumerate()
                .filter(|&(j, b)| {
                    b.is_some_and(|b| (b as i64) < sb || (!has_bits(j) && b as i64 <= sb))
                })
                .count()
        };
        let (front, back) = rvlc_strategy(l, n, l1, l2, n1, n2, f_mb, b_mb);
        let front = front.min(n1);
        let back = back.min(n2).min(n - front);
        let p_vop = self.hdr.vop_type != VopType::I;
        let zeros = [[0i16; 64]; 6];
        for (j, (mb, h)) in hdrs.iter().enumerate() {
            let (mbx, mby) = self.mb_xy(*mb);
            let coefs = if j < front {
                Some(&fwd[j])
            } else if j >= n - back {
                Some(&bwd[j])
            } else {
                None
            };
            if p_vop && h.kind == MbKind::Intra {
                self.rvlc_discarded_mbs += 1;
                self.copy_from_reference(mbx, mby);
                continue;
            }
            if j >= n - back && j >= front {
                self.rvlc_backward_mbs += has_bits(j) as u64;
            }
            let _ = match (coefs, h.kind) {
                (Some(c), _) => self.mb_texture(r, mbx, mby, h, Some(c)),
                (None, MbKind::Inter) => {
                    self.rvlc_discarded_mbs += has_bits(j) as u64;
                    let h = MbHdr { cbp: 0, ..*h };
                    self.mb_texture(r, mbx, mby, &h, Some(&zeros))
                }
                (None, MbKind::Intra) => {
                    self.rvlc_discarded_mbs += 1;
                    self.mb_texture(r, mbx, mby, h, Some(&zeros))
                }
                (None, MbKind::Skipped) => self.mb_texture(r, mbx, mby, h, None),
            };
        }
        self.record(err.unwrap_or_else(|| invalid("reversible-VLC texture damaged")));
        r.set_pos(end.unwrap_or(fend));
    }

    /// The co-located macroblock of the reference, pixels only (the
    /// macroblock's syntax state, which later VOPs read, is kept).
    fn copy_from_reference(&mut self, mbx: usize, mby: usize) {
        if let Some(src) = self.fwd {
            let mut px = MbPix::new();
            predict_mb(src, mbx, mby, &[[0, 0]; 4], false, false, false, &mut px);
            write_mb(self.cur, mbx, mby, &px);
        }
    }

    /// Where the texture partition that starts at `t0` ends: before the
    /// stuffing (a zero, then up to seven ones) that precedes the next
    /// resync marker or the end of the VOP's data. `None` when that
    /// stuffing is not there.
    fn texture_end(&self, r: &BitReader, t0: usize) -> Option<usize> {
        let mut t = r.clone();
        t.set_pos(t0);
        let p = if self.resync_enabled() && self.find_resync(&mut t) {
            t.pos()
        } else {
            let d = r.data();
            d.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1) * 8
        };
        let bit = |i: usize| {
            let mut t = r.clone();
            t.set_pos(i);
            t.peek(1)
        };
        let mut q = p;
        while q > t0 && bit(q - 1) == 1 {
            q -= 1;
            if p - q > 7 {
                return None;
            }
        }
        (q > t0 && bit(q - 1) == 0).then_some(q - 1)
    }

    /// Texture and reconstruction of an I-, P- or S-VOP macroblock.
    /// `pre`: the blocks' coefficients already read (scan order), for the
    /// reversible-VLC path, which reads a packet's texture before any of it
    /// is reconstructed.
    fn mb_texture(
        &mut self,
        r: &mut BitReader,
        mbx: usize,
        mby: usize,
        h: &MbHdr,
        pre: Option<&Coefs>,
    ) -> Result<()> {
        match h.kind {
            MbKind::Skipped => {
                let src = self
                    .fwd
                    .ok_or_else(|| invalid("a P-VOP without a reference"))?;
                if self.obmc {
                    self.obmc_pending[mby * self.st.mbw + mbx] = Some(Box::new(ObmcMb {
                        res: [[0; 64]; 6],
                        cbp: 0,
                        field_dct: false,
                        four: false,
                    }));
                    return Ok(());
                }
                let mut px = MbPix::new();
                predict_mb(src, mbx, mby, &[[0, 0]; 4], false, false, false, &mut px);
                write_mb(self.cur, mbx, mby, &px);
                Ok(())
            }
            MbKind::Intra => {
                for k in 0..6 {
                    self.intra_block(r, mbx, mby, k, h, pre.map(|p| &p[k]))?;
                }
                Ok(())
            }
            MbKind::Inter => {
                let src = self
                    .fwd
                    .ok_or_else(|| invalid("a P-VOP without a reference"))?;
                if self.obmc && !h.gmc && !h.field_pred {
                    // The prediction waits for the right neighbour's vectors.
                    let res = self.residual_blocks(r, h.cbp, h.qp, pre)?;
                    self.obmc_pending[mby * self.st.mbw + mbx] = Some(Box::new(ObmcMb {
                        res,
                        cbp: h.cbp,
                        field_dct: h.field_dct,
                        four: h.four,
                    }));
                    return Ok(());
                }
                let mut px = MbPix::new();
                match self.gmc.filter(|_| h.gmc) {
                    Some(g) => g.predict_mb(src, mbx, mby, self.hdr.rounding, &mut px),
                    None if h.field_pred => predict_fields(
                        src,
                        mbx,
                        mby,
                        &h.field_mvs,
                        h.field_ref,
                        self.hdr.rounding,
                        self.vol.quarter_sample,
                        &mut px,
                    ),
                    None => predict_mb(
                        src,
                        mbx,
                        mby,
                        &h.mvs,
                        h.four,
                        self.hdr.rounding,
                        self.vol.quarter_sample,
                        &mut px,
                    ),
                }
                write_mb(self.cur, mbx, mby, &px);
                self.residual(r, mbx, mby, h.cbp, h.qp, h.field_dct, pre)
            }
        }
    }

    /// The coded inter blocks of a macroblock, added to its prediction.
    #[allow(clippy::too_many_arguments)]
    fn residual(
        &mut self,
        r: &mut BitReader,
        mbx: usize,
        mby: usize,
        cbp: u8,
        qp: u32,
        field_dct: bool,
        pre: Option<&Coefs>,
    ) -> Result<()> {
        let res = self.residual_blocks(r, cbp, qp, pre)?;
        for (k, blk) in res.iter().enumerate() {
            if cbp >> (5 - k) & 1 != 0 {
                add_block(self.cur, mbx, mby, k, blk, field_dct);
            }
        }
        Ok(())
    }

    /// The coded inter blocks of a macroblock, read (or taken from `pre`),
    /// dequantised and transformed; zero where not coded.
    fn residual_blocks(
        &mut self,
        r: &mut BitReader,
        cbp: u8,
        qp: u32,
        pre: Option<&Coefs>,
    ) -> Result<[[i16; 64]; 6]> {
        // alternate_vertical_scan_flag puts every block on that scan.
        let scan = if self.hdr.alternate_vertical_scan {
            &ALT_VERTICAL
        } else {
            &ZIGZAG
        };
        let mut out = [[0i16; 64]; 6];
        for (k, blk) in out.iter_mut().enumerate() {
            if cbp >> (5 - k) & 1 == 0 {
                continue;
            }
            match pre {
                Some(p) => unscan(&p[k], scan, blk),
                None => read_coeffs(r, blk, scan, 0, false, self.sh)?,
            }
            self.quant.inter(blk, qp);
            idct(blk);
        }
        Ok(out)
    }

    /// Overlapped prediction of every macroblock left waiting, now that
    /// all the VOP's vectors are known, and its residual added.
    fn obmc_finish(&mut self) {
        let Some(src) = self.fwd else {
            return;
        };
        let mbw = self.st.mbw;
        let pending = std::mem::take(&mut self.obmc_pending);
        for (mb, p) in pending.iter().enumerate() {
            let Some(p) = p else {
                continue;
            };
            let (mbx, mby) = (mb % mbw, mb / mbw);
            let own: [[i32; 2]; 4] = std::array::from_fn(|k| self.st.get_mv(mbx, mby, k));
            let cur_gmc = self.mcsel.get(mb).copied().unwrap_or(false);
            let mcsel = &self.mcsel;
            let remote = crate::obmc::remote_vectors(self.st, mbx, mby, &own, |i| {
                mcsel.get(i).copied().unwrap_or(false) != cur_gmc
            });
            let mut px = MbPix::new();
            // Chrominance as without OBMC.
            predict_mb(
                src,
                mbx,
                mby,
                &own,
                p.four,
                self.hdr.rounding,
                self.vol.quarter_sample,
                &mut px,
            );
            crate::obmc::predict_luma(
                src,
                mbx,
                mby,
                &own,
                &remote,
                self.hdr.rounding,
                self.vol.quarter_sample,
                &mut px.y,
            );
            write_mb(self.cur, mbx, mby, &px);
            for (k, blk) in p.res.iter().enumerate() {
                if p.cbp >> (5 - k) & 1 != 0 {
                    add_block(self.cur, mbx, mby, k, blk, p.field_dct);
                }
            }
        }
        self.obmc_pending = pending;
    }

    /// One intra block: DC, AC, prediction, dequantisation, IDCT.
    fn intra_block(
        &mut self,
        r: &mut BitReader,
        mbx: usize,
        mby: usize,
        k: usize,
        h: &MbHdr,
        pre: Option<&[i16; 64]>,
    ) -> Result<()> {
        let luma = k < 4;
        let coded = h.cbp >> (5 - k) & 1 != 0;
        let mut blk = [0i16; 64];
        if self.sh {
            let dc = r.read(8)?;
            if dc == 0 || dc == 128 {
                return Err(invalid(format!("INTRADC {dc}")));
            }
            blk[0] = if dc == 255 { 128 } else { dc as i16 };
            if coded {
                read_coeffs(r, &mut blk, &ZIGZAG, 1, false, true)?;
            }
            self.quant.intra(&mut blk, h.qp, 8);
        } else {
            let mb = mby * self.st.mbw + mbx;
            let pred = self.st.intra_pred(mbx, mby, k, self.slice);
            let scan = match (h.ac_pred, pred.dir) {
                _ if self.hdr.alternate_vertical_scan => &ALT_VERTICAL,
                (false, _) => &ZIGZAG,
                (true, Dir::Up) => &ALT_HORIZONTAL,
                (true, Dir::Left) => &ALT_VERTICAL,
            };
            let scaler = dc_scaler(h.qp, luma);
            let dc_diff = if h.use_dc_vlc {
                match h.dc {
                    Some(d) => d[k],
                    None => read_dc_diff(r, luma)?,
                }
            } else {
                0
            };
            if let Some(p) = pre {
                unscan(p, scan, &mut blk);
            } else if coded {
                read_coeffs(
                    r,
                    &mut blk,
                    scan,
                    if h.use_dc_vlc { 1 } else { 0 },
                    true,
                    false,
                )?;
            }
            let dc_level = if h.use_dc_vlc { dc_diff } else { blk[0] as i32 }
                + round_div(pred.dc, scaler as i32);
            blk[0] = dc_level.clamp(-2048, 2047) as i16;
            if h.ac_pred
                && let Some((v, qpn)) = pred.ac
            {
                for i in 1..8 {
                    let idx = if pred.dir == Dir::Up { i } else { i * 8 };
                    let p = ac_pred_value(v[i - 1], qpn, h.qp);
                    blk[idx] = (blk[idx] as i32 + p).clamp(-2048, 2047) as i16;
                }
            }
            let dc_f = (blk[0] as i32 * scaler as i32).clamp(-2048, 2047);
            self.st.store_intra(mb, k, dc_f, &blk);
            self.quant.intra(&mut blk, h.qp, scaler);
        }
        idct(&mut blk);
        put_block(self.cur, mbx, mby, k, &blk, h.field_dct);
        Ok(())
    }

    /// One B-VOP macroblock.
    fn mb_b(&mut self, r: &mut BitReader, mbx: usize, mby: usize) -> Result<()> {
        let mb = mby * self.st.mbw + mbx;
        let fwd = self
            .fwd
            .ok_or_else(|| invalid("a B-VOP without a past reference"))?;
        let bwd = self
            .bwd
            .ok_or_else(|| invalid("a B-VOP without a future reference"))?;
        let col = self
            .col
            .ok_or_else(|| invalid("a B-VOP without a future reference"))?;
        self.st.slice[mb] = self.slice;
        let mut px = MbPix::new();
        if col.kind[mb] == MbKind::Skipped {
            // The co-located macroblock was not coded: neither is this
            // one, which is the past reference's, unmoved.
            predict_mb(fwd, mbx, mby, &[[0, 0]; 4], false, false, false, &mut px);
            write_mb(self.cur, mbx, mby, &px);
            return Ok(());
        }
        let qpel = self.vol.quarter_sample;
        let modb_one = r.read_bit()?;
        let (t, cbp) = if modb_one {
            (BType::Direct, 0)
        } else {
            let has_cbp = !r.read_bit()?;
            let t = if r.read_bit()? {
                BType::Direct
            } else if r.read_bit()? {
                BType::Interpolate
            } else if r.read_bit()? {
                BType::Backward
            } else if r.read_bit()? {
                BType::Forward
            } else {
                return Err(invalid("B-VOP mb_type 0000"));
            };
            let cbp = if has_cbp { r.read(6)? as u8 } else { 0 };
            if t != BType::Direct && cbp != 0 && r.read_bit()? {
                // dbquant: 10 is -2, 11 is +2.
                let d = if r.read_bit()? { 2 } else { -2 };
                self.qp = (self.qp as i32 + d).clamp(1, 31) as u32;
            }
            (t, cbp)
        };
        // interlaced_information() of a B-VOP macroblock.
        let mut field_dct = false;
        let mut field_pred = false;
        let mut refs = [[false; 2]; 2];
        if self.vol.interlaced && !modb_one {
            if cbp != 0 {
                field_dct = r.read_bit()?;
            }
            if t != BType::Direct {
                field_pred = r.read_bit()?;
                if field_pred {
                    if t != BType::Backward {
                        refs[0] = [r.read_bit()?, r.read_bit()?];
                    }
                    if t != BType::Forward {
                        refs[1] = [r.read_bit()?, r.read_bit()?];
                    }
                }
            }
        }
        if t == BType::Direct {
            // MVDB: absent (zero) when modb is 1, else coded with f_code 1.
            let mvd = if modb_one {
                [0, 0]
            } else {
                [read_mvd(r, 1)?, read_mvd(r, 1)?]
            };
            if col.field[mb] && col.kind[mb] == MbKind::Inter {
                let (trb, trd) = field_direct_distances(
                    self.field_times,
                    self.hdr.top_field_first,
                    col.field_ref[mb],
                );
                let colmv = if self.zero_field_direct {
                    [[0; 2]; 2]
                } else {
                    col.field_mv[mb]
                };
                let (mvf, mvb) = field_direct_vectors(colmv, mvd, trb, trd);
                field_direct_predict(
                    fwd,
                    bwd,
                    mbx,
                    mby,
                    &mvf,
                    &mvb,
                    col.field_ref[mb],
                    qpel,
                    &mut px,
                );
                self.field_direct_mbs += 1;
            } else {
                let (mvf, mvb) = direct_vectors(col, mbx, mby, mvd, self.trb, self.trd);
                predict_mb(fwd, mbx, mby, &mvf, true, false, qpel, &mut px);
                let mut pb = MbPix::new();
                predict_mb(bwd, mbx, mby, &mvb, true, false, qpel, &mut pb);
                px.average(&pb);
            }
        } else if field_pred {
            self.field_b_mbs += 1;
            let mut mvs = [[[0, 0]; 2]; 2];
            for (d, refd) in [(0, fwd), (1, bwd)] {
                let used = if d == 0 {
                    t != BType::Backward
                } else {
                    t != BType::Forward
                };
                if !used {
                    continue;
                }
                let fcode = if d == 0 {
                    self.hdr.fcode_forward
                } else {
                    self.hdr.fcode_backward
                };
                let m = read_field_mvs(r, self.pmv[d], fcode)?;
                mvs[d] = m;
                // Each field's predictor is its field vector in frame
                // units.
                self.pmv[d] = m.map(|v| [v[0], 2 * v[1]]);
                let _ = refd;
            }
            let mut pb = MbPix::new();
            if t != BType::Backward {
                predict_fields(fwd, mbx, mby, &mvs[0], refs[0], false, qpel, &mut px);
            }
            if t != BType::Forward {
                let dst = if t == BType::Backward {
                    &mut px
                } else {
                    &mut pb
                };
                predict_fields(bwd, mbx, mby, &mvs[1], refs[1], false, qpel, dst);
            }
            if t == BType::Interpolate {
                px.average(&pb);
            }
        } else {
            let mut mvf = [0, 0];
            let mut mvb = [0, 0];
            if t != BType::Backward {
                mvf = read_mv(r, self.pmv[0][0], self.hdr.fcode_forward)?;
                self.pmv[0] = [mvf; 2];
            }
            if t != BType::Forward {
                mvb = read_mv(r, self.pmv[1][0], self.hdr.fcode_backward)?;
                self.pmv[1] = [mvb; 2];
            }
            match t {
                BType::Forward => predict_mb(fwd, mbx, mby, &[mvf; 4], false, false, qpel, &mut px),
                BType::Backward => {
                    predict_mb(bwd, mbx, mby, &[mvb; 4], false, false, qpel, &mut px)
                }
                _ => {
                    predict_mb(fwd, mbx, mby, &[mvf; 4], false, false, qpel, &mut px);
                    let mut pb = MbPix::new();
                    predict_mb(bwd, mbx, mby, &[mvb; 4], false, false, qpel, &mut pb);
                    px.average(&pb);
                }
            }
        }
        write_mb(self.cur, mbx, mby, &px);
        self.residual(r, mbx, mby, cbp, self.qp, field_dct, None)
    }

    /// The GOB layer of a short-header picture (6.2.7.1; H.263 5.2): GOBs
    /// of one macroblock row up to CIF, two for 4CIF, four for 16CIF, each
    /// optionally opened by a GOB header that resets the quantiser and
    /// bounds motion vector prediction like a video packet.
    fn run_short(&mut self, r: &mut BitReader) {
        let total = self.total();
        let rows = match self.cur.h {
            0..=288 => 1,
            289..=576 => 2,
            _ => 4,
        };
        let per_gob = self.st.mbw * rows;
        let gobs = total.div_ceil(per_gob);
        let mut g = 0;
        while g < gobs {
            if g > 0
                && let Some((pos, gn)) = gob_header_at(r)
            {
                r.set_pos(pos);
                match self.gob_header(r) {
                    Ok(()) if gn as usize == g => {}
                    Ok(()) if (gn as usize) > g && (gn as usize) < gobs => {
                        self.record(invalid("GOBs missing"));
                        self.conceal(g * per_gob, gn as usize * per_gob);
                        g = gn as usize;
                    }
                    Ok(()) | Err(_) => {
                        self.record(invalid(format!("GOB number {gn} where {g} was due")));
                        self.conceal(g * per_gob, total);
                        return;
                    }
                }
            }
            let mut failed = false;
            for mb in g * per_gob..((g + 1) * per_gob).min(total) {
                if let Err(e) = self.mb(r, mb) {
                    self.record(e);
                    failed = true;
                    // Resume at the next GOB header, if there is one.
                    let mut next = None;
                    r.align();
                    while r.left() >= 24 {
                        if r.peek(17) == 1 {
                            let gn = r.peek_at(17, 5) as usize;
                            if gn > g && gn < gobs {
                                next = Some(gn);
                                break;
                            }
                        }
                        r.set_pos(r.pos() + 8);
                    }
                    match next {
                        Some(gn) => {
                            self.conceal(mb, gn * per_gob);
                            if self.gob_header(r).is_err() {
                                self.conceal(gn * per_gob, total);
                                return;
                            }
                            g = gn;
                        }
                        None => {
                            self.conceal(mb, total);
                            return;
                        }
                    }
                    break;
                }
            }
            if !failed {
                g += 1;
            }
        }
    }

    /// GBSC, GN, GFID, GQUANT.
    fn gob_header(&mut self, r: &mut BitReader) -> Result<()> {
        r.skip(17)?;
        r.read(5)?; // GN
        r.read(2)?; // GFID
        let q = r.read(5)?;
        if q == 0 {
            return Err(invalid("GQUANT is zero"));
        }
        self.new_packet(q);
        Ok(())
    }
}

/// Where a GOB start code begins, if one follows (after optional zero
/// stuffing to a byte boundary), and its GOB number.
fn gob_header_at(r: &BitReader) -> Option<(usize, u32)> {
    if r.peek(17) == 1 {
        return Some((r.pos(), r.peek_at(17, 5)));
    }
    let k = (8 - (r.pos() & 7)) & 7;
    if k > 0 && r.peek(k as u32) == 0 && r.peek_at(k, 17) == 1 {
        return Some((r.pos() + k, r.peek_at(k + 17, 5)));
    }
    None
}

/// The vectors of a direct-mode macroblock (7.6.9.5), per 8x8 block:
/// `MVF = TRB * MV / TRD + MVD`, and `MVB = (TRB - TRD) * MV / TRD` when
/// `MVD` is zero, else `MVF - MV`, where `MV` is the co-located block's
/// vector in the future reference (zero when that macroblock is intra).
pub(crate) fn direct_vectors(
    col: &Motion,
    mbx: usize,
    mby: usize,
    mvd: [i32; 2],
    trb: i32,
    trd: i32,
) -> ([[i32; 2]; 4], [[i32; 2]; 4]) {
    let mb = mby * col.mbw + mbx;
    let intra = col.kind[mb] == MbKind::Intra;
    let mut f = [[0; 2]; 4];
    let mut b = [[0; 2]; 4];
    for k in 0..4 {
        let m = if intra {
            [0, 0]
        } else {
            let v = col.mv[(2 * mby + (k >> 1)) * 2 * col.mbw + 2 * mbx + (k & 1)];
            [v[0] as i32, v[1] as i32]
        };
        for j in 0..2 {
            // In i64: the temporal distances of a damaged stream can be
            // anything.
            let (trb, trd, mj) = (trb as i64, trd as i64, m[j] as i64);
            let scaled = if trd != 0 { trb * mj / trd } else { 0 };
            let fv = (scaled + mvd[j] as i64).clamp(-(1 << 16), 1 << 16);
            let bv = if mvd[j] == 0 {
                if trd != 0 { (trb - trd) * mj / trd } else { 0 }
            } else {
                fv - mj
            };
            f[k][j] = fv as i32;
            b[k][j] = bv.clamp(-(1 << 16), 1 << 16) as i32;
        }
    }
    (f, b)
}

/// The identity scan: coefficients kept in scan order.
const IDENTITY_SCAN: [u8; 64] = {
    let mut s = [0u8; 64];
    let mut i = 0;
    while i < 64 {
        s[i] = i as u8;
        i += 1;
    }
    s
};

/// One block of reversible TCOEF events read backwards (Annex E.1.3):
/// from its `last` event back to the event before which the previous
/// block's `last` event (or, for the packet's first block, the partition
/// start) lies. Returns the coefficients in scan order from `start`.
fn read_block_back(
    br: &mut BackReader,
    start: usize,
    intra: bool,
    first: bool,
) -> Result<[i16; 64]> {
    let mut ev = Vec::with_capacity(16);
    let (last, run, level) = read_rvlc_event_back(br, intra)?;
    if !last {
        return Err(invalid("a block read backwards does not end with last"));
    }
    ev.push((run, level));
    while br.left() > 0 {
        let save = br.clone();
        let (last, run, level) = read_rvlc_event_back(br, intra)?;
        if last {
            if first {
                return Err(invalid("data before a packet's first block"));
            }
            *br = save;
            break;
        }
        ev.push((run, level));
        if ev.len() > 64 {
            return Err(invalid("more than 64 coefficients in a block"));
        }
    }
    if first && br.left() > 0 {
        return Err(invalid("data before a packet's first block"));
    }
    let mut c = [0i16; 64];
    let mut i = start;
    for &(run, level) in ev.iter().rev() {
        i += run as usize;
        if i >= 64 {
            return Err(invalid("coefficients run past the end of the block"));
        }
        c[i] = level as i16;
        i += 1;
    }
    Ok(c)
}

/// The threshold `T` of Annex E.1.4.4.2: bits next to a detected error
/// that are not trusted.
const RVLC_T: i64 = 90;

/// The strategies of Annex E.1.4.4.2.1: how many macroblocks of a packet
/// to keep from its beginning (decoded forwards) and from its end (decoded
/// backwards), given the partition's `l` bits and `n` macroblocks, the
/// bits (`l1`, `l2`) and whole macroblocks (`n1`, `n2`) each direction
/// decoded before its error, and `f_mb` / `b_mb`, the macroblocks with at
/// least one bit among the first / last `S` bits.
#[allow(clippy::too_many_arguments)]
pub(crate) fn rvlc_strategy(
    l: usize,
    n: usize,
    l1: usize,
    l2: usize,
    n1: usize,
    n2: usize,
    f_mb: impl Fn(i64) -> usize,
    b_mb: impl Fn(i64) -> usize,
) -> (usize, usize) {
    let (n, l) = (n as i64, l as i64);
    let (l1, l2) = (l1 as i64, l2 as i64);
    let (n1, n2) = ((n1 as i64).min(n - 1), (n2 as i64).min(n - 1));
    let f = |s| f_mb(s) as i64;
    let b = |s| b_mb(s) as i64;
    let (front, back) = if l1 + l2 < l {
        if n1 + n2 < n {
            // Strategy 1.
            (f(l1 - RVLC_T), b(l2 - RVLC_T))
        } else {
            // Strategy 2.
            (n - n2 - 1, n - n1 - 1)
        }
    } else if n1 + n2 < n {
        // Strategy 3.
        (n - b(l2), n - f(l1))
    } else {
        // Strategy 4.
        ((n - b(l2)).min(n - n2 - 1), (n - f(l1)).min(n - n1 - 1))
    };
    (front.clamp(0, n) as usize, back.clamp(0, n) as usize)
}

/// The temporal distances of field direct mode (7.7.2.3), in field
/// periods, for the B-VOP's top (0) and bottom (1) fields: `TRB[i] =
/// 2 (T(cur) // Tframe - T(past) // Tframe) + d[i]` and likewise `TRD[i]`
/// from the future reference, where `d[i]` (Table 7-12) is how many field
/// periods field `i` lies after the past reference field the co-located
/// macroblock's field `i` was predicted from: the fields' order in the
/// frame (`top_field_first`) decides which comes first.
pub(crate) fn field_direct_distances(
    [past, cur, future, tframe]: [i64; 4],
    top_field_first: bool,
    refs: [bool; 2],
) -> ([i64; 2], [i64; 2]) {
    let tf = tframe.max(1);
    let rd = |t: i64| crate::mbstate::round_div64(t, tf);
    let frames_b = rd(cur) - rd(past);
    let frames_d = rd(future) - rd(past);
    // Position of a field within its frame, in field periods.
    let pos = |bottom: bool| (bottom != !top_field_first) as i64;
    let mut trb = [0; 2];
    let mut trd = [0; 2];
    for i in 0..2 {
        let delta = pos(i == 1) - pos(refs[i]);
        trb[i] = 2 * frames_b + delta;
        trd[i] = 2 * frames_d + delta;
    }
    (trb, trd)
}

/// The four field vectors of a field direct macroblock (7.7.2.3) from the
/// co-located macroblock's field vectors `mv` and the one delta `mvd`:
/// `mvf[i] = TRB[i] MV[i] / TRD[i] + MVD`, `mvb[i] = (TRB[i] - TRD[i])
/// MV[i] / TRD[i]` where `MVD` is zero, else `mvf[i] - MV[i]`.
pub(crate) fn field_direct_vectors(
    mv: [[i16; 2]; 2],
    mvd: [i32; 2],
    trb: [i64; 2],
    trd: [i64; 2],
) -> ([[i32; 2]; 2], [[i32; 2]; 2]) {
    let mut f = [[0; 2]; 2];
    let mut b = [[0; 2]; 2];
    for i in 0..2 {
        for j in 0..2 {
            let m = mv[i][j] as i64;
            let scaled = if trd[i] != 0 { trb[i] * m / trd[i] } else { 0 };
            let fv = (scaled + mvd[j] as i64).clamp(-(1 << 16), 1 << 16);
            let bv = if mvd[j] == 0 {
                if trd[i] != 0 {
                    (trb[i] - trd[i]) * m / trd[i]
                } else {
                    0
                }
            } else {
                fv - m
            };
            f[i][j] = fv as i32;
            b[i][j] = bv.clamp(-(1 << 16), 1 << 16) as i32;
        }
    }
    (f, b)
}

/// Field direct prediction: each field forward from the past reference
/// field the co-located macroblock's field used, backward from the same
/// field of the future reference, averaged.
#[allow(clippy::too_many_arguments)]
pub(crate) fn field_direct_predict(
    fwd: &Pic,
    bwd: &Pic,
    mbx: usize,
    mby: usize,
    mvf: &[[i32; 2]; 2],
    mvb: &[[i32; 2]; 2],
    refs: [bool; 2],
    qpel: bool,
    out: &mut MbPix,
) {
    predict_fields(fwd, mbx, mby, mvf, refs, false, qpel, out);
    let mut pb = MbPix::new();
    predict_fields(bwd, mbx, mby, mvb, [false, true], false, qpel, &mut pb);
    out.average(&pb);
}

/// Whether what follows the last macroblock is the stuffing that should
/// be there: up to the byte boundary, `next_start_code()`'s zero then ones
/// (zeros, and ones without the zero, are accepted too; zeros are the
/// short video header's rule);
/// then only padding bytes — a further `0x7f` stuffing byte, or bytes of
/// ones then zeros (`0x00`, `0xff`, `0xc0`, ...) — or, for the short video
/// header, an end-of-sequence code. Real
/// encoders pad all three ways; what this rejects is macroblock data that
/// stopped short of the end, the sign of a stream read wrongly.
fn tail_ok(r: &BitReader, sh: bool) -> bool {
    let k = (8 - (r.pos() & 7)) & 7;
    if k > 0 {
        let bits = r.peek(k as u32);
        let stuffing = (1u32 << (k - 1)) - 1;
        if bits != 0 && bits != stuffing && bits != (1u32 << k) - 1 {
            return false;
        }
    }
    let mut t = r.clone();
    t.set_pos(r.pos() + k);
    if sh && t.left() >= 22 && t.peek(22) == super::SHORT_VIDEO_END_MARKER {
        return true;
    }
    while t.left() >= 8 {
        // Ones then zeros (0x00, 0x80, 0xc0, ... 0xff), or a 0x7f stuffing
        // byte.
        let b = t.peek(8) as u8;
        if b != 0x7f && b.leading_ones() + b.trailing_zeros() < 8 {
            return false;
        }
        t.set_pos(t.pos() + 8);
    }
    true
}

/// A field vector's predictor from a frame-unit predictor `pred`: its
/// vertical component halved (field lines), the division truncating toward
/// zero (`/`, clause 4).
#[inline]
pub(crate) fn field_pred(pred: [i32; 2]) -> [i32; 2] {
    [pred[0], pred[1] / 2]
}

/// The two field vectors of a field-predicted macroblock (top field's,
/// then bottom field's), each predicted from its predictor in `pred`
/// (frame units; [`field_pred`]). A P-VOP gives both the macroblock's one
/// predictor; a B-VOP keeps one per field (7.7.2.2).
pub(crate) fn read_field_mvs(
    r: &mut BitReader,
    pred: [[i32; 2]; 2],
    fcode: u32,
) -> Result<[[i32; 2]; 2]> {
    let top = read_mv(r, field_pred(pred[0]), fcode)?;
    let bot = read_mv(r, field_pred(pred[1]), fcode)?;
    Ok([top, bot])
}

/// The frame vector of a field-predicted macroblock.
#[inline]
pub(crate) fn field_to_frame(top: [i32; 2], bot: [i32; 2]) -> [i32; 2] {
    let sx = top[0] + bot[0];
    [(sx >> 1) | (sx & 1), top[1] + bot[1]]
}

/// Whether `r` is at `len` bits of resync marker: `len - 1` zeros, a one.
fn is_marker(r: &BitReader, len: usize) -> bool {
    r.left() >= len && {
        let hi = r.peek(16);
        let rest = len - 16;
        hi == 0 && r.peek_at(16, rest as u32) == 1
    }
}

/// The six DC differentials of an intra macroblock (data partitioning).
fn read_dcs(r: &mut BitReader) -> Result<[i32; 6]> {
    let mut d = [0; 6];
    for (k, v) in d.iter_mut().enumerate() {
        *v = read_dc_diff(r, k < 4)?;
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With field DCT, luminance blocks 0 and 1 hold the macroblock's even
    /// lines and 2 and 3 its odd lines; chrominance is unaffected.
    #[test]
    fn field_dct_block_placement() {
        let mut pic = Pic::new(32, 32);
        for k in 0..4 {
            let blk = [10 * (k as i16 + 1); 64];
            put_block(&mut pic, 1, 1, k, &blk, true);
        }
        let s = pic.ystride();
        for r in 0..16 {
            for c in 0..16 {
                let k = (r & 1) * 2 + c / 8;
                assert_eq!(
                    pic.y[(16 + r) * s + 16 + c],
                    10 * (k as u8 + 1),
                    "row {r} col {c}"
                );
            }
        }
        // Frame DCT: quadrants.
        let mut pic = Pic::new(32, 32);
        for k in 0..4 {
            put_block(&mut pic, 0, 0, k, &[10 * (k as i16 + 1); 64], false);
        }
        assert_eq!(pic.y[s + 1], 10);
        assert_eq!(pic.y[9 * s + 9], 40);
    }

    /// Field prediction with zero vectors from the same-parity fields is a
    /// copy; from the opposite parity it swaps the lines.
    #[test]
    fn field_prediction_selects_fields() {
        let mut pic = Pic::new(32, 32);
        let s = pic.ystride();
        for r in 0..32 {
            for c in 0..32 {
                pic.y[r * s + c] = (r * 7 + c) as u8;
            }
        }
        let mut px = MbPix::new();
        predict_fields(
            &pic,
            1,
            1,
            &[[0, 0]; 2],
            [false, true],
            false,
            false,
            &mut px,
        );
        for r in 0..16 {
            assert_eq!(px.y[r * 16], pic.y[(16 + r) * s + 16]);
        }
        predict_fields(
            &pic,
            1,
            1,
            &[[0, 0]; 2],
            [true, false],
            false,
            false,
            &mut px,
        );
        assert_eq!(px.y[0], pic.y[17 * s + 16]);
        assert_eq!(px.y[16], pic.y[16 * s + 16]);
        // A one-line field vector moves two frame lines.
        predict_fields(
            &pic,
            1,
            1,
            &[[0, 2], [0, 2]],
            [false, true],
            false,
            false,
            &mut px,
        );
        assert_eq!(px.y[0], pic.y[18 * s + 16]);
        assert_eq!(px.y[16], pic.y[19 * s + 16]);
    }

    /// The frame vector a field-predicted macroblock contributes to
    /// prediction: horizontal components averaged, vertical (in field
    /// lines) summed.
    #[test]
    fn field_vectors_to_frame_vector() {
        assert_eq!(field_to_frame([4, 3], [6, -1]), [5, 2]);
        assert_eq!(field_to_frame([3, 1], [4, 1]), [3, 2]);
        assert_eq!(field_to_frame([-3, 0], [-4, 0]), [-3, 0]);
    }

    /// AC prediction rescales the neighbour's coefficients to the current
    /// quantiser with `//` (nearest, halves away from zero).
    #[test]
    fn ac_prediction_scaling() {
        use crate::mbstate::ac_pred_value;
        assert_eq!(ac_pred_value(10, 8, 8), 10);
        assert_eq!(ac_pred_value(10, 4, 8), 5);
        assert_eq!(ac_pred_value(3, 4, 8), 2); // 1.5 -> 2
        assert_eq!(ac_pred_value(-3, 4, 8), -2);
        assert_eq!(ac_pred_value(7, 10, 4), 18); // 17.5 -> 18
        assert_eq!(ac_pred_value(1, 1, 31), 0);
    }

    #[test]
    fn intra_dc_vlc_threshold() {
        // Table 6-21: 0 always, 7 never, else running QP < 13, 15, ... 23.
        assert!(use_intra_dc_vlc(0, 31));
        assert!(!use_intra_dc_vlc(7, 1));
        assert!(use_intra_dc_vlc(1, 12));
        assert!(!use_intra_dc_vlc(1, 13));
        assert!(use_intra_dc_vlc(6, 22));
        assert!(!use_intra_dc_vlc(6, 23));
    }

    /// Direct mode (7.6.9.5) by hand: co-located vector 4, TRB 1, TRD 3:
    /// MVF = 1 * 4 / 3 = 1, MVB = (1 - 3) * 4 / 3 = -2 (truncating); with
    /// MVD 2, MVF = 3 and MVB = MVF - MV = -1.
    #[test]
    fn direct_mode_vectors() {
        let mut col = Motion::intra(1, 1);
        col.kind[0] = MbKind::Inter;
        col.mv = vec![[4, -4]; 4];
        let (f, b) = direct_vectors(&col, 0, 0, [0, 0], 1, 3);
        assert_eq!(f, [[1, -1]; 4]);
        assert_eq!(b, [[-2, 2]; 4]);
        let (f, b) = direct_vectors(&col, 0, 0, [2, 0], 1, 3);
        assert_eq!(f, [[3, -1]; 4]);
        assert_eq!(b, [[-1, 2]; 4]);
        // An intra co-located macroblock contributes nothing.
        col.kind[0] = MbKind::Intra;
        let (f, b) = direct_vectors(&col, 0, 0, [1, 1], 1, 3);
        assert_eq!((f, b), ([[1, 1]; 4], [[1, 1]; 4]));
    }

    #[test]
    fn vector_wrapping() {
        // f_code 1: [-32, 31] half samples.
        assert_eq!(wrap_mv(31, 1), 31);
        assert_eq!(wrap_mv(32, 1), -32);
        assert_eq!(wrap_mv(-33, 1), 31);
        // f_code 3: [-128, 127].
        assert_eq!(wrap_mv(130, 3), -126);
    }

    /// The four strategies of Annex E.1.4.4.2.1 by hand, on a packet of 10
    /// macroblocks of 100 bits each (so `f_mb(S)` = `b_mb(S)` = S / 100
    /// rounded up: a macroblock counts once one of its bits is in).
    #[test]
    fn rvlc_strategies() {
        let mbs = |s: i64| {
            if s <= 0 {
                0
            } else {
                (s as usize).div_ceil(100)
            }
        };
        // 1: L1 + L2 < L, N1 + N2 < N: f_mb(L1 - T), b_mb(L2 - T).
        assert_eq!(rvlc_strategy(1000, 10, 300, 400, 3, 4, mbs, mbs), (3, 4));
        assert_eq!(rvlc_strategy(1000, 10, 250, 395, 2, 3, mbs, mbs), (2, 4));
        // 2: L1 + L2 < L, N1 + N2 >= N: N - N2 - 1, N - N1 - 1.
        assert_eq!(rvlc_strategy(1000, 10, 300, 400, 5, 6, mbs, mbs), (3, 4));
        // 3: L1 + L2 >= L, N1 + N2 < N: N - b_mb(L2), N - f_mb(L1).
        assert_eq!(rvlc_strategy(1000, 10, 600, 500, 3, 4, mbs, mbs), (5, 4));
        // 4: both: the smaller of the two each way.
        assert_eq!(rvlc_strategy(1000, 10, 600, 500, 6, 5, mbs, mbs), (4, 3));
        // N1 and N2 count at most N - 1.
        assert_eq!(
            rvlc_strategy(1000, 10, 1000, 1000, 10, 10, mbs, mbs),
            (0, 0)
        );
    }

    /// Field direct mode by hand (7.7.2.3, Table 7-12). Frames one tick
    /// apart; past reference at 0, B-VOP at 1, future at 3: two field
    /// periods per frame. Co-located top field predicted from the past
    /// bottom field, bottom field from the past top field.
    #[test]
    fn field_direct_mode() {
        // Top field first: the top field is first in the frame.
        let (trb, trd) = field_direct_distances([0, 1, 3, 1], true, [true, false]);
        // Top field (position 0) from a bottom field (position 1): -1.
        // Bottom field (1) from a top field (0): +1.
        assert_eq!((trb, trd), ([1, 3], [5, 7]));
        // Bottom field first reverses the positions.
        let (trb, trd) = field_direct_distances([0, 1, 3, 1], false, [true, false]);
        assert_eq!((trb, trd), ([3, 1], [7, 5]));
        // Same-parity references: whole frames.
        let (trb, trd) = field_direct_distances([0, 1, 3, 1], true, [false, true]);
        assert_eq!((trb, trd), ([2, 2], [6, 6]));
        // Times in 1001ths at 30000 ticks a second: Tframe 1001.
        let (trb, trd) = field_direct_distances([0, 1001, 3003, 1001], true, [false, true]);
        assert_eq!((trb, trd), ([2, 2], [6, 6]));
        // The vectors: MV = (10, -6) and (4, 8), TRB 2 / TRD 6, MVD 0:
        // mvf = 2 MV / 6, mvb = -4 MV / 6, truncating.
        let (f, b) = field_direct_vectors([[10, -6], [4, 8]], [0, 0], [2, 2], [6, 6]);
        assert_eq!(f, [[3, -2], [1, 2]]);
        assert_eq!(b, [[-6, 4], [-2, -5]]);
        // With MVD (1, 0): mvf x gains 1 and mvb x = mvf x - MV x.
        let (f, b) = field_direct_vectors([[10, -6], [4, 8]], [1, 0], [2, 2], [6, 6]);
        assert_eq!(f, [[4, -2], [2, 2]]);
        assert_eq!(b, [[-6, 4], [-2, -5]]);
    }

    /// A field vector's predictor halves the frame predictor's vertical
    /// component toward zero (`/`), so -41 gives -20, not -21.
    #[test]
    fn field_predictor_truncates() {
        assert_eq!(field_pred([10, -41]), [10, -20]);
        assert_eq!(field_pred([6, -1]), [6, 0]);
        assert_eq!(field_pred([-3, 7]), [-3, 3]);
        assert_eq!(field_pred([0, -8]), [0, -4]);
    }
}
