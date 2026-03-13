#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"

failures=()
warnings=()

record_failure() {
  failures+=("$1")
}

record_warning() {
  warnings+=("$1")
}

normalize_req_version() {
  printf '%s\n' "$1" | sed -E 's/^[^0-9]*//; s/[[:space:]].*$//; s/,.*$//'
}

compat_key() {
  local version="$1"
  local major minor
  IFS='.' read -r major minor _ <<<"${version}"
  if [[ -z "${major}" ]]; then
    return 1
  fi
  if [[ "${major}" == "0" ]]; then
    printf '0.%s\n' "${minor:-0}"
  else
    printf '%s\n' "${major}"
  fi
}

compat_pattern() {
  local version="$1"
  local major minor
  IFS='.' read -r major minor _ <<<"${version}"
  if [[ "${major}" == "0" ]]; then
    printf '^0\\.%s(\\.|$)\n' "${minor:-0}"
  else
    printf '^%s(\\.|$)\n' "${major}"
  fi
}

version_gt() {
  local left="$1"
  local right="$2"
  [[ "${left}" != "${right}" ]] && [[ "$(printf '%s\n%s\n' "${right}" "${left}" | sort -V | tail -n1)" == "${left}" ]]
}

latest_matching_version() {
  local pattern="$1"
  grep -E "${pattern}" | sort -V | tail -n1 || true
}

check_zq() {
  local current latest
  current="$(sed -nE 's/^zq = .*tag = "v([^"]+)".*/\1/p' Cargo.toml)"
  if [[ -z "${current}" ]]; then
    record_warning "zq: unable to read current tag from Cargo.toml"
    return
  fi

  latest="$(
    git ls-remote --tags --refs https://github.com/alvnukov/zq 'refs/tags/v*' \
      | awk -F/ '{print $3}' \
      | sed 's/^v//' \
      | latest_matching_version "$(compat_pattern "${current}")"
  )"
  if [[ -z "${latest}" ]]; then
    record_warning "zq: unable to resolve latest compatible tag for ${current}"
    return
  fi
  if version_gt "${latest}" "${current}"; then
    record_failure "zq: ${current} -> ${latest}"
  fi
}

check_helm_apps() {
  local current latest
  current="$(sed -nE 's/.*HELM_APPS_GITHUB_REF.*"helm-apps-([^"]+)".*/\1/p' build.rs | head -n1)"
  if [[ -z "${current}" ]]; then
    record_warning "helm-apps: unable to read current default ref from build.rs"
    return
  fi

  latest="$(
    git ls-remote --tags --refs https://github.com/alvnukov/helm-apps 'refs/tags/helm-apps-*' \
      | awk -F/ '{print $3}' \
      | sed 's/^helm-apps-//' \
      | latest_matching_version "$(compat_pattern "${current}")"
  )"
  if [[ -z "${latest}" ]]; then
    record_warning "helm-apps: unable to resolve latest compatible tag for ${current}"
    return
  fi
  if version_gt "${latest}" "${current}"; then
    record_failure "helm-apps: ${current} -> ${latest}"
  fi
}

check_rust_registry_dependencies() {
  local metadata
  metadata="$(cargo metadata --format-version 1 --no-deps)"
  while IFS=$'\t' read -r name req; do
    [[ -n "${name}" ]] || continue
    local current lane latest
    current="$(normalize_req_version "${req}")"
    lane="$(compat_key "${current}")"
    latest="$(
      (
        cargo info "${name}@${lane}" 2>/dev/null \
          | awk '/^version:/ {print $2; exit}'
      ) || true
    )"
    if [[ -z "${latest}" ]]; then
      latest="$(
        (
          curl -fsSL --retry 2 --retry-delay 1 "https://crates.io/api/v1/crates/${name}" 2>/dev/null \
            | jq -r '.versions[] | select(.yanked | not) | .num' 2>/dev/null \
            | while read -r version; do
                [[ "$(compat_key "${version}")" == "${lane}" ]] && printf '%s\n' "${version}"
              done \
            | sort -V \
            | tail -n1
        ) || true
      )"
    fi
    if [[ -z "${latest}" ]]; then
      record_warning "rust: unable to resolve latest compatible version for ${name} (${current})"
      continue
    fi
    if version_gt "${latest}" "${current}"; then
      record_failure "rust: ${name} ${current} -> ${latest}"
    fi
  done < <(
    jq -r '
      .packages[]
      | select(.name == "happ")
      | .dependencies[]
      | select((.kind == null or .kind == "build") and (.source // "" | startswith("registry+")))
      | [.name, .req]
      | @tsv
    ' <<<"${metadata}"
  )
}

check_go_dependencies() {
  local helper_dir="src/go_compat/helm_ir_ffi_helper"
  while read -r name current; do
    [[ -n "${name}" ]] || continue
    local latest
    latest="$(
      cd "${helper_dir}" && GOPROXY='https://proxy.golang.org|direct' go list -mod=mod -u -m -f '{{if .Update}}{{.Update.Version}}{{end}}' "${name}"
    )"
    if [[ -n "${latest}" ]] && [[ "$(compat_key "${latest}")" == "$(compat_key "${current}")" ]] && version_gt "${latest}" "${current}"; then
      record_failure "go: ${name} ${current} -> ${latest}"
    fi
  done < <(
    awk '
      $1 == "require" && $2 == "(" && !seen { in_block = 1; seen = 1; next }
      in_block && $1 == ")" { in_block = 0; next }
      in_block { print $1, $2 }
    ' "${helper_dir}/go.mod"
  )
}

check_node_dependencies() {
  while IFS=$'\t' read -r name req; do
    [[ -n "${name}" ]] || continue
    local current latest
    current="$(normalize_req_version "${req}")"
    latest="$(
      cd web && npm view --json --fetch-retries=1 --fetch-timeout=15000 "${name}@${req}" version 2>/dev/null \
        | jq -r 'if type == "array" then .[-1] else . end'
    )"
    if [[ -z "${latest}" ]]; then
      record_warning "js: unable to resolve latest compatible version for ${name} (${req})"
      continue
    fi
    if version_gt "${latest}" "${current}"; then
      record_failure "js: ${name} ${current} -> ${latest}"
    fi
  done < <(
    jq -r '
      ((.dependencies // {}) + (.devDependencies // {}))
      | to_entries[]
      | [.key, .value]
      | @tsv
    ' web/package.json
  )
}

check_zq
check_helm_apps
check_rust_registry_dependencies
check_go_dependencies
check_node_dependencies

if ((${#warnings[@]} > 0)); then
  printf 'release dependency freshness check warnings:\n' >&2
  printf '  - %s\n' "${warnings[@]}" >&2
fi

if ((${#failures[@]} > 0)); then
  printf 'release dependency freshness check failed:\n' >&2
  printf '  - %s\n' "${failures[@]}" >&2
  exit 1
fi

printf 'release dependency freshness check passed.\n'
