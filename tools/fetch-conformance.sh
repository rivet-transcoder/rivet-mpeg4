#!/usr/bin/env bash
# Downloads the published conformance streams tests/conformance.rs checks
# into tests/conformance/ (or $MPEG4_CONFORMANCE), verifying their SHA-256.
#
# - itu-h263/: from ITU-T's own server, bitstreams contributed by Intel to
#   the H.263 (version 2) work (Intel_Draft20.zip): base_fmnq.263, Foreman
#   QCIF in baseline H.263 with every annex off, with base_fmnq.dec, its
#   decoded pictures; and dfijst_fmnq.263, the same source with annexes D,
#   F, I, J, S and T — H.263 version 2's extended picture type, which the
#   short video header does not include.
# - iso-14496-4/: from ISO's publicly available electronic inserts of
#   ISO/IEC 14496-4 (Amd 35), vcon-stp12L2.bits, a Simple Studio profile
#   conformance stream (a profile this crate refuses).
#
# The files are the standards bodies' publications, used as published;
# none is redistributed in this repository.
set -euo pipefail

root="${MPEG4_CONFORMANCE:-$(cd "$(dirname "$0")/.." && pwd)/tests/conformance}"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$root/itu-h263" "$root/iso-14496-4"

itu=https://www.itu.int/wftp3/av-arch/video-site/h263plus/bitstreams
iso=https://standards.iso.org/ittf/PubliclyAvailableStandards/ISO_IEC_14496-4_2004_Amd_35_2009_Bitstreams

ok() { # sha256 file
    [ -f "$2" ] && echo "$1  $2" | sha256sum -c --status
}

fetch() { # sha256 file url
    if ! ok "$1" "$2"; then
        echo "fetching $(basename "$2")"
        curl -fsSL --retry 3 -o "$2" "$3"
        echo "$1  $2" | sha256sum -c --quiet
    fi
}

h=$root/itu-h263
if ! ok 9fd7197df25fb29836e9b798137d24bd9e79c1dd177e45c23f8731fe3ea1aaa2 "$h/base_fmnq.263" ||
    ! ok 083ad5d17927d3b1a3afe37bff94335a836dd7bf2627af322929acf9412cf2dc "$h/base_fmnq.dec" ||
    ! ok 98386797527f6d667ba715178bb6674010fc07c49d741524140ca125864a1a2f "$h/dfijst_fmnq.263"; then
    fetch d8c37e8b97f49a0fc4cecb4c81f5f178dfd3e4f7d9d7c57d20c738d9708d875b \
        "$work/Intel_Draft20.zip" "$itu/Intel_Draft20.zip"
    unzip -o -q -j "$work/Intel_Draft20.zip" base_fmnq.263 base_fmnq.dec dfijst_fmnq.263 -d "$h"
    ok 9fd7197df25fb29836e9b798137d24bd9e79c1dd177e45c23f8731fe3ea1aaa2 "$h/base_fmnq.263"
    ok 083ad5d17927d3b1a3afe37bff94335a836dd7bf2627af322929acf9412cf2dc "$h/base_fmnq.dec"
    ok 98386797527f6d667ba715178bb6674010fc07c49d741524140ca125864a1a2f "$h/dfijst_fmnq.263"
fi
fetch 8f9c29802f4d56ac7a7109795057091f6efc7e41bb6ead453b38945cceb8490e \
    "$root/iso-14496-4/vcon-stp12L2.bits" "$iso/vcon-stp12L2.bits"
echo "conformance streams in $root"
