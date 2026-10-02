# rivet-mpeg4

[![CI](https://github.com/rivet-transcoder/rivet-mpeg4/actions/workflows/ci.yml/badge.svg)](https://github.com/rivet-transcoder/rivet-mpeg4/actions/workflows/ci.yml)

An **MPEG-4 Part 2 Visual** (ISO/IEC 14496-2) decoder and encoder in Rust:
no C, no system libraries, no build script, nothing to install on a build
host. Written from the standard — and, for the short video header, from
ITU-T H.263's baseline, which 14496-2 incorporates — not translated from
any other implementation. The decoder takes the Simple and Advanced Simple
Profile streams DivX, Xvid and their contemporaries made (B-VOPs,
quarter-sample motion, global motion compensation, interlaced field DCT,
packed bitstreams), and every VOP of the 23 real-world streams it was
checked on parses to the bit, except where the files themselves are cut
short or damaged (the figures are [below](#how-it-is-checked)).

Written for the **[rivet](https://github.com/rivet-transcoder/rivet)**
transcoder, where it is the MPEG-4 Visual codec on both sides: the decoder
that lets a DivX / Xvid / 3GP source be transcoded without a GPU that takes
MPEG-4, and the encoder for targets that still want MPEG-4 Part 2. Usable on
its own by anything that has MP4 / AVI / Matroska access units or a raw
elementary stream and wants planar pictures back, or pictures and wants an
MPEG-4 stream.

Published as `rivet-mpeg4`; **imported as `mpeg4`** (`use mpeg4::…`). One
dependency (`thiserror`), no features, no build script.

```toml
[dependencies]
mpeg4 = { package = "rivet-mpeg4", git = "https://github.com/rivet-transcoder/rivet-mpeg4", branch = "develop" }
```

## What it decodes

| | supported | refused with `Error::Unsupported` |
|---|---|---|
| **Headers** | visual object sequence, visual object, video object layer (in band, or from an MP4 `esds` / AVI `strf` / Matroska `CodecPrivate` via `Decoder::with_config`), GOV, VOP; quantiser matrices; complexity estimation headers (skipped); user data (DivX packing and version strings) | arbitrary shape, scalability, NEWPRED, reduced-resolution VOPs, `not_8_bit`, OBMC, the Studio and FGS object types, static sprites, sprite brightness change |
| **Simple Profile** | I- and P-VOPs; intra AC / DC prediction; both inverse quantisers (H.263 and MPEG with matrices and mismatch control); the three escape modes; half-sample, four-vector and unrestricted motion vectors (`vop_fcode` 1–7); video packets with resync markers and header extension; data partitioning; macroblock stuffing; not-coded VOPs | reversible VLCs (data partitioning with `reversible_vlc` set) |
| **Advanced Simple Profile** | B-VOPs (direct, interpolated, forward, backward; skipped macroblocks over not-coded ones); quarter-sample motion; global motion compensation (S-VOPs, one to three warping points, at any `sprite_warping_accuracy`); interlaced coding — field DCT, field prediction in P-, S- and B-VOPs, the alternate vertical scan | four-point (perspective) GMC; field direct mode (a direct-mode B macroblock over a field-predicted one) |
| **Streams** | one access unit per call, or any buffer of whole start-code units (a raw `.m4v` read in one go); DivX-style packed bitstreams (a P-VOP and its B-VOP in one access unit, then a placeholder) | — |
| **Short video header** | H.263 baseline pictures (sub-QCIF to 16CIF), GOB headers with and without stuffing, the H.263 escape | H.263 optional modes (UMV, SAC, AP, PB), PLUSPTYPE (H.263 version 2), CPM |

Output is 8-bit 4:2:0 planar [`Frame`](src/frame.rs)s — Y, then Cb, then
Cr in one buffer, tightly packed, cropped to the VOL's width and height —
in **display order**: an I- or P-VOP is held until the next one arrives (or
`flush`), unless the VOL declares `low_delay`. Each frame carries its time
(`modulo_time_base` and `vop_time_increment` accumulated, in ticks of
`vop_time_increment_resolution`), its VOP type and its decode index.

Damage inside a VOP is concealed rather than returned: the macroblocks from
the failure to the next video packet are copied from the reference, the
frame comes back with `concealed` set, and `Decoder::last_error` says what
was wrong. `Decoder::stats` counts VOPs decoded, concealed, dropped
(packed-stream placeholders, B-VOPs without references) and *misaligned*
(decoded without error, but the macroblock data did not end where the
VOP's stuffing begins). Header errors and unsupported tools are errors.

### Encoders that depart from the standard

Each was found by one of the sample streams failing to parse, and each
applies only where the standard's reading fails ([docs/SAMPLES.md](docs/SAMPLES.md)
has the detail):

- a VOL whose `vop_time_increment_resolution` does not match the
  increments its VOPs code (libavcodec 54) — the increment length is found
  from the marker bits;
- a verid 2 VOL without the verid 2 fields (OpenDivX);
- DivX 5.00's GMC trajectory: a marker per warping point rather than per
  code, and vectors in `1/s` samples rather than half samples;
- early Xvid's interlaced P-VOPs, which code `dct_type` in macroblocks
  without coded blocks;
- padding after a VOP's stuffing (DivX 5's extra `0x7f`, runs of ones and
  zeros) or no stuffing at all when the data ends byte aligned.

## What it encodes

Simple Profile I- and P-VOPs, and, with `EncoderConfig::b_frames`,
Advanced Simple B-VOPs:

- motion: a predictive diamond search over whole samples, then half-sample
  refinement, with a rate term from the actual vector code lengths;
  optional four-vector macroblocks; `vop_fcode` from the search range;
- macroblock decisions: intra by TMN's rule (deviation from the mean
  against the prediction error), not-coded when the vector is zero and
  nothing quantises to a coefficient; B macroblocks choose among direct,
  forward, backward and interpolated on prediction error plus vector cost;
- texture: the H.263 quantiser, intra DC and AC prediction (AC prediction
  per macroblock when it shrinks the coefficients), all three escape modes;
- rate: a constant quantiser (B-VOPs a quarter higher) or a bit-rate target,
  the quantiser moved per VOP by the running error;
- optional video packets (resync markers) of a given size;
- alternating `vop_rounding_type`, so P-VOP prediction does not drift in
  one direction.

`Encoder::encode` returns each call's VOPs in decode order — one per frame
without B-VOPs; with them, nothing while a frame waits to be a B-VOP, then
the next reference and the B-VOPs before it — and the first call's bytes
begin with the configuration headers (`Encoder::config`, the decoder
specific info for an `esds`). The encoder reconstructs with the decoder's
own prediction, inverse quantisation and IDCT, and
`EncoderConfig::keep_reconstructions` hands those pictures back.

Not encoded: quarter-sample motion, GMC, interlaced coding, the MPEG
quantiser, data partitioning, the short video header. The decoder handles
all of them.

## How it is checked

No other implementation is run, in tests or in CI.

- **Tables** (`tables.rs`, `vlc.rs`): every VLC is a prefix code with the
  Kraft sum the standard's start-code emulation leaves free; the intra
  TCOEF table uses exactly the inter table's 102 codewords; every coded
  event lies within LMAX, and the events per run are exactly what LMAX and
  RMAX describe; every codeword decodes to its own event; the scans are
  permutations, the zigzag walks anti-diagonals and the alternate
  horizontal scan is the alternate vertical one transposed.
- **Escape modes and DC differentials** from hand-assembled bits: type 1
  (level offset by LMAX), type 2 (run offset by RMAX + 1), type 3 with its
  markers, H.263's 8-bit escape and its forbidden levels.
- **The IDCT meets IEEE Std 1180-1990** (`idct.rs`, the standard's random
  generator, 10 000 blocks per run, six runs): peak error 1 in every run;
  worst per-pixel mean square error 0.0025 (limit 0.06), overall mean
  square error 0.0017 (0.02), per-pixel mean error 0.0010 (0.015), overall
  mean error 0.00007 (0.0015); zero in, zero out.
- **Inverse quantisation**: both methods against values worked by hand,
  including saturation, truncation toward zero and mismatch control.
- **Prediction**: DC direction and unavailable neighbours, AC rescaling
  (`//`), vector prediction at picture and packet edges, chroma vector
  rounding (Tables 7-6 / 7-7), wrapping by `vop_fcode`, field DCT layout
  and field prediction; the quarter-sample filter against a plain
  reference; GMC translations against ordinary motion compensation.
- **A hand-built short-header sequence**: GOB headers with and without
  stuffing, INTRADC, skipped and moved macroblocks.
- **Round trips** (`tests/roundtrip.rs`): the decoder must reproduce the
  encoder's reconstruction **byte for byte** (any difference is a desync
  between the two sides), and the PSNR against the source is gated loosely.
  On the synthetic test sequence (QCIF unless noted): all-intra 47.8 dB at
  quantiser 2, 42.4 at 5, 29.3 at 31; I and P-VOPs at quantiser 5 42.7 dB
  at a fifth of the all-intra size; four vectors 42.7; with B-VOPs 42.6–42.8
  and about 20% smaller than IPPP; video packets of 300 bytes at CIF 43.4;
  odd sizes down to 1x1; bit-rate targets of 100, 300 and 700 kb/s at CIF
  land at 112, 332 and 765 kb/s.
- **Real encoders' streams** (`tests/samples.rs`, fetched by
  `tools/fetch-samples.sh`; CI fetches them): 23 streams from DivX 5.00,
  5.01, 5.03 and 6.6, Xvid, OpenDivX, libavcodec and others — Simple and
  Advanced Simple, B-VOPs, packed streams, quarter-sample, two- and
  three-point GMC, interlaced field DCT, data partitioning, video packets,
  the MPEG quantiser, the short video header, sizes that are not multiples
  of 16. **Every VOP of every stream parses exactly to its stuffing**,
  except where the file itself is cut short or damaged (two files cut off
  mid-VOP, two damaged pictures in an H.263 call capture); pictures were
  inspected for drift. There is no reference output for these streams, so
  this checks syntax, VLCs and decisions exactly and reconstruction by eye.
  [docs/SAMPLES.md](docs/SAMPLES.md) lists them, their sources, and what
  each taught.
- **Malformed input**: property tests (`tests/fuzz.rs`, proptest; CI also
  runs them in a debug build, where overflow panics) feed arbitrary bytes,
  and valid streams with bits flipped, bytes cut and garbage spliced, to
  every entry point, and the sample streams damaged likewise: errors or
  concealment, never a panic.

Single-threaded decode speed, release build, AMD Ryzen 9 9950X: 640x480
Main profile with the MPEG quantiser about 1450 frames/s; 720x480 Xvid with
B-VOPs about 750; 640x408 DivX quarter-sample about 680; 856x472 Xvid GMC
with quarter-sample about 440.

### Points the standard leaves to a reading

Where the text allows more than one reading, or I could not settle one
from the standard alone, this is what the code does and what decided it:

- *Reference padding.* Motion compensation repeats the edge of the decoded
  macroblock grid, not of the VOP's `width` x `height`. Decided by DivX 5
  streams of such sizes, which drift otherwise.
- *Quarter-sample interpolation.* The 8-tap filter mirrors taps at the
  edge of the block's (N+1)-sample window repeating the edge sample; the
  diagonal quarter positions average four neighbours; chroma vectors halve
  the luma vector (toward zero) before the half-sample rule. Every
  quarter-sample stream above reconstructs without visible drift.
- *GMC.* `du`, `dv` in half samples; the warp through virtual points at
  `W'`, `H'`; `///` rounding halves upward; a GMC macroblock's vector for
  prediction is the rounded mean of its luminance displacements, **clipped
  to the `vop_fcode` range** — without the clip, an Xvid zoom whose warp
  outruns that range wraps its neighbours' vectors into misplaced blocks
  (the stream is named for that artifact; with the clip it is clean). Two-
  and three-point streams (DivX 5.01, Xvid) reconstruct cleanly.
- *Running QP for `intra_dc_vlc_thr`*: the previous coded macroblock's
  quantiser, the current one's at the start of a VOP or packet.
- *B-VOP over an S-VOP's not-coded macroblock*: coded normally, the GMC
  vector as the co-located one (the skipped reading misparses DivX 5).
- *Resync marker length in B-VOPs*: 16 + max(`vop_fcode_forward`,
  `vop_fcode_backward`) bits.
- *Field prediction* (unexercised by any sample: the interlaced stream
  above uses field DCT only): field vectors are predicted from the frame
  predictor with its vertical component halved (arithmetic shift), and
  contribute the mean of their horizontal and the sum of their vertical
  components to later prediction.
- *Complexity estimation*: which statistics each VOP type carries
  (unexercised).

## Provenance and licensing

Written from ISO/IEC 14496-2 and ITU-T H.263 and published literature
(IEEE Std 1180-1990 for the IDCT test); **no MPEG-4 Part 2 or H.263
implementation's source was read** — not FFmpeg's, Xvid, DivX, the MPEG-4
reference software or any other — and no other decoder was run, in
development or in tests. The VLC tables, scans and default matrices are the
standard's data, transcribed. The sample streams are other encoders'
output, used as data only.

**Patents.** MPEG-4 Visual may be subject to patent licensing in some
jurisdictions; Via LA administers a licensing programme for it. Nothing
here is a licence to any patent, and the authors make no claim about
whether anyone needs one.

## Using it

```rust
// Access units from MP4 / Matroska: the decoder specific info first.
let mut dec = mpeg4::Decoder::with_config(&esds_dsi)?;
for au in access_units {
    for frame in dec.decode(au)? {
        // frame.plane(0) / (1) / (2): Y, Cb, Cr; frame.timestamp in
        // ticks of frame.time_base; frame.vop_type; frame.concealed
    }
}
let rest = dec.flush();

// A raw elementary stream (.m4v, .cmp, H.263): any buffer of whole units.
let mut dec = mpeg4::Decoder::new();
let mut frames = dec.decode(&bytes)?;
frames.extend(dec.flush());

// Encoding: 4:2:0 frames in, VOPs out in decode order.
let mut cfg = mpeg4::EncoderConfig::new(640, 480, 25);
cfg.rate = mpeg4::RateControl::Bitrate(1_500_000);
cfg.b_frames = 2;
let mut enc = mpeg4::Encoder::new(cfg)?;
let dsi = enc.config().to_vec();
let mut stream = Vec::new();
for frame in &frames {
    stream.extend(enc.encode(frame)?);
}
stream.extend(enc.finish()?);
```

`examples/m4vdec.rs` decodes an AVI or raw stream, reports counts and
timing, and writes chosen frames as PNG.

## License

Open Encoding Attribution License v1.0 — a source-available (not OSI
open-source) license, royalty-free, with a commercial-attribution
requirement. See [LICENSE.md](LICENSE.md) and [NOTICE](NOTICE).
