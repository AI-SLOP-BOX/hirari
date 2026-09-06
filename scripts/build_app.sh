#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
APP_DIR="$ROOT_DIR/packaging/Aura DAW.app"

SIGNING_IDENTITY="-"
if [ "${AURA_RELEASE_MODE:-0}" = "1" ]; then
    : "${AURA_CODESIGN_IDENTITY:?AURA_CODESIGN_IDENTITY is required in release mode}"
    SIGNING_IDENTITY="$AURA_CODESIGN_IDENTITY"
    # A signed artifact must not be produced from a checkout that still
    # tracks local fixture clones.  Keep ordinary developer builds usable,
    # but fail closed for the release path.
    AURA_STRICT_SOURCE_HYGIENE=1 "$ROOT_DIR/scripts/audit_repository_hygiene.sh"
fi
STAGE_DIR=$(mktemp -d "${TMPDIR:-/tmp}/aura-app-stage.XXXXXX")
STAGED_APP="$STAGE_DIR/Aura DAW.app"
cleanup() { rm -rf "$STAGE_DIR"; }
trap cleanup EXIT INT TERM
APP_DIR_TARGET="$APP_DIR"
APP_DIR="$STAGED_APP"
CONTENTS_DIR="$APP_DIR/Contents"
BIN_DIR="$CONTENTS_DIR/MacOS"

cargo build --release -p aura-ui --manifest-path "$ROOT_DIR/Cargo.toml"

# Build the isolated plugin worker from the same source revision as the app.
# This prevents stale or missing worker binaries from making a packaged app
# report every external plugin as unavailable.
"$ROOT_DIR/scripts/build_plugin_worker.sh"

mkdir -p "$BIN_DIR" "$CONTENTS_DIR/Resources"
install -m 755 "$ROOT_DIR/target/release/aura-ui" "$BIN_DIR/Aura DAW"
install -m 644 "$ROOT_DIR/packaging/macos/Info.plist" "$CONTENTS_DIR/Info.plist"

# Every packaged build carries an explicit resource manifest.  The runtime
# uses this marker to distinguish a deliberately empty resource set from a
# stale/hand-assembled bundle whose Resources directory merely happens to
# exist.  Concrete Metal/UI assets can be added to the manifest as they are
# introduced without changing the bundle contract.
RESOURCE_MANIFEST="$CONTENTS_DIR/Resources/aura-resources.manifest"
printf '%s\n' \
    'aura.resources.v1' \
    'ui=slint-compiled' \
    'metal=optional' \
    'status=generated' > "$RESOURCE_MANIFEST"
chmod 644 "$RESOURCE_MANIFEST"

# Ship the license boundary with every application bundle.  Keeping these
# notices inside the artifact makes the MIT core, optional GPL components, and
# Slint/other third-party obligations visible to downstream redistributors
# without requiring access to the source repository.
for notice in LICENSE THIRD_PARTY_NOTICES.md LICENSE-COMBINED-DISTRIBUTION.md; do
    if [ ! -s "$ROOT_DIR/$notice" ]; then
        echo "Missing required distribution notice: $ROOT_DIR/$notice" >&2
        exit 1
    fi
    install -m 644 "$ROOT_DIR/$notice" "$CONTENTS_DIR/Resources/$notice"
done

# The isolated plug-in worker is part of the application runtime.  Omitting it
# makes every third-party plug-in appear to fail with MissingHelper when the
# packaged app is launched outside the repository.
if [ ! -x "$ROOT_DIR/build-tools/aura-plugin-host-worker" ]; then
    echo "Missing sandbox worker: $ROOT_DIR/build-tools/aura-plugin-host-worker" >&2
    exit 1
fi
install -m 755 "$ROOT_DIR/build-tools/aura-plugin-host-worker" "$BIN_DIR/aura-plugin-host-worker"

# Record the exact executable pair shipped in this bundle.  This catches a
# stale worker copied from a different build even when both files are valid
# Mach-O binaries and have the expected architecture.
hash_file() {
    file="$1"
    normalized="$file"
    cleanup_normalized=0
    # Release Rust binaries may already carry a toolchain signature.  Hash the
    # executable body, not an incidental signature that the app bundle step
    # will replace.
    if [ "$(uname -s)" = "Darwin" ] && command -v codesign >/dev/null 2>&1; then
        hash_tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/aura-build-hash.XXXXXX")
        normalized="$hash_tmp_dir/executable"
        cp "$file" "$normalized"
        codesign --remove-signature "$normalized" >/dev/null 2>&1 || true
        cleanup_normalized=1
    fi
    if command -v shasum >/dev/null 2>&1; then
        hash=$(shasum -a 256 "$normalized" | awk '{print $1}')
    elif command -v sha256sum >/dev/null 2>&1; then
        hash=$(sha256sum "$normalized" | awk '{print $1}')
    else
        echo "Unable to compute release executable hashes" >&2
        exit 1
    fi
    [ "$cleanup_normalized" -eq 0 ] || rm -rf "$hash_tmp_dir"
    printf '%s\n' "$hash"
}
BUILD_MANIFEST="$CONTENTS_DIR/Resources/aura-build.manifest"
{
    printf '%s\n' 'aura.build.v1'
    printf 'main_sha256=%s\n' "$(hash_file "$BIN_DIR/Aura DAW")"
    printf 'worker_sha256=%s\n' "$(hash_file "$BIN_DIR/aura-plugin-host-worker")"
    printf 'worker_source=build-tools/aura-plugin-host-worker\n'
} > "$BUILD_MANIFEST"
chmod 644 "$BUILD_MANIFEST"

# Ad-hoc signing makes the local preview bundle launchable without requiring
# a developer certificate. Distribution signing remains a separate release step.
if command -v codesign >/dev/null 2>&1; then
    # Rebuilds may leave a CodeResources manifest from a previous bundle
    # layout. Remove it before signing so verification reflects the current
    # Contents tree instead of stale resource entries.
    rm -rf "$CONTENTS_DIR/_CodeSignature"
    codesign --force --deep --sign "$SIGNING_IDENTITY" "$APP_DIR" >/dev/null
fi

# Re-record the post-signing executable bodies.  The first signature seals the
# resource manifest; this second pass makes the executable identity manifest
# part of that sealed tree without creating a hash/signature cycle.
{
    printf '%s\n' 'aura.build.v1'
    printf 'main_sha256=%s\n' "$(hash_file "$BIN_DIR/Aura DAW")"
    printf 'worker_sha256=%s\n' "$(hash_file "$BIN_DIR/aura-plugin-host-worker")"
    printf 'worker_source=build-tools/aura-plugin-host-worker\n'
} > "$BUILD_MANIFEST"
if command -v codesign >/dev/null 2>&1; then
    codesign --force --deep --sign "$SIGNING_IDENTITY" "$APP_DIR" >/dev/null
fi

# Publish atomically only after the complete bundle has been built and signed.
# A failed compile or signing step therefore leaves the previous preview bundle
# intact instead of deleting the last known-good artifact.
rm -rf "$APP_DIR_TARGET.new"
mv "$APP_DIR" "$APP_DIR_TARGET.new"
rm -rf "$APP_DIR_TARGET"
mv "$APP_DIR_TARGET.new" "$APP_DIR_TARGET"

echo "Built: $APP_DIR_TARGET"
