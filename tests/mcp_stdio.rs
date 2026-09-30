//! Drives `happ mcp --stdio` the way a real MCP client does: one JSON frame per
//! line, over the binary's actual stdin and stdout.
//!
//! The unit tests cover what each tool answers; this covers the contract a
//! client depends on and that no in-process test can prove -- that stdout
//! carries protocol frames and nothing else, that the handshake completes, and
//! that the process exits when its stdin closes.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_happ")
}

struct Client {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    next_id: i64,
}

impl Client {
    fn start(args: &[&str]) -> Self {
        let mut child = Command::new(bin())
            .arg("mcp")
            .arg("--stdio")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn happ mcp");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let mut client = Self {
            child,
            stdin,
            stdout,
            next_id: 0,
        };
        client.handshake();
        client
    }

    fn handshake(&mut self) {
        let result = self.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "integration-test", "version": "0" },
            }),
        );
        assert_eq!(result["serverInfo"]["name"], "happ");
        self.notify("notifications/initialized", json!({}));
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.stdin, "{message}").expect("write frame");
        self.stdin.flush().expect("flush");
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Sends a request and returns its `result`, failing on a protocol error.
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        }));

        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read frame");
        assert!(!line.trim().is_empty(), "server closed the connection");
        let response: Value = serde_json::from_str(line.trim())
            .unwrap_or_else(|err| panic!("not a JSON frame: {err}\n{line}"));

        assert_eq!(response["jsonrpc"], "2.0", "every frame is JSON-RPC 2.0");
        assert_eq!(response["id"], id, "responses must match their request id");
        assert!(
            response.get("error").is_none(),
            "{method} failed: {}",
            response["error"]
        );
        response["result"].clone()
    }

    /// Calls a tool and returns its text content.
    fn call_tool(&mut self, name: &str, arguments: Value) -> (String, bool) {
        let result = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        let text = result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        (text, result["isError"] == json!(true))
    }

    fn shutdown(mut self) {
        drop(self.stdin);
        let status = self.child.wait().expect("wait for happ");
        assert!(
            status.success(),
            "happ mcp must exit cleanly when stdin closes, got {status:?}"
        );
    }
}

fn chart_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("Chart.yaml"),
        "apiVersion: v2\nname: demo\nversion: 0.1.0\n",
    )
    .expect("Chart.yaml");
    std::fs::write(
        dir.path().join("values.yaml"),
        "global:\n  env: dev\napps-stateless:\n  api:\n    enabled: true\n    replicas:\n      _default: 1\n      prod: 4\n",
    )
    .expect("values.yaml");
    dir
}

#[test]
fn the_handshake_completes_and_advertises_both_tools() {
    let mut client = Client::start(&[]);
    let listed = client.request("tools/list", json!({}));
    let names: Vec<&str> = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert_eq!(names, vec!["helm_apps", "code"]);
    client.shutdown();
}

#[test]
fn a_chart_can_be_explored_end_to_end_over_the_wire() {
    let chart = chart_fixture();
    let path = chart.path().to_string_lossy().to_string();
    let mut client = Client::start(&[]);

    let (overview, failed) =
        client.call_tool("helm_apps", json!({ "op": "overview", "chart": path }));
    assert!(!failed, "{overview}");
    assert!(overview.contains("apps-stateless"), "{overview}");

    let (prod, failed) = client.call_tool(
        "helm_apps",
        json!({
            "op": "resolve", "chart": path,
            "group": "apps-stateless", "app": "api", "env": "prod",
        }),
    );
    assert!(!failed, "{prod}");
    assert!(prod.contains("replicas: 4"), "{prod}");

    client.shutdown();
}

#[test]
fn the_default_chart_from_the_command_line_is_used() {
    let chart = chart_fixture();
    let mut client = Client::start(&["--chart", &chart.path().to_string_lossy()]);
    let (apps, failed) = client.call_tool("helm_apps", json!({ "op": "apps" }));
    assert!(!failed, "{apps}");
    assert!(apps.contains("apps-stateless.api"), "{apps}");
    client.shutdown();
}

#[test]
fn external_code_operations_and_changed_diagnostics_work_over_the_wire() {
    let project = tempfile::tempdir().expect("tempdir");
    let file = project.path().join("agent.go");
    std::fs::write(&file, "func Agent() {}\n").expect("source");
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-lsp.mjs");
    let command = format!("go=node {}", fixture.display());
    let mut client = Client::start(&["--language-server", &command]);
    let file_arg = file.to_string_lossy();

    let (first, failed) =
        client.call_tool("code", json!({ "op": "diagnostics", "file": file_arg }));
    assert!(!failed && first.contains("first version error"), "{first}");
    std::fs::write(&file, "func Agent() { /* changed */ }\n").expect("changed source");
    let (second, failed) =
        client.call_tool("code", json!({ "op": "diagnostics", "file": file_arg }));
    assert!(!failed && second.contains("No diagnostics"), "{second}");

    for op in ["definition", "references", "hover", "symbols", "calls"] {
        let (text, failed) = client.call_tool(
            "code",
            json!({ "op": op, "file": file_arg, "symbol": "Agent" }),
        );
        assert!(!failed && text.contains("Agent"), "{op}: {text}");
        assert!(!text.starts_with("No "), "{op}: {text}");
    }
    let (outgoing, failed) = client.call_tool(
        "code",
        json!({
            "op": "calls", "file": file_arg, "symbol": "Agent", "direction": "outgoing",
        }),
    );
    assert!(!failed && outgoing.contains("Agent"), "{outgoing}");
    let (workspace, failed) = client.call_tool(
        "code",
        json!({
            "op": "symbols", "file": file_arg, "query": "Agent",
        }),
    );
    assert!(!failed && workspace.contains("Agent"), "{workspace}");
    let (languages, failed) = client.call_tool("code", json!({ "op": "languages" }));
    assert!(!failed, "{languages}");
    assert!(
        languages.contains("textDocument/documentSymbol"),
        "{languages}"
    );
    assert!(
        !languages.contains("textDocument/typeDefinition"),
        "{languages}"
    );
    client.shutdown();
}

#[test]
fn helm_apps_110_operations_work_over_the_wire() {
    let chart = chart_fixture();
    let extracted = Command::new(bin())
        .args(["library", "extract", "--out-dir"])
        .arg(chart.path().join("charts/helm-apps"))
        .output()
        .expect("extract library");
    assert!(
        extracted.status.success(),
        "{}",
        String::from_utf8_lossy(&extracted.stderr)
    );
    std::fs::write(chart.path().join("Chart.yaml"),
        "apiVersion: v2\nname: demo\nversion: 0.1.0\ndependencies:\n  - name: helm-apps\n    version: 1.10.1\n")
        .expect("chart dependency");
    std::fs::create_dir(chart.path().join("templates")).expect("templates");
    std::fs::write(
        chart.path().join("templates/init.yaml"),
        "{{- include \"apps-utils.init-library\" $ }}\n",
    )
    .expect("library wiring");
    std::fs::write(
        chart.path().join("values.yaml"),
        include_str!("fixtures/helm-apps-1.10-values.yaml"),
    )
    .expect("values");
    let mut client = Client::start(&["--chart", &chart.path().to_string_lossy()]);
    for op in [
        "overview",
        "apps",
        "lint",
        "contract",
        "template",
        "origin",
        "resolve",
        "diff",
        "query",
        "query_manifests",
    ] {
        let (text, failed) = client.call_tool("helm_apps", json!({
            "op": op, "group": "apps-daemonsets", "app": "node-agent",
            "name": if op == "template" { "apps-daemonsets" } else { "groups" },
            "from_env": "dev", "to_env": "prod",
            "query": if op == "query_manifests" { ".[] | select(.manifest.kind == \"DaemonSet\") | .manifest.kind" } else { ".[\"apps-daemonsets\"][\"node-agent\"].hostNetwork" },
        }));
        assert!(!failed, "{op}: {text}");
        if matches!(op, "overview" | "apps" | "contract" | "template") {
            assert!(text.contains("daemonsets"), "{op}: {text}");
        }
        if op == "query" {
            assert_eq!(text.trim(), "false");
        }
        if op == "query_manifests" {
            assert_eq!(text.trim(), "\"DaemonSet\"");
        }
    }

    let (text, failed) = client.call_tool(
        "helm_apps",
        json!({
            "op": "render", "group": "apps-daemonsets", "app": "node-agent",
            "renderer": "fast", "kind": "DaemonSet",
        }),
    );
    assert!(!failed, "{text}");
    let daemon: serde_json::Value = serde_yaml::from_str(&text).expect("DaemonSet YAML");
    assert_eq!(daemon["kind"], "DaemonSet");
    assert_eq!(
        daemon.pointer("/spec/template/spec/hostNetwork"),
        Some(&json!(false))
    );
    assert_eq!(
        daemon.pointer("/spec/template/spec/hostUsers"),
        Some(&json!(false))
    );
    assert_eq!(
        daemon.pointer("/spec/template/spec/containers/0/image"),
        Some(&json!("nginx:1.27"))
    );
    assert_eq!(
        daemon.pointer("/spec/template/spec/initContainers/0/image"),
        Some(&json!("busybox:1.36"))
    );

    for (version, expected_time_zone) in [("1.23", None), ("1.24", Some(json!("Etc/UTC")))] {
        let (text, failed) = client.call_tool(
            "helm_apps",
            json!({
            "op": "render", "group": "apps-cronjobs", "app": "cleanup",
            "kind": "CronJob",
            "set": { "global.compat.kubeVersion": version },
            }),
        );
        assert!(!failed, "{version}: {text}");
        let cron: serde_json::Value = serde_yaml::from_str(&text).expect("CronJob YAML");
        assert_eq!(cron["kind"], "CronJob");
        assert_eq!(cron.pointer("/spec/timeZone"), expected_time_zone.as_ref());
        assert_eq!(cron.pointer("/spec/suspend"), Some(&json!(false)));
    }
    let (text, failed) = client.call_tool(
        "helm_apps",
        json!({
            "op": "render", "group": "apps-daemonsets", "app": "node-agent",
            "set": { "apps-daemonsets.node-agent.replicas": 1 },
        }),
    );
    assert!(failed && text.contains("E_STRICT_UNKNOWN_KEY"), "{text}");
    client.shutdown();
}

fn library_chart_fixture(values: &str) -> tempfile::TempDir {
    let chart = chart_fixture();
    let library = chart.path().join("charts/helm-apps");
    let extracted = Command::new(bin())
        .args(["library", "extract", "--out-dir"])
        .arg(&library)
        .output()
        .expect("extract library");
    assert!(
        extracted.status.success(),
        "{}",
        String::from_utf8_lossy(&extracted.stderr)
    );
    let metadata: Value = serde_yaml::from_str(
        &std::fs::read_to_string(library.join("Chart.yaml")).expect("embedded chart metadata"),
    )
    .expect("chart metadata");
    assert_eq!(metadata["version"], json!("1.10.1"));
    std::fs::write(chart.path().join("Chart.yaml"),
        "apiVersion: v2\nname: demo\nversion: 0.1.0\ndependencies:\n  - name: helm-apps\n    version: 1.10.1\n")
        .expect("chart dependency");
    std::fs::create_dir(chart.path().join("templates")).expect("templates");
    std::fs::write(
        chart.path().join("templates/init.yaml"),
        "{{- include \"apps-utils.init-library\" $ }}\n",
    )
    .expect("library wiring");
    std::fs::write(chart.path().join("values.yaml"), values).expect("values");
    chart
}

fn render_resource(client: &mut Client, group: &str, app: &str, kind: &str, set: Value) -> Value {
    let (text, failed) = client.call_tool(
        "helm_apps",
        json!({
            "op": "render", "group": group, "app": app, "kind": kind,
            "renderer": "fast", "set": set,
        }),
    );
    assert!(!failed, "{group}.{app}: {text}");
    let resource: Value = serde_yaml::from_str(&text).expect("rendered resource");
    assert_eq!(resource["kind"], json!(kind));
    resource
}

#[test]
fn helm_apps_1101_renderer_corrections_work_over_the_wire() {
    let chart = library_chart_fixture(include_str!("fixtures/helm-apps-1.10.1-values.yaml"));
    let mut client = Client::start(&["--chart", &chart.path().to_string_lossy()]);
    let expected_spec = json!({"podSelector": {"matchLabels": {"prod": "yes", "app": "api"}}});
    for app in ["native", "default", "exact"] {
        let policy = render_resource(
            &mut client,
            "apps-network-policies",
            app,
            "NetworkPolicy",
            json!({}),
        );
        assert_eq!(policy["spec"], expected_spec);
        let (resolved, failed) = client.call_tool(
            "helm_apps",
            json!({
                "op": "query", "query": format!(".[\"apps-network-policies\"][\"{app}\"].spec"),
            }),
        );
        assert!(!failed, "{resolved}");
        assert_eq!(
            serde_json::from_str::<Value>(&resolved).expect("resolved spec"),
            expected_spec
        );
    }
    let (origin, failed) = client.call_tool(
        "helm_apps",
        json!({
            "op": "origin", "group": "apps-network-policies", "app": "native",
            "values_path": "spec.podSelector.matchLabels.prod",
        }),
    );
    assert!(
        !failed && origin.contains("spec.podSelector.matchLabels.prod: yes"),
        "{origin}"
    );
    assert!(!origin.contains("env key"), "{origin}");
    for group in ["apps-custom", "apps-stateless"] {
        let (resolved, failed) = client.call_tool(
            "helm_apps",
            json!({
                "op": "resolve", "group": group, "app": "policy", "values_path": "spec",
            }),
        );
        assert!(
            !failed && resolved.contains("podSelector") && resolved.contains("prod"),
            "{resolved}"
        );
        let (origin, failed) = client.call_tool("helm_apps", json!({
            "op": "origin", "group": group, "app": "policy", "values_path": "spec.podSelector.matchLabels.prod",
        }));
        assert!(
            !failed && origin.contains("spec.podSelector.matchLabels.prod: yes"),
            "{origin}"
        );
        assert!(!origin.contains("env key"), "{origin}");
        let policy = render_resource(&mut client, group, "policy", "NetworkPolicy", json!({}));
        assert_eq!(policy["spec"], expected_spec);
    }
    let policy = render_resource(
        &mut client,
        "apps-stateless",
        "volumes",
        "NetworkPolicy",
        json!({
            "apps-stateless.__GroupVars__.type": "apps-network-policies",
            "apps-stateless.volumes.spec": expected_spec,
        }),
    );
    assert_eq!(policy["spec"], expected_spec);
    let deployment = render_resource(
        &mut client,
        "apps-network-policies",
        "native",
        "Deployment",
        json!({
            "apps-network-policies.__GroupVars__.type": "apps-stateless",
            "apps-network-policies.native.containers.main.image": {"name": "alpine", "staticTag": "3"},
        }),
    );
    assert_eq!(
        deployment["spec"]["template"]["spec"]["containers"][0]["image"],
        json!("alpine:3")
    );
    for (app, selector) in [("default", "_default"), ("exact", "prod")] {
        let (origin, failed) = client.call_tool(
            "helm_apps",
            json!({
                "op": "origin", "group": "apps-network-policies", "app": app,
                "values_path": "spec",
            }),
        );
        assert!(
            !failed && origin.contains(&format!("env key '{selector}'")),
            "{origin}"
        );
        assert!(origin.contains("podSelector"), "{origin}");
    }
    let workload = render_resource(
        &mut client,
        "apps-stateless",
        "volumes",
        "Deployment",
        json!({}),
    );
    let mut names: Vec<_> = workload["spec"]["template"]["spec"]["volumes"]
        .as_array()
        .expect("volumes")
        .iter()
        .map(|volume| volume["name"].as_str().expect("volume name"))
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["app-data", "init-data", "volumes-data"]);
    let changed = render_resource(
        &mut client,
        "apps-stateless",
        "volumes",
        "Deployment",
        json!({"apps-stateless.volumes.containers.main.secretEnvVars.TOKEN": "new"}),
    );
    let checksum = "/spec/template/metadata/annotations/checksum~1config";
    assert!(workload.pointer(checksum).is_some(), "{workload}");
    assert_ne!(workload.pointer(checksum), changed.pointer(checksum));

    let (conflict, failed) = client.call_tool("helm_apps", json!({
        "op": "render", "group": "apps-stateless", "app": "volumes",
        "set": {"apps-stateless.volumes.containers.main.volumes": "- name: app-data\n  emptyDir: {}\n"},
    }));
    assert!(
        failed && conflict.contains("E_VOLUME_NAME_CONFLICT"),
        "{conflict}"
    );

    for (group, app, kind, pod_path) in [
        ("apps-jobs", "job", "Job", "/spec/template/spec"),
        (
            "apps-cronjobs",
            "cron",
            "CronJob",
            "/spec/jobTemplate/spec/template/spec",
        ),
    ] {
        let resource = render_resource(&mut client, group, app, kind, json!({}));
        assert_eq!(
            resource.pointer(&format!("{pod_path}/serviceAccountName")),
            Some(&json!("reviewer"))
        );
        let account = render_resource(&mut client, group, app, "ServiceAccount", json!({}));
        assert_eq!(account["metadata"]["name"], json!("reviewer"));
    }
    let child = render_resource(
        &mut client,
        "apps-stateless",
        "parent",
        "ConfigMap",
        json!({}),
    );
    assert_eq!(child["metadata"]["name"], json!("parent-config"));
    assert_eq!(child["data"]["parent"], json!("parent"));
    let (context, failed) = client.call_tool("helm_apps", json!({
        "op": "query_manifests", "kind": "Deployment",
        "query": ".[] | select(.manifest.metadata.name == \"sibling\") | .manifest.metadata.annotations",
    }));
    assert!(!failed, "{context}");
    let context: Value = serde_json::from_str(&context).expect("sibling context");
    assert_eq!(context["context-group"], json!("apps-stateless"));
    assert_eq!(context["context-type"], json!("apps-stateless"));
    assert_eq!(context["context-parent"], json!("false"));
    client.shutdown();
}

#[test]
fn helm_apps_render_skips_blank_optional_files_but_rejects_invalid_documents() {
    let chart = library_chart_fixture(&format!(
        "_include_from_file: optional.yaml\n_include_files: [optional.yaml, missing.yaml, extra.yaml]\n{}",
        include_str!("fixtures/helm-apps-1.10.1-values.yaml"),
    ));
    std::fs::write(chart.path().join("optional.yaml"), " \n").expect("empty optional include");
    std::fs::write(
        chart.path().join("extra.yaml"),
        "apps-configmaps:\n  extra:\n    enabled: true\n    data: |\n      source: included\n",
    )
    .expect("extra include");
    let mut client = Client::start(&["--chart", &chart.path().to_string_lossy()]);
    let extra = render_resource(
        &mut client,
        "apps-configmaps",
        "extra",
        "ConfigMap",
        json!({}),
    );
    assert_eq!(extra["data"]["source"], json!("included"));
    let policy = render_resource(
        &mut client,
        "apps-network-policies",
        "native",
        "NetworkPolicy",
        json!({}),
    );
    assert_eq!(
        policy["spec"]["podSelector"]["matchLabels"]["prod"],
        json!("yes")
    );
    for text in ["# comment\n", "null\n", "- item\n"] {
        std::fs::write(chart.path().join("optional.yaml"), text).expect("invalid include");
        let (message, failed) = client.call_tool(
            "helm_apps",
            json!({
                "op": "render", "group": "apps-network-policies", "app": "native",
            }),
        );
        assert!(
            failed && message.contains("values document must be a YAML map"),
            "{text:?}: {message}"
        );
    }
    client.shutdown();
}

#[test]
fn a_tool_error_is_content_rather_than_a_protocol_error() {
    let mut client = Client::start(&[]);
    let (message, failed) = client.call_tool(
        "helm_apps",
        json!({ "op": "overview", "chart": "/definitely/not/a/chart" }),
    );
    assert!(failed, "a bad path must be flagged as a tool error");
    assert!(message.contains("does not exist"), "{message}");
    client.shutdown();
}

#[test]
fn helm_apps_values_get_code_intelligence_from_happ_itself() {
    let chart = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        chart.path().join("Chart.yaml"),
        "apiVersion: v2\nname: demo\n",
    )
    .expect("Chart.yaml");
    let values = chart.path().join("values.yaml");
    std::fs::write(
        &values,
        "global:\n  env: dev\napps-statelss:\n  api:\n    enabled: true\n",
    )
    .expect("values.yaml");

    let mut client = Client::start(&[]);
    let (report, failed) = client.call_tool(
        "code",
        json!({ "op": "diagnostics", "file": values.to_string_lossy() }),
    );
    assert!(!failed, "{report}");
    assert!(report.contains("E_UNKNOWN_APPS_GROUP"), "{report}");
    client.shutdown();
}

#[test]
fn the_embedded_library_is_readable_as_a_resource() {
    let mut client = Client::start(&[]);
    let listed = client.request("resources/list", json!({}));
    let first = listed["resources"]
        .as_array()
        .and_then(|entries| entries.first())
        .expect("at least one resource");
    let uri = first["uri"].as_str().expect("uri").to_string();
    assert!(uri.starts_with("happ://helm-apps/"), "{uri}");

    let read = client.request("resources/read", json!({ "uri": uri }));
    assert!(read["contents"][0]["text"].is_string());
    client.shutdown();
}

#[test]
fn setup_writes_a_client_config_without_touching_anything_else() {
    let project = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        project.path().join(".mcp.json"),
        r#"{"mcpServers":{"other":{"command":"other-server"}}}"#,
    )
    .expect("seed config");

    let output = Command::new(bin())
        .args(["mcp", "setup", "-c", "claude"])
        .current_dir(project.path())
        .output()
        .expect("run setup");
    assert!(
        output.status.success(),
        "setup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let written: Value = serde_json::from_str(
        &std::fs::read_to_string(project.path().join(".mcp.json")).expect("read config"),
    )
    .expect("valid json");
    assert_eq!(written["mcpServers"]["other"]["command"], "other-server");
    assert_eq!(written["mcpServers"]["happ"]["args"][0], "mcp");
    assert_eq!(written["mcpServers"]["happ"]["args"][1], "--stdio");
}

#[test]
fn setup_dry_run_names_every_file_it_would_touch_and_writes_none_of_them() {
    let project = tempfile::tempdir().expect("tempdir");
    let output = Command::new(bin())
        .args(["mcp", "setup", "-c", "claude,opencode", "--dry-run"])
        .current_dir(project.path())
        .output()
        .expect("run setup");
    assert!(output.status.success());

    // Setup touches three kinds of file per client, so a dry run that listed
    // only the server entry would be quietly incomplete.
    let printed = String::from_utf8_lossy(&output.stdout);
    for expected in [
        ".mcp.json",
        "CLAUDE.md",
        ".claude/skills",
        "opencode.json",
        "AGENTS.md",
        ".opencode/skills",
        "would be",
    ] {
        assert!(
            printed.contains(expected),
            "no mention of {expected}:\n{printed}"
        );
    }

    for untouched in [".mcp.json", "opencode.json", "CLAUDE.md", "AGENTS.md"] {
        assert!(
            !project.path().join(untouched).exists(),
            "a dry run created {untouched}"
        );
    }
    assert!(!project.path().join(".claude").exists());
    assert!(!project.path().join(".opencode").exists());
}

#[test]
fn setup_run_twice_changes_nothing_the_second_time() {
    let project = tempfile::tempdir().expect("tempdir");
    let setup = || {
        let output = Command::new(bin())
            .args(["mcp", "setup", "-c", "claude,opencode"])
            .current_dir(project.path())
            .output()
            .expect("run setup");
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).into_owned()
    };

    setup();
    let before = fingerprint(project.path());
    let printed = setup();

    assert_eq!(
        before,
        fingerprint(project.path()),
        "a second setup must leave every file byte for byte as it was"
    );
    assert!(
        !printed.contains("registered") && printed.contains("already up to date"),
        "and it must say so rather than claim it did the work again:\n{printed}"
    );
}

/// Every file under `root`, with its contents and modification time, so a test
/// can tell "wrote the same bytes again" apart from "did not write".
fn fingerprint(root: &std::path::Path) -> Vec<(PathBuf, String, std::time::SystemTime)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let (Ok(text), Ok(stamp)) = (
                std::fs::read_to_string(&path),
                entry.metadata().and_then(|meta| meta.modified()),
            ) {
                out.push((path, text, stamp));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn an_unknown_setup_client_is_refused_by_name() {
    let output = Command::new(bin())
        .args(["mcp", "setup", "-c", "emacs"])
        .current_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")))
        .output()
        .expect("run setup");
    assert!(!output.status.success(), "unknown client must fail");
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(message.contains("emacs"), "{message}");
}
