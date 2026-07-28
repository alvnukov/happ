#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"

addr="${HAPP_UI_ADDR:-127.0.0.1:18088}"
open_browser=true
watch_bundle=true
install_deps=true
build_binary=true
watch_pid=""

usage() {
  cat <<'EOF'
Usage: scripts/ui-dev.sh [options]

Starts a local happ web UI development loop:
  - installs web deps on first run
  - builds CodeMirror bundle
  - optionally watches CodeMirror bundle changes
  - builds happ debug binary
  - starts happ --web

Options:
  --addr HOST:PORT    listen address for happ --web (default: 127.0.0.1:18088)
  --no-browser        do not auto-open browser
  --no-watch          do not watch CodeMirror bundle
  --skip-install      do not run npm ci automatically
  --skip-build        do not run cargo build before start
  -h, --help          show this help
EOF
}

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "error: required command not found: $1" >&2
    exit 1
  fi
}

cleanup() {
  if [[ -n "${watch_pid}" ]] && kill -0 "${watch_pid}" 2>/dev/null; then
    kill "${watch_pid}" 2>/dev/null || true
    wait "${watch_pid}" 2>/dev/null || true
  fi
}

while (($# > 0)); do
  case "$1" in
    --addr)
      addr="${2:?missing value for --addr}"
      shift 2
      ;;
    --no-browser)
      open_browser=false
      shift
      ;;
    --no-watch)
      watch_bundle=false
      shift
      ;;
    --skip-install)
      install_deps=false
      shift
      ;;
    --skip-build)
      build_binary=false
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

trap cleanup EXIT INT TERM

require_cmd cargo
require_cmd npm

if [[ ! -d web/node_modules ]]; then
  if [[ "${install_deps}" != true ]]; then
    echo "error: web/node_modules is missing; rerun without --skip-install" >&2
    exit 1
  fi
  (cd web && npm ci)
fi

(cd web && npm run build:cm)

if [[ "${watch_bundle}" == true ]]; then
  (
    cd web
    npm run build:cm:watch
  ) &
  watch_pid="$!"
fi

if [[ "${build_binary}" == true ]]; then
  cargo build --locked
fi

cat <<EOF
UI dev loop is ready.
Server: http://${addr}
Visual check against running server:
  (cd web && HAPP_WEB_BASE_URL=http://${addr} HAPP_WEB_SKIP_SERVER=1 npm run test:ui:headed)
EOF

exec ./target/debug/happ --web --web-addr "${addr}" --web-open-browser="${open_browser}" < /dev/null
