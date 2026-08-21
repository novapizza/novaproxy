#!/usr/bin/env bash
# Builds a macOS .pkg installer for NovaProxy.
#
# Tauri's bundler has no pkg target (`tauri build --bundles` offers app, dmg and
# ios only), so the installer is assembled here from the .app that `tauri build`
# already produced. Run `npm run tauri build -- --bundles app` first, or pass
# --build to have this script do it.
#
# Signing is opt-in: export PKG_SIGN_IDENTITY (a "Developer ID Installer: …"
# name) and, to sign the app itself, APP_SIGN_IDENTITY ("Developer ID
# Application: …"). Unset, the pkg is unsigned — installable locally, and
# nobody's certificate is spent on a local build.
set -euo pipefail

cd "$(dirname "$0")/.."
CONF=src-tauri/tauri.conf.json
APP_DIR=target/release/bundle/macos
OUT_DIR=target/release/bundle/pkg

if [[ "${1:-}" == "--build" ]]; then
  npm run tauri build -- --bundles app
fi

read -r NAME VERSION IDENT < <(python3 -c "
import json
c = json.load(open('$CONF'))
print(c['productName'], c['version'], c['identifier'])
")
APP="$APP_DIR/$NAME.app"
[[ -d "$APP" ]] || { echo "no $APP — run: npm run tauri build -- --bundles app" >&2; exit 1; }

# Match the name Tauri gives the dmg beside it: aarch64, not uname's arm64.
ARCH=$(uname -m); [[ "$ARCH" == arm64 ]] && ARCH=aarch64
PKG="$OUT_DIR/${NAME}_${VERSION}_${ARCH}.pkg"
mkdir -p "$OUT_DIR"

# pkgbuild takes a directory tree, not a bundle, and copies everything under
# --root. Staging one app keeps stray files in bundle/macos (the leftover rw.*
# images a failed dmg run leaves behind) out of the installer.
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
ditto "$APP" "$STAGE/$NAME.app"

if [[ -n "${APP_SIGN_IDENTITY:-}" ]]; then
  # Inside out: a nested binary signed after its container invalidates the
  # container's signature. --deep is deprecated and does not do this correctly.
  HELPER="$STAGE/$NAME.app/Contents/Resources/nova-helper"
  [[ -f "$HELPER" ]] && codesign --force --timestamp --options runtime \
    --sign "$APP_SIGN_IDENTITY" "$HELPER"
  codesign --force --timestamp --options runtime \
    --sign "$APP_SIGN_IDENTITY" "$STAGE/$NAME.app"
  codesign --verify --strict --verbose=2 "$STAGE/$NAME.app"
fi

SIGN_ARGS=()
[[ -n "${PKG_SIGN_IDENTITY:-}" ]] && SIGN_ARGS=(--sign "$PKG_SIGN_IDENTITY")
# macOS ships bash 3.2, where an empty array trips `set -u` on expansion.

pkgbuild \
  --root "$STAGE" \
  --install-location /Applications \
  --identifier "$IDENT" \
  --version "$VERSION" \
  ${SIGN_ARGS[@]+"${SIGN_ARGS[@]}"} \
  "$PKG"

echo
echo "Built $PKG"
pkgutil --check-signature "$PKG" | head -3 || true
