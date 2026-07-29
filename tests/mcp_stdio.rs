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
