//! The planar picture both sides use: what the decoder hands back and what
//! the encoder takes.

use crate::error::{Result, config};

/// The coding type of a VOP (`vop_coding_type`, clause 6.3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VopType {
    /// Intra-coded.
    I,
    /// Predictive-coded (forward prediction from the previous I/P-VOP).
    P,
    /// Bidirectionally predictive-coded.
    B,
    /// Sprite-coded (here: global motion compensation).
    S,
}

/// One plane of a [`Frame`]: where it sits in the frame's data. Samples are
/// 8-bit and tightly packed (stride == width).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plane {
    /// Byte offset of the plane's first sample in [`Frame::data`].
    pub offset: usize,
    /// Width in samples.
    pub width: u32,
    /// Height in samples.
    pub height: u32,
}

impl Plane {
    /// The plane's size in bytes.
    pub fn len(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// Whether the plane has no samples.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// An 8-bit 4:2:0 picture: one buffer holding the planes one after the
/// other (Y, then Cb, then Cr — the layout of a packed planar I420 frame),
/// each tightly packed. Chroma planes are `ceil(width / 2)` by
/// `ceil(height / 2)`.
///
/// The decoder returns frames in display order, cropped to the VOL's
/// `video_object_layer_width` x `video_object_layer_height`; the encoder
/// takes them the same way and ignores the timing fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Luma width.
    pub width: u32,
    /// Luma height.
    pub height: u32,
    /// The samples of every plane, packed.
    pub data: Vec<u8>,
    /// Y, then Cb, then Cr.
    pub planes: [Plane; 3],
    /// Display time in units of `1 / time_base` seconds: the VOP's
    /// `modulo_time_base` and `vop_time_increment` accumulated from the
    /// start of the stream (for the short video header, the temporal
    /// reference, in 1001/30000 s units, unwrapped).
    pub timestamp: i64,
    /// Ticks per second of [`Self::timestamp`]: the VOL's
    /// `vop_time_increment_resolution` (30000 for the short video header).
    pub time_base: u32,
    /// How the VOP was coded.
    pub vop_type: VopType,
    /// Decode-order index of the VOP that produced this frame (0 for the
    /// first decoded VOP).
    pub decode_index: u64,
    /// True when part of the picture could not be decoded — a damaged
    /// video packet — and was concealed by copying from the reference.
    pub concealed: bool,
}

impl Frame {
    /// The `(width, height)` of the chroma planes of a `width` x `height`
    /// 4:2:0 frame.
    pub fn chroma_size(width: u32, height: u32) -> (u32, u32) {
        (width.div_ceil(2), height.div_ceil(2))
    }

    fn layout(width: u32, height: u32) -> ([Plane; 3], usize) {
        let (cw, ch) = Self::chroma_size(width, height);
        let y = Plane { offset: 0, width, height };
        let cb = Plane { offset: y.len(), width: cw, height: ch };
        let cr = Plane { offset: cb.offset + cb.len(), width: cw, height: ch };
        let total = cr.offset + cr.len();
        ([y, cb, cr], total)
    }

    /// A black frame (Y 16, Cb and Cr 128, the video range's black).
    pub fn new(width: u32, height: u32) -> Frame {
        let (planes, total) = Self::layout(width, height);
        let mut data = vec![128u8; total];
        data[..planes[0].len()].fill(16);
        Frame {
            width,
            height,
            data,
            planes,
            timestamp: 0,
            time_base: 1,
            vop_type: VopType::I,
            decode_index: 0,
            concealed: false,
        }
    }

    /// A frame from three tightly packed planes, which must have the 4:2:0
    /// sizes of a `width` x `height` picture.
    pub fn from_planes(width: u32, height: u32, y: &[u8], cb: &[u8], cr: &[u8]) -> Result<Frame> {
        let mut f = Frame::new(width, height);
        let [py, pcb, pcr] = f.planes;
        if y.len() != py.len() || cb.len() != pcb.len() || cr.len() != pcr.len() {
            return Err(config(format!(
                "plane sizes {}/{}/{} do not fit a {width}x{height} 4:2:0 frame ({}/{}/{})",
                y.len(),
                cb.len(),
                cr.len(),
                py.len(),
                pcb.len(),
                pcr.len()
            )));
        }
        f.data[py.offset..py.offset + py.len()].copy_from_slice(y);
        f.data[pcb.offset..pcb.offset + pcb.len()].copy_from_slice(cb);
        f.data[pcr.offset..pcr.offset + pcr.len()].copy_from_slice(cr);
        Ok(f)
    }

    /// The samples of plane `i` (0 Y, 1 Cb, 2 Cr).
    pub fn plane(&self, i: usize) -> &[u8] {
        let p = &self.planes[i];
        &self.data[p.offset..p.offset + p.len()]
    }

    /// The samples of plane `i`, mutably.
    pub fn plane_mut(&mut self, i: usize) -> &mut [u8] {
        let p = self.planes[i];
        &mut self.data[p.offset..p.offset + p.len()]
    }

    /// Checks that the planes describe this frame's dimensions and fit its
    /// buffer.
    pub(crate) fn validate(&self) -> Result<()> {
        let (planes, total) = Self::layout(self.width, self.height);
        if self.planes != planes || self.data.len() < total {
            return Err(config(format!(
                "frame planes do not describe a {}x{} 4:2:0 picture",
                self.width, self.height
            )));
        }
        Ok(())
    }
}
