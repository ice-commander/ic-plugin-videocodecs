# ic-videocodecs

Video playback for Ice Commander through libmpv.

In the GTK frontend the plugin draws the picture itself: the host hands it an
OpenGL canvas, and libmpv renders into that framebuffer. libmpv opens files by
path, so for a file inside an archive or on a remote filesystem the host makes
a local copy to play from. In the web frontend the plugin sends a `media` node
that names the file, and the page plays it with the browser's player; libmpv
is not needed there.
The terminal frontend is refused at load time.

## What it plays

`.mp4 .mkv .avi .mov .webm .m4v .wmv .flv .ts .m2ts .mts .mpg .mpeg .3gp .ogv`

The plugin matches files by extension only.

Under the picture are a play/pause button (`space`), a seek slider, the clock
(elapsed / length) and a volume slider (0–100, starting at 80). The controls
are hidden while the host reports the pointer idle. If a file cannot be played,
the window shows "This file could not be played." The reason goes to the host
log.

mpv is started with `config=no`, so the user's `mpv.conf` is ignored. It also
gets `terminal=no`, `input-default-bindings=no`, `input-vo-keyboard=no`,
`hwdec=auto-safe`, `keep-open=yes` and `volume=80`. Options mpv rejects are
logged once the player starts.

## libmpv

libmpv is opened at run time, not linked. The plugin looks for it in this order:

1. the path in `IC_VIDEO_MPV`;
2. `video-codecs/` beside the plugin, and the plugin's own folder;
3. the system loader's search path.

The names it tries are `libmpv.2.dylib` and `libmpv.dylib` on macOS;
`libmpv-2.dll`, `mpv-2.dll` and `libmpv.dll` on Windows; and `libmpv.so.2` and
`libmpv.so` elsewhere. If the GTK frontend finds no libmpv, the plugin logs
`video: no libmpv was found` and refuses to load, so another viewer can still
take the extensions.

The application does not ship libmpv on any system; this plugin carries it in
`video-codecs/`: one library, LGPL and decode-only — FFmpeg without
`--enable-gpl`, mpv with `-Dgpl=false` — with FFmpeg, dav1d, libplacebo, libass
and Little CMS linked into it (on macOS FreeType, HarfBuzz and FriBidi too),
exporting only mpv's API, and `THIRD-PARTY-<system>.txt` beside it. On Linux it
takes what every desktop with GTK has from the system and plays through ALSA;
`video-codecs/fallback/` holds libva, loaded only where the system has none.

## Building

```sh
bash 3rdparty/build.sh   # the codecs of all three systems into 3rdparty/out; by hand, when a version changes
./build.sh               # the plugin, carrying this system's codecs into bin/video-codecs
./test.sh                # cargo test --workspace
./deploy-local.sh
./build-release.sh       # all three systems into distr/, with the codecs from 3rdparty/out as they are
```

Build output goes to `bin/`, and Cargo's target directory is `bin/target`.

`3rdparty/` holds what the plugin carries but did not write: the recipes and
licences in git, the result in `3rdparty/out/` and the sources and builds in
`3rdparty/cache/`, which are reused (`REBUILD=1` compiles again). The release
does not rebuild them, so a new version of the plugin ships the same codec
files with the same md5, and the updater leaves them alone.

- macOS: `3rdparty/libmpv-macos.sh` builds every dependency from source as a
  static library and links them into `libmpv.2.dylib`, which then links only
  the system; nothing but the build tools comes from Homebrew.
  `3rdparty/bundle-macos.py` names it `@loader_path`, signs it, and would refuse
  any library outside the system whose licence is GPL only.
- Linux: `3rdparty/libmpv-linux/` does the same in Docker on Debian 12, whose
  glibc is older than any supported distribution's.
- Windows: the LGPL `libmpv-2.dll` of
  [zhongfly/mpv-winbuild](https://github.com/zhongfly/mpv-winbuild), from
  `IC_LIBMPV_DLL` or `3rdparty/cache/libmpv-windows/`. It is loaded with its
  own folder searched first.

`IC_VIDEO_CODECS` makes `build.sh` carry another folder instead.

`deploy-local.sh` copies the plugin into the Ice Commander plugin folder:

- macOS: `~/Library/Application Support/ice-commander/plugins`
- Windows: `%APPDATA%/ice-commander/plugins`
- elsewhere: `${XDG_DATA_HOME:-~/.local/share}/ice-commander/plugins`

`IC_PLUGIN_DIR` overrides the folder. When the build carried libraries, the
script replaces `video-codecs/` there as a whole. That folder is separate
because the host would try to load any library left directly among the plugins
as a plugin. The script also removes libraries it deployed earlier that are no
longer built, using the list in `bin/.deployed`. Afterwards, switch the plugin
on in Settings → Plugins and restart.

## Known limitations

- `space` is the only key bound. `pressed()` handles ten-second `back`/`on`
  seeks (`STEP`), but the document declares no `keys` for them, so
  `Left` and `Right` do nothing. `Up` and `Down` are not bound to volume either. mpv's own key
  bindings are switched off.

## Licence

The code is MIT or Apache-2.0, at your option. The icons in
`src/video-view/assets/` are Icons8 artwork; see
[THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md). The carried codecs keep their
own licences, in `3rdparty/licenses/` and in `THIRD-PARTY-<system>.txt` beside them. Contributions are accepted
under the [DCO](DCO); sign off with `git commit -s`.
