#!/bin/sh
# Create an installer package that installs PowerBridge into /Applications:   sh scripts/make-pkg.sh
# Optional: INSTALLER_IDENTITY="Developer ID Installer: Your Name (TEAMID)" signs the package.
set -e
cd "$(dirname "$0")/.."
VERSION="$(tr -d '[:space:]' < VERSION)"

[ -d dist/PowerBridge.app ] || sh scripts/bundle.sh

OUT="dist/PowerBridge-$VERSION.pkg"
pkgbuild \
  --component dist/PowerBridge.app \
  --install-location /Applications \
  --identifier local.powerbridge \
  --version "$VERSION" \
  "$OUT"

if [ -n "$INSTALLER_IDENTITY" ]; then
  productsign --sign "$INSTALLER_IDENTITY" "$OUT" "$OUT.signed" && mv "$OUT.signed" "$OUT"
fi
echo "Built $OUT"
