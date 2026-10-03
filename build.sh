#!/bin/sh
set -e
cd "$(dirname "$0")"

case "$(uname -s)" in
    Darwin) EXT=dylib ;;
    MINGW*|MSYS*|CYGWIN*) EXT=dll ;;
    *) EXT=so ;;
esac

# What 3rdparty/build.sh made for this system; IC_VIDEO_CODECS points elsewhere.
if [ -z "$IC_VIDEO_CODECS" ]; then
    case "$(uname -s)" in
        Darwin) PLATFORM=macos ;;
        MINGW*|MSYS*|CYGWIN*) PLATFORM=windows ;;
        *) PLATFORM=linux ;;
    esac
    if [ -d "3rdparty/out/$PLATFORM/video-codecs" ]; then
        IC_VIDEO_CODECS="$PWD/3rdparty/out/$PLATFORM/video-codecs"
    else
        echo "  no 3rdparty/out/$PLATFORM/video-codecs — bash 3rdparty/build.sh $PLATFORM makes it."
        echo "  Without it the plugin loads only where libmpv is already installed."
    fi
fi

# A renamed crate leaves its old library in target/, which would be collected as current.
rm -f bin/target/release/*."$EXT"

cargo build --release
mkdir -p bin
rm -f "bin/"*."$EXT"

found=0
for library in bin/target/release/*."$EXT"; do
    [ -f "$library" ] || continue
    cp "$library" bin/
    found=$((found + 1))
    printf '%s\n' "  $(basename "$library")"
done

if [ "$found" -eq 0 ]; then
    echo "no plugin libraries were produced" >&2
    exit 1
fi

rm -rf bin/video-codecs
if [ -n "$IC_VIDEO_CODECS" ]; then
    if [ ! -d "$IC_VIDEO_CODECS" ]; then
        echo "IC_VIDEO_CODECS is not a folder: $IC_VIDEO_CODECS" >&2
        exit 1
    fi
    mkdir -p bin/video-codecs
    cp -a "$IC_VIDEO_CODECS"/. bin/video-codecs/
    carried=$(find bin/video-codecs -type f | wc -l)
    if [ "$carried" -eq 0 ]; then
        echo "nothing to carry from $IC_VIDEO_CODECS" >&2
        exit 1
    fi
    printf '%s\n' "  $carried media libraries in bin/video-codecs"
else
    echo "  no IC_VIDEO_CODECS: the plugin will need a libmpv on the machine"
fi

echo "$found plugin(s) in bin/"
