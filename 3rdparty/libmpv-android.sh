#!/bin/bash
# Builds libmpv for Android arm64 as one LGPL, decode-only library into the prefix given (3rdparty/build.sh passes
# cache/libmpv-android). FFmpeg, dav1d, libplacebo, libass, lcms2 and the C++ runtime are linked into it. From the
# system it links what the NDK gives every app, plus FreeType, HarfBuzz, FriBidi and Fontconfig, which Ice Commander's
# APK carries for its toolkit — the same split as on Linux, so a second FreeType stays out of the process. It exports
# mpv's own API and FFmpeg's av_jni_*: MediaCodec decoding wants the JavaVM handed over before mpv_create.
#
# Built here with the NDK, no Docker: the NDK's sysroot is the ABI. The sources are the Linux build's when its cache
# has them at these tags, so nothing is fetched twice.
#
#   ANDROID_NDK_HOME   the NDK (default /opt/android-sdk/ndk/27.0.12077973)
#   GTK_ROOT           the GTK stack's installed tree whose fonts libraries the APK ships (default: root-aarch64 of
#                      GTK_STACK, else of the release ice-commander/app/gtk-stack.lock pins, fetched into the cache
#                      as for the app)
#   GTK_STACK          an android-rust-gtk4-adwaita checkout, for work on the stack; its bin/tools/venv has meson
#                      and ninja when the system has none (the release archive has no tools)
set -e

FFMPEG_VERSION=8.1.2
MPV_TAG=v0.41.0
DAV1D_TAG=1.5.4
PLACEBO_TAG=v7.360.1
LIBASS_TAG=0.17.5
LCMS2_TAG=lcms2.19

API=31
TRIPLE=aarch64-linux-android

PREFIX="${1:?usage: libmpv-android.sh <prefix>}"
CACHE="$(cd "$(dirname "$PREFIX")" && pwd)"
WORK="${LIBMPV_SOURCES:-$CACHE/libmpv-android-src}"
LINUX_SOURCES="$CACHE/libmpv-linux-src"
NDK="${ANDROID_NDK_HOME:-/opt/android-sdk/ndk/27.0.12077973}"
TC="$NDK/toolchains/llvm/prebuilt/linux-x86_64"
if [ -n "${GTK_STACK:-}" ]; then
    GTK_STACK="$(cd "$GTK_STACK" && pwd)"
    GTK_ROOT="${GTK_ROOT:-$GTK_STACK/bin/gtk-stack/root-aarch64}"
elif [ -z "${GTK_ROOT:-}" ]; then
    IC_APP="$(cd "$(dirname "$0")/../../.." && pwd)/app"
    GTK_ROOT="$("$IC_APP/src/gtk-app/android/fetch-gtk-stack.sh" --abi arm64-v8a "$IC_APP/gtk-stack.lock")"
    GTK_ROOT="$GTK_ROOT/bin/gtk-stack/root-aarch64"
fi
JOBS="$(nproc)"

[ -x "$TC/bin/$TRIPLE$API-clang" ] || { echo "no NDK clang at $TC — set ANDROID_NDK_HOME" >&2; exit 1; }
[ -f "$GTK_ROOT/lib/arm64-v8a/pkgconfig/freetype2.pc" ] || {
    echo "no GTK stack at $GTK_ROOT — build it there, or set GTK_ROOT or GTK_STACK" >&2
    exit 1
}
GTK_ROOT="$(cd "$GTK_ROOT" && pwd)"
if ! command -v meson >/dev/null && [ -n "${GTK_STACK:-}" ] && [ -x "$GTK_STACK/bin/tools/venv/bin/meson" ]; then
    export PATH="$GTK_STACK/bin/tools/venv/bin:$PATH"
fi
missing=""
for tool in meson ninja pkg-config python3 git curl make; do
    command -v "$tool" >/dev/null || missing="$missing $tool"
done
[ -z "$missing" ] || {
    echo "missing:$missing (meson and ninja: install them, or set GTK_STACK to a checkout with bin/tools/venv)" >&2
    exit 1
}

mkdir -p "$WORK"
rm -rf "$PREFIX"
mkdir -p "$PREFIX/lib/pkgconfig"

# Our static libraries, then the toolkit's shared ones as the APK ships them. Their .pc files say prefix=/, so they
# are copied with the real place written in; PKG_CONFIG_SYSROOT_DIR would prefix our own absolute paths as well.
APP_PC="$WORK/app-pc"
rm -rf "$APP_PC"
mkdir -p "$APP_PC"
for pc in "$GTK_ROOT"/lib/arm64-v8a/pkgconfig/*.pc; do
    sed "s|^prefix=/\$|prefix=$GTK_ROOT|" "$pc" > "$APP_PC/$(basename "$pc")"
done
export PKG_CONFIG_LIBDIR="$PREFIX/lib/pkgconfig:$APP_PC"
unset PKG_CONFIG_PATH PKG_CONFIG_SYSROOT_DIR

# Android 15 devices may use 16 KB pages: every library must be aligned to them, and r27 does not do it by default.
PAGES="-Wl,-z,max-page-size=16384 -Wl,-z,common-page-size=16384"
CROSS="$WORK/android.cross"
cat > "$CROSS" <<EOF
[binaries]
c = '$TC/bin/$TRIPLE$API-clang'
cpp = '$TC/bin/$TRIPLE$API-clang++'
ar = '$TC/bin/llvm-ar'
nm = '$TC/bin/llvm-nm'
ranlib = '$TC/bin/llvm-ranlib'
strip = '$TC/bin/llvm-strip'
pkg-config = 'pkg-config'

[host_machine]
system = 'android'
cpu_family = 'aarch64'
cpu = 'aarch64'
endian = 'little'

[built-in options]
c_link_args = ['-Wl,-z,max-page-size=16384', '-Wl,-z,common-page-size=16384', '-Wl,--build-id=sha1']
cpp_link_args = ['-Wl,-z,max-page-size=16384', '-Wl,-z,common-page-size=16384', '-Wl,--build-id=sha1']
EOF

# A checkout of exactly this tag: the Linux build's copy when it has one, fetched otherwise.
source_of() {
    local name="$1" url="$2" tag="$3" recursive="$4"
    if [ -d "$WORK/$name" ] && [ "$(git -C "$WORK/$name" describe --tags --exact-match 2>/dev/null)" != "$tag" ]; then
        rm -rf "${WORK:?}/$name"
    fi
    [ -d "$WORK/$name" ] && return
    if [ "$(git -C "$LINUX_SOURCES/$name" describe --tags --exact-match 2>/dev/null)" = "$tag" ]; then
        cp -a "$LINUX_SOURCES/$name" "$WORK/$name"
        rm -rf "$WORK/$name/build"
    else
        git clone -q --depth 1 --branch "$tag" ${recursive:+--recursive --shallow-submodules} "$url" "$WORK/$name"
    fi
}

static() {
    local name="$1"
    shift
    echo "=== $name ==="
    rm -rf "$WORK/$name/build-android"
    meson setup "$WORK/$name/build-android" "$WORK/$name" --cross-file="$CROSS" --prefix="$PREFIX" --libdir=lib \
        --buildtype=release -Ddefault_library=static -Db_staticpic=true "$@"
    ninja -C "$WORK/$name/build-android" install
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

# The same LGPL set as the desktop, with MediaCodec for hardware decoding: no --enable-gpl, no --enable-version3.
echo "=== FFmpeg $FFMPEG_VERSION ==="
cd "$WORK"
if [ ! -f "ffmpeg-$FFMPEG_VERSION.tar.xz" ]; then
    if [ -f "$LINUX_SOURCES/ffmpeg-$FFMPEG_VERSION.tar.xz" ]; then
        cp "$LINUX_SOURCES/ffmpeg-$FFMPEG_VERSION.tar.xz" .
    else
        curl -sL -O "https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz"
    fi
fi
rm -rf "ffmpeg-$FFMPEG_VERSION"
tar xf "ffmpeg-$FFMPEG_VERSION.tar.xz"
cd "ffmpeg-$FFMPEG_VERSION"
./configure \
    --prefix="$PREFIX" \
    --target-os=android --arch=aarch64 --cpu=armv8-a --enable-cross-compile \
    --cc="$TC/bin/$TRIPLE$API-clang" --cxx="$TC/bin/$TRIPLE$API-clang++" \
    --ar="$TC/bin/llvm-ar" --ranlib="$TC/bin/llvm-ranlib" --nm="$TC/bin/llvm-nm" --strip="$TC/bin/llvm-strip" \
    --pkg-config=pkg-config \
    --enable-static --disable-shared --enable-pic \
    --pkg-config-flags=--static \
    --disable-programs --disable-doc --disable-debug \
    --disable-encoders --disable-muxers \
    --disable-devices --disable-avdevice \
    --enable-jni --enable-mediacodec --disable-vulkan \
    --enable-libdav1d
make -j"$JOBS"
make install
grep -q "#define CONFIG_GPL 0" config.h && grep -q "#define CONFIG_VERSION3 0" config.h || {
    echo "FFmpeg was NOT configured as LGPL 2.1" >&2
    exit 1
}

# mpv asks pkg-config without --static, so what a static library of ours needs in turn is moved into what every
# caller gets. The toolkit's .pc files are left alone: those libraries stay shared.
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
# libplacebo is partly C++ and mpv links as C: the runtime goes inside rather than leaning on the APK's
# libc++_shared.so.
sed -i 's|^Libs: \(.*\)$|Libs: \1 -lc++_static -lc++abi|' "$PREFIX/lib/pkgconfig/libplacebo.pc"

# Only mpv's API and FFmpeg's JNI hooks leave the library; nothing inside can stand in for a copy the toolkit loaded.
cat > "$WORK/libmpv.map" <<'EOF'
{
    global:
        mpv_*;
        av_jni_set_java_vm;
        av_jni_get_java_vm;
    local:
        *;
};
EOF

echo "=== libmpv $MPV_TAG ==="
source_of mpv https://github.com/mpv-player/mpv.git "$MPV_TAG"
rm -rf "$WORK/mpv/build-android"
# -Dc_link_args replaces the cross file's, so the page alignment is said again here. The version script reaches
# meson's test programs too, which have no av_jni_*: --undefined-version lets them link.
meson setup "$WORK/mpv/build-android" "$WORK/mpv" --cross-file="$CROSS" \
    --prefix="$PREFIX" --libdir=lib --buildtype=release \
    -Ddefault_library=shared \
    "-Dc_link_args=$PAGES -Wl,--build-id=sha1 -Wl,--version-script=$WORK/libmpv.map -Wl,--undefined-version -Wl,--icf=safe" \
    -Dgpl=false \
    -Dlibmpv=true \
    -Dcplayer=false \
    -Dbuild-date=false \
    -Dmanpage-build=disabled \
    -Dlibavdevice=disabled \
    -Dlua=disabled \
    -Djavascript=disabled \
    -Dlibarchive=disabled \
    -Dlibbluray=disabled \
    -Drubberband=disabled \
    -Duchardet=disabled \
    -Dzimg=disabled \
    -Dvulkan=disabled \
    -Dshaderc=disabled \
    -Dspirv-cross=disabled \
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
    -Dvaapi=disabled \
    -Dvaapi-x11=disabled \
    -Dpulse=disabled \
    -Dpipewire=disabled \
    -Dalsa=disabled \
    -Djpeg=disabled \
    -Dgl=enabled \
    -Degl-android=enabled \
    -Dplain-gl=enabled \
    -Dandroid-media-ndk=enabled \
    -Daaudio=enabled \
    -Daudiotrack=disabled \
    -Dopensles=disabled
ninja -C "$WORK/mpv/build-android" install

echo "=== verifying ==="
LIBMPV="$PREFIX/lib/libmpv.so"
[ -e "$LIBMPV" ] || { echo "no libmpv.so in $PREFIX/lib — the build did not produce one" >&2; exit 1; }
grep -q "#define HAVE_GPL 0" "$WORK/mpv/build-android/config.h" || { echo "mpv was NOT built in LGPL mode" >&2; exit 1; }
cp "$LIBMPV" "$PREFIX/lib/libmpv.unstripped.so"
"$TC/bin/llvm-strip" --strip-all "$LIBMPV"
READELF="$TC/bin/llvm-readelf"
ours=$("$READELF" -d "$LIBMPV" | grep NEEDED | grep -E "libav|libsw|libdav1d|libplacebo|libass|liblcms2|libc\+\+" || true)
if [ -n "$ours" ]; then
    echo "libmpv still links a library it should carry inside:" >&2
    echo "$ours" >&2
    exit 1
fi
misaligned=$("$READELF" -lW "$LIBMPV" | awk '$1 == "LOAD" && $NF != "0x4000"' || true)
if [ -n "$misaligned" ]; then
    echo "libmpv is not aligned to 16 KB pages:" >&2
    echo "$misaligned" >&2
    exit 1
fi
exported=$("$TC/bin/llvm-nm" -D --defined-only "$LIBMPV" | awk '{print $3}')
echo "$exported" | grep -qx av_jni_set_java_vm || { echo "libmpv does not export av_jni_set_java_vm" >&2; exit 1; }
leaked=$(echo "$exported" | grep -vE '^(mpv_|av_jni_)' || true)
if [ -n "$leaked" ]; then
    echo "libmpv exports more than mpv's API:" >&2
    echo "$leaked" | head >&2
    exit 1
fi
echo "OK: libmpv is LGPL, one library, 16 KB aligned, $(du -h "$LIBMPV" | cut -f1); from the system it links:"
"$READELF" -d "$LIBMPV" | awk '/NEEDED/{print "  " $NF}' | tr -d '[]'
