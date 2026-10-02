//! The reconstructed picture both sides work on: macroblock-aligned planes
//! with the VOP's true size kept beside them, because motion compensation
//! pads from the VOP boundary (clause 7.6.4), not the macroblock one.

use crate::frame::{Frame, VopType};

#[derive(Clone)]
pub(crate) struct Pic {
    /// VOP width and height.
    pub w: u32,
    pub h: u32,
    pub mbw: usize,
    pub mbh: usize,
    /// Luma, `mbw * 16` wide (the stride) and `mbh * 16` high.
    pub y: Vec<u8>,
    /// Chroma, `mbw * 8` by `mbh * 8`.
    pub cb: Vec<u8>,
    pub cr: Vec<u8>,
}

impl Pic {
    pub fn new(w: u32, h: u32) -> Pic {
        let mbw = w.div_ceil(16) as usize;
        let mbh = h.div_ceil(16) as usize;
        Pic {
            w,
            h,
            mbw,
            mbh,
            y: vec![16; mbw * 16 * mbh * 16],
            cb: vec![128; mbw * 8 * mbh * 8],
            cr: vec![128; mbw * 8 * mbh * 8],
        }
    }

    #[inline]
    pub fn ystride(&self) -> usize {
        self.mbw * 16
    }

    #[inline]
    pub fn cstride(&self) -> usize {
        self.mbw * 8
    }

    /// Valid chroma width and height (the luma size halved, rounded up).
    #[inline]
    pub fn chroma_size(&self) -> (u32, u32) {
        Frame::chroma_size(self.w, self.h)
    }

    /// The planes as `(data, stride, valid width, valid height)`.
    pub fn plane(&self, i: usize) -> (&[u8], usize, i32, i32) {
        let (cw, ch) = self.chroma_size();
        match i {
            0 => (&self.y, self.ystride(), self.w as i32, self.h as i32),
            1 => (&self.cb, self.cstride(), cw as i32, ch as i32),
            _ => (&self.cr, self.cstride(), cw as i32, ch as i32),
        }
    }

    /// The planes as motion compensation reads them: `(data, stride,
    /// width, height)` of the area whose edge samples are repeated outward
    /// (unrestricted motion vectors, 7.6.4).
    ///
    /// That area is the whole reconstructed macroblock grid, not the VOL's
    /// width and height: for a size that is not a multiple of 16 the
    /// samples decoded past the VOP's right and bottom edges are part of
    /// the reference. DivX 5 streams of such sizes drift at the bottom
    /// edge when padding starts at the VOP boundary instead, and decode
    /// cleanly this way.
    pub fn ref_plane(&self, i: usize) -> (&[u8], usize, i32, i32) {
        let (w, h) = ((self.mbw * 16) as i32, (self.mbh * 16) as i32);
        match i {
            0 => (&self.y, self.ystride(), w, h),
            1 => (&self.cb, self.cstride(), w / 2, h / 2),
            _ => (&self.cr, self.cstride(), w / 2, h / 2),
        }
    }

    /// Crops to a [`Frame`].
    pub fn to_frame(
        &self,
        timestamp: i64,
        time_base: u32,
        vop_type: VopType,
        decode_index: u64,
    ) -> Frame {
        let mut f = Frame::new(self.w, self.h);
        f.timestamp = timestamp;
        f.time_base = time_base;
        f.vop_type = vop_type;
        f.decode_index = decode_index;
        for i in 0..3 {
            let (src, stride, w, h) = self.plane(i);
            let (w, h) = (w as usize, h as usize);
            let dst = f.plane_mut(i);
            for row in 0..h {
                dst[row * w..row * w + w].copy_from_slice(&src[row * stride..row * stride + w]);
            }
        }
        f
    }

    /// From a [`Frame`] of the same size, the area beyond its edges
    /// filled by repeating the edge samples (what the encoder codes there).
    pub fn from_frame(f: &Frame) -> Pic {
        let mut p = Pic::new(f.width, f.height);
        let (cw, ch) = p.chroma_size();
        let strides = [p.ystride(), p.cstride(), p.cstride()];
        let heights = [p.mbh * 16, p.mbh * 8, p.mbh * 8];
        let sizes = [
            (f.width as usize, f.height as usize),
            (cw as usize, ch as usize),
            (cw as usize, ch as usize),
        ];
        for i in 0..3 {
            let src = f.plane(i);
            let (w, h) = sizes[i];
            let stride = strides[i];
            let dst = match i {
                0 => &mut p.y,
                1 => &mut p.cb,
                _ => &mut p.cr,
            };
            for row in 0..heights[i] {
                let sr = row.min(h - 1);
                let s = &src[sr * w..sr * w + w];
                let d = &mut dst[row * stride..row * stride + stride];
                d[..w].copy_from_slice(s);
                let edge = s[w - 1];
                d[w..].fill(edge);
            }
        }
        p
    }
}
