# rivet-mpeg4

[![CI](https://github.com/safewords/rivet-mpeg4/actions/workflows/ci.yml/badge.svg)](https://github.com/safewords/rivet-mpeg4/actions/workflows/ci.yml)

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

Written for the **[rivet](https://github.com/safewords/rivet)**
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
mpeg4 = { package = "rivet-mpeg4", git = "https://github.com/safewords/rivet-mpeg4", branch = "develop" }
```

## What it decodes

| | supported | refused with `Error::Unsupported` |
|---|---|---|
| **Headers** | visual object sequence, visual object (with `video_signal_type()`: format, range, colour description, reported on `VolHeader::video_signal`), video object layer (in band, or from an MP4 `esds` / AVI `strf` / Matroska `CodecPrivate` via `Decoder::with_config`), GOV, VOP; quantiser matrices; complexity estimation headers (skipped); user data (DivX packing and version strings) | arbitrary shape, scalability, NEWPRED, reduced-resolution VOPs, `not_8_bit`, the Studio and FGS object types, static sprites, sprite brightness change, interlaced coding together with data partitioning |
| **Simple Profile** | I- and P-VOPs; intra AC / DC prediction; both inverse quantisers (H.263 and MPEG with matrices and mismatch control); the three escape modes; half-sample, four-vector and unrestricted motion vectors (`vop_fcode` 1–7); video packets with resync markers and header extension; data partitioning; **reversible VLCs**, read forwards and, after damage, backwards with Annex E's recovery; macroblock stuffing; not-coded VOPs; **OBMC** (`obmc_disable` 0, 7.6.6) | — |
| **Advanced Simple Profile** | B-VOPs (direct, interpolated, forward, backward; skipped macroblocks over not-coded ones); quarter-sample motion; global motion compensation (S-VOPs, one to **four** warping points — four is the perspective warp — at any `sprite_warping_accuracy`); interlaced coding — field DCT, field prediction in P-, S- and B-VOPs, **field direct mode**, the alternate vertical scan | — |
| **Streams** | one access unit per call, or any buffer of whole start-code units (a raw `.m4v` read in one go); DivX-style packed bitstreams (a P-VOP and its B-VOP in one access unit, then a placeholder) | — |
| **Short video header** | H.263 baseline pictures (sub-QCIF to 16CIF), GOB headers with and without stuffing, the H.263 escape; beyond 14496-2's subset, H.263's **Advanced Prediction mode** (Annex F: OBMC and four vectors) | H.263's Unrestricted Motion Vector, Syntax-based Arithmetic Coding and PB-frames modes, PLUSPTYPE (H.263 version 2), CPM |

Why the refusals remain: arbitrary shape, scalability, static sprites
(and brightness change), NEWPRED, reduced resolution, `not_8_bit` and the
Studio / FGS object types are whole further toolsets outside Simple and
Advanced Simple, unimplemented; interlaced coding with data partitioning
is excluded by 14496-2 itself (Annex G's table of tool combinations, note
e: "Interlace does not support Data Partitioning nor RVLC"); H.263's other optional modes and
version 2 are not part of MPEG-4's short video header.

Output is 8-bit 4:2:0 planar [`Frame`](src/frame.rs)s — Y, then Cb, then
Cr in one buffer, tightly packed, cropped to the VOL's width and height —
in **display order**: an I- or P-VOP is held until the next one arrives (or
`flush`), unless the VOL declares `low_delay`. Each frame carries its time
(`modulo_time_base` and `vop_time_increment` accumulated, in ticks of
`vop_time_increment_resolution`), its VOP type and its decode index.

Damage inside a VOP is concealed rather than returned: the macroblocks from
the failure to the next video packet are copied from the reference, the
frame comes back with `concealed` set, and `Decoder::last_error` says what
was wrong. With reversible VLCs, a damaged texture partition is also read
backwards from its packet's end, and the macroblocks both directions
recovered are kept as Annex E.1.4.4.2.1's four strategies allow (the rest
are predicted from their intact motion, or in I-VOPs rebuilt from their
DC); `DecoderStats::rvlc_backward_mbs` and `rvlc_discarded_mbs` count them. `Decoder::stats` counts VOPs decoded, concealed, dropped
(packed-stream placeholders, B-VOPs without references) and *misaligned*
(decoded without error, but the macroblock data did not end where the
VOP's stuffing begins). Header errors and unsupported tools are errors.

### Encoders that depart from the standard

Each was found by another encoder's stream failing to parse, and each
applies only where the standard's reading fails ([docs/CONFORMANCE.md](docs/CONFORMANCE.md)
has the detail; the streams that showed the first five are no longer
fetched and those that showed the last two are not kept, so only Xvid's
resync markers are still checked by an external stream):

- a VOL whose `vop_time_increment_resolution` does not match the
  increments its VOPs code (libavcodec 54) — the increment length is found
  from the marker bits;
- a verid 2 VOL without the verid 2 fields (OpenDivX);
- DivX 5.00's GMC trajectory: a marker per warping point rather than per
  code, and vectors in `1/s` samples rather than half samples;
- early Xvid's interlaced P-VOPs, which code `dct_type` in macroblocks
  without coded blocks;
- padding after a VOP's stuffing (DivX 5's extra `0x7f`, runs of ones and
  zeros) or no stuffing at all when the data ends byte aligned;
- Xvid's resync markers in B-VOPs, a bit longer than 6.3.5.2's when both
  `vop_fcode`s are 1;
- Xvid's and libavcodec's field direct mode, which predicts as though the
  co-located macroblock's field vectors were zero (7.7.2.2 scales them):
  applied when the stream's user data names either encoder (`XviD...`,
  `Lavc...`);
- libavcodec's video packets in B-VOPs that start after a run of
  macroblocks over not-coded ones (which have no bits), their resync
  marker in front of that run.

## What it encodes

Simple Profile I- and P-VOPs, and, with `EncoderConfig::b_frames`,
`quarter_sample`, `quantiser` or `interlaced`, Advanced Simple:

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
- optional video packets (resync markers) of a given size; **data
  partitioning**, with or without **reversible VLCs** (`data_partitioning`,
  `reversible_vlc`);
- **quarter-sample motion** (the search refines to quarter samples) and the
  **MPEG quantiser** with the default or custom matrices (written in the
  VOL);
- **interlaced coding**: frame or field DCT per macroblock, frame or field
  prediction (each field's vector from either reference field) in P-VOPs,
  either field order; on request field prediction in B-VOPs
  (`b_field_prediction`) and field direct mode (`field_direct`), both off
  by default because Xvid's decoder misreads the first and Xvid's and
  libavcodec's decoders read the second their own way (below);
- **OBMC** (`obmc`): `obmc_disable` 0 — the encoder plans a VOP's motion
  before predicting, since a macroblock's overlapped prediction needs its
  right neighbour's vectors. OBMC is outside the Simple and Advanced
  Simple profiles, so such a stream claims a profile it exceeds;
- the **short video header** (`short_header`): H.263 baseline pictures at
  the five standard sizes, vectors kept inside the picture, GOB headers in
  place of video packets; with `obmc`, H.263's Advanced Prediction mode
  (OBMC and four vectors), which makes it an H.263 stream outside
  14496-2's subset;
- `Encoder::force_keyframe` makes the next frame an I-VOP; `video_signal`
  writes `video_signal_type()` (format, range, colour primaries, transfer
  and matrix) in the visual object header — 14496-2 carries it there, not
  in the VOL;
- alternating `vop_rounding_type`, so P-VOP prediction does not drift in
  one direction.

`Encoder::encode` returns each call's VOPs in decode order — one per frame
without B-VOPs; with them, nothing while a frame waits to be a B-VOP, then
the next reference and the B-VOPs before it — and the first call's bytes
begin with the configuration headers (`Encoder::config`, the decoder
specific info for an `esds`). The encoder reconstructs with the decoder's
own prediction, inverse quantisation and IDCT, and
`EncoderConfig::keep_reconstructions` hands those pictures back.

Not encoded: GMC (S-VOPs), which the decoder handles; field prediction
in B-VOPs (their non-direct macroblocks use frame prediction).

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
- **Reversible VLCs** (Table B-23, transcribed from 14496-2:2001 and
  checked against the 1998 committee draft's event columns): every code is
  a *core* plus one free bit, the core ending at its second `1` (a leading
  `1`; such cores are palindromes) or third `0` (a leading `0`), a rule
  that reads the same in either direction; the codes are exactly the
  construction's cores in ascending order with both free bits, `0000`
  being the escape and only the longest length cut short; codes with
  their sign are prefix-free forwards and backwards; each column codes 169
  distinct events with no level gaps. Every table event, and escapes (long
  runs, levels to 2047, both signs, `last` or not), read back the same
  forwards and backwards with the backward reader ending where the event
  began; the illegal forms Annex E.1.4.4.1 lists (escaped level 0, an
  escaped event the table codes, a leading `0000 0`, a missing marker) are
  refused both ways. The four recovery strategies by hand.
- **RVLC damage** (`tests/error_resilience.rs`): this encoder's data-
  partitioned RVLC streams with 16 bits cleared mid-texture. A QCIF P-VOP
  in one packet (texture 1594 bits, forward error after 648 bits / 38
  macroblocks, backward error after 945 bits / 60 macroblocks: strategy
  1): 33 macroblocks kept forwards, 60 backwards, 6 concealed with their
  motion, and only 5 macroblocks differ from the encoder's reconstruction;
  an I-VOP (16604 bits, 45 / 53 macroblocks): 46 forwards, 53 backwards,
  one rebuilt from its DC; with 200-byte packets, the damage stays in its
  packet (one macroblock differs). Every other frame is exact.
- **GMC with four points**: a translation-only trajectory equals ordinary
  motion compensation sample for sample (luma and chroma); an affine one
  matches the three-point warp to one unit; a true perspective one puts
  the corners where the trajectory says and the centre where the
  diagonals cross; a hand-built S-VOP of not-coded macroblocks decodes to
  the warp of its reference.
- **Field direct mode** (7.7.2.3, Table 7-12) by hand: field distances for
  both field orders and reference parities, and the four vectors.
- **OBMC**: the weights sum to 8 everywhere; equal vectors give plain
  motion compensation; the remote-vector rules (picture edge and intra:
  own vector; not coded: zero; never the macroblock below).
- **Round trips** (`tests/roundtrip.rs`): the decoder must reproduce the
  encoder's reconstruction **byte for byte** (any difference is a desync
  between the two sides), and the PSNR against the source is gated loosely.
  On the synthetic test sequence (QCIF unless noted): all-intra 47.8 dB at
  quantiser 2, 42.4 at 5, 29.3 at 31; I and P-VOPs at quantiser 5 42.7 dB
  at a fifth of the all-intra size; four vectors 42.7; with B-VOPs 42.6–42.8
  and about 20% smaller than IPPP; video packets of 300 bytes at CIF 43.4;
  odd sizes down to 1x1; bit-rate targets of 100, 300 and 700 kb/s at CIF
  land at 112, 332 and 765 kb/s. Every round trip also requires each VOP
  to end exactly at its stuffing. The tools added on 2026-10-03, all byte
  exact: data partitioning (CIF, 4MV) 43.45 dB, 27.6 KB for 12 frames,
  28.9 KB with RVLC, 29.9 KB with RVLC and 300-byte packets; the MPEG
  quantiser 47.7 / 42.7 / 34.8 dB at quantisers 2 / 5 / 20 (default
  matrices; flat and custom ones too); quarter-sample 42.4 dB (with two
  B-VOPs 42.4); all Advanced Simple tools with data partitioning, RVLC and
  packets at CIF 43.5; short header 41.7 (sub-QCIF), 42.5 (QCIF), 43.3 (CIF
  with GOB headers) dB; interlaced CIF (synthetic interlaced content)
  41.4 dB at under a quarter of the progressive coding's size, with B-VOPs
  (field-predicted B macroblocks, both quantisers) 40.7–41.4 dB, with
  quarter-sample and field direct mode 40.9; OBMC 42.6–42.8 dB (one or four vectors, quarter-
  sample, packets); H.263 Advanced Prediction 42.5 / 42.8 dB (one / four
  vectors); forced I-VOPs land where asked, with and without B-VOPs.
- **Published conformance streams** (`tests/conformance.rs`, fetched by
  `tools/fetch-conformance.sh` from ITU-T's and ISO's servers; CI fetches
  them and requires them): baseline H.263 (the short video header) from
  ITU-T's H.263 bitstream archive against its published decoded pictures —
  **all 68 pictures within one sample** (the inverse DCTs' rounding), no
  drift; an H.263 version 2 stream (PLUSPTYPE) and an ISO/IEC 14496-4
  Simple Studio stream, refused by name. ISO's Simple and Advanced Simple
  conformance streams are not publicly available.
- **Xvid, as a black box** (`tools/xvid-vectors.sh` builds Xvid 1.3.7 from
  its release tarball and runs its example encoder and decoder; CI runs
  it): Xvid's streams for each Advanced Simple tool — B-VOPs, packed
  B-VOPs, quarter-sample, GMC, interlaced (both field orders), the MPEG
  quantiser, video packets, an odd size, quantisers 1 and 31 — decoded by
  this decoder against Xvid's own decoder's pictures, and this crate's
  encoder's streams decoded by Xvid's decoder against this decoder's. All
  agree to a sample or two (the inverse DCTs' rounding, 56–66 dB), and the
  quarter-sample streams of P-VOPs alone to five (about 50 dB: the 8-tap
  filter carries the rounding differences along a chain of P-VOPs).
  [docs/CONFORMANCE.md](docs/CONFORMANCE.md) has the figures, what the
  comparison found, and what the sample streams used until 2026-10-03
  covered that these do not.
- **Malformed input**: property tests (`tests/fuzz.rs`, proptest; CI also
  runs them in a debug build, where overflow panics) feed arbitrary bytes,
  and valid streams with bits flipped, bytes cut and garbage spliced, to
  every entry point, and the conformance and Xvid streams damaged likewise:
  errors or concealment, never a panic.

Single-threaded decode speed, release build, AMD Ryzen 9 9950X, measured
on other encoders' streams (2026-10-02, before the SIMD kernels below):
640x480 Main profile with the MPEG quantiser about 1450 frames/s; 720x480
Xvid with B-VOPs about 750; 640x408 DivX quarter-sample about 680; 856x472
Xvid GMC with quarter-sample about 440.

### Points the standard leaves to a reading

Where the text allows more than one reading, or I could not settle one
from the standard alone, this is what the code does and what decided it:

- *Reference padding.* Motion compensation repeats the edge of the decoded
  macroblock grid, not of the VOP's `width` x `height`. Decided by DivX 5
  streams of such sizes, which drift otherwise.
- *Quarter-sample interpolation* (7.6.2.2). Two passes, each rounded
  with `rounding_control` and clipped to eight bits before the next:
  every row of the block's (N+1)-sample window interpolated horizontally
  (the 8-tap filter for the half position, the average of the nearest
  integer and half-sample values for a quarter one), then every column of
  those values vertically the same way. The filter mirrors taps at the
  edge of the window, repeating the edge sample. Chroma vectors halve the
  luma vector (toward zero) before the half-sample rule. Until 2026-10-03
  the decoder averaged half-sample values on a two-dimensional grid
  instead (the diagonal quarter positions from four of them), which
  differed from Xvid by one at about a third of the samples at a
  horizontal quarter position combined with a vertical half or quarter
  one; the two-pass reading is the standard's and Xvid's, and agrees with
  Xvid to the inverse DCTs' rounding (docs/CONFORMANCE.md).
- *GMC.* `du`, `dv` in half samples; the warp through virtual points at
  `W'`, `H'`; `///` rounding halves upward; a GMC macroblock's vector for
  prediction is the rounded mean of its luminance displacements, **clipped
  to the `vop_fcode` range** — without the clip, an Xvid zoom whose warp
  outruns that range wraps its neighbours' vectors into misplaced blocks
  (the stream is named for that artifact; with the clip it is clean). Two-
  and three-point streams (DivX 5.01, Xvid) reconstructed cleanly; Xvid's
  GMC streams match Xvid's decoder to the IDCT's rounding.
- *Running QP for `intra_dc_vlc_thr`*: the previous coded macroblock's
  quantiser, the current one's at the start of a VOP or packet.
- *B-VOP over an S-VOP's not-coded macroblock*: coded normally, the GMC
  vector as the co-located one (the skipped reading misparses DivX 5).
- *Resync marker length in B-VOPs*: 16 + max(`vop_fcode_forward`,
  `vop_fcode_backward`) bits, and 18 where that gives 17 (Xvid's length).
- *Field prediction*: field vectors are predicted from the frame predictor
  with its vertical component halved by `/` (toward zero: -41 gives -20),
  and in P-VOPs contribute the mean of their horizontal and the sum of
  their vertical components to later prediction. In B-VOPs each direction
  keeps a predictor per field (7.7.2.2): a frame vector sets both, a field
  vector its own field's (in frame units), and a frame vector is predicted
  from the top field's. A field reads the frame's padding (7.6.4): below
  the picture the top field takes the frame's last line, above it the
  bottom field the first. Until 2026-10-03 the halving was an arithmetic
  shift, B-VOP field vectors shared one predictor that took the
  macroblock's frame vector, and each field was padded from its own edge
  lines; an interlaced B-VOP stream written with libavcodec (MPEG
  quantiser, field DCT and prediction, one B-VOP) had 50–85 wrong
  macroblocks in every B-VOP, and its P-VOPs drifted at the edges.
  Xvid's decoder reads this crate's P-VOPs the same way (the shift, or
  each field's own padding, breaks the agreement).
- *Field direct mode, as encoders write it*: Xvid's and libavcodec's
  decoders take the co-located field vectors as zero — `MVf` and `MVb`
  are `MVD` (`MVb` zero when `MVD` is), from the reference fields
  7.7.2.2 names — and their encoders code for that. The decoder follows
  them when the stream's user data names either; otherwise it scales the
  vectors as 7.7.2.2 does. The encoder uses field direct mode only with
  `EncoderConfig::field_direct`.
- *Video packets after uncoded B macroblocks*: a B-VOP macroblock over a
  not-coded one has no bits, so a resync marker in front of a run of them
  can belong to a packet whose `macroblock_number` is the run's end
  (libavcodec writes them so); those macroblocks are decoded (copied),
  not concealed.
- *Complexity estimation*: which statistics each VOP type carries
  (unexercised).
- *RVLC escape*: Table B-23 prints the escape as `0000s` at both ends and
  its syntax as `0000 1` … `0000 s`; E.1.4.4.1 calls a leading escape other
  than `0000 1` an error when reading backwards. Both directions here
  require `0000 1` in front, so they accept the same streams.
- *RVLC recovery*: `f_mb(S)` counts a macroblock "once one of its bits is
  decoded", so a strategy can name the macroblock in which the error was
  found; the kept counts are capped at the macroblocks each direction
  decoded whole (`N1`, `N2`). A partition read forwards without an error
  but not ending at the stuffing before the next resync marker is damaged
  (E.1.4.4.1's stuffing rule); when both directions read the whole
  partition (an error neither direction localised), strategy 4 keeps
  nothing. E.1.4.4.2.2's concealment of every intra macroblock of a damaged
  packet is applied in P- and S-VOPs; in I-VOPs, where it would discard
  everything, the strategies decide.
- *Field direct mode*: the text computes the backward prediction from
  `mvb[1]` for both fields, read here as `mvb[0]` for the top field and
  `mvb[1]` for the bottom (the forward one uses both); the `MVD[i]` of the
  backward rule as the one `MVD[0]`; Table 7-12's `d[i]` as the position of
  field `i` in its frame minus that of the field it was predicted from (the
  printed table, garbled in the copy read, agrees wherever legible);
  `Tframe` as the first B-VOP's distance from its past reference after the
  VOL.
- *OBMC across packets*: 7.6.6 bounds remote vectors only by the VOP and,
  in S-VOPs, by `mcsel`; video packet and GOB boundaries are crossed (as
  H.263 F.3 says outside slice mode).
- *Four-point GMC*: chroma uses 7.8.5's chroma formula with `Ic = 4 ic +
  1`; a zero denominator (disallowed) falls back to no warp.

### NEON on ARM hardware

CI runs on x86-64 Linux only, so the NEON (aarch64) code paths are not tested
there. They are verified by hand on ARM hardware (an aarch64 Linux machine,
or Apple silicon) after a change to them and before a release:

```sh
MPEG4_REQUIRE_SIMD=1 cargo test --release
MPEG4_FORCE_SCALAR=1 cargo test --release
```

The first run checks the NEON kernels bit-exact against the scalar ones; the
second runs the whole suite on the scalar kernels. The conformance and Xvid
streams can be run the same way, as the `conformance` job in
`.github/workflows/ci.yml` does.

## Speed

The sample-processing kernels (`src/dsp`) have SIMD versions chosen at
run time — SSE4.1 and AVX2 on x86-64, NEON on aarch64 — beside scalar
ones that define them: the 8x8 inverse and forward DCTs, quarter-sample
interpolation (the 8-tap filter, both passes, with its block-edge
mirroring), half-sample interpolation with rounding control, and the
encoder's SAD. Every level computes **the same integers** as the scalar
code (the transforms keep their exact arithmetic, splitting the second
pass's 40-bit products into two 32-bit halves), so pictures do not depend
on the CPU and the encoder's reconstruction stays the decoder's.
`MPEG4_FORCE_SCALAR=1` selects the scalar kernels; `mpeg4::kernel_level()`
names the level in use. The tests compare every kernel with the scalar
one on random and edge inputs at every level the CPU has, IEEE 1180 is
measured at every level, and CI decodes the conformance and Xvid streams
with both and requires every picture to be the same.

Elsewhere: the forward quantisers multiply by reciprocals (exact, checked
exhaustively), GMC warps of up to three points are evaluated as running
sums (the same integers) and, on AVX2, a block at a time in vector lanes
with gathered samples (2.4x the running sums; NEON has no gather),
whole-sample search candidates are compared with the reference in place,
and the per-macroblock allocations are gone.

The encoder uses threads (`EncoderConfig::threads`, 0 for one per core,
up to 16) and writes **the same stream for any count**: B-VOP macroblock
rows are independent (vector prediction restarts at each row) and are
coded in parallel; a P-VOP's motion search runs as a wavefront on other
threads (each macroblock once those above and to its upper right are
searched — all that vector prediction and the candidates read) while one
thread codes the macroblocks in order. Video packets end where the bits
fall, so a VOP with them is coded on one thread. The decoder is
single-threaded: a VOP's macroblocks depend on their neighbours, and
video packets, the only independent part, are rare. A reconstruction
thread behind the parser was tried (2026-10-04) and did not pay: with the
SIMD kernels reconstruction is a small part of decoding, parsing alone
ran at 590-600 frames/s on the 720p clips against 560-610 for the whole
decoder, and the pipelined decoder measured 520-585.

Release build, Ryzen 9 9950X, a natural 1620x1080 clip cropped or
extended to size, 30 frames; `sp`: Simple Profile, H.263 quantiser,
half-sample, I- and P-VOPs; `asp`: two B-VOPs, quarter-sample, four
vectors, MPEG quantiser (frames per second, 2026-10-04, fastest of five
runs; before = this crate before the SIMD kernels and threads):

| | before | scalar kernels | 1 thread | 16 threads |
|---|---:|---:|---:|---:|
| 720p `sp` encode | 60 | 75 | 171 | 232 |
| 720p `asp` encode | 13 | 13 | 78 | 263 |
| 1080p `sp` encode | 26 | 34 | 78 | 112 |
| 1080p `asp` encode | 5.0 | 5.5 | 38 | 130 |
| 720p `sp` decode | 316 | 420 | 675 | |
| 720p `asp` decode | 217 | 203 | 639 | |
| 1080p `sp` decode | 135 | 153 | 249 | |
| 1080p `asp` decode | 105 | 109 | 213 | |

`cargo run --release --example m4vbench -- CLIP.y4m 1280x720 30 asp 5 16`
reproduces a row (it also checks every decoded picture against the
encoder's reconstruction); `cargo test --release --lib kernel_speed --
--ignored --nocapture` times each kernel at every level (an IDCT 2.3x,
16x16 quarter-sample interpolation 15-24x, half-sample 13-20x on AVX2).

## Provenance and licensing

Written from ISO/IEC 14496-2 (the 2001 edition's text, and the 1998
committee draft's, for the reversible VLC table, the field direct, OBMC,
GMC and error-resilience clauses) and ITU-T H.263 (the 01/2005 edition,
for Annex F) and published literature (IEEE Std 1180-1990 for the IDCT
test); **no MPEG-4 Part 2 or H.263
implementation's source was read** — not FFmpeg's, Xvid, DivX, the MPEG-4
reference software or any other. The VLC tables, scans and default
matrices are the standard's data, transcribed. Xvid's encoder and decoder
are run in the tests as black boxes (built from the release tarball by
`tools/xvid-vectors.sh`, never read); the conformance streams are ITU-T's
and ISO's publications, used as data.

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
timing, and writes chosen frames as PNG; `examples/m4vbench.rs` times
encoding and decoding a YUV4MPEG2 clip.

## License

Open Encoding Attribution License v1.0 — a source-available (not OSI
open-source) license, royalty-free, with a commercial-attribution
requirement. See [LICENSE.md](LICENSE.md) and [NOTICE](NOTICE).
