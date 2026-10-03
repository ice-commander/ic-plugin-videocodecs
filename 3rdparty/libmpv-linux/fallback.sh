#!/bin/bash
# Runs in the Debian 12 image: copies what libmpv links that a desktop may lack — the VA-API loader — into /out/fallback, and the Debian
# copyright file of each into /out/fallback-licences. The plugin loads one only where the system has none.
set -e
SYSTEM=/usr/lib/x86_64-linux-gnu
rm -rf /out/fallback /out/fallback-licences
mkdir -p /out/fallback /out/fallback-licences
for soname in libva.so.2 libva-drm.so.2; do
    cp -L "$SYSTEM/$soname" "/out/fallback/$soname"
    package=$(dpkg -S "$(readlink -f "$SYSTEM/$soname")" | cut -d: -f1)
    version=$(dpkg-query -W -f='${Version}' "$package")
    cp "/usr/share/doc/$package/copyright" "/out/fallback-licences/$soname $package $version"
done
