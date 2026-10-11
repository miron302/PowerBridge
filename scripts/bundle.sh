#!/bin/sh
# Build PowerBridge.app (SwiftUI front end + Rust translator core) on an Intel Mac:  scripts/bundle.sh
set -e
cd "$(dirname "$0")/.."
VERSION="$(tr -d '[:space:]' < VERSION)"   # single source of truth for the version number

cargo build --release -p ppcx -p ppcx-ffi          # target/release/{ppcx,libppcx_ffi.a}
swift build -c release --package-path swift         # links libppcx_ffi.a into the SwiftUI app
BIN="$(swift build -c release --package-path swift --show-bin-path)/PowerBridge"

APP=dist/PowerBridge.app
rm -rf "$APP"; mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/PowerBridge"
cp target/release/ppcx "$APP/Contents/Resources/ppcx"   # used when creating Intel wrapper apps
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
codesign --force --deep -s - "$APP" 2>/dev/null || true   # ad-hoc sign so recent macOS will launch it
cp target/release/ppcx dist/ppcx
echo "Built $APP version $VERSION (and dist/ppcx command-line tool)"
