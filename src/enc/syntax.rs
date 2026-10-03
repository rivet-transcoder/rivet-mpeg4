//! A coded I- or P-VOP macroblock held as syntax, so it can be written in
//! macroblock order (6.2.6) or, data partitioned, split across a video
//! packet's partitions (6.2.5.2: motion / DC, then the second partition,
//! then texture) — with the reversible VLCs of Table B-23 for the texture
//! when the VOL asks for them.

use crate::bits::BitWriter;
use crate::dec::vop::{DC_MARKER, MOTION_MARKER};

use super::write::{
    put_cbpy, put_coeffs, put_coeffs_rvlc, put_coeffs_short, put_dc_diff, put_mcbpc_i, put_mcbpc_p,
    put_mvd,
};

/// One macroblock of an I- or P-VOP.
#[derive(Clone)]
pub(crate) struct MbSyntax {
    /// The short video header's syntax (H.263 baseline): no `ac_pred_flag`,
    /// `dc` holds 8-bit INTRADC codes, H.263's escape.
    pub sh: bool,
    /// In a P-VOP (the `not_coded` bit and the P MCBPC table).
    pub p: bool,
    /// `not_coded` (P-VOPs): nothing else is written.
    pub not_coded: bool,
    /// `mb_type` (Table 6-25): 0 inter, 2 inter4v, 3 intra; 1 and 4 with
    /// `dquant`.
    pub mb_type: u8,
    /// Coded block pattern, block 0 in bit 5.
    pub cbp: u8,
    pub ac_pred: bool,
    /// `dquant` code (Table 6-22) for `mb_type` 1 and 4.
    pub dquant: u32,
    /// Vector differences, already wrapped, as `(x, y)` pairs.
    pub mvd: Vec<[i32; 2]>,
    pub fcode: u32,
    /// Intra DC differentials (the encoder always codes the DC with its
    /// own VLC: `intra_dc_vlc_thr` 0).
    pub dc: [i32; 6],
    /// Each block's levels (raster order) and the scan they are coded in.
    pub blocks: [[i16; 64]; 6],
    pub scans: [&'static [u8; 64]; 6],
}

impl MbSyntax {
    /// A macroblock with nothing coded yet (inter, no blocks).
    pub fn new(p: bool, fcode: u32) -> MbSyntax {
        MbSyntax {
            sh: false,
            p,
            not_coded: false,
            mb_type: 0,
            cbp: 0,
            ac_pred: false,
            dquant: 0,
            mvd: Vec::new(),
            fcode,
            dc: [0; 6],
            blocks: [[0; 64]; 6],
            scans: [&crate::tables::ZIGZAG; 6],
        }
    }

    fn intra(&self) -> bool {
        self.mb_type >= 3
    }

    fn mcbpc(&self, w: &mut BitWriter) {
        if self.p {
            put_mcbpc_p(w, self.mb_type, self.cbp & 3);
        } else {
            put_mcbpc_i(w, self.mb_type, self.cbp & 3);
        }
    }

    fn cbpy(&self, w: &mut BitWriter) {
        let y = self.cbp >> 2;
        put_cbpy(w, if self.intra() { y } else { 15 - y });
    }

    fn dquant(&self, w: &mut BitWriter) {
        if self.mb_type == 1 || self.mb_type == 4 {
            w.put(2, self.dquant);
        }
    }

    fn mvs(&self, w: &mut BitWriter) {
        for d in &self.mvd {
            put_mvd(w, d[0], self.fcode);
            put_mvd(w, d[1], self.fcode);
        }
    }

    fn dcs(&self, w: &mut BitWriter) {
        for k in 0..6 {
            put_dc_diff(w, self.dc[k], k < 4);
        }
    }

    fn block(&self, w: &mut BitWriter, k: usize, rvlc: bool) {
        if self.cbp >> (5 - k) & 1 == 0 {
            return;
        }
        let intra = self.intra();
        let start = intra as usize;
        if self.sh {
            put_coeffs_short(w, &self.blocks[k], self.scans[k], start);
        } else if rvlc {
            put_coeffs_rvlc(w, &self.blocks[k], self.scans[k], start, intra);
        } else {
            put_coeffs(w, &self.blocks[k], self.scans[k], start, intra);
        }
    }

    /// The macroblock in macroblock order (not data partitioned).
    pub fn write(&self, w: &mut BitWriter) {
        if self.p {
            w.put(1, self.not_coded as u32);
            if self.not_coded {
                return;
            }
        }
        self.mcbpc(w);
        if self.intra() && !self.sh {
            w.put(1, self.ac_pred as u32);
        }
        self.cbpy(w);
        self.dquant(w);
        self.mvs(w);
        for k in 0..6 {
            if self.intra() && self.sh {
                w.put(8, self.dc[k] as u32);
            } else if self.intra() {
                put_dc_diff(w, self.dc[k], k < 4);
            }
            self.block(w, k, false);
        }
    }
}

/// Writes a data-partitioned video packet's macroblocks (6.2.5.2): for an
/// I-VOP the MCBPC, `dquant` and DC of each, `dc_marker`, then each one's
/// `ac_pred_flag` and CBPY; for a P-VOP `not_coded`, MCBPC and vectors,
/// `motion_marker`, then each coded one's `ac_pred_flag`, CBPY, `dquant`
/// and intra DC; then every block's coefficients, with the reversible
/// table when `rvlc`.
pub(crate) fn write_partitioned(w: &mut BitWriter, mbs: &[MbSyntax], i_vop: bool, rvlc: bool) {
    for m in mbs {
        if i_vop {
            m.mcbpc(w);
            m.dquant(w);
            m.dcs(w);
        } else {
            w.put(1, m.not_coded as u32);
            if !m.not_coded {
                m.mcbpc(w);
                m.mvs(w);
            }
        }
    }
    if i_vop {
        w.put(19, DC_MARKER);
    } else {
        w.put(17, MOTION_MARKER);
    }
    for m in mbs {
        if i_vop {
            w.put(1, m.ac_pred as u32);
            m.cbpy(w);
        } else if !m.not_coded {
            if m.intra() {
                w.put(1, m.ac_pred as u32);
            }
            m.cbpy(w);
            m.dquant(w);
            if m.intra() {
                m.dcs(w);
            }
        }
    }
    for m in mbs {
        if m.p && m.not_coded {
            continue;
        }
        for k in 0..6 {
            m.block(w, k, rvlc);
        }
    }
}
