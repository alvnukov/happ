#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HAPP_BIN="${HAPP_BIN:-$ROOT_DIR/target/debug/happ}"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/happ-remote-smoke.XXXXXX)}"

CHART_REFS=(
  "ingress-nginx/ingress-nginx"
  "argo/argo"
  "argo/argo-cd"
  "argo/argo-events"
  "argo/argo-rollouts"
  "argo/argo-workflows"
  "argo/argo-ci"
  "argo/argo-lite"
  "argo/argocd-notifications"
  "argo/argocd-image-updater"
  "argo/argocd-applicationset"
  "grafana/grafana"
  "grafana/loki-stack"
  "grafana/promtail"
  "grafana/alloy"
  "grafana/pyroscope"
  "grafana/k6-operator"
  "grafana/grafana-agent"
  "grafana/grafana-agent-operator"
  "grafana/cloudcost-exporter"
)

mkdir -p "$WORK_DIR/charts" "$WORK_DIR/out" "$WORK_DIR/values"

run_cmd() {
  echo "+ $*"
  "$@"
}

pull_chart_with_retry() {
  local ref="$1"
  local dst="$2"
  local attempt
  for attempt in 1 2 3; do
    if run_cmd helm pull "$ref" --untar --untardir "$dst"; then
      return 0
    fi
    echo "pull attempt $attempt failed for $ref" >&2
    sleep $((attempt * 2))
  done
  return 1
}

if [[ ! -x "$HAPP_BIN" ]]; then
  echo "happ binary not found/executable: $HAPP_BIN" >&2
  exit 2
fi

run_cmd helm repo add ingress-nginx https://kubernetes.github.io/ingress-nginx || true
run_cmd helm repo add argo https://argoproj.github.io/argo-helm || true
run_cmd helm repo add grafana https://grafana.github.io/helm-charts || true
run_cmd helm repo update

failed=()
idx=0
for ref in "${CHART_REFS[@]}"; do
  idx=$((idx + 1))
  chart_root="$WORK_DIR/charts/$idx"
  mkdir -p "$chart_root"
  echo ""
  echo "== [$idx/${#CHART_REFS[@]}] $ref =="
  if ! pull_chart_with_retry "$ref" "$chart_root"; then
    failed+=("$ref (pull)")
    continue
  fi

  chart_name="${ref#*/}"
  chart_path="$chart_root/$chart_name"
  if [[ ! -f "$chart_path/Chart.yaml" ]]; then
    failed+=("$ref (chart path)")
    continue
  fi

  common_values_args=()

  out_default="$WORK_DIR/out/${idx}-default-values.yaml"
  if ! run_cmd "$HAPP_BIN" chart \
      --path "$chart_path" \
      --release-name "smoke-$idx-default" \
      --import-strategy raw \
      "${common_values_args[@]}" \
      --output "$out_default"; then
    failed+=("$ref (import-default)")
    continue
  fi

  override_file="$WORK_DIR/values/${idx}-override.yaml"
  : >"$override_file"
  if rg -n '^fullnameOverride:' "$chart_path/values.yaml" >/dev/null 2>&1; then
    printf "fullnameOverride: \"smoke-%s-override\"\n" "$idx" >"$override_file"
  elif rg -n '^nameOverride:' "$chart_path/values.yaml" >/dev/null 2>&1; then
    printf "nameOverride: \"smoke-%s-override\"\n" "$idx" >"$override_file"
  elif rg -n '^namespaceOverride:' "$chart_path/values.yaml" >/dev/null 2>&1; then
    printf "namespaceOverride: \"smoke-%s\"\n" "$idx" >"$override_file"
  fi

  out_override="$WORK_DIR/out/${idx}-override-values.yaml"
  if [[ -s "$override_file" ]]; then
    if ! run_cmd "$HAPP_BIN" chart \
        --path "$chart_path" \
        --release-name "smoke-$idx-override" \
        --import-strategy raw \
        "${common_values_args[@]}" \
        --values "$override_file" \
        --output "$out_override"; then
      failed+=("$ref (import-override)")
      continue
    fi
  else
    if ! run_cmd "$HAPP_BIN" chart \
        --path "$chart_path" \
        --release-name "smoke-$idx-override" \
        --import-strategy raw \
        "${common_values_args[@]}" \
        --output "$out_override"; then
      failed+=("$ref (import-override-no-values)")
      continue
    fi
  fi
done

echo ""
if [[ ${#failed[@]} -gt 0 ]]; then
  echo "Remote chart smoke FAILED (${#failed[@]} issues):" >&2
  for item in "${failed[@]}"; do
    echo "- $item" >&2
  done
  echo "Artifacts: $WORK_DIR" >&2
  exit 1
fi

echo "Remote chart smoke OK (${#CHART_REFS[@]} charts)."
echo "Artifacts: $WORK_DIR"
