#!/usr/bin/env node
// A development harness for happ's MCP server.
//
// An MCP client spawns a stdio server once, at connect time, and never again.
// That makes iterating on the server painful: every recompile needs a client
// restart, which for an agent means losing the conversation.
//
// This wrapper breaks that coupling. It is itself the MCP server the client
// connects to -- stable, dependency-free, and never rebuilt -- while the real
// `happ mcp --stdio` runs as a child it owns. Requests are proxied through with
// their ids remapped; the child can be stopped, rebuilt, reconfigured and
// restarted underneath a live session, and the client is told to re-read the
// tool list via `notifications/tools/list_changed`.
//
// Everything the harness itself offers lives behind one extra tool, `happ_dev`,
// so the proxied surface stays exactly what the real server publishes.
//
// stdout carries protocol frames and nothing else. Logs go to stderr and to
// .tmp/mcp-dev/.

import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const WRAPPER_VERSION = "1.0.0";
const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.resolve(HERE, "..");
const STATE_DIR = path.join(REPO, ".tmp", "mcp-dev");
const CONFIG_PATH = path.join(STATE_DIR, "config.json");
const CHILD_LOG = path.join(STATE_DIR, "child.stderr.log");
const EVENT_LOG = path.join(STATE_DIR, "events.log");

/// Kept small on purpose: every knob here is one the model may have to reason
/// about, and the defaults are the ones this repo actually builds with.
const DEFAULT_CONFIG = {
  // 1.94.0 rather than the default stable, which this workspace's dependency
  // set refuses to build under.
  toolchain: "1.94.0",
  profile: "debug",
  chart: null,
  languageServers: [],
  extraArgs: [],
  // Language servers can take a couple of minutes to warm up on a cold cache,
  // and a tool call that waits on one must not be cut off before they finish.
  requestTimeoutMs: 300000,
};

const STDERR_TAIL_LINES = 500;
const BUILD_TIMEOUT_MS = 900000;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

let config = loadConfig();
let child = null; // { proc, args, binary, startedAt, initResult, stderrTail }
let starting = null; // in-flight startChild(), so parallel calls share one spawn
let lastChildExit = null;
let lastBuild = null;
let clientInitParams = null;
let initialized = false;
const pending = new Map(); // wrapperId -> { resolve, timer, method }
let nextRequestId = 0;
const startedAt = Date.now();

// ---------------------------------------------------------------------------
// Plumbing
// ---------------------------------------------------------------------------

function ensureStateDir() {
  fs.mkdirSync(STATE_DIR, { recursive: true });
}

function loadConfig() {
  try {
    const stored = JSON.parse(fs.readFileSync(CONFIG_PATH, "utf8"));
    return { ...DEFAULT_CONFIG, ...stored };
  } catch {
    return { ...DEFAULT_CONFIG };
  }
}

function saveConfig() {
  ensureStateDir();
  fs.writeFileSync(CONFIG_PATH, `${JSON.stringify(config, null, 2)}\n`);
}

function log(message) {
  const line = `${new Date().toISOString()} ${message}`;
  process.stderr.write(`${line}\n`);
  try {
    ensureStateDir();
    fs.appendFileSync(EVENT_LOG, `${line}\n`);
  } catch {
    // Logging must never take the server down.
  }
}

/// The only function allowed to touch stdout.
function send(message) {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

function respond(id, result) {
  send({ jsonrpc: "2.0", id, result });
}

function respondError(id, code, message) {
  send({ jsonrpc: "2.0", id, error: { code, message } });
}

function notify(method, params) {
  const message = { jsonrpc: "2.0", method };
  if (params !== undefined) message.params = params;
  send(message);
}

/// Tells the client its cached tool and resource lists are stale. Pointless
/// before the handshake, since the client has nothing cached yet.
function announceCatalogChanged() {
  if (!initialized) return;
  notify("notifications/tools/list_changed");
  notify("notifications/resources/list_changed");
}

function textResult(text, isError = false) {
  return { content: [{ type: "text", text }], isError };
}

function tail(text, lines) {
  const all = text.split("\n");
  return all.length <= lines ? text : all.slice(-lines).join("\n");
}

function humanDuration(ms) {
  if (ms < 1000) return `${ms}ms`;
  const seconds = Math.round(ms / 100) / 10;
  if (seconds < 90) return `${seconds}s`;
  return `${Math.round(seconds / 6) / 10}m`;
}

// ---------------------------------------------------------------------------
// Child process
// ---------------------------------------------------------------------------

function binaryPath() {
  return path.join(REPO, "target", config.profile, "happ");
}

function childArgs() {
  const args = ["mcp", "--stdio", "--parent-pid", String(process.pid)];
  if (config.chart) args.push("--chart", config.chart);
  for (const spec of config.languageServers) args.push("--language-server", spec);
  args.push(...config.extraArgs);
  return args;
}

function childRunning() {
  return child !== null && child.proc.exitCode === null && !child.proc.killed;
}

/// Starts the child and completes its LSP-style handshake, so that by the time
/// this resolves the child is ready to answer requests.
async function startChild() {
  const binary = binaryPath();
  if (!fs.existsSync(binary)) {
    throw new Error(
      `no ${config.profile} binary at ${path.relative(REPO, binary)} -- run happ_dev op='rebuild'`,
    );
  }

  const args = childArgs();
  log(`starting ${binary} ${args.join(" ")}`);
  const proc = spawn(binary, args, {
    cwd: REPO,
    stdio: ["pipe", "pipe", "pipe"],
  });

  const instance = {
    proc,
    args,
    binary,
    startedAt: Date.now(),
    initResult: null,
    stderrTail: [],
  };

  proc.on("error", (err) => {
    log(`child failed to spawn: ${err.message}`);
    instance.stderrTail.push(`spawn failed: ${err.message}`);
    handleChildGone(instance, `spawn failed: ${err.message}`);
  });

  proc.on("exit", (code, signal) => {
    handleChildGone(instance, `exited with code=${code} signal=${signal}`);
  });

  createInterface({ input: proc.stdout, crlfDelay: Infinity }).on("line", (line) => {
    if (!line.trim()) return;
    let message;
    try {
      message = JSON.parse(line);
    } catch (err) {
      log(`child emitted a non-JSON frame (${err.message}): ${line.slice(0, 200)}`);
      return;
    }
    onChildMessage(message);
  });

  createInterface({ input: proc.stderr, crlfDelay: Infinity }).on("line", (line) => {
    instance.stderrTail.push(line);
    if (instance.stderrTail.length > STDERR_TAIL_LINES) instance.stderrTail.shift();
    try {
      ensureStateDir();
      fs.appendFileSync(CHILD_LOG, `${line}\n`);
    } catch {
      // ignore
    }
  });

  child = instance;

  // Replay the client's own handshake so the child negotiates the version the
  // client asked for, not one this wrapper invented.
  const params = clientInitParams ?? {
    protocolVersion: "2025-06-18",
    capabilities: {},
    clientInfo: { name: "happ-mcp-dev", version: WRAPPER_VERSION },
  };
  instance.initResult = await childRequest("initialize", params, 30000);
  childNotify("notifications/initialized", {});
  log(`child ready, pid ${proc.pid}`);
  return instance;
}

function handleChildGone(instance, reason) {
  if (child !== instance) return;
  child = null;
  lastChildExit = { at: Date.now(), reason, stderrTail: instance.stderrTail.slice(-40) };
  log(`child gone: ${reason}`);

  // Anything still waiting must be told, or the client hangs until its own
  // timeout with no idea why.
  const complaint = instance.stderrTail.slice(-10).join("\n");
  for (const [id, entry] of pending) {
    clearTimeout(entry.timer);
    entry.reject(
      new Error(
        `happ mcp ${reason} while answering ${entry.method}${complaint ? `\n${complaint}` : ""}`,
      ),
    );
    pending.delete(id);
  }
}

async function stopChild(reason = "requested") {
  if (!childRunning()) {
    child = null;
    return false;
  }
  const instance = child;
  log(`stopping child (${reason})`);
  const exited = new Promise((resolve) => instance.proc.once("exit", resolve));
  instance.proc.kill("SIGTERM");
  const timer = setTimeout(() => instance.proc.kill("SIGKILL"), 3000);
  await exited;
  clearTimeout(timer);
  child = null;
  return true;
}

/// Starts the child if it is not running. Returns `null` on success or the
/// reason it could not start, so callers can report it rather than throw.
async function ensureChild() {
  if (childRunning()) return null;
  if (starting) {
    try {
      await starting;
      return null;
    } catch (err) {
      return err.message;
    }
  }
  starting = startChild();
  try {
    await starting;
    return null;
  } catch (err) {
    await stopChild("failed handshake").catch(() => {});
    return err.message;
  } finally {
    starting = null;
  }
}

function childRequest(method, params, timeoutMs = config.requestTimeoutMs) {
  return new Promise((resolve, reject) => {
    if (!child) {
      reject(new Error("happ mcp is not running"));
      return;
    }
    const id = `w${nextRequestId++}`;
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`${method} timed out after ${humanDuration(timeoutMs)}`));
    }, timeoutMs);
    pending.set(id, { resolve, reject, timer, method });
    child.proc.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
  });
}

function childNotify(method, params) {
  if (!child) return;
  child.proc.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method, params })}\n`);
}

function onChildMessage(message) {
  if (message.id === undefined || message.id === null) {
    // A notification from the server: pass it straight through.
    if (message.method) notify(message.method, message.params);
    return;
  }
  const entry = pending.get(message.id);
  if (!entry) {
    log(`child answered an id nobody was waiting for: ${message.id}`);
    return;
  }
  pending.delete(message.id);
  clearTimeout(entry.timer);
  if (message.error) {
    entry.reject(
      Object.assign(new Error(message.error.message ?? "child reported an error"), {
        rpc: message.error,
      }),
    );
  } else {
    entry.resolve(message.result ?? {});
  }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

function runCommand(command, args, { timeoutMs = BUILD_TIMEOUT_MS } = {}) {
  return new Promise((resolve) => {
    const began = Date.now();
    const proc = spawn(command, args, {
      cwd: REPO,
      stdio: ["ignore", "pipe", "pipe"],
      env: { ...process.env, CARGO_TERM_COLOR: "never" },
    });
    let output = "";
    const capture = (chunk) => {
      output += chunk.toString();
      if (output.length > 400000) output = output.slice(-400000);
    };
    proc.stdout.on("data", capture);
    proc.stderr.on("data", capture);
    const timer = setTimeout(() => proc.kill("SIGKILL"), timeoutMs);
    proc.on("error", (err) => {
      clearTimeout(timer);
      resolve({ ok: false, code: null, output: String(err), durationMs: Date.now() - began });
    });
    proc.on("close", (code) => {
      clearTimeout(timer);
      resolve({ ok: code === 0, code, output, durationMs: Date.now() - began });
    });
  });
}

function cargo(...args) {
  return ["cargo", [`+${config.toolchain}`, ...args]];
}

// ---------------------------------------------------------------------------
// The happ_dev tool
// ---------------------------------------------------------------------------

const DEV_TOOL = {
  name: "happ_dev",
  description: [
    "Controls the happ MCP server this session is talking to, without restarting the session.",
    "The other tools here are served by a child `happ mcp --stdio` process that this harness owns;",
    "these operations rebuild, restart and reconfigure that child in place.",
    "",
    "op='status'  — is the child running, which binary and args, when it was built, what it publishes.",
    "op='rebuild' — cargo build the configured profile, then restart the child on the new binary.",
    "               On a compile error the old binary is put back so the session keeps working.",
    "op='restart' — restart the child on the current binary (picks up an external `cargo build`).",
    "op='stop'    — stop the child; the next tool call starts it again.",
    "op='logs'    — the child's stderr and this harness's event log. Args: lines (default 60).",
    "op='config'  — with no arguments, show the launch configuration; with any of chart, profile,",
    "               toolchain, language_servers, args, timeout_ms, reset, change it and restart.",
    "op='check'   — cargo fmt --check, cargo lint, and the mcp tests, reported together.",
    "op='raw'     — send one JSON-RPC method straight to the child and return its reply verbatim.",
    "               Args: method (required), params. For exercising the protocol layer itself.",
  ].join("\n"),
  inputSchema: {
    type: "object",
    properties: {
      op: {
        type: "string",
        enum: ["status", "rebuild", "restart", "stop", "logs", "config", "check", "raw"],
        description: "The operation to perform.",
      },
      lines: { type: "integer", description: "op='logs': how many lines of each log to show." },
      method: { type: "string", description: "op='raw': JSON-RPC method to send to the child." },
      params: { type: "object", description: "op='raw': params for that method." },
      chart: {
        type: "string",
        description: "op='config': default chart for calls that omit one. Empty string clears it.",
      },
      profile: {
        type: "string",
        enum: ["debug", "release"],
        description: "op='config': which cargo profile's binary to run.",
      },
      toolchain: { type: "string", description: "op='config': cargo toolchain to build with." },
      language_servers: {
        type: "array",
        items: { type: "string" },
        description: "op='config': --language-server overrides, e.g. ['go=/opt/bin/gopls'].",
      },
      args: {
        type: "array",
        items: { type: "string" },
        description: "op='config': extra arguments appended to the child's command line.",
      },
      timeout_ms: { type: "integer", description: "op='config': per-request timeout." },
      reset: { type: "boolean", description: "op='config': restore every default." },
    },
    required: ["op"],
    additionalProperties: false,
  },
};

/// Operations that replace the child run one at a time. Two rebuilds at once
/// would fight over the same output binary, and a restart racing a rebuild
/// could bring back the process the rebuild just stopped.
let lifecycle = Promise.resolve();
function exclusively(action) {
  const run = lifecycle.then(action, action);
  lifecycle = run.then(
    () => {},
    () => {},
  );
  return run;
}

async function devTool(args) {
  const op = args?.op;
  switch (op) {
    case "status":
      return textResult(await statusReport());
    case "rebuild":
      return await exclusively(rebuild);
    case "restart":
      return await exclusively(restart);
    case "stop":
      return await exclusively(async () => {
        const stopped = await stopChild("op='stop'");
        return textResult(stopped ? "child stopped" : "child was not running");
      });
    case "logs":
      return textResult(logsReport(args.lines ?? 60));
    case "config":
      return await exclusively(() => configure(args));
    case "check":
      return await check();
    case "raw":
      return await raw(args);
    default:
      return textResult(
        `unknown op '${op ?? ""}' -- expected one of ${DEV_TOOL.inputSchema.properties.op.enum.join(", ")}`,
        true,
      );
  }
}

async function statusReport() {
  const lines = [];
  const binary = binaryPath();
  lines.push(`harness    running for ${humanDuration(Date.now() - startedAt)}, pid ${process.pid}`);
  lines.push(`repo       ${REPO}`);

  if (childRunning()) {
    lines.push(
      `child      running, pid ${child.proc.pid}, up ${humanDuration(Date.now() - child.startedAt)}`,
    );
    lines.push(`command    ${child.binary} ${child.args.join(" ")}`);
  } else {
    lines.push("child      not running (it starts on the next tool call)");
    if (lastChildExit) {
      lines.push(`last exit  ${lastChildExit.reason}`);
      if (lastChildExit.stderrTail.length) {
        lines.push(`           ${lastChildExit.stderrTail.slice(-3).join("\n           ")}`);
      }
    }
  }

  if (fs.existsSync(binary)) {
    const stat = fs.statSync(binary);
    const age = humanDuration(Date.now() - stat.mtimeMs);
    lines.push(
      `binary     ${path.relative(REPO, binary)}, ${(stat.size / 1048576).toFixed(1)}MiB, built ${age} ago`,
    );
  } else {
    lines.push(`binary     ${path.relative(REPO, binary)} MISSING -- run op='rebuild'`);
  }

  lines.push(
    `config     profile=${config.profile} toolchain=${config.toolchain} chart=${config.chart ?? "-"} timeout=${humanDuration(config.requestTimeoutMs)}`,
  );
  if (config.languageServers.length) {
    lines.push(`           language servers: ${config.languageServers.join(", ")}`);
  }
  if (config.extraArgs.length) lines.push(`           extra args: ${config.extraArgs.join(" ")}`);

  if (lastBuild) {
    lines.push(
      `last build ${lastBuild.ok ? "ok" : "FAILED"} in ${humanDuration(lastBuild.durationMs)} (${lastBuild.command})`,
    );
  }

  if (childRunning()) {
    try {
      const listed = await childRequest("tools/list", {}, 15000);
      const names = (listed.tools ?? []).map((tool) => tool.name);
      lines.push(`publishes  ${names.join(", ")} (+ happ_dev from this harness)`);
    } catch (err) {
      lines.push(`publishes  could not be listed: ${err.message}`);
    }
    if (child.stderrTail.length) {
      lines.push(`stderr     ${child.stderrTail.length} line(s) captured -- see op='logs'`);
    }
  }

  if (pending.size) lines.push(`in flight  ${pending.size} request(s)`);
  return lines.join("\n");
}

async function rebuild() {
  // Stop first: leaving the old process running would keep answering with the
  // old code, and the point of a rebuild is to stop doing that.
  await stopChild("rebuilding");

  const profileArgs = config.profile === "release" ? ["--release"] : [];
  const [command, args] = cargo("build", "--bin", "happ", ...profileArgs);
  log(`rebuilding: ${command} ${args.join(" ")}`);
  const result = await runCommand(command, args);
  lastBuild = {
    ok: result.ok,
    durationMs: result.durationMs,
    command: `${command} ${args.join(" ")}`,
    at: Date.now(),
  };

  if (!result.ok) {
    // The previous binary is untouched on a failed build, so bring it back:
    // a broken edit should cost the new behaviour, not the whole session.
    const restarted = await ensureChild();
    return textResult(
      [
        `build FAILED in ${humanDuration(result.durationMs)} (exit ${result.code})`,
        restarted
          ? `the previous binary could not be restarted either: ${restarted}`
          : "the previous binary is running again, so the other tools still work",
        "",
        tail(result.output.trimEnd(), 80),
      ].join("\n"),
      true,
    );
  }

  const failure = await ensureChild();
  announceCatalogChanged();
  if (failure) {
    return textResult(
      `build ok in ${humanDuration(result.durationMs)}, but the new binary would not start:\n${failure}`,
      true,
    );
  }
  const warnings = result.output.split("\n").filter((line) => line.startsWith("warning:")).length;
  return textResult(
    [
      `build ok in ${humanDuration(result.durationMs)}${warnings ? ` (${warnings} warning line(s))` : ""}`,
      `restarted on ${path.relative(REPO, binaryPath())}`,
      await statusReport(),
    ].join("\n\n"),
  );
}

async function restart() {
  await stopChild("op='restart'");
  const failure = await ensureChild();
  announceCatalogChanged();
  if (failure) return textResult(`restart failed: ${failure}`, true);
  return textResult(`restarted\n\n${await statusReport()}`);
}

function logsReport(lines) {
  const parts = [];
  const live = child?.stderrTail ?? lastChildExit?.stderrTail ?? [];
  parts.push(
    `# happ mcp stderr (${live.length ? `last ${Math.min(lines, live.length)} of ${live.length} live` : "nothing captured this run"})`,
  );
  if (live.length) parts.push(live.slice(-lines).join("\n"));
  else if (fs.existsSync(CHILD_LOG)) {
    parts.push(tail(fs.readFileSync(CHILD_LOG, "utf8").trimEnd(), lines));
  }

  parts.push("", "# harness events");
  parts.push(
    fs.existsSync(EVENT_LOG)
      ? tail(fs.readFileSync(EVENT_LOG, "utf8").trimEnd(), lines)
      : "(none)",
  );
  parts.push("", `logs on disk: ${path.relative(REPO, CHILD_LOG)}, ${path.relative(REPO, EVENT_LOG)}`);
  return parts.join("\n");
}

async function configure(args) {
  const changes = [];

  if (args.reset) {
    config = { ...DEFAULT_CONFIG };
    changes.push("reset to defaults");
  }
  if (args.chart !== undefined) {
    config.chart = args.chart === "" ? null : args.chart;
    changes.push(`chart=${config.chart ?? "-"}`);
  }
  if (args.profile !== undefined) {
    config.profile = args.profile;
    changes.push(`profile=${config.profile}`);
  }
  if (args.toolchain !== undefined) {
    config.toolchain = args.toolchain;
    changes.push(`toolchain=${config.toolchain}`);
  }
  if (args.language_servers !== undefined) {
    config.languageServers = args.language_servers;
    changes.push(`language servers=${config.languageServers.join(", ") || "-"}`);
  }
  if (args.args !== undefined) {
    config.extraArgs = args.args;
    changes.push(`extra args=${config.extraArgs.join(" ") || "-"}`);
  }
  if (args.timeout_ms !== undefined) {
    config.requestTimeoutMs = args.timeout_ms;
    changes.push(`timeout=${humanDuration(config.requestTimeoutMs)}`);
  }

  if (!changes.length) {
    return textResult(
      [
        "launch configuration (pass any of these as arguments to change it):",
        JSON.stringify(config, null, 2),
        "",
        `child command: ${binaryPath()} ${childArgs().join(" ")}`,
      ].join("\n"),
    );
  }

  saveConfig();
  await stopChild("configuration changed");
  const failure = await ensureChild();
  announceCatalogChanged();
  const summary = `changed: ${changes.join(", ")}`;
  if (failure) return textResult(`${summary}\nbut the child would not restart: ${failure}`, true);
  return textResult(`${summary}\n\n${await statusReport()}`);
}

async function check() {
  const steps = [
    { label: "fmt", ...spread(cargo("fmt", "--all", "--", "--check")) },
    { label: "lint", ...spread(cargo("lint")) },
    { label: "tests (mcp)", ...spread(cargo("test", "mcp")) },
  ];

  const report = [];
  let failed = false;
  for (const step of steps) {
    const result = await runCommand(step.command, step.args);
    report.push(
      `${result.ok ? "ok  " : "FAIL"} ${step.label} — ${humanDuration(result.durationMs)}`,
    );
    if (!result.ok) {
      failed = true;
      report.push(tail(result.output.trimEnd(), 40), "");
    }
  }
  return textResult(report.join("\n"), failed);
}

function spread([command, args]) {
  return { command, args };
}

async function raw(args) {
  if (!args.method) return textResult("op='raw' needs a method", true);
  const failure = await ensureChild();
  if (failure) return textResult(failure, true);
  try {
    const result = await childRequest(args.method, args.params ?? {});
    return textResult(JSON.stringify(result, null, 2));
  } catch (err) {
    const rpc = err.rpc ? `\n${JSON.stringify(err.rpc, null, 2)}` : "";
    return textResult(`${args.method} failed: ${err.message}${rpc}`, true);
  }
}

// ---------------------------------------------------------------------------
// MCP surface
// ---------------------------------------------------------------------------

const DEV_INSTRUCTIONS = [
  "",
  "",
  "This server is running behind a development harness. `happ_dev` rebuilds, restarts and",
  "reconfigures the happ MCP server in place -- after editing happ's Rust sources, call",
  "`happ_dev` with op='rebuild' and the other tools here are served by the new build. There is no",
  "need to restart this session to pick up a change.",
].join("\n");

function fallbackInitializeResult(requestedVersion, reason) {
  return {
    protocolVersion: requestedVersion ?? "2025-06-18",
    capabilities: { tools: { listChanged: true }, resources: { listChanged: true } },
    serverInfo: { name: "happ-dev", version: WRAPPER_VERSION },
    instructions: [
      `The happ MCP server is not running: ${reason}`,
      "Only `happ_dev` is available until it starts. Call it with op='status' for the reason and",
      "op='rebuild' to build and start it.",
    ].join("\n"),
  };
}

async function onInitialize(id, params) {
  clientInitParams = params;
  const failure = await ensureChild();
  if (failure) {
    log(`initialize with no child: ${failure}`);
    respond(id, fallbackInitializeResult(params?.protocolVersion, failure));
    return;
  }

  const base = child.initResult ?? {};
  respond(id, {
    ...base,
    protocolVersion: base.protocolVersion ?? params?.protocolVersion ?? "2025-06-18",
    capabilities: {
      ...(base.capabilities ?? {}),
      // The harness can change the catalog mid-session even though the child
      // cannot, so these must be advertised regardless of what the child said.
      tools: { ...(base.capabilities?.tools ?? {}), listChanged: true },
      resources: { ...(base.capabilities?.resources ?? {}), listChanged: true },
    },
    serverInfo: {
      name: "happ-dev",
      version: `${base.serverInfo?.version ?? "?"} (harness ${WRAPPER_VERSION})`,
    },
    instructions: `${base.instructions ?? ""}${DEV_INSTRUCTIONS}`,
  });
}

async function onToolsList(id) {
  const failure = await ensureChild();
  if (failure) {
    respond(id, { tools: [DEV_TOOL] });
    return;
  }
  try {
    const listed = await childRequest("tools/list", {}, 30000);
    respond(id, { tools: [...(listed.tools ?? []), DEV_TOOL] });
  } catch (err) {
    log(`tools/list failed: ${err.message}`);
    respond(id, { tools: [DEV_TOOL] });
  }
}

async function onToolsCall(id, params) {
  if (params?.name === DEV_TOOL.name) {
    try {
      respond(id, await devTool(params.arguments ?? {}));
    } catch (err) {
      respond(id, textResult(`happ_dev failed: ${err.stack ?? err.message}`, true));
    }
    return;
  }

  const failure = await ensureChild();
  if (failure) {
    // A tool error rather than a protocol error: the model can read it, and
    // `happ_dev` tells it what to do next.
    respond(id, textResult(`the happ MCP server is not running: ${failure}`, true));
    return;
  }
  try {
    respond(id, await childRequest("tools/call", params));
  } catch (err) {
    respond(id, textResult(`${params?.name ?? "tool"} failed: ${err.message}`, true));
  }
}

async function proxy(id, method, params) {
  const failure = await ensureChild();
  if (failure) {
    respondError(id, -32603, `the happ MCP server is not running: ${failure}`);
    return;
  }
  try {
    respond(id, await childRequest(method, params));
  } catch (err) {
    if (err.rpc) respondError(id, err.rpc.code ?? -32603, err.rpc.message ?? String(err));
    else respondError(id, -32603, err.message);
  }
}

async function handleMessage(message) {
  const { id, method, params } = message;

  if (method === undefined) return; // a response to something we never asked
  if (id === undefined || id === null) {
    if (method === "notifications/initialized") {
      initialized = true;
      return; // the child got its own during startup
    }
    if (childRunning()) childNotify(method, params);
    return;
  }

  switch (method) {
    case "initialize":
      return await onInitialize(id, params);
    // Answered here so a liveness check succeeds even while the child is being
    // rebuilt -- a failed ping can make a client drop the connection.
    case "ping":
      return respond(id, {});
    case "tools/list":
      return await onToolsList(id);
    case "tools/call":
      return await onToolsCall(id, params ?? {});
    default:
      return await proxy(id, method, params ?? {});
  }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

// Messages are handled as they arrive rather than one after another. Serialising
// them would put `happ_dev` behind whatever tool call is currently stuck, and
// `happ_dev` is the thing you reach for precisely when one is: op='restart'
// kills the child, which fails the stuck request instead of waiting it out.
// Ordering toward the child costs nothing to give up, since it reads its stdin
// one line at a time regardless and every reply is matched back by id.
createInterface({ input: process.stdin, crlfDelay: Infinity }).on("line", (line) => {
  if (!line.trim()) return;
  let message;
  try {
    message = JSON.parse(line);
  } catch (err) {
    send({
      jsonrpc: "2.0",
      id: null,
      error: { code: -32700, message: `invalid JSON frame: ${err.message}` },
    });
    return;
  }
  Promise.resolve()
    .then(() => handleMessage(message))
    .catch((err) => {
      log(`handler crashed: ${err.stack ?? err.message}`);
      if (message.id !== undefined && message.id !== null) {
        respondError(message.id, -32603, `harness error: ${err.message}`);
      }
    });
});

process.stdin.on("close", () => {
  stopChild("client disconnected").finally(() => process.exit(0));
});

for (const signal of ["SIGTERM", "SIGINT"]) {
  process.on(signal, () => {
    stopChild(signal).finally(() => process.exit(0));
  });
}

log(`harness ${WRAPPER_VERSION} up, repo ${REPO}`);
