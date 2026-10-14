# Changelog

## 1.0.1

- **Universal app**: PowerBridge now builds as a single app that runs natively on Apple silicon and Intel Macs.
  `scripts/bundle.sh` builds both and merges them; `scripts/make-dmg.sh` packs the result into a disk image;
  `scripts/release.sh` does everything (see `docs/RELEASING.md`).
- **Standalone apps**: "Make Intel App" is now **Make Standalone App**, and the apps it creates are named
  "*Name* (PowerBridge)" (they run on both Apple silicon and Intel).
- **Wider program support**: real file I/O (`fopen`, `fread`, `fgets`, `fseek`, …), `strtol`/`strtod`/`atof`,
  `strstr`, case-insensitive string compares, the math library, and `stdin` input functions.
- **Real-world symbol names**: decorated imports such as `_fwrite$UNIX2003` and `_printf$LDBL128` are now recognised
  (common in binaries built for Mac OS X 10.5).
- The build script now prefers rustup's Rust and checks that it can build for both chips, with a clear message if a
  Homebrew-installed Rust is in the way.
- `ppcx --version`; version number now comes from a single `VERSION` file.
- New docs: `docs/RELEASING.md` (universal app, DMG, PKG, signing, notarization) and `docs/ROADMAP.md` (why large apps
  like Word and games don't run yet, and what it would take).

## 1.0.0 (previously 0.1.0)

- First release: SwiftUI app, `.app` bundle analysis, standalone wrapper apps, installer package (`.pkg`/`.mpkg`)
  support, retro icon.
