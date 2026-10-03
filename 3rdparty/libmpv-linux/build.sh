#!/bin/bash
# Runs in the Debian 12 image: builds libmpv as one LGPL, decode-only library into /out. FFmpeg, dav1d, libplacebo,
# libass and lcms2 are built from source as static libraries and linked into it, and it exports only mpv's own API.
# What it links from the system is what every desktop with GTK has — FreeType, HarfBuzz, FriBidi, Fontconfig, EGL,
# ALSA — plus libva, which the plugin carries as a fallback. Sound goes through ALSA alone: libpulse is not on every
# desktop, and a PulseAudio or PipeWire server takes ALSA output anyway. Sharing the system's font stack also keeps a
# second FreeType out of a process whose toolkit already loaded one.
#
# FFmpeg then reports "LGPL version 2.1 or later" and mpv's config.h has HAVE_GPL 0, checked at the end.
set -e

FFMPEG_VERSION="${FFMPEG_VERSION:-8.1.2}"
MPV_TAG="${MPV_TAG:-v0.41.0}"
DAV1D_TAG="${DAV1D_TAG:-1.5.4}"
PLACEBO_TAG="${PLACEBO_TAG:-v7.360.1}"
LIBASS_TAG="${LIBASS_TAG:-0.17.5}"
LCMS2_TAG="${LCMS2_TAG:-lcms2.19}"

PREFIX=/out
WORK=/work/src
JOBS="$(nproc)"

mkdir -p "$WORK"
rm -rf "${PREFIX:?}"/*
mkdir -p "$PREFIX/lib/pkgconfig"
# Ours first, then the system's for the libraries libmpv takes from it.
export PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig"

source_of() {
    local name="$1" url="$2" tag="$3" recursive="$4"
    if [ -d "$WORK/$name" ] && [ "$(git -C "$WORK/$name" describe --tags --exact-match 2>/dev/null)" != "$tag" ]; then
        rm -rf "${WORK:?}/$name"
    fi
    if [ ! -d "$WORK/$name" ]; then
        git clone -q --depth 1 --branch "$tag" ${recursive:+--recursive --shallow-submodules} "$url" "$WORK/$name"
    fi
}

static() {
    local name="$1"
    shift
    echo "=== $name ==="
    rm -rf "$WORK/$name/build"
    meson setup "$WORK/$name/build" "$WORK/$name" --prefix="$PREFIX" --libdir=lib --buildtype=release \
        -Ddefault_library=static -Db_staticpic=true "$@"
    ninja -C "$WORK/$name/build" install
}

source_of dav1d https://code.videolan.org/videolan/dav1d.git "$DAV1D_TAG"
static dav1d -Denable_tools=false -Denable_tests=false

source_of libass https://github.com/libass/libass.git "$LIBASS_TAG"
static libass -Dfontconfig=enabled -Dcoretext=disabled -Ddirectwrite=disabled -Dlibunibreak=disabled \
    -Dtest=disabled -Dcompare=disabled -Dprofile=disabled -Dfuzz=disabled -Dcheckasm=disabled

source_of lcms2 https://github.com/mm2/Little-CMS.git "$LCMS2_TAG"
static lcms2 -Dutils=false -Djpeg=disabled -Dtiff=disabled -Dtests=disabled

source_of libplacebo https://code.videolan.org/videolan/libplacebo.git "$PLACEBO_TAG" recursive
static libplacebo -Dvulkan=disabled -Dglslang=disabled -Dshaderc=disabled -Dopengl=enabled -Dd3d11=disabled \
    -Dlcms=enabled -Ddovi=disabled -Dlibdovi=disabled -Dunwind=disabled -Dxxhash=disabled \
    -Ddemos=false -Dtests=false -Dbench=false -Dfuzz=false

# No --enable-gpl, no --enable-version3. --enable-libdav1d is required: FFmpeg's own av1 decoder is hardware-only.
# VA-API stays; VDPAU and Xlib are left out, so libmpv links neither libvdpau nor libX11.
# Unpacked afresh from the cached archive: a tree that went through Windows has lost its exec bits.
echo "=== FFmpeg $FFMPEG_VERSION ==="
cd "$WORK"
[ -f "ffmpeg-$FFMPEG_VERSION.tar.xz" ] || curl -sL -O "https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz"
rm -rf "ffmpeg-$FFMPEG_VERSION"
tar xf "ffmpeg-$FFMPEG_VERSION.tar.xz"
cd "ffmpeg-$FFMPEG_VERSION"
./configure \
    --prefix="$PREFIX" \
    --enable-static --disable-shared --enable-pic \
    --pkg-config-flags=--static \
    --disable-programs --disable-doc --disable-debug \
    --disable-encoders --disable-muxers \
    --disable-devices --disable-avdevice \
    --enable-vaapi --disable-vdpau --disable-xlib \
    --enable-libdav1d
make -j"$JOBS"
make install

# mpv asks pkg-config without --static, so what a static library of ours needs in turn is moved into what every
# caller gets. The system's .pc files are left alone: those libraries stay shared.
for pc in "$PREFIX"/lib/pkgconfig/*.pc; do
    python3 -I - "$pc" <<'PY'
import re, sys
path = sys.argv[1]
fields = {}
lines = []
for line in open(path).read().splitlines():
    found = re.match(r"^(Libs|Libs\.private|Requires|Requires\.private):\s*(.*)$", line)
    if found:
        fields[found.group(1)] = found.group(2)
    else:
        lines.append(line)
libs = " ".join(filter(None, [fields.get("Libs", ""), fields.get("Libs.private", "")]))
requires = ", ".join(filter(None, [fields.get("Requires", ""), fields.get("Requires.private", "")]))
lines.append("Libs: " + libs)
if requires:
    lines.append("Requires: " + requires)
open(path, "w").write("\n".join(lines) + "\n")
PY
done

# libplacebo is partly C++ and mpv links as C: the C++ runtime is named here. --exclude-libs keeps every symbol of
# the static libraries inside, so none can stand in for a system copy loaded by the toolkit.
echo "=== libmpv $MPV_TAG ==="
source_of mpv https://github.com/mpv-player/mpv.git "$MPV_TAG"
rm -rf "$WORK/mpv/build"
meson setup "$WORK/mpv/build" "$WORK/mpv" \
    --prefix="$PREFIX" --libdir=lib --buildtype=release \
    -Ddefault_library=shared \
    "-Dc_link_args=-lstdc++ -Wl,--exclude-libs,ALL" \
    -Dgpl=false \
    -Dlibmpv=true \
    -Dcplayer=false \
    -Dbuild-date=false \
    -Dlibavdevice=disabled \
    -Dlua=disabled \
    -Djavascript=disabled \
    -Dlibbluray=disabled \
    -Drubberband=disabled \
    -Duchardet=disabled \
    -Dzimg=disabled \
    -Dvulkan=disabled \
    -Dcdda=disabled \
    -Ddvdnav=disabled \
    -Dsdl2-audio=disabled \
    -Dsdl2-video=disabled \
    -Dsdl2-gamepad=disabled \
    -Dwayland=disabled \
    -Dx11=disabled \
    -Dgl-x11=disabled \
    -Dx11-clipboard=disabled \
    -Dvdpau=disabled \
    -Dpulse=disabled \
    -Dpipewire=disabled \
    -Dalsa=enabled \
    -Dvaapi-x11=disabled \
    -Djpeg=disabled
ninja -C "$WORK/mpv/build" install

echo "=== verifying ==="
LIBMPV="$PREFIX/lib/libmpv.so.2"
[ -e "$LIBMPV" ] || { echo "no libmpv.so.2 in $PREFIX/lib — the build did not produce one" >&2; exit 1; }
grep -q "#define HAVE_GPL 0" "$WORK/mpv/build/config.h" || { echo "mpv was NOT built in LGPL mode" >&2; exit 1; }
ours=$(readelf -d "$LIBMPV" | grep NEEDED | grep -E "libav|libsw|libpostproc|libdav1d|libplacebo|libass|liblcms2" || true)
if [ -n "$ours" ]; then
    echo "libmpv still links a library it should carry inside:" >&2
    echo "$ours" >&2
    exit 1
fi
echo "OK: libmpv is LGPL and one library; from the system it links:"
readelf -d "$LIBMPV" | awk '/NEEDED/{print "  " $5}' | tr -d '[]'
