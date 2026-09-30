// Deterministic LSP peer for protocol and document-version regression tests.
let input = Buffer.alloc(0);
let initializeId;
let configurations = 0;
let uri;
const range = { start: { line: 0, character: 0 }, end: { line: 0, character: 5 } };
const symbol = () => ({ name: "Agent", kind: 12, uri, range, selectionRange: range });
const send = (message) => {
  const body = JSON.stringify({ jsonrpc: "2.0", ...message });
  process.stdout.write(`Content-Length: ${Buffer.byteLength(body)}\r\n\r\n${body}`);
};
const notify = (method, params) => send({ method, params });
const status = (quiescent) => notify("experimental/serverStatus", { quiescent, health: "ok" });
const publish = (version, diagnostics) => notify("textDocument/publishDiagnostics", { uri, version, diagnostics });
function handle(message) {
  if (!message.method) {
    const count = message.id === "config-two" ? 2 : 0;
    if (!Array.isArray(message.result) || message.result.length !== count || message.result.some(v => v !== null)) {
      send({ id: initializeId, error: { code: -32603, message: "wrong configuration response" } });
      return;
    }
    if (++configurations === 2) {
      send({ id: initializeId, result: { capabilities: {
        textDocumentSync: 1, definitionProvider: true, referencesProvider: true,
        hoverProvider: true, documentSymbolProvider: {}, workspaceSymbolProvider: true,
        callHierarchyProvider: {}, typeDefinitionProvider: false, diagnosticProvider: false,
      } } });
    }
    return;
  }
  const params = message.params ?? {};
  switch (message.method) {
    case "initialize":
      initializeId = message.id;
      send({ id: "config-two", method: "workspace/configuration", params: { items: [{ section: "a" }, { section: "b" }] } });
      send({ id: "config-empty", method: "workspace/configuration", params: { items: [] } });
      return;
    case "initialized": status(true); return;
    case "textDocument/didOpen":
      uri = params.textDocument.uri;
      publish(params.textDocument.version, [{ range, severity: 1, message: "first version error" }]);
      status(true);
      return;
    case "textDocument/didChange":
      uri = params.textDocument.uri;
      status(false);
      publish(params.textDocument.version - 1, [{ range, severity: 1, message: "stale version error" }]);
      status(true);
      setImmediate(() => publish(params.textDocument.version, []));
      return;
    case "exit": process.exit(0); return;
    case "textDocument/definition":
    case "textDocument/references": send({ id: message.id, result: [{ uri, range }] }); return;
    case "textDocument/hover": send({ id: message.id, result: { contents: { kind: "plaintext", value: "func Agent()" } } }); return;
    case "textDocument/documentSymbol": send({ id: message.id, result: [symbol()] }); return;
    case "workspace/symbol": send({ id: message.id, result: [{ name: "Agent", kind: 12, location: { uri, range } }] }); return;
    case "textDocument/prepareCallHierarchy": send({ id: message.id, result: [symbol()] }); return;
    case "callHierarchy/incomingCalls": send({ id: message.id, result: [{ from: symbol(), fromRanges: [range] }] }); return;
    case "callHierarchy/outgoingCalls": send({ id: message.id, result: [{ to: symbol(), fromRanges: [range] }] }); return;
    case "shutdown": send({ id: message.id, result: null }); return;
    default:
      if (message.id !== undefined) send({ id: message.id, error: { code: -32601, message: message.method } });
  }
}
process.stdin.on("data", chunk => {
  input = Buffer.concat([input, chunk]);
  while (true) {
    const split = input.indexOf("\r\n\r\n");
    if (split < 0) break;
    const size = Number(/Content-Length: (\d+)/i.exec(input.subarray(0, split).toString())[1]);
    if (input.length < split + 4 + size) break;
    const body = input.subarray(split + 4, split + 4 + size);
    input = input.subarray(split + 4 + size);
    handle(JSON.parse(body.toString()));
  }
});
process.stdin.on("end", () => process.exit(0));
