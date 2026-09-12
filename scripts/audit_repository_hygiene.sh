#!/bin/sh
set -eu

repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repository_root"

failures=0

require_tracked() {
    path=$1
    if ! git ls-files --error-unmatch "$path" >/dev/null 2>&1; then
        echo "AURA_HYGIENE_ERROR required source file is not tracked: $path" >&2
        failures=$((failures + 1))
    fi
}

require_absent_from_index() {
    pattern=$1
    matches=$(git ls-files | grep -E "$pattern" || true)
    if [ -n "$matches" ]; then
        echo "AURA_HYGIENE_ERROR generated or local artifacts are tracked:" >&2
        echo "$matches" >&2
        failures=$((failures + 1))
    fi
}

require_tracked Cargo.lock
require_tracked rust-toolchain.toml
require_tracked README.md
require_tracked INSTALL.md
require_tracked CONTRIBUTING.md
require_tracked SECURITY.md
require_tracked CODE_OF_CONDUCT.md
require_tracked CHANGELOG.md
require_tracked ROADMAP.md

# Third-party fixture repositories may legitimately contain reference audio;
# only reject generated build trees/bundles and transient diagnostics here.
require_absent_from_index '(^|/)(target|build|build-tools|dist)/|(^|/)(Aura|Aura) DAW\.app/|\.log$|\.tmp$|\.journal$|\.DS_Store$'

# Local fixture clones are useful while developing, but must never be part of
# a source or release review. Keep the default audit compatible with existing
# dirty checkouts; strict CI/release jobs opt in and fail closed until the
# clone is removed from the Git index.
if [ "${AURA_STRICT_SOURCE_HYGIENE:-0}" = "1" ]; then
    require_absent_from_index '(^|/)third_party_synths(/|$)|(^|/)\.openutau-review(/|$)'
fi

if [ "$failures" -ne 0 ]; then
    echo "Repository hygiene failed: $failures policy violation(s)" >&2
    exit 1
fi

echo "Repository hygiene: OK"
