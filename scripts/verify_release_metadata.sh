#!/bin/sh
set -eu

metadata_dir=${1:-release-metadata}
checksums="$metadata_dir/SHA256SUMS"

if [ ! -s "$checksums" ]; then
    echo "error: checksum manifest is missing: $checksums" >&2
    exit 1
fi

manifest_dir=$(CDPATH= cd -- "$(dirname "$checksums")" && pwd)
while IFS='  ' read -r expected path; do
    [ -n "${expected:-}" ] || continue
    case "$expected" in
        *[!0-9a-fA-F]*) echo "error: invalid SHA-256 value" >&2; exit 1 ;;
    esac
    [ "${#expected}" -eq 64 ] || { echo "error: invalid SHA-256 length" >&2; exit 1; }
    target=${path#"$metadata_dir/"}
    [ -f "$manifest_dir/$target" ] || { echo "error: checksum target missing: $target" >&2; exit 1; }
    actual=$(shasum -a 256 "$manifest_dir/$target" | awk '{print $1}')
    [ "$actual" = "$expected" ] || { echo "error: checksum mismatch: $target" >&2; exit 1; }
    echo "verified: $target"
done < "$checksums"
