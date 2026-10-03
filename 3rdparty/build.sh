#!/bin/bash
# Builds what this plugin carries but did not write — libmpv, one LGPL, decode-only library with its dependencies inside —
# into 3rdparty/out/<system>/video-codecs, with THIRD-PARTY-<system>.txt. Run by hand, only when a version changes:
# build-release.sh takes out/ as it is, so the files keep their md5 and the updater does not fetch them again.
#
#   bash 3rdparty/build.sh [macos] [linux] [windows]     none named: all three
#
#   macos    built here (libmpv-macos.sh), every dependency inside; bundle-macos.py names and signs it
#   linux    built in Docker on Debian 12 (libmpv-linux/), whose glibc is older than any supported distribution's
#   windows  the prebuilt LGPL libmpv-2.dll of zhongfly/mpv-winbuild: IC_LIBMPV_DLL, or cache/libmpv-windows/libmpv-2.dll
#
# Sources and builds are kept in 3rdparty/cache and reused; REBUILD=1 compiles again.
set -e
cd "$(dirname "$0")"
HERE="$PWD"
CACHE="$HERE/cache"
platforms="${*:-macos linux windows}"

for platform in $platforms; do
    echo "=== $platform ==="
    OUT="$HERE/out/$platform/video-codecs"
    NOTES="$CACHE/$platform.notes"
    rm -rf "$OUT" "$NOTES" "$NOTES.texts"
    mkdir -p "$OUT" "$CACHE"
    : > "$NOTES"

    case "$platform" in
        macos)
            PREFIX="$CACHE/libmpv-macos"
            if [ -n "$REBUILD" ] || [ ! -f "$PREFIX/lib/libmpv.2.dylib" ]; then
                bash libmpv-macos.sh "$PREFIX"
            fi
            python3 -I bundle-macos.py "$PREFIX/lib/libmpv.2.dylib" "$OUT" "$NOTES"
            ;;
        linux)
            PREFIX="$CACHE/libmpv-linux"
            image=ic-libmpv-linux
            docker build -q --platform linux/amd64 -t "$image" libmpv-linux >/dev/null
            if [ -n "$REBUILD" ] || [ ! -e "$PREFIX/lib/libmpv.so.2" ]; then
                mkdir -p "$PREFIX" "$CACHE/libmpv-linux-src"
                docker run --rm --platform linux/amd64 \
                    -v "$PREFIX:/out" -v "$CACHE/libmpv-linux-src:/work/src" "$image"
            fi
            docker run --rm --platform linux/amd64 -v "$PREFIX:/out" -v "$HERE/libmpv-linux:/recipe:ro" \
                --entrypoint bash "$image" /recipe/fallback.sh
            # Each library under the soname it is asked for by: the versioned file behind the link is what is copied.
            for library in "$PREFIX"/lib/*.so.*; do
                soname=$(objdump -p "$library" | awk '/SONAME/{print $2}')
                [ "$(basename "$library")" = "$soname" ] || continue
                cp -L "$library" "$OUT/$soname"
                printf '  %-32s built by libmpv-linux/build.sh\n' "$soname" >> "$NOTES"
            done
            mkdir -p "$OUT/fallback"
            cp "$PREFIX"/fallback/* "$OUT/fallback/"
            for licence in "$PREFIX"/fallback-licences/*; do
                read -r soname package version <<< "$(basename "$licence")"
                printf '  %-32s Debian 12 %s %s, loaded only where the system has none\n' \
                    "fallback/$soname" "$package" "$version" >> "$NOTES"
                printf '\n\n===== %s %s: copyright =====\n\n' "$package" "$version" >> "$NOTES.texts"
                cat "$licence" >> "$NOTES.texts"
            done
            ;;
        windows)
            DLL="${IC_LIBMPV_DLL:-$CACHE/libmpv-windows/libmpv-2.dll}"
            if [ ! -f "$DLL" ]; then
                echo "no Windows libmpv at $DLL: put the LGPL libmpv-2.dll of zhongfly/mpv-winbuild there, or set IC_LIBMPV_DLL" >&2
                exit 1
            fi
            cp "$DLL" "$OUT/libmpv-2.dll"
            printf '  %-32s zhongfly/mpv-winbuild, LGPL build, its components linked in statically\n' \
                libmpv-2.dll >> "$NOTES"
            ;;
        *)
            echo "unknown system $platform: macos, linux or windows" >&2
            exit 1
            ;;
    esac

    {
        cat licenses/NOTICE.txt
        echo
        cat "$NOTES"
        texts="LGPL-2.1.txt FFmpeg.md mpv.txt dav1d.txt libass.txt lcms2.txt"
        # FreeType and HarfBuzz are inside libmpv on macOS and Windows; Linux takes the system's.
        [ "$platform" = linux ] || texts="$texts FreeType-FTL.txt HarfBuzz.txt"
        for text in $texts; do
            printf '\n\n===== %s =====\n\n' "$text"
            cat "licenses/$text"
        done
        if [ -f "$NOTES.texts" ]; then
            cat "$NOTES.texts"
        fi
    } > "$OUT/THIRD-PARTY-$platform.txt"
    rm -f "$NOTES" "$NOTES.texts"
    echo "  3rdparty/out/$platform/video-codecs: $(find "$OUT" -type f | wc -l | tr -d ' ') file(s), $(du -sh "$OUT" | cut -f1)"
done
