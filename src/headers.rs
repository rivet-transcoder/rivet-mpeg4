//! The headers above the macroblock layer (clause 6.2 syntax, 6.3
//! semantics): visual object sequence, visual object, video object layer,
//! group of VOPs and VOP, read and written.

use crate::bits::{BitReader, BitWriter};
use crate::error::{Result, invalid, unsupported};
use crate::frame::VopType;
use crate::tables::{DEFAULT_INTER_MATRIX, DEFAULT_INTRA_MATRIX, ZIGZAG};
use crate::vlc;

/// Start code values (the byte after `00 00 01`), Table 6-3.
pub(crate) mod sc {
    pub const VO_FIRST: u8 = 0x00;
    pub const VOL_FIRST: u8 = 0x20;
    pub const VOL_LAST: u8 = 0x2f;
    pub const VOS: u8 = 0xb0;
    pub const USER_DATA: u8 = 0xb2;
    pub const GOV: u8 = 0xb3;
    pub const VISUAL_OBJECT: u8 = 0xb5;
    pub const VOP: u8 = 0xb6;
}

/// `sprite_enable` (Table 6-? of 6.3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpriteMode {
    /// No sprites.
    None,
    /// Static sprites (not supported).
    Static,
    /// Global motion compensation (S-VOPs).
    Gmc,
}

/// `define_vop_complexity_estimation_header()`: which statistics each VOP
/// header carries. The decoder only needs to skip them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Complexity {
    pub method: u32,
    pub opaque: bool,
    pub transparent: bool,
    pub intra_cae: bool,
    pub inter_cae: bool,
    pub no_update: bool,
    pub upsampling: bool,
    pub intra_blocks: bool,
    pub inter_blocks: bool,
    pub inter4v_blocks: bool,
    pub not_coded_blocks: bool,
    pub dct_coefs: bool,
    pub dct_lines: bool,
    pub vlc_symbols: bool,
    pub vlc_bits: bool,
    pub apm: bool,
    pub npm: bool,
    pub interpolate_mc_q: bool,
    pub forw_back_mc_q: bool,
    pub halfpel2: bool,
    pub halfpel4: bool,
    pub sadct: bool,
    pub quarterpel: bool,
}

impl Complexity {
    fn parse(r: &mut BitReader) -> Result<Self> {
        let mut c = Complexity { method: r.read(2)?, ..Default::default() };
        if c.method > 1 {
            return Err(unsupported(format!("complexity estimation method {}", c.method)));
        }
        if !r.read_bit()? {
            // shape_complexity_estimation_disable == 0
            c.opaque = r.read_bit()?;
            c.transparent = r.read_bit()?;
            c.intra_cae = r.read_bit()?;
            c.inter_cae = r.read_bit()?;
            c.no_update = r.read_bit()?;
            c.upsampling = r.read_bit()?;
        }
        if !r.read_bit()? {
            c.intra_blocks = r.read_bit()?;
            c.inter_blocks = r.read_bit()?;
            c.inter4v_blocks = r.read_bit()?;
            c.not_coded_blocks = r.read_bit()?;
        }
        r.marker("in the complexity estimation header")?;
        if !r.read_bit()? {
            c.dct_coefs = r.read_bit()?;
            c.dct_lines = r.read_bit()?;
            c.vlc_symbols = r.read_bit()?;
            c.vlc_bits = r.read_bit()?;
        }
        if !r.read_bit()? {
            c.apm = r.read_bit()?;
            c.npm = r.read_bit()?;
            c.interpolate_mc_q = r.read_bit()?;
            c.forw_back_mc_q = r.read_bit()?;
            c.halfpel2 = r.read_bit()?;
            c.halfpel4 = r.read_bit()?;
        }
        r.marker("in the complexity estimation header")?;
        if c.method == 1 && !r.read_bit()? {
            c.sadct = r.read_bit()?;
            c.quarterpel = r.read_bit()?;
        }
        Ok(c)
    }

    /// Bits of `read_vop_complexity_estimation_header()` in a VOP of this
    /// type: 8 per enabled statistic, 4 for `dcecs_vlc_bits`.
    fn vop_bits(&self, t: VopType) -> usize {
        let shape = [self.opaque, self.transparent, self.intra_cae, self.inter_cae, self.no_update, self.upsampling];
        let texture = [self.intra_blocks, self.not_coded_blocks, self.dct_coefs, self.dct_lines, self.vlc_symbols];
        let motion = [self.inter_blocks, self.inter4v_blocks, self.apm, self.npm, self.forw_back_mc_q, self.halfpel2, self.halfpel4];
        let n = |s: &[bool]| s.iter().filter(|&&b| b).count() * 8;
        let vlc_bits = if self.vlc_bits { 4 } else { 0 };
        match t {
            VopType::I => n(&shape) + n(&texture) + vlc_bits + n(&[self.sadct]),
            VopType::P => n(&shape) + n(&texture) + vlc_bits + n(&motion) + n(&[self.sadct, self.quarterpel]),
            VopType::B => {
                n(&shape) + n(&texture) + vlc_bits + n(&motion) + n(&[self.interpolate_mc_q, self.sadct, self.quarterpel])
            }
            VopType::S => n(&texture) + vlc_bits + n(&motion) + n(&[self.interpolate_mc_q]),
        }
    }
}

/// What a video object layer header says about the stream: the decoder
/// configuration ("decoder specific info" in an MP4 `esds`, or in-band).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct VolHeader {
    /// `profile_and_level_indication` of the visual object sequence header,
    /// when one was seen.
    pub profile_and_level: Option<u8>,
    /// `video_object_type_indication` (1 Simple, 17 Advanced Simple, ...).
    pub object_type: u8,
    /// `video_object_layer_verid` (or the visual object's verid).
    pub verid: u8,
    /// Luma width (`video_object_layer_width`).
    pub width: u32,
    /// Luma height (`video_object_layer_height`).
    pub height: u32,
    /// Pixel aspect ratio `(width, height)`; `(0, 0)` when unspecified.
    pub pixel_aspect: (u8, u8),
    /// `vop_time_increment_resolution`: ticks per second.
    pub time_resolution: u32,
    /// `fixed_vop_time_increment`, when the VOL declares a fixed rate.
    pub fixed_vop_time_increment: Option<u32>,
    /// `low_delay`, when `vol_control_parameters` is present.
    pub low_delay: Option<bool>,
    /// `interlaced`.
    pub interlaced: bool,
    /// `obmc_disable`.
    pub obmc_disable: bool,
    /// `sprite_enable`.
    pub sprite: SpriteMode,
    /// `no_of_sprite_warping_points`.
    pub sprite_warping_points: u32,
    /// `sprite_warping_accuracy` (0..=3: 1/2 to 1/16 sample).
    pub sprite_warping_accuracy: u32,
    /// `sprite_brightness_change`.
    pub sprite_brightness_change: bool,
    /// `quant_type`: true for the MPEG (matrix) quantiser, method 1.
    pub mpeg_quant: bool,
    /// Intra quantiser matrix, raster order (the default unless loaded).
    pub intra_matrix: [u8; 64],
    /// Non-intra quantiser matrix, raster order.
    pub inter_matrix: [u8; 64],
    /// `quarter_sample`.
    pub quarter_sample: bool,
    /// `resync_marker_disable`.
    pub resync_marker_disable: bool,
    /// `data_partitioned`.
    pub data_partitioned: bool,
    /// `reversible_vlc`.
    pub reversible_vlc: bool,
    /// `newpred_enable`.
    pub newpred: bool,
    /// `reduced_resolution_vop_enable`.
    pub reduced_resolution: bool,
    /// `scalability`.
    pub scalability: bool,
    /// Bits of `vop_time_increment`.
    pub time_increment_bits: u32,
    pub(crate) complexity: Option<Complexity>,
}

impl VolHeader {
    /// Macroblocks across.
    pub fn mb_width(&self) -> usize {
        self.width.div_ceil(16) as usize
    }

    /// Macroblocks down.
    pub fn mb_height(&self) -> usize {
        self.height.div_ceil(16) as usize
    }

    /// Refuses the tools this crate does not implement, naming them.
    pub(crate) fn check_supported(&self) -> Result<()> {
        if self.interlaced {
            return Err(unsupported("interlaced coding"));
        }
        if !self.obmc_disable {
            return Err(unsupported("overlapped block motion compensation"));
        }
        if self.sprite == SpriteMode::Static {
            return Err(unsupported("static sprites"));
        }
        if self.sprite == SpriteMode::Gmc && self.sprite_brightness_change {
            return Err(unsupported("sprite brightness change"));
        }
        if self.reversible_vlc {
            return Err(unsupported("reversible VLCs"));
        }
        if self.newpred {
            return Err(unsupported("NEWPRED"));
        }
        if self.scalability {
            return Err(unsupported("scalability"));
        }
        Ok(())
    }
}

/// Bits needed for `vop_time_increment` at a resolution (at least one).
pub(crate) fn time_increment_bits(resolution: u32) -> u32 {
    let mut bits = 1;
    while (1u64 << bits) < resolution as u64 {
        bits += 1;
    }
    bits
}

/// Reads a quantiser matrix: up to 64 bytes in zigzag order, a zero ending
/// it early and the last value repeating to the end.
fn read_matrix(r: &mut BitReader) -> Result<[u8; 64]> {
    let mut m = [0u8; 64];
    let mut last = 0u8;
    let mut i = 0;
    while i < 64 {
        let v = r.read(8)? as u8;
        if v == 0 {
            break;
        }
        m[ZIGZAG[i] as usize] = v;
        last = v;
        i += 1;
    }
    if i == 0 {
        return Err(invalid("a quantiser matrix begins with zero"));
    }
    for &z in &ZIGZAG[i..] {
        m[z as usize] = last;
    }
    Ok(m)
}

/// Visual object header fields the VOL depends on.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct VisualObject {
    pub verid: u8,
}

/// `VisualObject()` after its start code.
pub(crate) fn parse_visual_object(r: &mut BitReader) -> Result<VisualObject> {
    let mut verid = 1;
    if r.read_bit()? {
        verid = r.read(4)? as u8;
        r.read(3)?; // visual_object_priority
    }
    let ty = r.read(4)?;
    if ty != 1 {
        return Err(unsupported(format!("visual object type {ty} (only video, 1)")));
    }
    if r.read_bit()? {
        // video_signal_type
        r.read(3)?; // video_format
        r.read(1)?; // video_range
        if r.read_bit()? {
            r.read(24)?; // colour_primaries, transfer_characteristics, matrix_coefficients
        }
    }
    Ok(VisualObject { verid })
}

/// `VideoObjectLayer()` after its start code.
pub(crate) fn parse_vol(r: &mut BitReader, vo: VisualObject, pl: Option<u8>) -> Result<VolHeader> {
    r.read(1)?; // random_accessible_vol
    let object_type = r.read(8)? as u8;
    if object_type == 0x12 {
        return Err(unsupported("fine granularity scalability"));
    }
    let mut verid = if vo.verid == 0 { 1 } else { vo.verid };
    if r.read_bit()? {
        verid = r.read(4)? as u8;
        r.read(3)?; // video_object_layer_priority
    }
    let pixel_aspect = match r.read(4)? {
        1 => (1, 1),
        2 => (12, 11),
        3 => (10, 11),
        4 => (16, 11),
        5 => (40, 33),
        15 => (r.read(8)? as u8, r.read(8)? as u8),
        _ => (0, 0),
    };
    let mut low_delay = None;
    if r.read_bit()? {
        // vol_control_parameters
        let chroma_format = r.read(2)?;
        if chroma_format != 1 {
            return Err(unsupported(format!("chroma_format {chroma_format} (only 4:2:0)")));
        }
        low_delay = Some(r.read_bit()?);
        if r.read_bit()? {
            // vbv_parameters: bit rate, buffer size and occupancy, in halves.
            r.read(15)?;
            r.lenient_marker()?;
            r.read(15)?;
            r.lenient_marker()?;
            r.read(15)?;
            r.lenient_marker()?;
            r.read(3)?;
            r.read(11)?;
            r.lenient_marker()?;
            r.read(15)?;
            r.lenient_marker()?;
        }
    }
    let shape = r.read(2)?;
    if shape == 3 && verid != 1 {
        r.read(4)?; // video_object_layer_shape_extension
    }
    if shape != 0 {
        return Err(unsupported("arbitrary shape (video_object_layer_shape other than rectangular)"));
    }
    r.lenient_marker()?;
    let time_resolution = r.read(16)?;
    if time_resolution == 0 {
        return Err(invalid("vop_time_increment_resolution is zero"));
    }
    r.lenient_marker()?;
    let bits = time_increment_bits(time_resolution);
    let fixed_vop_time_increment = if r.read_bit()? { Some(r.read(bits)?) } else { None };
    r.lenient_marker()?;
    let width = r.read(13)?;
    r.lenient_marker()?;
    let height = r.read(13)?;
    r.lenient_marker()?;
    if width == 0 || height == 0 {
        return Err(invalid(format!("VOL size {width}x{height}")));
    }
    let interlaced = r.read_bit()?;
    let obmc_disable = r.read_bit()?;
    let sprite = match if verid == 1 { r.read(1)? } else { r.read(2)? } {
        0 => SpriteMode::None,
        1 => SpriteMode::Static,
        2 => SpriteMode::Gmc,
        v => return Err(invalid(format!("sprite_enable {v}"))),
    };
    let mut sprite_warping_points = 0;
    let mut sprite_warping_accuracy = 0;
    let mut sprite_brightness_change = false;
    if sprite != SpriteMode::None {
        if sprite != SpriteMode::Gmc {
            for _ in 0..4 {
                r.read(13)?; // sprite_width, height, left and top coordinates
                r.lenient_marker()?;
            }
        }
        sprite_warping_points = r.read(6)?;
        sprite_warping_accuracy = r.read(2)?;
        sprite_brightness_change = r.read_bit()?;
        if sprite != SpriteMode::Gmc {
            r.read(1)?; // low_latency_sprite_enable
        }
        if sprite_warping_points > 4 {
            return Err(invalid(format!("{sprite_warping_points} sprite warping points")));
        }
    }
    if r.read_bit()? {
        // not_8_bit
        let quant_precision = r.read(4)?;
        let bits_per_pixel = r.read(4)?;
        if quant_precision != 5 || bits_per_pixel != 8 {
            return Err(unsupported(format!(
                "{bits_per_pixel}-bit video (quant_precision {quant_precision})"
            )));
        }
    }
    let mpeg_quant = r.read_bit()?;
    let mut intra_matrix = DEFAULT_INTRA_MATRIX;
    let mut inter_matrix = DEFAULT_INTER_MATRIX;
    if mpeg_quant {
        if r.read_bit()? {
            intra_matrix = read_matrix(r)?;
        }
        if r.read_bit()? {
            inter_matrix = read_matrix(r)?;
        }
    }
    let quarter_sample = if verid != 1 { r.read_bit()? } else { false };
    let complexity = if r.read_bit()? { None } else { Some(Complexity::parse(r)?) };
    let resync_marker_disable = r.read_bit()?;
    let data_partitioned = r.read_bit()?;
    let reversible_vlc = if data_partitioned { r.read_bit()? } else { false };
    let mut newpred = false;
    let mut reduced_resolution = false;
    if verid != 1 {
        newpred = r.read_bit()?;
        if newpred {
            r.read(3)?; // requested_upstream_message_type, newpred_segment_type
        }
        reduced_resolution = r.read_bit()?;
    }
    let scalability = r.read_bit()?;
    Ok(VolHeader {
        profile_and_level: pl,
        object_type,
        verid,
        width,
        height,
        pixel_aspect,
        time_resolution,
        fixed_vop_time_increment,
        low_delay,
        interlaced,
        obmc_disable,
        sprite,
        sprite_warping_points,
        sprite_warping_accuracy,
        sprite_brightness_change,
        mpeg_quant,
        intra_matrix,
        inter_matrix,
        quarter_sample,
        resync_marker_disable,
        data_partitioned,
        reversible_vlc,
        newpred,
        reduced_resolution,
        scalability,
        time_increment_bits: bits,
        complexity,
    })
}

/// `group_of_vop()` after its start code: the time code in seconds.
pub(crate) fn parse_gov(r: &mut BitReader) -> Result<u32> {
    let h = r.read(5)?;
    let m = r.read(6)?;
    r.lenient_marker()?;
    let s = r.read(6)?;
    r.read(2)?; // closed_gov, broken_link
    Ok(h * 3600 + m * 60 + s)
}

/// The fields of a VOP header the decoding process uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VopHeader {
    pub vop_type: VopType,
    pub modulo_time_base: u32,
    pub time_increment: u32,
    pub coded: bool,
    pub rounding: bool,
    pub intra_dc_vlc_thr: u32,
    pub quant: u32,
    pub fcode_forward: u32,
    pub fcode_backward: u32,
    /// Sprite / GMC warping vectors `(du, dv)` per point.
    pub warping: Vec<(i32, i32)>,
}

fn vop_type_of(v: u32) -> VopType {
    match v {
        0 => VopType::I,
        1 => VopType::P,
        2 => VopType::B,
        _ => VopType::S,
    }
}

/// `warping_mv_code()`: a `dmv_length` VLC, that many bits coded like a DC
/// differential, and a marker bit.
fn warping_mv_code(r: &mut BitReader) -> Result<i32> {
    let len = vlc::dmv_length().decode(r)?;
    let v = if len == 0 {
        0
    } else {
        let code = r.read(len)? as i32;
        if code >> (len - 1) == 1 { code } else { code - ((1 << len) - 1) }
    };
    r.lenient_marker()?;
    Ok(v)
}

/// `sprite_trajectory()`.
pub(crate) fn parse_sprite_trajectory(r: &mut BitReader, points: u32) -> Result<Vec<(i32, i32)>> {
    (0..points)
        .map(|_| {
            let du = warping_mv_code(r)?;
            let dv = warping_mv_code(r)?;
            Ok((du, dv))
        })
        .collect()
}

/// `VideoObjectPlane()` up to the first macroblock, after the start code.
pub(crate) fn parse_vop(r: &mut BitReader, vol: &VolHeader) -> Result<VopHeader> {
    let vop_type = vop_type_of(r.read(2)?);
    let mut modulo_time_base = 0;
    while r.read_bit()? {
        modulo_time_base += 1;
        if modulo_time_base > 3600 {
            return Err(invalid("modulo_time_base runs on"));
        }
    }
    r.lenient_marker()?;
    let time_increment = r.read(vol.time_increment_bits)?;
    r.lenient_marker()?;
    let mut h = VopHeader {
        vop_type,
        modulo_time_base,
        time_increment,
        coded: r.read_bit()?,
        rounding: false,
        intra_dc_vlc_thr: 0,
        quant: 0,
        fcode_forward: 1,
        fcode_backward: 1,
        warping: Vec::new(),
    };
    if !h.coded {
        return Ok(h);
    }
    if vop_type == VopType::S && vol.sprite != SpriteMode::Gmc {
        return Err(unsupported("S-VOP without global motion compensation"));
    }
    if vop_type == VopType::P || (vop_type == VopType::S && vol.sprite == SpriteMode::Gmc) {
        h.rounding = r.read_bit()?;
    }
    if vol.reduced_resolution && matches!(vop_type, VopType::I | VopType::P) && r.read_bit()? {
        return Err(unsupported("reduced-resolution VOPs"));
    }
    if let Some(c) = &vol.complexity {
        r.skip(c.vop_bits(vop_type))?;
    }
    h.intra_dc_vlc_thr = r.read(3)?;
    if vop_type == VopType::S && vol.sprite_warping_points > 0 {
        h.warping = parse_sprite_trajectory(r, vol.sprite_warping_points)?;
    }
    h.quant = r.read(5)?;
    if h.quant == 0 {
        return Err(invalid("vop_quant is zero"));
    }
    if vop_type != VopType::I {
        h.fcode_forward = r.read(3)?;
        if h.fcode_forward == 0 {
            return Err(invalid("vop_fcode_forward is zero"));
        }
    }
    if vop_type == VopType::B {
        h.fcode_backward = r.read(3)?;
        if h.fcode_backward == 0 {
            return Err(invalid("vop_fcode_backward is zero"));
        }
    }
    Ok(h)
}

// ---------------------------------------------------------------- writing

/// Writes a start code (`00 00 01 xx`); the writer must be aligned.
pub(crate) fn put_start_code(w: &mut BitWriter, code: u8) {
    w.put_bytes(&[0, 0, 1, code]);
}

/// The configuration the encoder writes into its VOL.
pub(crate) struct VolParams {
    pub profile_and_level: u8,
    pub width: u32,
    pub height: u32,
    pub time_resolution: u32,
    pub fixed_increment: Option<u32>,
    pub resync_markers: bool,
}

/// Visual object sequence, visual object and video object layer headers:
/// what goes in an MP4 `esds` as decoder specific info.
pub(crate) fn write_config(p: &VolParams) -> Vec<u8> {
    let mut w = BitWriter::new();
    put_start_code(&mut w, sc::VOS);
    w.put(8, p.profile_and_level as u32);
    put_start_code(&mut w, sc::VISUAL_OBJECT);
    w.put(1, 0); // is_visual_object_identifier
    w.put(4, 1); // visual_object_type: video
    w.put(1, 0); // video_signal_type
    w.stuff();
    put_start_code(&mut w, sc::VO_FIRST);
    put_start_code(&mut w, sc::VOL_FIRST);
    w.put(1, 0); // random_accessible_vol
    w.put(8, 1); // video_object_type_indication: Simple
    w.put(1, 0); // is_object_layer_identifier
    w.put(4, 1); // aspect_ratio_info: square samples
    w.put(1, 1); // vol_control_parameters
    w.put(2, 1); // chroma_format 4:2:0
    w.put(1, 1); // low_delay
    w.put(1, 0); // vbv_parameters
    w.put(2, 0); // video_object_layer_shape: rectangular
    w.put(1, 1);
    w.put(16, p.time_resolution);
    w.put(1, 1);
    let bits = time_increment_bits(p.time_resolution);
    match p.fixed_increment {
        Some(inc) => {
            w.put(1, 1);
            w.put(bits, inc);
        }
        None => w.put(1, 0),
    }
    w.put(1, 1);
    w.put(13, p.width);
    w.put(1, 1);
    w.put(13, p.height);
    w.put(1, 1);
    w.put(1, 0); // interlaced
    w.put(1, 1); // obmc_disable
    w.put(1, 0); // sprite_enable
    w.put(1, 0); // not_8_bit
    w.put(1, 0); // quant_type: H.263
    w.put(1, 1); // complexity_estimation_disable
    w.put(1, !p.resync_markers as u32); // resync_marker_disable
    w.put(1, 0); // data_partitioned
    w.put(1, 0); // scalability
    w.stuff();
    w.into_bytes()
}

/// A VOP header up to the first macroblock.
pub(crate) fn write_vop_header(
    w: &mut BitWriter,
    time_increment_bits: u32,
    h: &VopHeader,
) {
    put_start_code(w, sc::VOP);
    w.put(
        2,
        match h.vop_type {
            VopType::I => 0,
            VopType::P => 1,
            VopType::B => 2,
            VopType::S => 3,
        },
    );
    for _ in 0..h.modulo_time_base {
        w.put(1, 1);
    }
    w.put(1, 0);
    w.put(1, 1);
    w.put(time_increment_bits, h.time_increment);
    w.put(1, 1);
    w.put(1, h.coded as u32);
    if !h.coded {
        return;
    }
    if h.vop_type == VopType::P {
        w.put(1, h.rounding as u32);
    }
    w.put(3, h.intra_dc_vlc_thr);
    w.put(5, h.quant);
    if h.vop_type != VopType::I {
        w.put(3, h.fcode_forward);
    }
    if h.vop_type == VopType::B {
        w.put(3, h.fcode_backward);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_increment_bit_counts() {
        assert_eq!(time_increment_bits(1), 1);
        assert_eq!(time_increment_bits(2), 1);
        assert_eq!(time_increment_bits(25), 5);
        assert_eq!(time_increment_bits(30), 5);
        assert_eq!(time_increment_bits(32), 5);
        assert_eq!(time_increment_bits(33), 6);
        assert_eq!(time_increment_bits(30000), 15);
        assert_eq!(time_increment_bits(65535), 16);
    }

    #[test]
    fn config_round_trips() {
        let p = VolParams {
            profile_and_level: 3,
            width: 352,
            height: 288,
            time_resolution: 30000,
            fixed_increment: Some(1001),
            resync_markers: true,
        };
        let b = write_config(&p);
        assert_eq!(&b[..5], &[0, 0, 1, 0xb0, 3]);
        let units = crate::dec::split_units(&b);
        let mut vo = VisualObject::default();
        let mut vol = None;
        for (code, body) in units {
            let mut r = BitReader::new(body);
            match code {
                sc::VISUAL_OBJECT => vo = parse_visual_object(&mut r).unwrap(),
                sc::VOL_FIRST => vol = Some(parse_vol(&mut r, vo, Some(3)).unwrap()),
                _ => {}
            }
        }
        let vol = vol.unwrap();
        assert_eq!((vol.width, vol.height), (352, 288));
        assert_eq!(vol.time_resolution, 30000);
        assert_eq!(vol.fixed_vop_time_increment, Some(1001));
        assert_eq!(vol.low_delay, Some(true));
        assert!(!vol.resync_marker_disable);
        assert!(!vol.mpeg_quant);
        assert_eq!(vol.object_type, 1);
        vol.check_supported().unwrap();
    }

    #[test]
    fn matrix_load_repeats_the_last_value() {
        let mut w = BitWriter::new();
        for v in [8u32, 16, 20, 0] {
            w.put(8, v);
        }
        let b = w.into_bytes();
        let m = read_matrix(&mut BitReader::new(&b)).unwrap();
        assert_eq!(m[0], 8);
        assert_eq!(m[1], 16);
        assert_eq!(m[8], 20);
        assert!(m[16..].iter().all(|&v| v == 20));
    }
}
