# Debian 12 on purpose: its glibc is older than Fedora's, Arch's and Debian 13's, so what
# is built here runs on all of them. The other way round it would not.
FROM debian:12

# Only what libmpv links from the system: font shaping, the GL and VA-API stack, ALSA. libass, lcms2 and the rest
# are built from source into libmpv, so their -dev packages are deliberately absent and cannot be picked up instead.
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential pkg-config git curl ca-certificates \
        ninja-build nasm python3 python3-pip python3-setuptools \
        libegl1-mesa-dev libgl1-mesa-dev libdrm-dev libva-dev \
        libasound2-dev \
        libfreetype6-dev libfontconfig1-dev libfribidi-dev libharfbuzz-dev zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*

# Debian 12 carries meson 1.0.1 and mpv 0.41 wants 1.3 or newer.
RUN pip3 install --break-system-packages --no-cache-dir 'meson>=1.3'

WORKDIR /work
COPY build.sh /usr/local/bin/build-lgpl-media
RUN chmod +x /usr/local/bin/build-lgpl-media
CMD ["/usr/local/bin/build-lgpl-media"]
