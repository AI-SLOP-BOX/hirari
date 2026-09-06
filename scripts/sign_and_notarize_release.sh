#!/bin/sh
set -eu

# Sign the already-verified app bundle for distribution.  build_app.sh uses an
# ad-hoc signature so local UI smoke can launch; this script is the explicit
# production boundary and refuses to silently ship that preview signature.
ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
APP_DIR=${1:-"$ROOT_DIR/packaging/Aura DAW.app"}
case "$APP_DIR" in
    /*) : ;;
    *) APP_DIR="$ROOT_DIR/$APP_DIR" ;;
esac

if [ "$(uname -s)" != "Darwin" ]; then
    echo "release signing requires macOS" >&2
    exit 1
fi
command -v codesign >/dev/null 2>&1 || { echo "codesign is required" >&2; exit 1; }
test -d "$APP_DIR" || { echo "application bundle not found: $APP_DIR" >&2; exit 1; }
: "${AURA_CODESIGN_IDENTITY:?set AURA_CODESIGN_IDENTITY to a Developer ID Application identity}"

# Ensure the bundle passed to signing is structurally complete before mutating
# its signature.  This also catches stale/hand-assembled bundles.
"$ROOT_DIR/scripts/verify_release_bundle.sh" "$APP_DIR"

codesign --force --deep --options runtime --timestamp \
    --sign "$AURA_CODESIGN_IDENTITY" "$APP_DIR"
codesign --verify --deep --strict --verbose=2 "$APP_DIR"

# A production identity must not resolve to the ad-hoc marker or an empty
# identity.  `codesign -dv` writes details to stderr, so capture both streams.
DETAILS=$(codesign -dv --verbose=4 "$APP_DIR" 2>&1 || true)
case "$DETAILS" in
    *"Authority=Developer ID Application"*) : ;;
    *) echo "bundle is not signed by a Developer ID Application identity" >&2; exit 1 ;;
esac

# Notarization is optional only for local signing.  If a profile is supplied,
# require the complete wait/staple/validate sequence and fail closed on any
# missing tool or rejected submission.
if [ -n "${AURA_NOTARY_PROFILE:-}" ]; then
    command -v xcrun >/dev/null 2>&1 || { echo "xcrun is required for notarization" >&2; exit 1; }
    ARCHIVE=$(mktemp "${TMPDIR:-/tmp}/aura-notary.XXXXXX.zip")
    rm -f "$ARCHIVE"
    ditto -c -k --sequesterRsrc --keepParent "$APP_DIR" "$ARCHIVE"
    xcrun notarytool submit "$ARCHIVE" --keychain-profile "$AURA_NOTARY_PROFILE" --wait
    xcrun stapler staple "$APP_DIR"
    xcrun stapler validate "$APP_DIR"
    rm -f "$ARCHIVE"
fi

echo "Release signing verified: $APP_DIR"
