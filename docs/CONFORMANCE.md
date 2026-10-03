# Conformance and reference streams

The decoder is checked against pictures that come from outside this
crate (`tests/conformance.rs`), as well as against its own encoder
(`tests/roundtrip.rs`) and by the unit tests. Two tools fetch or make the
external sets into `tests/conformance/` (or `$MPEG4_CONFORMANCE`); CI runs
both and sets `MPEG4_REQUIRE_CONFORMANCE=1`, so a missing set fails rather
than skips. None of the files is in the repository.

## Published conformance streams (`tools/fetch-conformance.sh`)

| set | file | source | what it checks |
|---|---|---|---|
| `itu-h263` | `base_fmnq.263`, `base_fmnq.dec` | ITU-T, `www.itu.int/wftp3/av-arch/video-site/h263plus/bitstreams/Intel_Draft20.zip` (contributed by Intel to the H.263 version 2 work) | Foreman QCIF, 68 pictures of baseline H.263 with every annex off — the short video header — against the decoded pictures published with it: **every picture within one sample** (the inverse DCTs' rounding; luma PSNR 67–74 dB), no drift |
| `itu-h263` | `dfijst_fmnq.263` | the same archive | annexes D, F, I, J, S and T (H.263 version 2's extended picture type): refused, naming PLUSPTYPE |
| `iso-14496-4` | `vcon-stp12L2.bits` | ISO, publicly available electronic insert of ISO/IEC 14496-4 Amd 35 (`standards.iso.org/ittf/PubliclyAvailableStandards/ISO_IEC_14496-4_2004_Amd_35_2009_Bitstreams/`) | a Simple Studio profile stream: refused, naming the Studio profiles |

ISO/IEC 14496-4's MPEG-4 Visual (Part 2) conformance streams for the
Simple and Advanced Simple profiles are not among ISO's publicly available
electronic inserts (only the Studio profile set of Amd 35 is, beside AVC
and SVC sets that do not concern this crate), so they are not used.

Searched for on 2026-10-03 and not found as public data: any stream with
reversible VLCs or other MPEG-4 error-resilience tools (none in ISO's
public inserts), and any H.263 stream with the Advanced Prediction mode
(Annex F) alone. ITU-T's H.263 archive (`h263plus/bitstreams/`: the
Intel set, the 9804 and 9807 anchors) has Annex F only together with
Annex D or in H.263 version 2's extended picture type, both refused. So
reversible VLCs, OBMC, Annex F, four-point GMC and field direct mode are
checked by this crate's own encoder's streams (byte-exact round trips,
damage recovery) and by hand-worked values, not by another encoder.

## Xvid as a black box (`tools/xvid-vectors.sh`)

The script downloads Xvid 1.3.7's release tarball from xvid.com (checked
against its SHA-256), builds the library and its two example programs,
`xvid_encraw` and `xvid_decraw`, and runs them. No Xvid source is read.

- **`xvid/`**: Xvid's encoder codes a synthetic source
  (`tools/synth_yuv.py`: a textured plane under a camera that pans, zooms
  and turns, a square crossing it at a sub-sample speed, grain; for the
  interlaced cases the bottom field sampled half a frame later) with one
  tool or combination per stream, 12-picture GOPs; Xvid's decoder decodes
  each, and `tests/conformance.rs` compares this decoder's pictures with
  Xvid's.
- **`rivet-enc/`**: this crate's encoder (`examples/m4venc.rs`: IPPP, four
  vectors, B-VOPs, video packets with and without B-VOPs and four vectors, a wide
  search with a larger `vop_fcode`, an odd size, quantisers 1 and 31,
  bit-rate control) read by Xvid's decoder, against this crate's decode
  (which equals the encoder's reconstruction byte for byte).

Every stream must decode without error, concealment or misalignment, to as
many pictures as Xvid's. The two inverse DCTs round differently within
what IEEE 1180 allows, so pictures agree to a sample or two and drift a
little over a GOP. Recorded 2026-10-03 (largest difference over all
pictures, worst picture's luma PSNR):

| stream | tools | largest difference | worst PSNR | gate |
|---|---|---|---|---|
| `simple` | Simple Profile, I/P | 3 | 58.7 dB | 4, 52 dB |
| `bframes` | B-VOPs | 2 | 62.0 dB | 4, 52 dB |
| `packed` | packed B-VOPs, not-coded placeholders | 2 | 62.0 dB | 4, 52 dB |
| `mpegquant` | MPEG quantiser, B-VOPs | 3 | 58.2 dB | 4, 52 dB |
| `gmc` | GMC (S-VOPs) | 3 | 58.9 dB | 4, 52 dB |
| `interlaced` | interlaced, top field first | 3 | 56.5 dB | 4, 52 dB |
| `interlaced_b` | interlaced, bottom field first, B-VOPs | 2 | 60.0 dB | 4, 52 dB |
| `slices` | video packets in I-, P- and B-VOPs | 2 | 62.0 dB | 4, 52 dB |
| `fine`, `coarse` | quantiser 1; quantiser 31 with B-VOPs | 3, 2 | 57.0, 60.4 dB | 4, 52 dB |
| `qpel`, `qpel_b` | quarter-sample, without and with B-VOPs | 5, 4 | 47.1, 50.2 dB | 8, 44 dB |
| `gmc_qpel_b` | GMC, quarter-sample, B-VOPs | 3 | 51.6 dB | 8, 44 dB |
| `oddsize` | 200x150, quarter-sample, B-VOPs | 4 | 50.0 dB | 8, 44 dB |

What the comparison found:

- **Resync markers in B-VOPs.** 6.3.5.2 makes a B-VOP's marker
  16 + max(`vop_fcode_forward`, `vop_fcode_backward`) bits; with both codes
  1 that is 17, and Xvid writes 18. Every B-VOP of Xvid's `slices` stream
  was concealed until the decoder accepted the longer marker there (a one
  after a 17th zero can only be a marker). The other way, Xvid's decoder
  misread this crate's B-VOPs with video packets (17-bit markers) until the
  encoder used `vop_fcode` 2 or more in streams with both, where the two
  lengths agree.
- **Quarter-sample interpolation.** Pictures predicted at quarter-sample
  positions that lie between two half-sample rows or columns — a
  horizontal quarter position combined with a vertical half or quarter one
  — differ from Xvid's by one at about a third of their samples; every
  other position agrees to the inverse DCTs' rounding. Xvid's predictions
  there are what filtering the horizontally interpolated rows vertically
  gives (checked by computing that variant: it removes the difference);
  this decoder averages the neighbouring half-sample values as 7.6.2.1
  describes the quarter positions. The decoder keeps its reading; the
  difference drifts within a GOP to the figures above, and the gate for
  the quarter-sample streams is set accordingly. Which reading other
  decoders and encoders follow is not settled here.

## What is no longer checked

Until 2026-10-03 the decoder was also checked against 25 other encoders'
streams downloaded from FFmpeg's sample hosting (samples.ffmpeg.org and
fate-suite.ffmpeg.org). They are no longer used: the project takes no
test data from FFmpeg. There was no reference output for them — the check
was that every VOP parsed exactly to its stuffing, and pictures were
inspected by eye — and the published and Xvid sets above cover most of
what they did, with reference pictures. What only those streams exercised,
and is now covered by no external stream:

- the workarounds for encoders that depart from the standard (below):
  libavcodec 54's VOL time increment resolution, OpenDivX's verid 2 VOL,
  DivX 5.00's GMC trajectory layout and units, early Xvid's interlaced
  `dct_type`, DivX 5's `0x7f` padding, the ones-and-zeros padding and the
  omitted stuffing byte — the code is unchanged, but no test stream shows
  it still applies;
- motion compensation padding from the macroblock grid for sizes that are
  not multiples of 16 against an encoder that depends on it (DivX 5, 624x350;
  Xvid's 200x150 `oddsize` agrees with this decoder, but does not
  distinguish the two readings by its content);
- the GMC vector clip, found on an Xvid zoom whose warp outran the
  `vop_fcode` range; the synthetic source zooms too slowly to reach it;
- data partitioning from another encoder (and stuffing before its
  markers): Xvid does not write it; this crate's encoder now does, with
  and without reversible VLCs, so the round trips and damage tests check
  it against that encoder only;
- real-world H.263 damage (a video-call capture with damaged GOBs) and
  DivX packed streams in AVI; the raw packed stream above and the fuzz
  tests stand in for them;
- encoders other than Xvid and this crate's: DivX 5 / 6, UB Video, RMP4,
  libavcodec.

## What the former sample streams taught (history)

The standard is the reference; where encoders that are in wide use depart
from it, the decoder recognises them. Each of these was found by a sample
failing to parse (one, the GMC clip, by a sample's pictures), and each
workaround applies only where the standard reading fails:

- **libavcodec 54 (`demo.m4v`)**: the VOL says `vop_time_increment_resolution`
  5 (three-bit increments) while every VOP codes a 15-bit increment. When
  the marker bit after the increment is missing, the decoder tries the
  other lengths and keeps the one that parses.
- **OpenDivX (`10-short.avi`)**: the VOL declares `video_object_layer_verid`
  2 but omits `newpred_enable` and `reduced_resolution_vop_enable`. When the
  verid 1 reading ends the header cleanly and the verid 2 one does not, the
  former is taken.
- **DivX 5.00 (`01.avi`, "DivX500Build413")**: the GMC trajectory carries one
  marker bit per warping point instead of one per code, and its vectors are
  in `1/s` sample units instead of half samples. Recognised by the marker
  layout or the user data string. DivX 5.01 and Xvid follow the standard on
  both counts (confirmed by `dx502_b_qpel.avi` and
  `xvid_gmcqpel_artifact.avi`, which are garbage read the DivX 5.00 way).
- **Early Xvid, interlaced (`ttm1.avi`)**: `dct_type` is coded in every coded
  P-VOP macroblock, including those with no coded blocks. An interlaced VOP
  that fails under the standard's condition is decoded again this way, and
  the stream keeps that reading if it parses.
- **Padding after the VOP**: DivX 5 appends a `0x7f` byte after its
  stuffing, `color16.avi`'s encoder pads with runs of one bits then zeros,
  the RMP4 encoder omits the stuffing byte when the data ends byte aligned. These
  are all accepted as the end of a VOP.
- **Picture edges**: for sizes that are not a multiple of 16, DivX 5
  (`test.b-frames.divx5.avi`, 624x350) drifts at the bottom edge unless
  motion compensation pads from the edge of the decoded macroblock grid
  rather than from the VOP's `width` x `height`; the decoder (and encoder)
  do the former.
- **GMC vectors as predictors** (`xvid_gmcqpel_artifact.avi`): during a
  zoom, the mean warp displacement of a GMC macroblock (−133 quarter
  samples) can exceed the range `vop_fcode` gives vectors (±128). Used as a
  predictor unclipped, it makes the neighbouring vectors wrap (−133 + 0 →
  123): misplaced blocks that spread through the S- and B-VOPs that follow
  — the artifact the file is named after. Clipped to the range, every
  frame is clean. This parses identically either way, so only the
  pictures showed it.
- **Stuffing before markers** (`ErrDec_mpeg4datapart-64_qcif.m4v`):
  macroblock stuffing may come right before a data-partitioning marker or a
  resync marker, so the decoder looks for the marker again after each
  stuffing code.
