#!/usr/bin/env bash
# Single authoritative C++ extinction metric.
#
# DEFINITION (do not vary between sessions):
#   project-owned + tracked C/C++/ObjC/ObjC++ source LOC,
#   counting .inc bodies as source,
#   EXCLUDING vendor, generated, test, and archive trees.
#
#   Extensions: .cpp .cc .cxx .mm .m .c .hpp .hh .h .inc
#   LOC rule:   total lines per `wc -l` (physical lines)
#
# Usage: scripts/measure_cpp_loc.sh [git-ref]   (default HEAD)
set -euo pipefail

REF="${1:-HEAD}"
REPO="$(git rev-parse --show-toplevel)"

git -C "$REPO" ls-tree -r --name-only "$REF" |
  grep -Ei '[.](cpp|cc|cxx|mm|m|c|hpp|hh|h|inc)$' |
  grep -Ev '(^|/)(vendor|external|generated|tests|archive|third_party)/' > /tmp/mcl_files.$$ || true

total=0; files=0
hdr_f=0; hdr_l=0; inc_f=0; inc_l=0; imp_f=0; imp_l=0

while IFS= read -r f; do
  [ -z "$f" ] && continue
  loc=$(git -C "$REPO" show "$REF:$f" 2>/dev/null | wc -l | tr -d ' ')
  case "$f" in
    *.hpp|*.hh|*.h) hdr_f=$((hdr_f+1)); hdr_l=$((hdr_l+loc)) ;;
    *.inc)          inc_f=$((inc_f+1)); inc_l=$((inc_l+loc)) ;;
    *)              imp_f=$((imp_f+1)); imp_l=$((imp_l+loc)) ;;
  esac
  total=$((total+loc)); files=$((files+1))
done < /tmp/mcl_files.$$
rm -f /tmp/mcl_files.$$

printf 'ref                          : %s\n' "$REF"
printf 'files                        : %d\n' "$files"
printf 'TOTAL PROJECT C++/ObjC++ LOC   : %d\n' "$total"
printf '  headers (.hpp/.hh/.h)       : %4d files %7d LOC\n' "$hdr_f" "$hdr_l"
printf '  includes (.inc)             : %4d files %7d LOC\n' "$inc_f" "$inc_l"
printf '  implementations (.cpp/.cc/.mm/.m): %3d files %7d LOC\n' "$imp_f" "$imp_l"
