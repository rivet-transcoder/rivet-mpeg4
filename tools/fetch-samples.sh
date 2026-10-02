#!/usr/bin/env bash
# Downloads the sample bitstreams tests/samples.rs checks against into
# tests/samples/ (or $MPEG4_SAMPLES), and verifies their SHA-256.
#
# They are encoder output — DivX 4/5/6, Xvid, OpenDivX, libavcodec, RealMagic,
# UB Video, Microsoft's H.263 — published as test data on samples.ffmpeg.org
# and fate-suite.ffmpeg.org. They are used as data only; no other decoder is
# run. They are not redistributed in this repository: most are clips of
# films and broadcasts whose licences are unknown. docs/SAMPLES.md says what
# each one exercises.
set -euo pipefail

dir="${MPEG4_SAMPLES:-$(dirname "$0")/../tests/samples}"
mkdir -p "$dir"
cd "$dir"

fate=https://fate-suite.ffmpeg.org
v=https://samples.ffmpeg.org/V-codecs
a=https://samples.ffmpeg.org/archive/all

while read -r sum name url; do
    [ -z "$sum" ] && continue
    if [ ! -f "$name" ] || ! echo "$sum  $name" | sha256sum -c --status; then
        echo "fetching $name"
        curl -fsSL --retry 3 -o "$name" "$url"
    fi
    echo "$sum  $name" | sha256sum -c --quiet
done <<EOF
da1983816306e0e4003da606e139896fe9394564df17fc9be1171f1f5c7e2441 demo.m4v $fate/mpeg4/demo.m4v
450a89377f024ce11d176d2be0fa3756df005aa45294866b383fee18a17cbf48 packed_bframes.avi $fate/mpeg4/packed_bframes.avi
aee58db19e76c334b126b2390aed661e6134995b7fdc614ea652222926153689 xvid_vlc_trac7411.h263 $fate/mpeg4/xvid_vlc_trac7411.h263
a1a40010f9865f9beadee8d63c36202d827d31c0058503a28081555cba0766e3 resize_down-up.h263 $fate/mpeg4/resize_down-up.h263
c7389f15ceb3381be18ab21c932ef3cda952466bcd2ade824faf36a880ea860e mpeg4_sstp_dpcm.m4v $fate/mpeg4/mpeg4_sstp_dpcm.m4v
fd3040d7c3f93d7a9fe6501df9f1d48f636359af58f9cbcef46eb84e481d4be3 ttm1.avi $v/XVID/ttm1.avi
5785235c3bd9b22723c7eccadcdf6ed6933dd5d9c39a01ce510b03cc88531d24 test.b-frames.divx5.avi $v/DX50-DivX5/test.b-frames.divx5.avi
8a0c64f59f1cf8d115acd61752d493821c9514156e4abc789b710b7640609f86 01.avi $v/DX50-DivX5/01.avi
aae369158eefc1bfe44c01dc6bf2c1c42e58f102fef72b0fc6e0689a838f26e0 divx.5.0.5-qpel.avi $v/DX50-DivX5/divx61/divx.5.0.5-qpel.avi
fd2315e7a78945e832f1bd079798e723bb439c10e8227fb16d994b85eca70e19 divx.6.6.1-qpel.avi $v/DX50-DivX5/divx61/divx.6.6.1-qpel.avi
40bfdcb2d4ffedcfbeabed2953d1d6ef3289f36de4ace3e8a7471c389b8f2851 color16.avi $v/MPEG4/color16.avi
a8bbdde83b830209e6a9c3d52bba815122342f60460f6c8bdc3b726ed950729d 10-short.avi $v/OpenDivX/10-short.avi
8a044eba3cd3be35a37c0e06286fb82565206d0f3f2fa16c59413e7d42fdfeac messenger.h263 $v/h263/h263-raw/messenger.h263
a0529354df05ebd327bbe9e6884aafda968d8f25367330df98602385dcf65c05 greenlines.rmp4.p.avi $v/RMP4/greenlines.rmp4.p.avi
fe70b1b6b24673cfa3f4603558d1035df0b248af4bb7aa45a0a5f3aeb5941aa3 greenlines.rmp4.p.di.avi $v/RMP4/greenlines.rmp4.p.di.avi
a838d6568d7716d8b58eb1b3de39e82528b5e654e6a1f03e43737d4e0995e2ca clip10-640x480-550k.avi $v/UMP4/clip10-640x480-550k.avi
0e020a3bfce4cf99b1d08821a21884f584e797be3adf3958382297dd1bfb3365 0x4D475844-wow.avi $v/DXGM/0x4D475844-wow.avi
773b7d711e1a45fee5b391404e89b5bd02f440f946ef5871122203e7f85a739e xvid_gmcqpel_artifact.avi $a/avi%2Bmpeg4%2B%2B%2Bxvid_gmcqpel_artifact.avi
0e2bc36a25680ec6cab0561184f847504bb3324e45a8fb0eb6dd4951292042f4 ErrDec_mpeg4datapart-64_qcif.m4v $a/m4v%2Bmpeg4%2B%2B%2BErrDec_mpeg4datapart-64_qcif.m4v
528a50f9bd5cad45ae23bbb5a435eb994e9098e42e945fa2cc8725b5454acfd1 DivX51-Qpel.avi $a/avi%2Bmpeg4%2B%2B%2BDivX51-Qpel.avi
f52e8498f225cde1790f4714e5ddf6099e5f723596b5aaf9939379dec507f91b dx502_b_qpel.avi $a/avi%2Bmpeg4%2B%2B%2Bdx502_b_qpel.avi
f3e999d7fea1be5d43ba74a631e78de4284850dc03efde753896a879a239aa0e vdpart-bug.avi $a/avi%2Bmpeg4%2B%2B%2Bvdpart-bug.avi
3e274d2623731303bdb85c7dc2506de068fe2d0a84550f3709393ad94ef51ce2 qprd_cmp_b-frames_naq1.avi $a/avi%2Bmpeg4%2B%2B%2Bqprd_cmp_b-frames_naq1.avi
1e70bcd9d1ea1bb2aea1e5c1a6a066e9b1bc102376db3c55e426a36f91fd6dce prezentaciaXvid.avi $a/avi%2Bmpeg4%2B%2B%2BprezentaciaXvid.avi
60e72bba1fb992e67305c98d1386d519789094ee5dd057b4a55bdaed77abb612 mpeg4-from-nc4000-w10.cmp $a/nc%2Bmpeg4%2B%2B%2Bmpeg4-from-nc4000-w10.cmp
EOF
echo "samples in $dir"
