#!/bin/sh
set -eu

repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
app_path=${AURA_APP_PATH:-"$repository_root/packaging/Aura DAW.app"}
binary="$app_path/Contents/MacOS/Aura DAW"

if [ ! -x "$binary" ]; then
    echo "error: packaged Aura executable is missing: $binary" >&2
    exit 1
fi

if ! command -v open >/dev/null 2>&1; then
    echo "error: macOS 'open' command is required for GUI launch smoke" >&2
    exit 1
fi

open -n "$app_path"
found_pid=""
cleanup() {
    if [ -n "$found_pid" ]; then
        kill "$found_pid" 2>/dev/null || true
        wait "$found_pid" 2>/dev/null || true
    fi
}
trap cleanup EXIT INT TERM

attempt=0
while [ "$attempt" -lt 30 ]; do
    found_pid=$(pgrep -f "$binary" | head -1 || true)
    [ -n "$found_pid" ] && break
    attempt=$((attempt + 1))
    sleep 1
done

if [ -z "$found_pid" ]; then
    echo "error: Aura app did not appear after 30 seconds" >&2
    exit 1
fi

if ! kill -0 "$found_pid" 2>/dev/null; then
    echo "error: Aura app exited before launch smoke completed" >&2
    exit 1
fi

echo "GUI launch smoke passed: pid=$found_pid app=$app_path"
