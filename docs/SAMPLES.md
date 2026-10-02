# Sample bitstreams

The decoder is checked against real encoders' output, used as **data**:
the files below are fetched by `tools/fetch-samples.sh` (with their SHA-256)
and checked by `tests/samples.rs`. No other decoder was run, and none of
these files is in the repository — most are clips of films and broadcasts
whose licences are unknown.

There is no reference output for them, so the check is the parse. Every
coded VOP is decoded and the decoder records whether its macroblock data
ended exactly where the VOP's stuffing begins (`DecoderStats::
misaligned_vops`). A wrong VLC table, a field read in the wrong order or
under the wrong condition, a vector rule that misreads a skip — any of
these desynchronises a VOP long before its end; that every VOP of every
stream below parses to the bit is strong evidence that the syntax is read
as the encoders wrote it. Reconstruction (prediction, interpolation,
inverse quantisation, the IDCT) does not affect the parse, so frames were
also inspected by eye: the figures in the table were recorded after the
pictures were checked for drift — the residue of wrong prediction, which
compounds over a GOP — and none showed any.

Recorded 2026-10-02.

| file | source | encoder | tools | VOPs | concealed | misaligned |
|---|---|---|---|---|---|---|
| `demo.m4v` | fate-suite `mpeg4/` | libavcodec 54.3 | Simple, video packets, 320x240 | 42 | 0 | 0 |
| `packed_bframes.avi` | fate-suite `mpeg4/` | — | packed B-VOPs, 544x352 | 16 (4 placeholders dropped) | 0 | 0 |
| `xvid_vlc_trac7411.h263` | fate-suite `mpeg4/` | Xvid | Advanced Simple, MPEG quantiser, 720x576 | 20 | 0 | 0 |
| `resize_down-up.h263` | fate-suite `mpeg4/` | — | 400x300 (neither dimension a multiple of 16) | 150 | 0 | 0 |
| `ttm1.avi` | samples `V-codecs/XVID/` | Xvid (2002) | interlaced: field DCT; 624x336 | 234 | 1 (file cut short) | 0 |
| `test.b-frames.divx5.avi` | samples `V-codecs/DX50-DivX5/` | DivX 5.00 | B-VOPs, 624x350 | 800 | 0 | 0 |
| `01.avi` | samples `V-codecs/DX50-DivX5/` | DivX 5.00 | GMC (2 points), quarter-sample, B-VOPs | 239 | 0 | 0 |
| `divx.5.0.5-qpel.avi` | samples `V-codecs/DX50-DivX5/divx61/` | DivX 5.03 | quarter-sample, packed B-VOPs, 640x480 | 281 (138 dropped) | 0 | 0 |
| `divx.6.6.1-qpel.avi` | samples `V-codecs/DX50-DivX5/divx61/` | DivX 6.6.1 | quarter-sample, packed B-VOPs | 281 (97 dropped) | 0 | 0 |
| `color16.avi` | samples `V-codecs/MPEG4/` | — | MPEG quantiser, 712x368 | 117 | 0 | 0 |
| `10-short.avi` | samples `V-codecs/OpenDivX/` | OpenDivX | Simple, 352x240 | 152 | 1 (file cut short) | 0 |
| `messenger.h263` | samples `V-codecs/h263/h263-raw/` | an H.263 video-call capture | short video header, QCIF | 96 | 2 | 94 (record padding) |
| `greenlines.rmp4.p.avi` | samples `V-codecs/RMP4/` | fourcc RMP4 | 640x480 | 290 | 0 | 0 |
| `greenlines.rmp4.p.di.avi` | samples `V-codecs/RMP4/` | fourcc RMP4 | 640x480 | 290 | 0 | 0 |
| `clip10-640x480-550k.avi` | samples `V-codecs/UMP4/` | UB Video | Main object type, MPEG quantiser | 354 | 0 | 0 |
| `0x4D475844-wow.avi` | samples `V-codecs/DXGM/` | fourcc DXGM | 1024x464 | 648 | 0 | 0 |
| `xvid_gmcqpel_artifact.avi` | samples `archive/` | Xvid ("XviD0047") | GMC (3 points), quarter-sample, B-VOPs, 856x472 | 743 | 0 | 0 |
| `ErrDec_mpeg4datapart-64_qcif.m4v` | samples `archive/` | — | data partitioning (no RVLC), video packets, QCIF | 281 | 0 | 0 |
| `DivX51-Qpel.avi` | samples `archive/` | DivX 5.03 | quarter-sample, B-VOPs, 640x408 | 600 | 0 | 0 |
| `dx502_b_qpel.avi` | samples `archive/` | DivX 5.01 | GMC, B-VOPs, packed, 720x540 | 600 (298 dropped) | 0 | 0 |
| `vdpart-bug.avi` | samples `archive/` | — | 640x480, B-VOPs | 16 | 0 | 0 |
| `qprd_cmp_b-frames_naq1.avi` | samples `archive/` | Xvid | B-VOPs, 720x480 | 255 | 0 | 0 |
| `prezentaciaXvid.avi` | samples `archive/` | Xvid | 432x320 | 228 | 0 | 0 |
| `mpeg4_sstp_dpcm.m4v` | fate-suite `mpeg4/` | — | Simple Studio profile | refused | | |
| `mpeg4-from-nc4000-w10.cmp` | samples `archive/` | — | NEWPRED | refused | | |

Full URLs are in `tools/fetch-samples.sh`: `https://fate-suite.ffmpeg.org/`,
`https://samples.ffmpeg.org/V-codecs/` and `https://samples.ffmpeg.org/archive/all/`.
"Dropped" VOPs are the not-coded placeholders a packed bitstream puts after
each packed access unit. `messenger.h263` is a capture written in fixed
32 KB records, each picture followed by fill: the fill is what "misaligned"
counts there, and two of its pictures are damaged at a GOB start (their
remaining GOBs are concealed).

Looked at and set aside: `divx4.avi` (an ASF file, not AVI — the test
harness reads AVI and raw streams only), `clip5.avi` (a camera stream that
never sends a VOL header).

## What the samples taught

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
