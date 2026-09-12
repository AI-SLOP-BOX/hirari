#!/usr/bin/env bash
set -euo pipefail

# Publication gate for the source repository.  It deliberately checks only
# tracked paths: build products and local fixtures must never become part of a
# source release just because they exist in a developer checkout.
root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root_dir"

required=(LICENSE THIRD_PARTY_NOTICES.md LICENSE-COMBINED-DISTRIBUTION.md docs/PUBLICATION_SCOPE.md)
for path in "${required[@]}"; do
  [[ -f "$path" ]] || { echo "MISSING_REQUIRED_FILE $path" >&2; exit 1; }
done

# `packaging/macos/Info.plist` is tracked source metadata; only generated
# bundles under packaging are forbidden from a source publication.
forbidden_re='(^|/)(target|build|dist|build-tools|\.openutau-review)(/|$)|^packaging/Aura DAW\.app(/|$)'
if git ls-files | grep -E "$forbidden_re"; then
  echo "FORBIDDEN_TRACKED_PATH" >&2
  exit 1
fi

forbidden_media=$(git ls-files -z | xargs -0 -r file --mime-type | awk -F': *' '$2 ~ /^(application\/(x-dosexec|zip)|audio\/|video\/)/ && $1 !~ /^examples\/reference\/aura_codex_original\.wav$/ {print}')
if [[ -n "$forbidden_media" ]]; then
  printf '%s\n' "$forbidden_media"
  echo "GENERATED_BINARY_OR_MEDIA_TRACKED" >&2
  exit 1
fi

echo "PUBLICATION_SCOPE_READY required_files=${#required[@]}"
