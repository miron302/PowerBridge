#!/bin/sh
# Pack PowerBridge.app into a disk image (.dmg):   sh scripts/make-dmg.sh
# The disk image itself is architecture-neutral; the universal app inside runs natively on Apple silicon and Intel.
#
# Optional environment variables:
#   SIGN_IDENTITY   "Developer ID Application: Your Name (TEAMID)"  - also signs the .dmg
#   NOTARY_PROFILE  name saved with `xcrun notarytool store-credentials`  - notarizes and staples the .dmg
set -e
cd "$(dirname "$0")/.."
VERSION="$(tr -d '[:space:]' < VERSION)"
[ -d dist/PowerBridge.app ] || sh scripts/bundle.sh
APP=dist/PowerBridge.app
DMG="dist/PowerBridge-$VERSION.dmg"
VOL="PowerBridge $VERSION"

WORK="$(mktemp -d /tmp/powerbridge-dmg.XXXXXX)"
STAGE="$WORK/stage"; mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/PowerBridge.app"
ln -s /Applications "$STAGE/Applications"
cat > "$STAGE/Read Me First.txt" <<TXT
PowerBridge $VERSION

Install:   drag PowerBridge into the Applications folder.
First run: right-click (or Control-click) PowerBridge and choose Open, then click Open.
           If macOS still refuses: System Settings > Privacy & Security > Open Anyway.

PowerBridge opens old PowerPC programs and installer packages. Apps with windows (Cocoa/Carbon) are not supported yet.
TXT

echo "==> Creating disk image"
RW="$WORK/rw.dmg"
hdiutil create -srcfolder "$STAGE" -volname "$VOL" -fs HFS+ -format UDRW -ov "$RW" >/dev/null
MNT="$WORK/mnt"; mkdir "$MNT"
hdiutil attach "$RW" -mountpoint "$MNT" -nobrowse -noverify -noautoopen >/dev/null
cp assets/PowerBridge.icns "$MNT/.VolumeIcon.icns"
SetFile -c icnC "$MNT/.VolumeIcon.icns" 2>/dev/null || true
SetFile -a C "$MNT" 2>/dev/null || true               # use the custom volume icon (SetFile ships with Xcode tools)
sync
for i in 1 2 3 4 5; do hdiutil detach "$MNT" >/dev/null 2>&1 && break; sleep 2; done
rm -f "$DMG"
hdiutil convert "$RW" -format UDZO -imagekey zlib-level=9 -o "$DMG" >/dev/null
rm -rf "$WORK"

if [ -n "$SIGN_IDENTITY" ]; then
  echo "==> Signing the disk image"
  codesign --force --timestamp -s "$SIGN_IDENTITY" "$DMG"
fi
if [ -n "$NOTARY_PROFILE" ]; then
  echo "==> Notarizing (this can take a few minutes)"
  xcrun notarytool submit "$DMG" --keychain-profile "$NOTARY_PROFILE" --wait
  xcrun stapler staple "$DMG"
fi

echo
echo "Built $DMG"
shasum -a 256 "$DMG"
