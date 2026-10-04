//! An MPEG-4 Part 2 Visual (ISO/IEC 14496-2) decoder and encoder.
//!
//! Rust, no C, no system libraries, no build script. Written from the
//! standard — clause 6 (syntax and semantics), clause 7 (the decoding
//! process) and Annex B (the variable length codes) — and, for the short
//! video header, ITU-T H.263's baseline, which 14496-2 incorporates. It is
//! not a translation of any other implementation.
//!
//! - [`Decoder`]: Simple and Advanced Simple Profile streams — I-, P-, B-
//!   and S-VOPs, AC / DC prediction, both inverse quantisers, four-vector
//!   macroblocks, unrestricted and quarter-sample motion vectors, global
//!   motion compensation, interlaced field DCT and field prediction, video
//!   packets, data partitioning with reversible VLCs (read backwards after
//!   damage), OBMC, DivX-style packed bitstreams and the H.263 short video
//!   header (with H.263's Advanced Prediction mode) — to 8-bit 4:2:0
//!   [`Frame`]s in display order, with damage concealed and counted
//!   ([`DecoderStats`]).
//! - [`Encoder`]: Simple Profile I- and P-VOPs and, on request, Advanced
//!   Simple tools — B-VOPs, quarter-sample motion, the MPEG quantiser with
//!   matrices, interlaced coding — with four-vector macroblocks, AC / DC
//!   prediction, video packets, data partitioning and reversible VLCs,
//!   OBMC, the short video header, forced I-VOPs, `video_signal_type()`,
//!   and constant-quantiser or bit-rate control.
//!
//! What is not implemented is refused with [`Error::Unsupported`], naming
//! the tool; the crate README lists them.
//!
//! ```
//! use mpeg4::{Encoder, EncoderConfig, Decoder, Frame};
//!
//! let mut enc = Encoder::new(EncoderConfig::new(64, 48, 25))?;
//! let frame = Frame::new(64, 48);
//! let mut stream = enc.encode(&frame)?;
//! stream.extend(enc.finish()?);
//!
//! let mut dec = Decoder::new();
//! let mut frames = dec.decode(&stream)?;
//! frames.extend(dec.flush());
//! assert_eq!(frames.len(), 1);
//! assert_eq!((frames[0].width, frames[0].height), (64, 48));
//! # Ok::<(), mpeg4::Error>(())
//! ```

#![warn(missing_docs)]

pub(crate) mod bits;
mod dec;
mod dsp;
mod enc;
mod error;
mod frame;
pub(crate) mod gmc;
mod headers;
pub(crate) mod idct;
pub(crate) mod mbstate;
pub(crate) mod mc;
pub(crate) mod obmc;
pub(crate) mod picture;
pub(crate) mod quant;
pub(crate) mod tables;
pub(crate) mod vlc;

pub use dec::{Decoder, DecoderStats};
pub use dsp::kernel_level;
pub use enc::{Encoder, EncoderConfig, Quantiser, RateControl};
pub use error::{Error, Result};
pub use frame::{Frame, Plane, VopType};
pub use headers::{ColourDescription, SpriteMode, VideoSignal, VolHeader};
