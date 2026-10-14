#!/bin/sh
# Build everything for a release (universal app, .dmg, .pkg) and write checksums:   sh scripts/release.sh
# See docs/RELEASING.md for signing and notarization.
set -e
cd "$(dirname "$0")/.."
VERSION="$(tr -d '[:space:]' < VERSION)"

cargo test
sh scripts/bundle.sh
sh scripts/make-dmg.sh
sh scripts/make-pkg.sh

( cd dist && shasum -a 256 "PowerBridge-$VERSION.dmg" "PowerBridge-$VERSION.pkg" > SHA256SUMS.txt )
echo
echo "Release $VERSION is ready in dist/:"
ls -1 dist
