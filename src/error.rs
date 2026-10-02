//! The crate's one error type.

/// What went wrong. Every malformed input comes back as one of these; neither
/// the decoder nor the encoder panics on bytes or frames it is given.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The bitstream breaks the syntax or the semantics of ISO/IEC 14496-2:
    /// a header field out of range, a codeword that matches nothing, data
    /// that runs past the end of the buffer, a VOP before any VOL header.
    #[error("invalid MPEG-4 Visual data: {0}")]
    Invalid(String),
    /// Valid MPEG-4 Visual this crate does not implement, named: arbitrary
    /// shape, interlaced coding, sprites, scalability, reversible VLCs and
    /// the other tools the README lists as absent. A caller with another
    /// decoder available can hand the stream to it.
    #[error("unsupported MPEG-4 Visual feature: {0}")]
    Unsupported(String),
    /// A configuration or input the caller gave that cannot be coded: a
    /// frame size of zero or above the standard's 13-bit limit, a frame
    /// whose planes do not match its dimensions, a time base out of range.
    #[error("invalid MPEG-4 encoder configuration: {0}")]
    Config(String),
}

#[cold]
#[inline(never)]
pub(crate) fn invalid(msg: impl Into<String>) -> Error {
    Error::Invalid(msg.into())
}

#[cold]
#[inline(never)]
pub(crate) fn unsupported(msg: impl Into<String>) -> Error {
    Error::Unsupported(msg.into())
}

#[cold]
#[inline(never)]
pub(crate) fn config(msg: impl Into<String>) -> Error {
    Error::Config(msg.into())
}

/// `Result` with this crate's [`Error`].
pub type Result<T> = std::result::Result<T, Error>;
