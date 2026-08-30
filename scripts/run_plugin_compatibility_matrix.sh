#!/bin/sh
set -eu

# Produce an auditable inventory before running format-specific smoke tests.
# Missing vendors are reported as SKIP rather than being mistaken for passes.
OUT=${AURA_PLUGIN_MATRIX_REPORT:-artifacts/plugin-compatibility.tsv}
mkdir -p "$(dirname "$OUT")"
printf 'format\tkind\tpath\tarchitecture\tpreset_state\taudio_process\tgui\trecovery\tstatus\n' >"$OUT"
host_arch=$(uname -m)
bundle_kind() {
  path=$1
  format=$2
  case "$format" in
    AU)
      plist="$path/Contents/Info.plist"
      au_type=$(/usr/libexec/PlistBuddy -c 'Print :AudioComponents:0:type' "$plist" 2>/dev/null || true)
      if printf '%s' "$au_type" | grep -Eiq 'aumu|aufn|instrument|synth|generator'; then
        printf instrument
      else
        printf effect
      fi
      ;;
    VST3)
      moduleinfo="$path/Contents/Resources/moduleinfo.json"
      if [ -f "$moduleinfo" ] && grep -Eiq 'Instrument|Synth|Generator' "$moduleinfo"; then
        printf instrument
      elif [ -f "$moduleinfo" ] && grep -Eiq 'Fx|Effect|Analyzer' "$moduleinfo"; then
        printf effect
      else
        printf unknown
      fi
      ;;
    CLAP)
      # CLAP files do not have a portable bundle metadata path; preserve
      # unknown unless a sidecar explicitly declares the feature set.
      sidecar="$path.json"
      if [ -f "$sidecar" ] && grep -Eiq 'instrument|synth|generator' "$sidecar"; then
        printf instrument
      elif [ -f "$sidecar" ] && grep -Eiq 'effect|analyzer' "$sidecar"; then
        printf effect
      else
        printf unknown
      fi
      ;;
    *)
      printf unknown
      ;;
  esac
}
binary_arch() {
  path=$1
  # `file` reports the architectures of a Mach-O bundle's executable.  Keep
  # Unknown explicit for bundles whose binary cannot be inspected.
  executable=$(find "$path" -type f -perm -111 2>/dev/null | head -n 1 || true)
  [ -n "$executable" ] || { printf unknown; return; }
  description=$(file -b "$executable" 2>/dev/null || true)
  case "$description" in
    *'universal binary'*|*'two architectures'*|*'three architectures'*) printf universal ;;
    *arm64*) printf arm64 ;;
    *x86_64*) printf x86_64 ;;
    *) printf unknown ;;
  esac
}
scan() {
  format=$1
  path=$2
  kind=$(bundle_kind "$path" "$format")
  architecture=$(binary_arch "$path")
  if [ -e "$path" ]; then status=FOUND; else status=SKIP; fi
  printf '%s\t%s\t%s\t%s\tUNVERIFIED\tUNVERIFIED\tUNVERIFIED\tUNVERIFIED\t%s\n' "$format" "$kind" "$path" "$architecture" "$status" >>"$OUT"
}

for root in "$HOME/Library/Audio/Plug-Ins/Components" /Library/Audio/Plug-Ins/Components; do
  [ -d "$root" ] || continue
  for path in "$root"/*.component; do [ -e "$path" ] && scan AU "$path"; done
done
for root in "$HOME/Library/Audio/Plug-Ins/VST3" /Library/Audio/Plug-Ins/VST3; do
  [ -d "$root" ] || continue
  for path in "$root"/*.vst3; do [ -e "$path" ] && scan VST3 "$path"; done
done
for root in "$HOME/Library/Audio/Plug-Ins/CLAP" /Library/Audio/Plug-Ins/CLAP; do
  [ -d "$root" ] || continue
  for path in "$root"/*.clap; do [ -e "$path" ] && scan CLAP "$path"; done
done

printf 'plugin compatibility inventory written: %s\n' "$OUT"
MARKDOWN_OUT=${AURA_PLUGIN_MATRIX_MARKDOWN:-${OUT%.tsv}.md}
{
  printf '| Format | Kind | Path | Architecture | Preset/state | Audio process | GUI | Recovery | Status |\n'
  printf '|---|---|---|---|---|---|---|---|---|\n'
  awk -F '\t' 'NR > 1 { gsub(/\|/, "\\\\|", $3); printf "| %s | %s | `%s` | %s | %s | %s | %s | %s | %s |\n", $1,$2,$3,$4,$5,$6,$7,$8,$9 }' "$OUT"
} >"$MARKDOWN_OUT"
printf 'plugin compatibility markdown written: %s\n' "$MARKDOWN_OUT"
