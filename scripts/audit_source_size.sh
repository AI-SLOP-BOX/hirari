#!/usr/bin/env bash
set -euo pipefail

# Keep first-party implementation files small enough to review. Vendored
# headers and generated build output are intentionally excluded.
limit="${AURA_SOURCE_LINE_LIMIT:-600}"
status=0
while IFS= read -r -d '' file; do
  case "$file" in
    ./src/external/*|./target/*|./build/*) continue ;;
  esac
  lines=$(wc -l < "$file" | tr -d ' ')
  if (( lines > limit )); then
    printf '%5d %s\n' "$lines" "$file"
    status=1
  fi
done < <(find . -path './.git' -prune -o -type f \( -name '*.rs' -o -name '*.hpp' -o -name '*.cpp' -o -name '*.inc' \) -print0)

if (( status != 0 )); then
  echo "SOURCE_SIZE_LIMIT_EXCEEDED (limit=${limit})" >&2
  exit "$status"
fi
echo "SOURCE_SIZE_OK (limit=${limit})"
