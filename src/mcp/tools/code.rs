//! Code intelligence for any language happ can reach a server for.
//!
//! The operations are the questions a reader of unfamiliar code actually asks
//! -- where is this defined, who calls it, what is its type, what is broken --
//! rather than a transcription of the LSP method list. Positions are one-based
//! everywhere, and a symbol can be named instead of pointed at, because a model
//! knows `ServeStdio` but not that it starts at line 412 column 8.

use serde_json::{json, Value as JsonValue};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::{limit_schema, optional_str, optional_u32, required_str, truncate};
use crate::mcp::bridge::{
    detect_language, from_lsp_position, path_to_uri, registry, to_lsp_position, uri_to_path,
    LanguageSpec,
};
use crate::mcp::ServerContext;

pub(crate) const NAME: &str = "code";

pub(crate) fn tool() -> JsonValue {
    json!({
        "name": NAME,
        "description": "\
    Code intelligence backed by real language servers. happ starts the right server for a file and \
keeps it warm; the language is detected from the file name, so `lang` is rarely needed.

  languages                        which languages are available, installed and running
  diagnostics  file                errors and warnings for a file
  definition   file+symbol         where a symbol is defined
  references   file+symbol         every use of a symbol
  hover        file+symbol         type, signature and docs
  symbols      file                symbols declared in a file
  symbols      query               search the whole project by name, no file needed
  calls        file+symbol         callers, or callees with direction='outgoing'

Address a symbol by name (`symbol`) or by position (`line`, `column`, both one-based). The name \
    need only *appear* in `file`: asking about a function that is called there but declared \
    elsewhere is the normal way to find out where it comes from. helm-apps values files are served \
    by happ itself -- for charts, the `helm_apps` tool answers far more.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "op": {
                    "type": "string",
                    "enum": [
                        "languages", "diagnostics", "definition", "references",
                        "hover", "symbols", "calls",
                    ],
                    "description": "Operation to run.",
                },
                "file": {
                    "type": "string",
                    "description": "Path to the source file the question is about.",
                },
                "symbol": {
                    "type": "string",
                    "description": "Name of the symbol to address, e.g. 'ServeStdio'.",
                },
                "line": {
                    "type": "integer",
                    "description": "One-based line, as an alternative to 'symbol'.",
                },
                "column": {
                    "type": "integer",
                    "description": "One-based column, used with 'line'.",
                },
                "query": {
                    "type": "string",
                    "description": "For op='symbols': search the whole project for this name. \
                                    Needs no 'file'; the project is the working directory.",
                },
                "direction": {
                    "type": "string",
                    "enum": ["incoming", "outgoing"],
                    "description": "For op='calls': callers (default) or callees.",
                },
                "lang": {
                    "type": "string",
                    "description": "Force a language id instead of detecting it from the file name.",
                },
                "limit": limit_schema("result lines"),
            },
            "required": ["op"],
        },
    })
}

pub(crate) fn call(context: &ServerContext, args: &JsonValue) -> Result<String, String> {
    let op = required_str(args, "op")?;
    let text = match op.as_str() {
        "languages" => languages(context)?,
        "diagnostics" => diagnostics(context, args)?,
        "definition" => locate(context, args, "textDocument/definition", "definition")?,
        "references" => references(context, args)?,
        "hover" => hover(context, args)?,
        "symbols" => symbols(context, args)?,
        "calls" => calls(context, args)?,
        other => {
            return Err(format!(
                "unknown op '{other}' -- expected one of: languages, diagnostics, definition, \
                 references, hover, symbols, calls"
            ))
        }
    };
    Ok(truncate(text, args, "result lines"))
}

// --- operations -------------------------------------------------------------

fn languages(context: &ServerContext) -> Result<String, String> {
    let mut lines = vec!["# languages happ can answer for".to_string(), String::new()];
    for spec in registry::LANGUAGES {
        let command = context.providers.command_for(spec);
        let state = if spec.in_process {
            "built in".to_string()
        } else if let Some(program) = command.first() {
            if std::path::Path::new(program).is_file() {
                format!("ready ({program})")
            } else {
                format!("not installed -- {}", spec.install_hint)
            }
        } else {
            "misconfigured: no command".to_string()
        };
        let selects = if spec.extensions.is_empty() {
            spec.filenames.join(", ")
        } else {
            spec.extensions
                .iter()
                .map(|extension| format!(".{extension}"))
                .collect::<Vec<String>>()
                .join(" ")
        };
        lines.push(format!(
            "{}\t{state}\n\t{} | {selects}",
            spec.id, spec.label
        ));
    }
    let running = context.providers.running();
    if running.is_empty() {
        lines.push("\nNo server is running yet; the first call for a file starts one.".to_string());
    } else {
        lines.push("\n# running now".to_string());
        for provider in running {
            lines.push(format!(
                "{}\t{}\n\tsupports: {}",
                provider.language,
                provider.root.display(),
                provider.methods.join(" ")
            ));
        }
    }
    lines.push(
        "\nOverride a server with --language-server <lang>=<command> when happ starts.".to_string(),
    );
    Ok(lines.join("\n"))
}

fn diagnostics(context: &ServerContext, args: &JsonValue) -> Result<String, String> {
    let (spec, file) = target_file(args)?;
    let found = context
        .providers
        .with_provider(spec, &file, |provider| provider.diagnostics(&file))?;

    if found.is_empty() {
        return Ok(empty_answer(
            context,
            spec,
            &file,
            format!("No diagnostics: {} is clean.", display_path(&file)),
        ));
    }
    let mut lines: Vec<String> = found
        .iter()
        .map(|diagnostic| {
            let code = diagnostic
                .code
                .as_ref()
                .map(|code| format!(" [{code}]"))
                .unwrap_or_default();
            format!(
                "{}:{}:{} {}{code} {}",
                display_path(&file),
                diagnostic.line,
                diagnostic.column,
                diagnostic.severity,
                diagnostic.message
            )
        })
        .collect();
    lines.push(format!("\n{} diagnostics.", found.len()));
    Ok(lines.join("\n"))
}

fn locate(
    context: &ServerContext,
    args: &JsonValue,
    method: &str,
    what: &str,
) -> Result<String, String> {
    let (spec, file) = target_file(args)?;
    let anchors = resolve_anchors(context, spec, &file, args)?;

    let (asked, result) = ask_until_answered(
        context,
        spec,
        &file,
        &anchors,
        method,
        &json!({}),
        |value| !decode_locations(value).is_empty(),
    )?;

    let locations = decode_locations(&result);
    if locations.is_empty() {
        return Ok(empty_answer(
            context,
            spec,
            &file,
            format!("No {what} found for {}.", anchors[asked].describe()),
        ));
    }
    Ok(format!(
        "# {what} of {}\n{}",
        anchors[asked].describe(),
        render_locations(&locations)
    ))
}

fn references(context: &ServerContext, args: &JsonValue) -> Result<String, String> {
    let (spec, file) = target_file(args)?;
    let anchors = resolve_anchors(context, spec, &file, args)?;

    let (asked, result) = ask_until_answered(
        context,
        spec,
        &file,
        &anchors,
        "textDocument/references",
        &json!({ "context": { "includeDeclaration": false } }),
        |value| !decode_locations(value).is_empty(),
    )?;

    let locations = decode_locations(&result);
    if locations.is_empty() {
        return Ok(empty_answer(
            context,
            spec,
            &file,
            format!(
                "No references to {} -- it is used nowhere else.",
                anchors[asked].describe()
            ),
        ));
    }
    Ok(format!(
        "# {} references to {}\n{}",
        locations.len(),
        anchors[asked].describe(),
        render_locations(&locations)
    ))
}

fn hover(context: &ServerContext, args: &JsonValue) -> Result<String, String> {
    let (spec, file) = target_file(args)?;
    let anchors = resolve_anchors(context, spec, &file, args)?;

    let (asked, result) = ask_until_answered(
        context,
        spec,
        &file,
        &anchors,
        "textDocument/hover",
        &json!({}),
        |value| !decode_hover(value).trim().is_empty(),
    )?;

    let text = decode_hover(&result);
    if text.trim().is_empty() {
        return Ok(empty_answer(
            context,
            spec,
            &file,
            format!("No hover information for {}.", anchors[asked].describe()),
        ));
    }
    Ok(format!("# {}\n{text}", anchors[asked].describe()))
}

fn symbols(context: &ServerContext, args: &JsonValue) -> Result<String, String> {
    if let Some(query) = optional_str(args, "query") {
        let (spec, file) = search_scope(args)?;
        let found = workspace_symbols(context, spec, &file, &query)?;
        if found.is_empty() {
            return Ok(empty_answer(
                context,
                spec,
                &file,
                format!("No symbol matching '{query}' in this project."),
            ));
        }

        // A bare name matches the whole standard library as readily as the
        // project: gopls answers "Store" with a hundred hits from sync/atomic
        // onwards. The caller asked about their project, and every dependency
        // hit is context they pay for and did not want.
        let root = spec.workspace_root(&file);
        let (mine, external): (Vec<&SymbolHit>, Vec<&SymbolHit>) =
            found.iter().partition(|hit| hit.file.starts_with(&root));
        let shown = if mine.is_empty() { &external } else { &mine };

        let omitted = if mine.is_empty() || external.is_empty() {
            String::new()
        } else {
            format!(
                "\n\n({} further matches outside this project were omitted.)",
                external.len()
            )
        };

        return Ok(format!(
            "# {} symbols matching '{query}'\n{}{omitted}",
            shown.len(),
            shown
                .iter()
                .map(|hit| hit.render())
                .collect::<Vec<String>>()
                .join("\n")
        ));
    }

    let (spec, file) = target_file(args)?;
    let found = document_symbols(context, spec, &file)?;
    if found.is_empty() {
        return Ok(empty_answer(
            context,
            spec,
            &file,
            format!("No symbols in {}.", display_path(&file)),
        ));
    }
    Ok(format!(
        "# symbols in {}\n{}",
        display_path(&file),
        found
            .iter()
            .map(SymbolHit::render)
            .collect::<Vec<String>>()
            .join("\n")
    ))
}

fn calls(context: &ServerContext, args: &JsonValue) -> Result<String, String> {
    let (spec, file) = target_file(args)?;
    let anchors = resolve_anchors(context, spec, &file, args)?;
    let outgoing = optional_str(args, "direction").as_deref() == Some("outgoing");
    let method = if outgoing {
        "callHierarchy/outgoingCalls"
    } else {
        "callHierarchy/incomingCalls"
    };

    let (asked, prepared) = ask_until_answered(
        context,
        spec,
        &file,
        &anchors,
        "textDocument/prepareCallHierarchy",
        &json!({}),
        |value| value.as_array().is_some_and(|items| !items.is_empty()),
    )?;

    let direction = if outgoing { "callees" } else { "callers" };
    let result = match prepared.as_array().and_then(|items| items.first()) {
        Some(item) => context.providers.with_provider(spec, &file, |provider| {
            provider.request(method, json!({ "item": item }))
        })?,
        None => JsonValue::Null,
    };

    let Some(entries) = result.as_array().filter(|entries| !entries.is_empty()) else {
        return Ok(empty_answer(
            context,
            spec,
            &file,
            format!("No {direction} found for {}.", anchors[asked].describe()),
        ));
    };

    let mut lines = vec![format!("# {direction} of {}", anchors[asked].describe())];
    for entry in entries {
        let item = entry
            .get(if outgoing { "to" } else { "from" })
            .unwrap_or(&JsonValue::Null);
        let name = item
            .get("name")
            .and_then(JsonValue::as_str)
            .unwrap_or("<unnamed>");
        let location = item
            .get("uri")
            .and_then(JsonValue::as_str)
            .and_then(uri_to_path)
            .map(|path| {
                let (line, column) = from_lsp_position(
                    item.get("selectionRange")
                        .and_then(|range| range.get("start"))
                        .unwrap_or(&JsonValue::Null),
                );
                format!("{}:{line}:{column}", display_path(&path))
            })
            .unwrap_or_default();
        lines.push(format!("{name}\t{location}"));
    }
    lines.push(format!("\n{} {direction}.", entries.len()));
    Ok(lines.join("\n"))
}

// --- addressing -------------------------------------------------------------

/// A resolved point in a file, and how the caller described it.
struct Anchor {
    line: u32,
    column: u32,
    label: String,
    /// Whether this position declares the symbol or merely uses it.
    declared: bool,
}

impl Anchor {
    fn describe(&self) -> String {
        if self.declared {
            format!("{} ({}:{})", self.label, self.line, self.column)
        } else {
            format!("{} (used at {}:{})", self.label, self.line, self.column)
        }
    }
}

/// Every position worth asking the server about, best first.
///
/// A declaration in this file comes first, then the places the name is used. The
/// distinction matters because a name is very often used in one file and
/// declared in another -- `truncate` is called here and defined in the parent
/// module -- and "where does this come from?" is the question a reader asks
/// most. A language server resolves a usage to whatever declared it, so pointing
/// at a usage answers that question instead of refusing it.
fn resolve_anchors(
    context: &ServerContext,
    spec: &'static LanguageSpec,
    file: &Path,
    args: &JsonValue,
) -> Result<Vec<Anchor>, String> {
    if let (Some(line), Some(column)) = (optional_u32(args, "line"), optional_u32(args, "column")) {
        return Ok(vec![Anchor {
            line,
            column,
            label: format!("{}:{line}:{column}", display_path(file)),
            declared: true,
        }]);
    }

    let symbol = optional_str(args, "symbol").ok_or_else(|| {
        "address the code with 'symbol', or with 'line' and 'column' together".to_string()
    })?;

    let in_file = document_symbols(context, spec, file)?;

    // Servers decorate method names with their receiver -- gopls reports
    // `(*Store).Put` -- so matching exactly would reject the name a caller
    // would naturally use. Tried in descending specificity.
    let matchers: [fn(&SymbolHit, &str) -> bool; 3] = [
        |hit, wanted| hit.name == wanted,
        |hit, wanted| undecorated_name(&hit.name) == wanted,
        |hit, wanted| simple_name(&hit.name) == wanted,
    ];
    let mut anchors = Vec::new();
    for matcher in matchers {
        let found: Vec<&SymbolHit> = in_file.iter().filter(|hit| matcher(hit, &symbol)).collect();
        let Some(hit) = found.first() else {
            continue;
        };
        // Same-named members on different receivers: take the first, but name
        // the alternatives rather than silently picking one of several.
        let label = if found.len() > 1 {
            format!(
                "{} (also matched: {})",
                hit.name,
                found
                    .iter()
                    .skip(1)
                    .map(|other| other.name.as_str())
                    .collect::<Vec<&str>>()
                    .join(", ")
            )
        } else {
            hit.name.clone()
        };
        anchors.push(Anchor {
            line: hit.line,
            column: hit.column,
            label,
            declared: true,
        });
        break;
    }

    anchors.extend(usages(file, &symbol));
    if !anchors.is_empty() {
        return Ok(anchors);
    }

    let near: Vec<String> = in_file
        .iter()
        .filter(|hit| {
            hit.name.contains(&symbol) || simple_name(&hit.name).contains(simple_name(&symbol))
        })
        .map(|hit| hit.name.clone())
        .collect();
    if near.is_empty() {
        return Err(format!(
            "'{symbol}' is neither declared nor used in {} (a mention in a comment does not \
             count) -- use op='symbols' to list what is there, or op='symbols' with 'query' to \
             search the whole project",
            display_path(file)
        ));
    }
    Err(format!(
        "'{symbol}' does not appear in {}. Close matches: {}",
        display_path(file),
        near.join(", ")
    ))
}

/// How many usages are worth trying before giving up on a name.
const MAX_USAGE_ANCHORS: usize = 4;

/// Positions where `symbol` appears in `file` as a whole word, in reading order.
///
/// Comment tails are dropped, so a name discussed in prose does not outrank the
/// code that uses it, and only the first few matches are kept: they exist to be
/// tried in turn, and a name used forty times is answered by the first of them.
fn usages(file: &Path, symbol: &str) -> Vec<Anchor> {
    let wanted = simple_name(symbol);
    let Ok(text) = std::fs::read_to_string(file) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let code = line.split("//").next().unwrap_or(line);
        for (offset, _) in code.match_indices(wanted) {
            if !is_whole_word(code, offset, wanted.len()) {
                continue;
            }
            out.push(Anchor {
                line: index as u32 + 1,
                // LSP counts a column in UTF-16 code units, so a line with
                // non-ASCII text before the name would otherwise be off.
                column: code[..offset].encode_utf16().count() as u32 + 1,
                label: wanted.to_string(),
                declared: false,
            });
            if out.len() >= MAX_USAGE_ANCHORS {
                return out;
            }
        }
    }
    out
}

/// Whether the match at `offset` is a whole identifier rather than part of a
/// longer one: `truncate` must not match inside `truncate_all`.
fn is_whole_word(text: &str, offset: usize, length: usize) -> bool {
    let free = |byte: Option<&u8>| !byte.is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
    free(
        offset
            .checked_sub(1)
            .and_then(|before| text.as_bytes().get(before)),
    ) && free(text.as_bytes().get(offset + length))
}

/// Asks the server at each candidate position until one answers.
///
/// Only the first candidate can be a declaration; the rest are usages, and a
/// usage inside a macro body or a string resolves to nothing at all. Trying the
/// next one costs a round trip and saves a refusal.
fn ask_until_answered(
    context: &ServerContext,
    spec: &'static LanguageSpec,
    file: &Path,
    anchors: &[Anchor],
    method: &str,
    extra: &JsonValue,
    answered: impl Fn(&JsonValue) -> bool,
) -> Result<(usize, JsonValue), String> {
    let uri = path_to_uri(file)?;
    context.providers.with_provider(spec, file, |provider| {
        provider.open(file)?;
        let mut last = JsonValue::Null;
        for (index, anchor) in anchors.iter().enumerate() {
            let mut params = json!({
                "textDocument": { "uri": uri },
                "position": to_lsp_position(anchor.line, anchor.column),
            });
            if let (Some(target), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
                for (key, value) in extra {
                    target.insert(key.clone(), value.clone());
                }
            }
            last = provider.request(method, params)?;
            if answered(&last) {
                return Ok((index, last));
            }
        }
        // Nothing answered: report against the best candidate, which is the one
        // the caller most likely meant.
        Ok((0, last))
    })
}

/// `(*Store).Put` -> `Store.Put`: receiver kept, pointer syntax dropped.
fn undecorated_name(name: &str) -> String {
    name.replace(['(', ')', '*', '&'], "")
}

/// `(*Store).Put` -> `Put`: just the member, for a caller who names only that.
fn simple_name(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// Where a project-wide search looks when the caller named no file.
///
/// A search for a symbol across a project is the one question that need not
/// start from a file, and requiring one is a trap: it is what a caller reaches
/// for precisely when they do not know which file to name.
fn search_scope(args: &JsonValue) -> Result<(&'static LanguageSpec, PathBuf), String> {
    if optional_str(args, "file").is_some() {
        return target_file(args);
    }

    let here =
        std::env::current_dir().map_err(|err| format!("resolve current directory: {err}"))?;
    if let Some(forced) = optional_str(args, "lang") {
        let spec = registry::spec_for_language(&forced).ok_or_else(|| {
            format!("unknown language '{forced}' -- op='languages' lists the ones happ knows")
        })?;
        return Ok((spec, here));
    }

    match registry::specs_rooted_at(&here).as_slice() {
        [spec] => Ok((*spec, here)),
        [] => Err(format!(
            "{} is not the root of a project happ recognises -- name a 'file' in the project to \
             search, or pass 'lang'",
            here.display()
        )),
        several => Err(format!(
            "several projects share this directory ({}) -- pass 'lang' to say which one to search",
            several
                .iter()
                .map(|spec| spec.id)
                .collect::<Vec<&str>>()
                .join(", ")
        )),
    }
}

/// The file a question is about, plus the language that answers for it.
fn target_file(args: &JsonValue) -> Result<(&'static LanguageSpec, PathBuf), String> {
    let raw = optional_str(args, "file")
        .ok_or_else(|| "this op needs a 'file' to work from".to_string())?;
    let file = absolute(&raw)?;
    if !file.exists() {
        return Err(format!("no such file: {}", file.display()));
    }

    if let Some(forced) = optional_str(args, "lang") {
        let spec = registry::spec_for_language(&forced).ok_or_else(|| {
            format!("unknown language '{forced}' -- op='languages' lists the ones happ knows")
        })?;
        return Ok((spec, file));
    }

    let spec = detect_language(&file).ok_or_else(|| {
        format!(
            "no language server is registered for {} -- op='languages' lists what happ supports, \
             or pass 'lang' to force one",
            display_path(&file)
        )
    })?;
    Ok((spec, file))
}

fn absolute(path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        return Ok(path);
    }
    std::env::current_dir()
        .map_err(|err| format!("resolve current directory: {err}"))
        .map(|cwd| cwd.join(path))
}

/// The working directory, read once: it cannot change while the server runs.
fn working_dir() -> Option<&'static Path> {
    static CWD: OnceLock<Option<PathBuf>> = OnceLock::new();
    CWD.get_or_init(|| std::env::current_dir().ok()).as_deref()
}

/// How a path is written in an answer: relative to the working directory when
/// it lies inside it, absolute otherwise.
///
/// Most answers are lists of positions, and the repeated project prefix on every
/// line is pure cost -- a hundred references to a symbol spent a hundred copies
/// of `/Users/someone/src/project/` on saying nothing. [`absolute`] resolves the
/// short form against the same directory, so what comes back can be passed
/// straight back in.
fn display_path(path: &Path) -> String {
    working_dir()
        .and_then(|cwd| path.strip_prefix(cwd).ok())
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Turns a confident-looking empty answer into an honest one.
///
/// A language server that could not load the workspace answers every request
/// emptily, so "no symbols in this file" reads as fact when it is really
/// "I could not analyse this project". Whatever the server said about itself
/// belongs in that answer.
fn empty_answer(
    context: &ServerContext,
    spec: &'static LanguageSpec,
    file: &Path,
    said: String,
) -> String {
    let health = context
        .providers
        .with_provider(spec, file, |provider| Ok(provider.health()))
        .ok()
        .flatten();
    match health {
        Some(complaint) => format!(
            "{said}\n\nThe {} language server reported a problem, so this may be a failure to \
             analyse rather than an empty result:\n{complaint}",
            spec.id
        ),
        None => said,
    }
}

// --- decoding ---------------------------------------------------------------

struct SymbolHit {
    name: String,
    kind: String,
    file: PathBuf,
    line: u32,
    column: u32,
    container: Option<String>,
}

impl SymbolHit {
    fn render(&self) -> String {
        let container = self
            .container
            .as_ref()
            .map(|owner| format!(" in {owner}"))
            .unwrap_or_default();
        format!(
            "{}\t{}{container}\t{}:{}:{}",
            self.name,
            self.kind,
            display_path(&self.file),
            self.line,
            self.column
        )
    }
}

fn document_symbols(
    context: &ServerContext,
    spec: &'static LanguageSpec,
    file: &Path,
) -> Result<Vec<SymbolHit>, String> {
    let uri = path_to_uri(file)?;
    let result = context.providers.with_provider(spec, file, |provider| {
        provider.open(file)?;
        provider.request(
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": uri } }),
        )
    })?;

    let mut out = Vec::new();
    collect_symbols(&result, file, None, &mut out);
    Ok(out)
}

fn workspace_symbols(
    context: &ServerContext,
    spec: &'static LanguageSpec,
    file: &Path,
    query: &str,
) -> Result<Vec<SymbolHit>, String> {
    let result = context.providers.with_provider(spec, file, |provider| {
        provider.request("workspace/symbol", json!({ "query": query }))
    })?;

    let mut out = Vec::new();
    collect_symbols(&result, file, None, &mut out);
    Ok(out)
}

/// Reads both symbol shapes the protocol allows: the flat `SymbolInformation`
/// list and the nested `DocumentSymbol` tree.
fn collect_symbols(
    value: &JsonValue,
    file: &Path,
    container: Option<&str>,
    out: &mut Vec<SymbolHit>,
) {
    let Some(entries) = value.as_array() else {
        return;
    };
    for entry in entries {
        let Some(name) = entry.get("name").and_then(JsonValue::as_str) else {
            continue;
        };
        let kind = symbol_kind(entry.get("kind").and_then(JsonValue::as_u64));

        let (hit_file, position) = match entry.get("location") {
            Some(location) => (
                location
                    .get("uri")
                    .and_then(JsonValue::as_str)
                    .and_then(uri_to_path)
                    .unwrap_or_else(|| file.to_path_buf()),
                location.get("range").and_then(|range| range.get("start")),
            ),
            None => (
                file.to_path_buf(),
                entry
                    .get("selectionRange")
                    .or_else(|| entry.get("range"))
                    .and_then(|range| range.get("start")),
            ),
        };
        let (line, column) = from_lsp_position(position.unwrap_or(&JsonValue::Null));

        out.push(SymbolHit {
            name: name.to_string(),
            kind,
            file: hit_file,
            line,
            column,
            container: entry
                .get("containerName")
                .and_then(JsonValue::as_str)
                .map(ToString::to_string)
                .or_else(|| container.map(ToString::to_string)),
        });

        if let Some(children) = entry.get("children") {
            collect_symbols(children, file, Some(name), out);
        }
    }
}

fn symbol_kind(kind: Option<u64>) -> String {
    match kind {
        Some(1) => "file",
        Some(2) => "module",
        Some(3) => "namespace",
        Some(4) => "package",
        Some(5) => "class",
        Some(6) => "method",
        Some(7) => "property",
        Some(8) => "field",
        Some(9) => "constructor",
        Some(10) => "enum",
        Some(11) => "interface",
        Some(12) => "function",
        Some(13) => "variable",
        Some(14) => "constant",
        Some(15) => "string",
        Some(16) => "number",
        Some(17) => "boolean",
        Some(18) => "array",
        Some(19) => "object",
        Some(20) => "key",
        Some(21) => "null",
        Some(22) => "enum-member",
        Some(23) => "struct",
        Some(24) => "event",
        Some(25) => "operator",
        Some(26) => "type-parameter",
        _ => "symbol",
    }
    .to_string()
}

struct Location {
    file: PathBuf,
    line: u32,
    column: u32,
}

/// Reads every location shape the protocol allows: a single `Location`, a list
/// of them, or `LocationLink`s.
fn decode_locations(value: &JsonValue) -> Vec<Location> {
    let entries: Vec<&JsonValue> = match value {
        JsonValue::Array(items) => items.iter().collect(),
        JsonValue::Null => Vec::new(),
        single => vec![single],
    };

    entries
        .into_iter()
        .filter_map(|entry| {
            let uri = entry
                .get("uri")
                .or_else(|| entry.get("targetUri"))
                .and_then(JsonValue::as_str)?;
            let range = entry
                .get("range")
                .or_else(|| entry.get("targetSelectionRange"))
                .or_else(|| entry.get("targetRange"))?;
            let (line, column) = from_lsp_position(range.get("start")?);
            Some(Location {
                file: uri_to_path(uri)?,
                line,
                column,
            })
        })
        .collect()
}

fn render_locations(locations: &[Location]) -> String {
    locations
        .iter()
        .map(|location| {
            let line = read_source_line(&location.file, location.line)
                .map(|text| format!("\t{}", text.trim()))
                .unwrap_or_default();
            format!(
                "{}:{}:{}{line}",
                display_path(&location.file),
                location.line,
                location.column
            )
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// Quotes the referenced line, which is usually all the model needs and saves
/// it a follow-up file read.
fn read_source_line(file: &Path, line: u32) -> Option<String> {
    let text = std::fs::read_to_string(file).ok()?;
    text.lines()
        .nth(line.saturating_sub(1) as usize)
        .map(ToString::to_string)
}

fn decode_hover(value: &JsonValue) -> String {
    let Some(contents) = value.get("contents") else {
        return String::new();
    };
    match contents {
        JsonValue::String(text) => text.clone(),
        JsonValue::Object(map) => map
            .get("value")
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
            .to_string(),
        JsonValue::Array(items) => items
            .iter()
            .map(|item| match item {
                JsonValue::String(text) => text.clone(),
                other => other
                    .get("value")
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
            .collect::<Vec<String>>()
            .join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> ServerContext {
        ServerContext::for_tests()
    }

    #[test]
    fn listing_languages_needs_no_server_running() {
        let text = call(&context(), &json!({ "op": "languages" })).expect("languages");
        assert!(text.contains("helm-apps"), "{text}");
        assert!(text.contains("go"), "{text}");
        assert!(text.contains("rust"), "{text}");
    }

    #[test]
    fn an_unknown_op_lists_the_ones_that_exist() {
        let err = call(&context(), &json!({ "op": "teleport" }))
            .err()
            .expect("unknown op must fail");
        assert!(err.contains("definition"), "{err}");
    }

    #[test]
    fn a_file_with_no_registered_language_says_so() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, "hello\n").expect("write");
        let err = call(
            &context(),
            &json!({ "op": "diagnostics", "file": file.to_string_lossy() }),
        )
        .err()
        .expect("unsupported language must fail");
        assert!(err.contains("no language server is registered"), "{err}");
    }

    #[test]
    fn a_missing_file_is_reported_before_any_server_starts() {
        let err = call(
            &context(),
            &json!({ "op": "hover", "file": "/nowhere/at/all/main.go" }),
        )
        .err()
        .expect("missing file must fail");
        assert!(err.contains("no such file"), "{err}");
    }

    #[test]
    fn helm_apps_values_are_answered_in_process() {
        let chart = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            chart.path().join("Chart.yaml"),
            "apiVersion: v2\nname: demo\n",
        )
        .expect("chart");
        let values = chart.path().join("values.yaml");
        std::fs::write(
            &values,
            "global:\n  env: dev\napps-statelss:\n  api:\n    enabled: true\n",
        )
        .expect("values");

        let text = call(
            &context(),
            &json!({ "op": "diagnostics", "file": values.to_string_lossy() }),
        )
        .expect("diagnostics");
        assert!(text.contains("E_UNKNOWN_APPS_GROUP"), "{text}");
    }

    #[test]
    fn addressing_needs_a_symbol_or_a_full_position() {
        let chart = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            chart.path().join("Chart.yaml"),
            "apiVersion: v2\nname: demo\n",
        )
        .expect("chart");
        let values = chart.path().join("values.yaml");
        std::fs::write(&values, "global:\n  env: dev\n").expect("values");

        let err = call(
            &context(),
            &json!({ "op": "hover", "file": values.to_string_lossy(), "line": 2 }),
        )
        .err()
        .expect("a lone line number is not an address");
        assert!(err.contains("'line' and 'column' together"), "{err}");
    }

    /// gopls reports a Go method as `(*Store).Put`; a caller says `Put`.
    /// Verified against gopls v0.23.0.
    #[test]
    fn a_method_can_be_named_without_its_receiver_decoration() {
        assert_eq!(undecorated_name("(*Store).Put"), "Store.Put");
        assert_eq!(simple_name("(*Store).Put"), "Put");
        assert_eq!(simple_name("Store.Put"), "Put");
        assert_eq!(simple_name("New"), "New");
        assert_eq!(undecorated_name("New"), "New");
    }

    /// The question this exists to answer: a name is used in one file and
    /// declared in another, and the caller asks the file they are reading.
    #[test]
    fn a_name_used_but_not_declared_here_still_has_a_position() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("main.rs");
        std::fs::write(
            &file,
            "// truncate is discussed here\nuse super::truncate;\n\nfn go() {\n    truncate(text);\n}\n",
        )
        .expect("write");

        let found = usages(&file, "truncate");
        assert_eq!(
            (found[0].line, found[0].column),
            (2, 12),
            "the comment on line 1 must not outrank the import on line 2"
        );
        assert_eq!((found[1].line, found[1].column), (5, 5));
        assert!(!found[0].declared);
        assert_eq!(found[0].describe(), "truncate (used at 2:12)");
    }

    #[test]
    fn a_name_is_matched_whole_and_not_inside_a_longer_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("main.rs");
        std::fs::write(
            &file,
            "fn go() {\n    truncate_all(x);\n    my_truncate(y);\n}\n",
        )
        .expect("write");
        assert!(
            usages(&file, "truncate").is_empty(),
            "'truncate' occurs only inside longer identifiers here"
        );

        assert!(is_whole_word("a truncate b", 2, 8));
        assert!(!is_whole_word("truncate_all", 0, 8));
        assert!(!is_whole_word("my_truncate", 3, 8));
        assert!(is_whole_word("(truncate)", 1, 8));
    }

    /// LSP columns are UTF-16 code units, so a line whose prefix is not ASCII
    /// would otherwise point the server at the wrong character.
    #[test]
    fn a_column_counts_the_units_lsp_counts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "let s = \"привет\"; truncate(s);\n").expect("write");
        let found = usages(&file, "truncate");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].column, 19, "six Cyrillic chars are six units");
    }

    #[test]
    fn only_the_first_few_usages_are_worth_trying() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "thing\n".repeat(50)).expect("write");
        assert_eq!(usages(&file, "thing").len(), MAX_USAGE_ANCHORS);
    }

    #[test]
    fn a_path_inside_the_working_directory_is_written_short() {
        let cwd = working_dir().expect("a working directory");
        assert_eq!(display_path(&cwd.join("src/lib.rs")), "src/lib.rs");
        assert_eq!(
            display_path(Path::new("/elsewhere/lib.rs")),
            "/elsewhere/lib.rs",
            "a path outside it has no short form to give"
        );
    }

    #[test]
    fn locations_decode_from_every_shape_the_protocol_allows() {
        let single = decode_locations(&json!({
            "uri": "file:///src/main.go",
            "range": { "start": { "line": 9, "character": 4 } },
        }));
        assert_eq!(single.len(), 1);
        assert_eq!((single[0].line, single[0].column), (10, 5));

        let links = decode_locations(&json!([{
            "targetUri": "file:///src/lib.rs",
            "targetSelectionRange": { "start": { "line": 0, "character": 0 } },
        }]));
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].file, PathBuf::from("/src/lib.rs"));

        assert!(decode_locations(&JsonValue::Null).is_empty());
    }

    #[test]
    fn nested_document_symbols_are_flattened_with_their_owner() {
        let mut out = Vec::new();
        collect_symbols(
            &json!([{
                "name": "Server",
                "kind": 23,
                "selectionRange": { "start": { "line": 2, "character": 0 } },
                "children": [{
                    "name": "serve",
                    "kind": 6,
                    "selectionRange": { "start": { "line": 5, "character": 4 } },
                }],
            }]),
            Path::new("/src/server.go"),
            None,
            &mut out,
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].kind, "struct");
        assert_eq!(out[1].name, "serve");
        assert_eq!(out[1].container.as_deref(), Some("Server"));
        assert_eq!((out[1].line, out[1].column), (6, 5));
    }

    #[test]
    fn hover_text_survives_all_three_content_encodings() {
        assert_eq!(decode_hover(&json!({ "contents": "plain" })), "plain");
        assert_eq!(
            decode_hover(&json!({ "contents": { "kind": "markdown", "value": "fenced" } })),
            "fenced"
        );
        assert_eq!(
            decode_hover(&json!({ "contents": ["one", { "value": "two" }] })),
            "one\ntwo"
        );
    }
}
