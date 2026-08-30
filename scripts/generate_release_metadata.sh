#!/bin/sh
set -eu

repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
output_dir=${1:-"$repository_root/release-metadata"}
artifact=${2:-}

mkdir -p "$output_dir"
cd "$repository_root"

if ! command -v cargo-cyclonedx >/dev/null 2>&1; then
    echo "error: cargo-cyclonedx is required (cargo install --locked cargo-cyclonedx)" >&2
    exit 1
fi

cargo cyclonedx --all --format json --override-filename aura-sbom
sbom_count=0
for sbom in "$repository_root"/*/aura-sbom.json; do
    [ -f "$sbom" ] || continue
    package_name=$(basename "$(dirname "$sbom")")
    mv "$sbom" "$output_dir/${package_name}-sbom.cdx.json"
    sbom_count=$((sbom_count + 1))
done
if [ "$sbom_count" -eq 0 ]; then
    echo "error: cargo-cyclonedx did not produce a workspace SBOM" >&2
    exit 1
fi

if [ -n "$artifact" ]; then
    if [ ! -e "$artifact" ]; then
        echo "error: release artifact does not exist: $artifact" >&2
        exit 1
    fi
    checksum_target=$artifact
    if [ -d "$artifact" ]; then
        # Bundle checksums must cover the complete directory tree.  `ditto`
        # preserves macOS bundle metadata while producing a portable file
        # that can be hashed and uploaded with other release artifacts.
        bundle_name=$(basename "$artifact")
        archive="$output_dir/${bundle_name%.app}.zip"
        rm -f "$archive"
        if command -v ditto >/dev/null 2>&1; then
            ditto -c -k --sequesterRsrc --keepParent "$artifact" "$archive"
        elif command -v zip >/dev/null 2>&1; then
            # Portable fallback for Linux CI and clean containers. Metadata
            # forks are macOS-specific, but the bundle contents remain fully
            # covered by the checksum.
            archive_dir=$(CDPATH= cd -- "$(dirname "$archive")" && pwd)
            archive_file="$archive_dir/$(basename "$archive")"
            (cd "$(dirname "$artifact")" && zip -qry "$archive_file" "$bundle_name")
        else
            echo "error: ditto or zip is required to archive a bundle" >&2
            exit 1
        fi
        checksum_target=$archive
    fi
    shasum -a 256 "$checksum_target" > "$output_dir/SHA256SUMS"
fi

git rev-parse HEAD > "$output_dir/commit.txt"
git status --porcelain > "$output_dir/worktree-status.txt"
echo "Release metadata written to $output_dir"
