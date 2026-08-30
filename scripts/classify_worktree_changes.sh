#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

report=${1:-"${TMPDIR:-/tmp}/aura-worktree-classification-$(date +%Y%m%d-%H%M%S).tsv"}
mkdir -p "$(dirname "$report")"
printf 'status\tcategory\tpath\n' >"$report"

category_for() {
    path=$1
    case "$path" in
        target/*|build/*|build-tools/*|dist/*|packaging/*|archive/*|*.app/*|*.log|*.tmp|*.journal|*.wav|*.aiff|*.flac|*.metallib|*.zip|*.txt)
            printf '%s' generated ;;
        third_party/*|vendor/*|.openutau-review/*)
            printf '%s' third_party ;;
        docs/*|*.md|LICENSE|THIRD_PARTY_NOTICES.md)
            printf '%s' documentation ;;
        scripts/*|.github/*|*.yml|*.yaml|*.toml)
            printf '%s' tooling ;;
        *)
            printf '%s' product_source ;;
    esac
}

# Treat nested engine repositories as a single auditable product boundary;
# recursively expanding their generated build trees makes this report
# unbounded and can stall review tooling.
{
    # Keep the top-level audit bounded; nested engine repositories are listed
    # as a single boundary below rather than expanding thousands of files.
    git diff --name-status --no-renames -- . ':!aura-core-bridge'
    if [ -d aura-core-bridge/.git ] || [ -f aura-core-bridge/.git ]; then
        printf 'M\taura-core-bridge/\n'
    fi
    git ls-files --others --exclude-standard -- . ':!aura-core-bridge' | sed 's/^/?\t/'
} | while IFS= read -r line; do
    [ -n "$line" ] || continue
    status=$(printf '%s' "$line" | cut -f1)
    path=$(printf '%s' "$line" | cut -f2-)
    # Git quotes unusual paths; keep the raw spelling for an audit trail.
    printf '%s\t%s\t%s\n' "$status" "$(category_for "$path")" "$path" >>"$report"
done

printf 'AURA_WORKTREE_CLASSIFICATION=%s\n' "$report"
awk -F '\t' 'NR > 1 {count[$2]++} END {for (key in count) printf "%s=%d\n", key, count[key]}' "$report" | sort
