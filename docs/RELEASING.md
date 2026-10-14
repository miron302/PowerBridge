# Releasing PowerBridge: universal app, DMG, PKG

This is the complete guide to turning the source code into a **universal app** (runs natively on Apple silicon *and*
Intel Macs) and packing it into a **disk image (.dmg)** and an **installer (.pkg)**.

> **Status:** the scripts below were written and logic-tested with stand-in tools on Linux, but they have **not been
> run on a real Mac by the author**. If a step fails, the error text tells you which tool complained; send it over.

---

## 1. One-time setup

On the Mac you build on (Apple silicon or Intel, macOS 12 or newer):

```
xcode-select --install                                                  # Apple's command line tools
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh           # Rust (press 1), then reopen Terminal
```

You do **not** need to install the Apple silicon / Intel Rust targets by hand; `bundle.sh` adds them.

## 2. Build the universal app

From the project folder:

```
sh scripts/bundle.sh
```

What happens: the translator core is compiled once for Apple silicon (`arm64`) and once for Intel (`x86_64`), the Swift
app is compiled for both, and `lipo` merges each pair into one universal file. Takes a few minutes. Result:

```
dist/PowerBridge.app        the universal app
dist/ppcx                   the command-line tool (universal)
```

The last lines of the output show the architectures. You want both:

```
app architectures:        x86_64 arm64
translator architectures: x86_64 arm64
```

Check it yourself any time:

```
lipo -archs dist/PowerBridge.app/Contents/MacOS/PowerBridge
file dist/PowerBridge.app/Contents/MacOS/PowerBridge
```

Quick single-architecture build while developing (faster):

```
ARCHS="$(uname -m)" sh scripts/bundle.sh
```

### Test both halves

On an **Apple silicon** Mac you can test both slices of the same app:

```
arch -arm64  dist/PowerBridge.app/Contents/MacOS/PowerBridge     # native Apple silicon
arch -x86_64 dist/PowerBridge.app/Contents/MacOS/PowerBridge     # Intel slice, through Rosetta 2
```

(or: Finder → PowerBridge.app → Get Info → tick "Open using Rosetta" to launch the Intel slice normally.)
On an Intel Mac only the Intel slice can run, so the Apple silicon slice is best tested on an Apple silicon Mac.

Then try, with at least one of each: drop in a PowerPC program, open a `.pkg`, press **Make Standalone App**,
open the app it creates.

## 3. Pack it into a DMG

```
sh scripts/make-dmg.sh
```

Creates `dist/PowerBridge-1.0.1.dmg` (the version comes from the `VERSION` file) containing PowerBridge.app, a shortcut
to Applications to drag onto, and a short "Read Me First" note. It prints a SHA-256 checksum at the end.

A disk image has no architecture of its own: the **same .dmg works on Apple silicon and Intel** because the app inside
is universal.

Open the DMG once yourself to check it looks right (double-click it, drag the app out, run it).

### Doing it by hand (no script)

1. Make a folder, e.g. `PowerBridge-dmg`, and put `dist/PowerBridge.app` in it.
2. In Terminal: `ln -s /Applications PowerBridge-dmg/Applications`
3. `hdiutil create -volname "PowerBridge" -srcfolder PowerBridge-dmg -ov -format UDZO PowerBridge.dmg`

(Or: Disk Utility → File → New Image → Image from Folder.)

## 4. The installer package (optional)

```
sh scripts/make-pkg.sh
```

Creates `dist/PowerBridge-1.0.1.pkg`, which installs PowerBridge into /Applications.

## 5. Everything at once

```
sh scripts/release.sh
```

Runs the tests, builds the universal app, the DMG and the PKG, and writes `dist/SHA256SUMS.txt`.

---

## 6. Signing and notarization (so other people can open it without warnings)

Without this, anyone who downloads your DMG sees "can't be opened because Apple cannot check it for malicious software"
and has to right-click → Open the first time (or run `xattr -cr /Applications/PowerBridge.app`). That's fine for
friends and testers. For public distribution, sign and notarize:

**You need:** a paid Apple Developer Program membership (about US$99/year).

1. In Xcode → Settings → Accounts → Manage Certificates, create **Developer ID Application** (and, for the PKG,
   **Developer ID Installer**) certificates. List them with `security find-identity -v -p codesigning`.
2. Create an app-specific password at appleid.apple.com, then store credentials once:

   ```
   xcrun notarytool store-credentials "powerbridge-notary" \
       --apple-id "you@example.com" --team-id "TEAMID" --password "abcd-efgh-ijkl-mnop"
   ```
3. Release with signing + notarization:

   ```
   SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" \
   INSTALLER_IDENTITY="Developer ID Installer: Your Name (TEAMID)" \
   NOTARY_PROFILE="powerbridge-notary" \
   sh scripts/release.sh
   ```

   The scripts sign the helper tool and the app with the hardened runtime, sign the DMG, submit it to Apple, wait for
   the verdict, and staple the ticket so it also opens offline.
4. Verify: `spctl -a -vvv -t install dist/PowerBridge-1.0.1.dmg` and
   `xcrun stapler validate dist/PowerBridge-1.0.1.dmg`.

## 7. Publishing checklist

- [ ] `VERSION` is right (everything else follows from it)
- [ ] `sh scripts/release.sh` finished without errors
- [ ] `lipo -archs` shows both `x86_64` and `arm64`
- [ ] Tested on Apple silicon (native) and the Intel slice (`arch -x86_64`, or an Intel Mac)
- [ ] DMG opens, app launches, About shows the right version
- [ ] Upload the `.dmg` (and `.pkg`) plus `SHA256SUMS.txt`; write release notes from `CHANGELOG.md`

## Troubleshooting

| Problem | Fix |
| --- | --- |
| `lipo: can't open input file` | One of the architecture builds failed earlier; scroll up for the first error. |
| `can't find crate for `std`` / "the `aarch64-apple-darwin` target may not be installed" (even though `rustup` says it is) | A Homebrew-installed Rust is being used instead of rustup's. Run `which -a cargo rustc`; if the first result isn't in `~/.cargo/bin`, run `export PATH="$HOME/.cargo/bin:$PATH"` and try again, or remove it with `brew uninstall rust` and open a new Terminal window. (`bundle.sh` now prefers rustup automatically and explains this if it can't.) |
| `error: could not find ... target` | Run `rustup target add aarch64-apple-darwin x86_64-apple-darwin` and retry. |
| Swift link error about `ppcx_ffi` | Run `sh scripts/bundle.sh` (not `swift build` alone) so the Rust library is built first. |
| `SetFile: command not found` while making the DMG | Harmless: only the custom disk icon is skipped. |
| `hdiutil: create failed - Resource busy` | Eject any "PowerBridge" disk in Finder and run the script again. |
| Notarization says *Invalid* | `xcrun notarytool log <submission-id> --keychain-profile powerbridge-notary` explains why. |
