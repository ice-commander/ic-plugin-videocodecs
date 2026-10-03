#!/bin/bash
# Builds libmpv for macOS as one LGPL, decode-only library into the prefix given (3rdparty/build.sh passes
# cache/libmpv-macos). FFmpeg, dav1d, libplacebo, libass, lcms2, FreeType, HarfBuzz and FriBidi are built from source
# as static libraries and linked into it; it links nothing but the system and exports only mpv's own API.
#
# Nothing but the build tools comes from Homebrew: its ffmpeg is --enable-gpl, and a library found there would be
# linked as a separate dylib. FFmpeg then reports "LGPL version 2.1 or later" and mpv's config.h has HAVE_GPL 0.
#
#   brew install meson ninja nasm pkg-config
# plus the Xcode command line tools.
set -e

FFMPEG_VERSION=8.1.2
MPV_TAG=v0.41.0
DAV1D_TAG=1.5.4
PLACEBO_TAG=v7.360.1
LIBASS_TAG=0.17.5
LCMS2_TAG=lcms2.19
FREETYPE_TAG=VER-2-14-3
HARFBUZZ_TAG=14.6.0
FRIBIDI_TAG=v1.0.17

PREFIX="${1:?usage: libmpv-macos.sh <prefix>}"
WORK="${LIBMPV_SOURCES:-$(dirname "$PREFIX")/libmpv-macos-src}"

missing=""
for tool in meson ninja nasm pkg-config clang git curl; do
    command -v "$tool" >/dev/null || missing="$missing $tool"
done
if [ -n "$missing" ]; then
    echo "missing:$missing — brew install$missing (clang: the Xcode command line tools)" >&2
    exit 1
fi

mkdir -p "$WORK"
rm -rf "$PREFIX"
mkdir -p "$PREFIX/lib/pkgconfig"

# Only what is built here is visible to pkg-config: nothing from Homebrew can be picked up.
export PKG_CONFIG_LIBDIR="$PREFIX/lib/pkgconfig"
unset PKG_CONFIG_PATH
export MACOSX_DEPLOYMENT_TARGET=11.0

# A checkout of exactly this tag, fetched again when the tag changed.
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

source_of freetype https://gitlab.freedesktop.org/freetype/freetype.git "$FREETYPE_TAG"
static freetype -Dbrotli=disabled -Dbzip2=disabled -Dharfbuzz=disabled -Dpng=disabled -Dzlib=internal -Dtests=disabled

source_of fribidi https://github.com/fribidi/fribidi.git "$FRIBIDI_TAG"
static fribidi -Ddocs=false -Dbin=false -Dtests=false

source_of harfbuzz https://github.com/harfbuzz/harfbuzz.git "$HARFBUZZ_TAG"
static harfbuzz -Dfreetype=enabled -Dglib=disabled -Dgobject=disabled -Dcairo=disabled -Dchafa=disabled \
    -Dicu=disabled -Dgraphite=disabled -Dgraphite2=disabled -Dcoretext=disabled -Dpng=disabled -Dzlib=disabled \
    -Dtests=disabled -Dintrospection=disabled -Ddocs=disabled -Dutilities=disabled -Dbenchmark=disabled \
    -Dsubset=disabled -Draster=disabled -Dvector=disabled -Dgpu=disabled -Dwasm=disabled

source_of libass https://github.com/libass/libass.git "$LIBASS_TAG"
static libass -Dfontconfig=disabled -Dcoretext=enabled -Ddirectwrite=disabled -Dlibunibreak=disabled \
    -Dtest=disabled -Dcompare=disabled -Dprofile=disabled -Dfuzz=disabled -Dcheckasm=disabled

source_of lcms2 https://github.com/mm2/Little-CMS.git "$LCMS2_TAG"
static lcms2 -Dutils=false -Djpeg=disabled -Dtiff=disabled -Dtests=disabled

source_of libplacebo https://code.videolan.org/videolan/libplacebo.git "$PLACEBO_TAG" recursive
static libplacebo -Dvulkan=disabled -Dglslang=disabled -Dshaderc=disabled -Dopengl=enabled -Dd3d11=disabled \
    -Dlcms=enabled -Ddovi=disabled -Dlibdovi=disabled -Dunwind=disabled -Dxxhash=disabled \
    -Ddemos=false -Dtests=false -Dbench=false -Dfuzz=false

# No --enable-gpl, no --enable-version3. --enable-libdav1d is required: FFmpeg's own av1 decoder is hardware-only.
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
    --enable-videotoolbox --enable-audiotoolbox \
    --enable-libdav1d
make -j"$(sysctl -n hw.ncpu)"
make install

# mpv asks pkg-config without --static, so what a static library needs in turn is moved into what every caller gets.
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

# -Dlibavdevice=disabled: libavdevice is not built here. The rest switches off what a preview never uses, the
# standalone player's window, media keys and touch bar among them. Cocoa stays, VideoToolbox's GL interop needs it, and
# with it Swift (mpv 0.41 does not build Cocoa without it): its runtime is part of macOS, under /usr/lib/swift.
# HarfBuzz and libplacebo are partly C++, and mpv links as C, Objective-C here: the C++ runtime is named for both.
# Only mpv's own API is exported: what the static libraries define stays inside.
echo "=== libmpv $MPV_TAG ==="
source_of mpv https://github.com/mpv-player/mpv.git "$MPV_TAG"
rm -rf "$WORK/mpv/build"
meson setup "$WORK/mpv/build" "$WORK/mpv" \
    --prefix="$PREFIX" --libdir=lib --buildtype=release \
    -Ddefault_library=shared \
    "-Dc_link_args=-lc++ -Wl,-exported_symbol,_mpv_*" \
    "-Dobjc_link_args=-lc++ -Wl,-exported_symbol,_mpv_*" \
    -Dmacos-cocoa-cb=disabled \
    -Dmacos-media-player=disabled \
    -Dmacos-touchbar=disabled \
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
    -Dx11=disabled \
    -Dgl-x11=disabled \
    -Dx11-clipboard=disabled \
    -Djpeg=disabled
ninja -C "$WORK/mpv/build" install

echo "=== verifying ==="
LIBMPV="$PREFIX/lib/libmpv.2.dylib"
[ -f "$LIBMPV" ] || { echo "no libmpv.2.dylib in $PREFIX/lib" >&2; exit 1; }
grep -q "#define HAVE_GPL 0" "$WORK/mpv/build/config.h" || { echo "mpv was NOT built in LGPL mode" >&2; exit 1; }
outside=$(otool -L "$LIBMPV" | tail -n +2 | awk '{print $1}' | grep -v -E "^(/usr/lib/|/System/)" | grep -v "libmpv" || true)
if [ -n "$outside" ]; then
    echo "libmpv links outside the system, so it is not one library:" >&2
    echo "$outside" >&2
    exit 1
fi
foreign=$(nm -gU "$LIBMPV" | awk '{print $3}' | grep -v '^_mpv_' | head -5)
if [ -n "$foreign" ]; then
    echo "libmpv exports more than mpv's API, e.g.: $foreign" >&2
    exit 1
fi
echo "OK: libmpv is LGPL, one library, linking only the system ($(du -h "$LIBMPV" | cut -f1))"
