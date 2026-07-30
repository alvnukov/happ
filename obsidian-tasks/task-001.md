---
id: task-001
title: Add fleet-wide manifest query operation
status: todo
priority: high
model_level: high
task_type: feature
tags:
    - mcp
    - helm-apps
    - fleet-query
acceptance_criteria:
    - Tool schema advertises query_manifests and its input shape
    - The operation queries manifests across enabled applications in one call
    - Each jq record contains group, app, and parsed manifest
    - kind/resource filters are applied before jq
    - Disabled applications are not force-rendered into fleet results
    - Existing render and values query behavior remains unchanged
    - Focused Rust tests and formatting/lint checks pass
verification_plan:
    - cargo fmt --check
    - cargo test mcp::tools::helm_apps::tests::query_manifests
    - cargo clippy --lib --locked -- -D warnings
created_at: "2026-07-30T19:44:09.020005Z"
updated_at: "2026-07-30T19:44:09.020005Z"
---

## Body

Add helm_apps op=query_manifests that renders enabled applications server-side, parses Kubernetes documents, applies optional kind/resource prefilters, and runs jq over compact provenance-aware records without returning the entire fleet render.

## Acceptance Criteria

- Tool schema advertises query_manifests and its input shape
- The operation queries manifests across enabled applications in one call
- Each jq record contains group, app, and parsed manifest
- kind/resource filters are applied before jq
- Disabled applications are not force-rendered into fleet results
- Existing render and values query behavior remains unchanged
- Focused Rust tests and formatting/lint checks pass

## Verification Plan

1. cargo fmt --check
2. cargo test mcp::tools::helm_apps::tests::query_manifests
3. cargo clippy --lib --locked -- -D warnings
