#!/usr/bin/env bash
# Makes the Xvid reference sets tests/conformance.rs checks:
#
# - xvid/: streams Xvid's encoder (xvid_encraw) codes from a synthetic
#   source (tools/synth_yuv.py), one per coding tool, each with the
#   pictures Xvid's decoder (xvid_decraw) decodes from it;
# - rivet-enc/: this crate's encoder's streams (examples/m4venc.rs), each
#   with the pictures Xvid's decoder decodes from it.
#
# Xvid is used as a black box: this downloads its release tarball from
# xvid.com (checked against its SHA-256), builds the library and its two
# example programs, and runs them. None of its source is read.
#
# Each set is a directory under $MPEG4_CONFORMANCE (default
# tests/conformance/) of <name>.m4v, <name>.yuv (Xvid's pictures, I420, in
# display order) and <name>.size ("W H"). Needs a C compiler, make, curl,
# python3 and cargo (Linux or macOS).
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
sets="${MPEG4_CONFORMANCE:-$root/tests/conformance}"
work="${XVID_WORK:-$root/target/xvid}"
ver=1.3.7
sum=abbdcbd39555691dd1c9b4d08f0a031376a3b211652c0d8b3b8aa9be1303ce2d
mkdir -p "$sets/xvid" "$sets/rivet-enc" "$work"
cd "$work"

if [ ! -x prefix/bin/xvid_decraw ]; then
    tgz=xvidcore-$ver.tar.gz
    [ -f "$tgz" ] || curl -fsSL --retry 3 -o "$tgz" "https://downloads.xvid.com/downloads/$tgz"
    echo "$sum  $tgz" | sha256sum -c --quiet
    rm -rf xvidcore && tar xzf "$tgz"
    # Quiet unless it fails; then the log says why.
    if ! (cd xvidcore/build/generic &&
        ./configure --prefix="$work/prefix" --disable-assembly &&
        make -j"$(nproc 2>/dev/null || echo 4)" &&
        make install) >"$work/build.log" 2>&1; then
        tail -40 "$work/build.log"
        exit 1
    fi
    mkdir -p prefix/bin
    defs="-DARCH_IS_64BIT -DARCH_IS_GENERIC -DARCH_IS_LITTLE_ENDIAN"
    for tool in xvid_encraw xvid_decraw; do
        # shellcheck disable=SC2086
        cc -O2 -w $defs -Iprefix/include -Ixvidcore/src -o prefix/bin/$tool \
            xvidcore/examples/$tool.c -Lprefix/lib -lxvidcore -lm -lpthread
    done
fi
export LD_LIBRARY_PATH="$work/prefix/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export DYLD_LIBRARY_PATH="$work/prefix/lib${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}"
enc=$work/prefix/bin/xvid_encraw
dec=$work/prefix/bin/xvid_decraw

# Xvid's decode of <dir>/<name>.m4v to <dir>/<name>.yuv.
xvid_decode() {
    local d=$1 name=$2 tmp
    tmp=$(mktemp -d)
    if ! (cd "$tmp" && "$dec" -i "$d/$name.m4v" -d -c i420 -f yuv >"$work/dec.log" 2>&1); then
        cat "$work/dec.log"
        exit 1
    fi
    # The decoder writes its pictures, in output order, to one file.
    cat "$tmp"/* >"$d/$name.yuv"
    rm -rf "$tmp"
    echo "$(basename "$d")/$name: $(wc -c <"$d/$name.m4v") bytes"
}

# name  width height frames source(i: interlaced)  encoder options
cases=$(cat <<'CASES'
simple       320 240 30 -  -max_bframes 0 -cq 4
bframes      320 240 30 -  -max_bframes 2 -nopacked -cq 4
packed       320 240 30 -  -max_bframes 2 -cq 4
qpel         320 240 30 -  -max_bframes 0 -qpel -cq 4
qpel_b       320 240 30 -  -max_bframes 2 -nopacked -qpel -cq 4
gmc          320 240 30 -  -max_bframes 0 -gmc -cq 4
gmc_qpel_b   320 240 30 -  -max_bframes 2 -gmc -qpel -cq 4
mpegquant    320 240 30 -  -max_bframes 2 -nopacked -qtype 1 -cq 4
interlaced   320 240 30 i  -max_bframes 0 -interlaced 2 -cq 4
interlaced_b 320 240 30 i  -max_bframes 2 -nopacked -interlaced 1 -cq 4
slices       320 240 30 -  -max_bframes 2 -nopacked -slices 4 -cq 4
oddsize      200 150 30 -  -max_bframes 2 -nopacked -qpel -cq 4
fine         320 240 12 -  -max_bframes 0 -cq 1
coarse       320 240 30 -  -max_bframes 2 -nopacked -cq 31
CASES
)

out=$sets/xvid
while read -r name w h n src opts; do
    [ -n "$name" ] || continue
    flags=""
    [ "$src" = i ] && flags=--interlaced
    srcf=src_${w}x${h}_${n}${src}.yuv
    # shellcheck disable=SC2086
    [ -f "$srcf" ] || python3 "$root/tools/synth_yuv.py" "$w" "$h" "$n" $flags >"$srcf"
    # shellcheck disable=SC2086
    if ! "$enc" -i "$srcf" -w "$w" -h "$h" -frames "$n" -framerate 25 -max_key_interval 12 \
        -threads 1 $opts -o "$out/$name.m4v" >enc.log 2>&1; then
        cat enc.log
        exit 1
    fi
    echo "$w $h" >"$out/$name.size"
    xvid_decode "$out" "$name"
done <<<"$cases"

# This crate's encoder, read by Xvid's decoder.
(cd "$root" && cargo run --quiet --release --example m4venc -- "$sets/rivet-enc")
for m in "$sets/rivet-enc"/*.m4v; do
    xvid_decode "$sets/rivet-enc" "$(basename "$m" .m4v)"
done
