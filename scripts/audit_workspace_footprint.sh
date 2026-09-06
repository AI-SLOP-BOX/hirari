#!/bin/sh
set -eu

# Read-only footprint audit. It never removes or rewrites user files; use it
# before packaging or sharing a checkout that has been used for fixture work.
ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT_DIR"

max_bytes=${AURA_WORKSPACE_MAX_BYTES:-1073741824}
total_kib=0
records=$(mktemp "${TMPDIR:-/tmp}/aura-footprint.XXXXXX")
trap 'rm -f "$records"' EXIT INT TERM

printf '%s\n' '== Aura workspace footprint (read-only) =='
for path in dist build-tools artifacts packaging .openutau-review; do
    if [ ! -e "$path" ]; then
        continue
    fi
    kib=$(du -sk "$path" 2>/dev/null | awk 'NR == 1 { print $1 }')
    kib=${kib:-0}
    total_kib=$((total_kib + kib))
    printf '%-18s %8s KiB\n' "$path" "$kib"
done

printf '%s\n' '' 'Largest generated entries:'
du -sk dist build-tools artifacts packaging .openutau-review 2>/dev/null \
    | sort -nr | head -20 || true

for root in dist build-tools artifacts packaging; do
    [ -d "$root" ] || continue
    find "$root" -type f -size +1c -print0 2>/dev/null \
        | while IFS= read -r -d '' file; do
            if command -v shasum >/dev/null 2>&1; then
                hash=$(shasum -a 256 "$file" | awk '{ print $1 }')
            else
                hash=$(sha256sum "$file" | awk '{ print $1 }')
            fi
            size=$(wc -c < "$file" | tr -d ' ')
            printf '%s\t%s\t%s\n' "$hash" "$size" "$file"
        done
done >"$records"

duplicates=$(awk -F '\t' '
    { count[$1 FS $2]++; paths[$1 FS $2] = paths[$1 FS $2] "\n  " $3 }
    END {
        for (key in count) if (count[key] > 1) {
            print "DUPLICATE_CONTENT" key paths[key]; found=1
        }
        exit(found ? 0 : 1)
    }
' "$records" || true)
if [ -n "$duplicates" ]; then
    printf '%s\n%s\n' 'Duplicate generated file contents:' "$duplicates"
else
    printf '%s\n' 'Duplicate generated file contents: none'
fi

total_bytes=$((total_kib * 1024))
if [ "$total_bytes" -gt "$max_bytes" ]; then
    printf 'AURA_FOOTPRINT_ERROR generated workspace footprint %s bytes exceeds %s\n' \
        "$total_bytes" "$max_bytes" >&2
    exit 1
fi

printf 'Workspace footprint: %s bytes (limit %s)\n' "$total_bytes" "$max_bytes"
