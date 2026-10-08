#!/usr/bin/env bash
# Back-compat: Windows installer + portable zip now live in package-windows.sh.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exec "$ROOT/scripts/package-windows.sh" "$@"
