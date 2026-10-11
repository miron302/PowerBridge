#!/bin/sh
# Create an installer package that installs PowerBridge into /Applications:  sh scripts/make-pkg.sh
set -e
cd "$(dirname "$0")/.."
VERSION="$(tr -d '[:space:]' < VERSION)"

[ -d dist/PowerBridge.app ] || sh scripts/bundle.sh

pkgbuild \
  --component dist/PowerBridge.app \
  --install-location /Applications \
  --identifier local.powerbridge \
  --version "$VERSION" \
  "dist/PowerBridge-$VERSION.pkg"

echo "Built dist/PowerBridge-$VERSION.pkg"
