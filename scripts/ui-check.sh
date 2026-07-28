#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"

mode="full"
headed=false
debug=false
update_snapshots=false
skip_install=false
skip_build=false
reuse_running_server=false
addr="${HAPP_UI_ADDR:-}"

usage() {
  cat <<'EOF'
Usage: scripts/ui-check.sh [options]

Runs the happ UI verification loop:
  - builds CodeMirror bundle
  - builds happ debug binary
  - runs Playwright smoke and/or visual checks

Options:
  --visual            run only visual snapshot tests
  --e2e               run only e2e tests
  --headed            run Playwright headed
  --debug             run Playwright in debug mode
  --update-snapshots  refresh visual snapshots
  --reuse-running     target an already running happ --web server
  --addr HOST:PORT    server address for --reuse-running (default: 127.0.0.1:18088)
  --skip-install      do not run npm ci automatically
  --skip-build        do not run cargo build before tests
  -h, --help          show this help
EOF
}

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "error: required command not found: $1" >&2
    exit 1
  fi
}

while (($# > 0)); do
  case "$1" in
    --visual)
      mode="visual"
      shift
      ;;
    --e2e)
      mode="e2e"
      shift
      ;;
    --headed)
      headed=true
      shift
      ;;
    --debug)
      debug=true
      shift
      ;;
    --update-snapshots)
      update_snapshots=true
      shift
      ;;
    --reuse-running)
      reuse_running_server=true
      shift
      ;;
    --addr)
      addr="${2:?missing value for --addr}"
      shift 2
      ;;
    --skip-install)
      skip_install=true
      shift
      ;;
    --skip-build)
      skip_build=true
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

if [[ -z "${addr}" ]]; then
  if [[ "${reuse_running_server}" == true ]]; then
    addr="127.0.0.1:18088"
  else
    addr="127.0.0.1:$((20000 + (RANDOM % 20000)))"
  fi
fi

require_cmd cargo
require_cmd npm

if [[ ! -d web/node_modules ]]; then
  if [[ "${skip_install}" == true ]]; then
    echo "error: web/node_modules is missing; rerun without --skip-install" >&2
    exit 1
  fi
  (cd web && npm ci)
fi

(cd web && npm run build:cm)

if [[ "${skip_build}" != true ]]; then
  cargo build --locked
fi

playwright_script="test:ui"
if [[ "${mode}" == "visual" ]]; then
  playwright_script="test:ui:visual"
fi
if [[ "${mode}" == "e2e" ]]; then
  playwright_script="test:ui:e2e"
fi
if [[ "${update_snapshots}" == true ]]; then
  if [[ "${mode}" != "visual" ]]; then
    echo "error: --update-snapshots is supported only with --visual" >&2
    exit 1
  fi
  playwright_script="test:ui:visual:update"
fi
if [[ "${headed}" == true ]]; then
  if [[ "${mode}" != "full" ]]; then
    echo "error: --headed currently supports only the full UI suite" >&2
    exit 1
  fi
  playwright_script="test:ui:headed"
fi
if [[ "${debug}" == true ]]; then
  if [[ "${mode}" != "full" ]]; then
    echo "error: --debug currently supports only the full UI suite" >&2
    exit 1
  fi
  playwright_script="test:ui:debug"
fi

if [[ "${reuse_running_server}" == true ]]; then
  (
    cd web
    HAPP_WEB_BASE_URL="http://${addr}" \
    HAPP_WEB_SKIP_SERVER=1 \
    npm run "${playwright_script}"
  )
  exit 0
fi

(
  cd web
  HAPP_WEB_BASE_URL="http://${addr}" \
  HAPP_WEB_LISTEN_ADDR="${addr}" \
  npm run "${playwright_script}"
)
