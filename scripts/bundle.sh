#!/bin/sh
# Build PowerBridge.app as a UNIVERSAL app (Apple silicon + Intel):   sh scripts/bundle.sh
#
#   Quick single-architecture build:   ARCHS="$(uname -m)" sh scripts/bundle.sh
#   Sign with a Developer ID:          SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" sh scripts/bundle.sh
#
# Requires: Xcode command line tools, Rust (rustup). The Rust targets are added automatically.
set -e
cd "$(dirname "$0")/.."
VERSION="$(tr -d '[:space:]' < VERSION)"        # single source of truth for the version number
ARCHS="${ARCHS:-arm64 x86_64}"
. scripts/rust-env.sh                            # prefers rustup's Rust and verifies it can build for each chip
export MACOSX_DEPLOYMENT_TARGET=12.0

rust_target() {
  case "$1" in
    arm64)  echo aarch64-apple-darwin ;;
    x86_64) echo x86_64-apple-darwin ;;
    *) echo "unsupported architecture: $1" >&2; exit 1 ;;
  esac
}

echo "==> Building the translator core for: $ARCHS"
LIBS=""; CLIS=""
for A in $ARCHS; do
  T="$(rust_target "$A")"
  ensure_rust_target "$T" || exit 1
  cargo build --release --target "$T" -p ppcx -p ppcx-ffi
  LIBS="$LIBS target/$T/release/libppcx_ffi.a"
  CLIS="$CLIS target/$T/release/ppcx"
done
mkdir -p target/universal
lipo -create $LIBS -output target/universal/libppcx_ffi.a
lipo -create $CLIS -output target/universal/ppcx

echo "==> Building the app for: $ARCHS"
EXES=""
for A in $ARCHS; do
  swift build -c release --package-path swift --arch "$A"
  EXES="$EXES $(swift build -c release --package-path swift --arch "$A" --show-bin-path)/PowerBridge"
done

APP=dist/PowerBridge.app
rm -rf "$APP"; mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create $EXES -output "$APP/Contents/MacOS/PowerBridge"
cp target/universal/ppcx "$APP/Contents/Resources/ppcx"     # used when creating standalone wrapper apps
cp assets/PowerBridge.icns "$APP/Contents/Resources/PowerBridge.icns"
cat > "$APP/Contents/Info.plist" <<PL
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>PowerBridge</string>
<key>CFBundleDisplayName</key><string>PowerBridge</string>
<key>CFBundleExecutable</key><string>PowerBridge</string>
<key>CFBundleIconFile</key><string>PowerBridge</string>
<key>CFBundleIdentifier</key><string>local.powerbridge</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>$VERSION</string>
<key>CFBundleShortVersionString</key><string>$VERSION</string>
<key>LSMinimumSystemVersion</key><string>12.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PL

IDENTITY="${SIGN_IDENTITY:--}"                              # "-" means ad-hoc signing
echo "==> Signing (${SIGN_IDENTITY:-ad-hoc})"
if [ "$IDENTITY" = "-" ]; then
  codesign --force --deep -s - "$APP" || echo "warning: ad-hoc signing failed"
else
  codesign --force --options runtime --timestamp -s "$IDENTITY" "$APP/Contents/Resources/ppcx"
  codesign --force --options runtime --timestamp -s "$IDENTITY" "$APP"
fi

cp target/universal/ppcx dist/ppcx
echo
echo "Built $APP  (version $VERSION)"
echo "  app architectures:        $(lipo -archs "$APP/Contents/MacOS/PowerBridge")"
echo "  translator architectures: $(lipo -archs "$APP/Contents/Resources/ppcx")"
