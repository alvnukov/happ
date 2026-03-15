use clap::{CommandFactory, Parser};
use clap_complete::{generate, Shell};
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
#[cfg(unix)]
use std::os::fd::AsRawFd;

use crate::cli::{Cli, Command};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Source(#[from] crate::source::Error),
    #[error(transparent)]
    Output(#[from] crate::output::Error),
    #[error(transparent)]
    ComposeInspect(#[from] crate::composeinspect::Error),
    #[error(transparent)]
    Lsp(#[from] crate::lsp::Error),
    #[error(transparent)]
    ChartAnalyzer(#[from] crate::chart_analyzer::Error),
    #[error("convert: {0}")]
    Convert(String),
    #[error("dyff differences found")]
    DyffDifferent,
    #[error("dyff invalid format '{0}' (expected text or json)")]
    DyffFormat(String),
    #[error("dyff invalid color '{0}' (expected auto|always|never)")]
    DyffColor(String),
}

pub fn run() -> Result<(), Error> {
    let cli = Cli::parse();
    run_with(cli)
}

pub fn run_with(cli: Cli) -> Result<(), Error> {
    if let Some(outcome) = handle_top_level_modes(&cli)? {
        return Ok(outcome);
    }

    let web_open_browser = cli.web_open_browser;
    let Some(command) = cli.command else {
        return Err(Error::Convert(
            "no command provided (use --help, --web or --studio)".to_string(),
        ));
    };

    match command {
        Command::Chart(args) => run_chart_command(&args),
        Command::Library(args) => run_library_command(&args),
        Command::Batch(args) => run_batch_command(&args),
        Command::Manifests(args) => run_manifests_command(&args),
        Command::Compose(args) => run_compose_command(&args),
        Command::Validate(args) => run_validate_command(&args),
        Command::Lsp(args) => Ok(crate::lsp::run(args)?),
        Command::Completion(args) => run_completion_command(&args),
        Command::Jq(args) => run_jq_command(&args),
        Command::Yq(args) => run_yq_command(&args),
        Command::ComposeInspect(args) => run_compose_inspect_command(&args),
        Command::Dyff(args) => run_dyff_command(&args),
        Command::Inspect(args) => run_inspect_command(web_open_browser, &args),
    }
}

fn handle_top_level_modes(cli: &Cli) -> Result<Option<()>, Error> {
    if cli.studio && cli.command.is_none() {
        crate::lsp::run(crate::cli::LspArgs {
            stdio: true,
            parent_pid: None,
        })?;
        return Ok(Some(()));
    }
    if cli.studio {
        return Err(Error::Convert(
            "--studio cannot be combined with subcommands".to_string(),
        ));
    }
    if cli.web && cli.command.is_none() {
        let stdin_text = if cli.web_stdin {
            read_stdin_to_eof()?
        } else {
            read_stdin_available_nonblocking()?
        };
        crate::inspectweb::serve_tools(&cli.web_addr, cli.web_open_browser, stdin_text)
            .map_err(Error::Convert)?;
        return Ok(Some(()));
    }
    Ok(None)
}

fn run_batch_command(args: &crate::cli::BatchArgs) -> Result<(), Error> {
    let report = crate::batch::run_batch_convert(args, |import_args| {
        run_chart_command(import_args).map_err(|e| e.to_string())
    })
    .map_err(Error::Convert)?;
    eprintln!(
        "batch convert: total={}, succeeded={}, failed={}",
        report.total, report.succeeded, report.failed
    );
    Ok(())
}

fn run_library_command(args: &crate::cli::LibraryArgs) -> Result<(), Error> {
    match &args.command {
        crate::cli::LibraryCommand::Version => {
            let version = crate::assets::embedded_helm_apps_version().ok_or_else(|| {
                Error::Convert("embedded helm-apps chart version is unavailable".to_string())
            })?;
            println!("{version}");
            Ok(())
        }
        crate::cli::LibraryCommand::Extract(extract_args) => {
            crate::assets::extract_helm_apps_chart(std::path::Path::new(&extract_args.out_dir))
                .map_err(|e| Error::Convert(format!("extract embedded helm-apps chart: {e}")))?;
            Ok(())
        }
    }
}

fn reject_verify_equivalence_for_non_chart(args: &crate::cli::ImportArgs) -> Result<(), Error> {
    if args.verify_equivalence {
        return Err(Error::Convert(
            "--verify-equivalence is supported only for chart source".to_string(),
        ));
    }
    Ok(())
}

fn write_values_and_optional_chart(
    args: &crate::cli::ImportArgs,
    values: &serde_yaml::Value,
) -> Result<(), Error> {
    if let Some(out) = args.out_chart_dir.as_deref() {
        crate::output::generate_consumer_chart(
            out,
            args.chart_name.as_deref(),
            values,
            args.library_chart_path.as_deref(),
            false,
        )?;
    }
    if args.out_chart_dir.is_none() || args.output.is_some() {
        crate::output::write_values(args.output.as_deref(), values)?;
    }
    Ok(())
}

fn run_manifests_command(args: &crate::cli::ImportArgs) -> Result<(), Error> {
    reject_verify_equivalence_for_non_chart(args)?;
    let docs = crate::source::load_documents_for_manifests(&args.path)?;
    let values = crate::convert::build_values(args, &docs).map_err(Error::Convert)?;
    write_values_and_optional_chart(args, &values)
}

fn run_compose_command(args: &crate::cli::ImportArgs) -> Result<(), Error> {
    reject_verify_equivalence_for_non_chart(args)?;
    let report = crate::composeinspect::load(&args.path)?;
    let values = crate::composeimport::build_values(args, &report);
    write_values_and_optional_chart(args, &values)
}

fn run_validate_command(args: &crate::cli::ValidateArgs) -> Result<(), Error> {
    crate::source::validate_values_file(&args.values)?;
    println!("OK");
    Ok(())
}

fn resolve_completion_shell(args: &crate::cli::CompletionArgs) -> Result<Shell, Error> {
    let shell_name = args
        .shell_flag
        .as_deref()
        .or(args.shell.as_deref())
        .ok_or_else(|| {
            Error::Convert(
                "missing shell, expected one of: bash,zsh,fish,powershell,elvish".to_string(),
            )
        })?;
    match shell_name {
        "bash" => Ok(Shell::Bash),
        "zsh" => Ok(Shell::Zsh),
        "fish" => Ok(Shell::Fish),
        "powershell" => Ok(Shell::PowerShell),
        "elvish" => Ok(Shell::Elvish),
        _ => Err(Error::Convert(format!(
            "unsupported shell '{}', expected one of: bash,zsh,fish,powershell,elvish",
            shell_name
        ))),
    }
}

fn write_completion_output(output_path: Option<&str>, output: &[u8]) -> Result<(), Error> {
    if let Some(path) = output_path {
        fs::write(path, output)
            .map_err(|e| Error::Convert(format!("write completion to {path}: {e}")))?;
    } else {
        io::stdout()
            .write_all(output)
            .map_err(|e| Error::Convert(format!("write completion to stdout: {e}")))?;
    }
    Ok(())
}

fn run_completion_command(args: &crate::cli::CompletionArgs) -> Result<(), Error> {
    let shell = resolve_completion_shell(args)?;
    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();
    let mut output = Vec::<u8>::new();
    generate(shell, &mut cmd, bin_name, &mut output);
    write_completion_output(args.output.as_deref(), &output)
}

fn run_query_command(
    args: &crate::cli::QueryArgs,
    tool: &str,
    parse_input_docs: fn(&str) -> Result<Vec<serde_json::Value>, crate::query::Error>,
) -> Result<(), Error> {
    let input = crate::source::read_input(&args.input)?;
    let mode = parse_doc_selection(args)?;
    let docs = parse_input_docs(&input)
        .map_err(|e| Error::Convert(format_query_error(tool, &args.query, &input, &e)))?;
    let stream = select_docs(docs, mode, tool)?;
    let out = crate::query::run_query_stream(&args.query, stream)
        .map_err(|e| Error::Convert(format_query_error(tool, &args.query, &input, &e)))?;
    print_query_output(&out, args.compact, args.raw_output)?;
    Ok(())
}

fn run_jq_command(args: &crate::cli::QueryArgs) -> Result<(), Error> {
    run_query_command(args, "jq", crate::query::parse_input_docs_prefer_json)
}

fn run_yq_command(args: &crate::cli::QueryArgs) -> Result<(), Error> {
    run_query_command(args, "yq", crate::query::parse_input_docs_prefer_yaml)
}

fn run_compose_inspect_command(args: &crate::cli::ComposeInspectArgs) -> Result<(), Error> {
    if args.web {
        let report = crate::composeinspect::load(&args.path).map_err(Error::ComposeInspect)?;
        let source_yaml =
            std::fs::read_to_string(&report.source_path).map_err(crate::source::Error::Io)?;
        let report_yaml = serde_yaml::to_string(&report).map_err(crate::source::Error::Yaml)?;
        let import_args = crate::cli::ImportArgs {
            path: args.path.clone(),
            env: "dev".into(),
            group_name: "apps-k8s-manifests".into(),
            group_type: "apps-k8s-manifests".into(),
            min_include_bytes: 24,
            include_status: false,
            output: None,
            out_chart_dir: None,
            chart_name: None,
            library_chart_path: None,
            import_strategy: "raw".into(),
            allow_template_includes: Vec::new(),
            unsupported_template_mode: "error".into(),
            verify_equivalence: false,
            release_name: "imported".into(),
            namespace: None,
            values_files: Vec::new(),
            set_values: Vec::new(),
            set_string_values: Vec::new(),
            set_file_values: Vec::new(),
            set_json_values: Vec::new(),
            kube_version: None,
            api_versions: Vec::new(),
            include_crds: false,
            write_rendered_output: None,
        };
        let values = crate::composeimport::build_values(&import_args, &report);
        let values_yaml = crate::output::values_yaml(&values)?;
        crate::inspectweb::serve_compose(
            &args.addr,
            args.open_browser,
            source_yaml,
            report_yaml,
            values_yaml,
        )
        .map_err(Error::Convert)?;
        return Ok(());
    }
    crate::composeinspect::resolve_and_write(&args.path, &args.format, args.output.as_deref())
        .map_err(Error::ComposeInspect)
}

fn run_dyff_command(args: &crate::cli::DyffArgs) -> Result<(), Error> {
    let from = crate::source::read_input(&args.from)?;
    let to = crate::source::read_input(&args.to)?;
    let diff = crate::dyfflike::between_yaml(
        &from,
        &to,
        crate::dyfflike::DiffOptions {
            ignore_order_changes: args.ignore_order,
            ignore_whitespace_change: args.ignore_whitespace,
        },
    )
    .map_err(crate::source::Error::Yaml)?;
    let entries = parse_diff_entries(&diff);
    let has_diff = !entries.is_empty();
    let rendered = format_dyff_output(args, &entries, std::io::stdout().is_terminal())?;

    if let Some(out) = args.output.as_deref() {
        fs::write(out, rendered.as_bytes()).map_err(crate::output::Error::Io)?;
    }
    if !args.quiet {
        if has_diff || args.format.eq_ignore_ascii_case("json") {
            println!("{rendered}");
        } else {
            println!("No differences.");
        }
    }
    if args.fail_on_diff && has_diff {
        return Err(Error::DyffDifferent);
    }
    Ok(())
}

fn build_inspect_import_args(args: &crate::cli::InspectArgs) -> crate::cli::ImportArgs {
    crate::cli::ImportArgs {
        path: args.path.clone(),
        env: "dev".into(),
        group_name: "apps-k8s-manifests".into(),
        group_type: "apps-k8s-manifests".into(),
        min_include_bytes: 24,
        include_status: false,
        output: None,
        out_chart_dir: None,
        chart_name: None,
        library_chart_path: None,
        import_strategy: "helpers".into(),
        allow_template_includes: Vec::new(),
        unsupported_template_mode: "error".into(),
        verify_equivalence: false,
        release_name: args.release_name.clone(),
        namespace: args.namespace.clone(),
        values_files: args.values_files.clone(),
        set_values: args.set_values.clone(),
        set_string_values: args.set_string_values.clone(),
        set_file_values: args.set_file_values.clone(),
        set_json_values: args.set_json_values.clone(),
        kube_version: args.kube_version.clone(),
        api_versions: args.api_versions.clone(),
        include_crds: args.include_crds,
        write_rendered_output: None,
    }
}

fn run_inspect_command(
    web_open_browser: bool,
    args: &crate::cli::InspectArgs,
) -> Result<(), Error> {
    let import_args = build_inspect_import_args(args);
    let rendered = crate::source::render_chart(&import_args, &args.path)?;
    let docs = crate::source::parse_documents(&rendered)?;
    let values = crate::convert::build_values(&import_args, &docs).map_err(Error::Convert)?;
    let values_yaml = crate::output::values_yaml(&values)?;
    if args.web {
        crate::inspectweb::serve(&args.addr, web_open_browser, rendered, values_yaml)
            .map_err(Error::Convert)?;
        return Ok(());
    }
    println!("{values_yaml}");
    Ok(())
}

fn run_chart_command(args: &crate::cli::ImportArgs) -> Result<(), Error> {
    let analyzed = crate::chart_analyzer::analyze_chart(args)?;
    let docs = analyzed.documents;
    let values = analyzed.values;
    let values_yaml_for_chart = crate::output::values_yaml(&values)?;
    let mut verify_chart_dir: Option<String> = None;
    let mut verify_temp_dir: Option<tempfile::TempDir> = None;
    if let Some(out) = args.out_chart_dir.as_deref() {
        crate::output::generate_consumer_chart(
            out,
            args.chart_name.as_deref(),
            &values,
            args.library_chart_path.as_deref(),
            false,
        )?;
        let _ = crate::output::sync_imported_include_helpers_from_source_chart(
            &args.path,
            out,
            &values_yaml_for_chart,
        )?;
        let _ = crate::output::ensure_values_examples_for_imported_helpers(out)?;
        let _ = crate::output::copy_chart_crds_if_any(&args.path, out)?;
        verify_chart_dir = Some(out.to_string());
    }
    if args.verify_equivalence {
        if verify_chart_dir.is_none() {
            let tmp = tempfile::Builder::new()
                .prefix("happ-verify-")
                .tempdir()
                .map_err(|e| Error::Convert(format!("create verify temp dir: {e}")))?;
            let generated_chart_dir = tmp.path().join("chart");
            let generated_chart_dir_text = generated_chart_dir.to_string_lossy().to_string();
            crate::output::generate_consumer_chart(
                &generated_chart_dir_text,
                args.chart_name.as_deref(),
                &values,
                args.library_chart_path.as_deref(),
                false,
            )?;
            let _ = crate::output::sync_imported_include_helpers_from_source_chart(
                &args.path,
                &generated_chart_dir_text,
                &values_yaml_for_chart,
            )?;
            let _ = crate::output::ensure_values_examples_for_imported_helpers(
                &generated_chart_dir_text,
            )?;
            let _ = crate::output::copy_chart_crds_if_any(&args.path, &generated_chart_dir_text)?;
            verify_chart_dir = Some(generated_chart_dir_text);
            verify_temp_dir = Some(tmp);
        }
        let verify_chart_dir = verify_chart_dir.as_deref().ok_or_else(|| {
            Error::Convert("internal error: verify chart dir is missing".to_string())
        })?;
        let summary = verify_chart_equivalence(args, &docs, verify_chart_dir)?;
        eprintln!("verify equivalence: {summary}");
    }
    if args.out_chart_dir.is_none() || args.output.is_some() {
        crate::output::write_values(args.output.as_deref(), &values)?;
    }
    drop(verify_temp_dir);
    Ok(())
}

fn read_stdin_to_eof() -> Result<Option<String>, Error> {
    if std::io::stdin().is_terminal() {
        return Ok(None);
    }
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| Error::Convert(format!("read stdin: {e}")))?;
    if buf.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(buf))
    }
}

fn read_stdin_available_nonblocking() -> Result<Option<String>, Error> {
    if std::io::stdin().is_terminal() {
        return Ok(None);
    }
    #[cfg(unix)]
    {
        let stdin = std::io::stdin();
        let fd = stdin.as_raw_fd();
        let orig_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if orig_flags < 0 {
            return Err(Error::Convert(
                "read stdin: failed to get fd flags".to_string(),
            ));
        }
        let nonblocking_flags = orig_flags | libc::O_NONBLOCK;
        if unsafe { libc::fcntl(fd, libc::F_SETFL, nonblocking_flags) } < 0 {
            return Err(Error::Convert(
                "read stdin: failed to set O_NONBLOCK".to_string(),
            ));
        }

        let mut bytes = Vec::<u8>::new();
        {
            let mut handle = stdin.lock();
            let mut chunk = [0_u8; 8192];
            loop {
                match handle.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => bytes.extend_from_slice(&chunk[..n]),
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                    Err(err) => {
                        let _ = unsafe { libc::fcntl(fd, libc::F_SETFL, orig_flags) };
                        return Err(Error::Convert(format!("read stdin: {err}")));
                    }
                }
            }
        }

        let _ = unsafe { libc::fcntl(fd, libc::F_SETFL, orig_flags) };
        if bytes.is_empty() {
            return Ok(None);
        }
        let text = String::from_utf8_lossy(&bytes).to_string();
        if text.trim().is_empty() {
            Ok(None)
        } else {
            Ok(Some(text))
        }
    }
    #[cfg(not(unix))]
    {
        Ok(None)
    }
}

fn verify_chart_equivalence(
    args: &crate::cli::ImportArgs,
    source_docs: &[serde_yaml::Value],
    generated_chart_dir: &str,
) -> Result<String, Error> {
    let mut generated_render_args = args.clone();
    generated_render_args.path = generated_chart_dir.to_string();
    generated_render_args.values_files.clear();
    generated_render_args.set_values.clear();
    generated_render_args.set_string_values.clear();
    generated_render_args.set_file_values.clear();
    generated_render_args.set_json_values.clear();
    generated_render_args.verify_equivalence = false;
    generated_render_args.output = None;
    generated_render_args.out_chart_dir = None;
    generated_render_args.write_rendered_output = None;

    let generated_docs = crate::source::load_documents_for_chart(&generated_render_args)?;
    let result = crate::verify::equivalent(source_docs, &generated_docs);
    if !result.equal {
        return Err(Error::Convert(format!(
            "verify equivalence failed: {}",
            result.summary
        )));
    }
    Ok(result.summary)
}

fn print_query_output(
    values: &[serde_json::Value],
    compact: bool,
    raw_output: bool,
) -> Result<(), Error> {
    let output = crate::query::format_output_json_lines(values, compact, raw_output)
        .map_err(|e| Error::Convert(format!("encode json: {e}")))?;
    if !output.is_empty() {
        println!("{output}");
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocSelection {
    First,
    All,
    Index(usize),
}

fn parse_doc_selection(args: &crate::cli::QueryArgs) -> Result<DocSelection, Error> {
    match crate::query::parse_doc_mode(&args.doc_mode, args.doc_index)
        .map_err(|e| Error::Convert(format!("query: {e}")))?
    {
        zq::DocMode::First => Ok(DocSelection::First),
        zq::DocMode::All => Ok(DocSelection::All),
        zq::DocMode::Index(idx) => Ok(DocSelection::Index(idx)),
    }
}

fn select_docs(
    mut docs: Vec<serde_json::Value>,
    mode: DocSelection,
    tool: &str,
) -> Result<Vec<serde_json::Value>, Error> {
    match mode {
        DocSelection::All => Ok(docs),
        DocSelection::First => Ok(docs.into_iter().next().into_iter().collect()),
        DocSelection::Index(i) => {
            if i >= docs.len() {
                return Err(Error::Convert(format!(
                    "{}: --doc-index={} is out of range for {} document(s)",
                    tool,
                    i,
                    docs.len()
                )));
            }
            Ok(vec![docs.swap_remove(i)])
        }
    }
}

fn format_query_error(tool: &str, query: &str, input: &str, err: &crate::query::Error) -> String {
    crate::query::format_query_error(tool, query, input, err)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffKind {
    Added,
    Removed,
    Changed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiffEntry {
    kind: DiffKind,
    path: String,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
struct DiffSummary {
    total: usize,
    changed: usize,
    added: usize,
    removed: usize,
}

fn parse_diff_entries(diff: &str) -> Vec<DiffEntry> {
    let mut out = Vec::new();
    for line in diff.lines().map(str::trim).filter(|x| !x.is_empty()) {
        if let Some(path) = line.strip_prefix("added: ") {
            out.push(DiffEntry {
                kind: DiffKind::Added,
                path: path.to_string(),
            });
            continue;
        }
        if let Some(path) = line.strip_prefix("removed: ") {
            out.push(DiffEntry {
                kind: DiffKind::Removed,
                path: path.to_string(),
            });
            continue;
        }
        if let Some(path) = line.strip_prefix("changed: ") {
            out.push(DiffEntry {
                kind: DiffKind::Changed,
                path: path.to_string(),
            });
        }
    }
    out
}

fn diff_summary(entries: &[DiffEntry]) -> DiffSummary {
    let mut s = DiffSummary {
        total: entries.len(),
        changed: 0,
        added: 0,
        removed: 0,
    };
    for e in entries {
        match e.kind {
            DiffKind::Added => s.added += 1,
            DiffKind::Removed => s.removed += 1,
            DiffKind::Changed => s.changed += 1,
        }
    }
    s
}

fn color_policy(mode: &str, is_tty: bool) -> Result<bool, Error> {
    let mode = mode.trim().to_ascii_lowercase();
    match mode.as_str() {
        "" | "auto" => Ok(is_tty),
        "always" => Ok(true),
        "never" => Ok(false),
        other => Err(Error::DyffColor(other.to_string())),
    }
}

fn format_dyff_output(
    args: &crate::cli::DyffArgs,
    entries: &[DiffEntry],
    is_tty: bool,
) -> Result<String, Error> {
    match args.format.trim().to_ascii_lowercase().as_str() {
        "" | "text" => format_dyff_text(args, entries, is_tty),
        "json" => format_dyff_json(args, entries),
        "github" => format_dyff_github(args, entries),
        other => Err(Error::DyffFormat(other.to_string())),
    }
}

fn format_dyff_json(args: &crate::cli::DyffArgs, entries: &[DiffEntry]) -> Result<String, Error> {
    let _ = color_policy(&args.color, false)?;
    let summary = diff_summary(entries);
    let mut payload = serde_json::json!({
        "equal": entries.is_empty(),
        "from": args.label_from.as_deref().unwrap_or(&args.from),
        "to": args.label_to.as_deref().unwrap_or(&args.to),
        "summary": summary,
        "entries": entries.iter().map(|e| {
            let t = match e.kind {
                DiffKind::Added => "added",
                DiffKind::Removed => "removed",
                DiffKind::Changed => "changed",
            };
            serde_json::json!({"type": t, "path": e.path})
        }).collect::<Vec<_>>(),
    });
    if args.summary_only {
        payload["entries"] = serde_json::json!([]);
    }
    serde_json::to_string_pretty(&payload)
        .map_err(|e| Error::Convert(format!("dyff json encode: {e}")))
}

fn format_dyff_text(
    args: &crate::cli::DyffArgs,
    entries: &[DiffEntry],
    is_tty: bool,
) -> Result<String, Error> {
    let use_color = color_policy(&args.color, is_tty)?;
    let from_label = args.label_from.as_deref().unwrap_or(&args.from);
    let to_label = args.label_to.as_deref().unwrap_or(&args.to);
    let mut lines = vec![format!("Compare: {from_label} -> {to_label}")];

    if !args.summary_only {
        for e in entries {
            let (name, ansi) = match e.kind {
                DiffKind::Added => ("added", "\u{1b}[32m"),
                DiffKind::Removed => ("removed", "\u{1b}[31m"),
                DiffKind::Changed => ("changed", "\u{1b}[33m"),
            };
            let t = if use_color {
                format!("{ansi}{name}\u{1b}[0m")
            } else {
                name.to_string()
            };
            lines.push(format!("{t:>8}  {}", e.path));
        }
    }
    if args.stats {
        let s = diff_summary(entries);
        lines.push(format!(
            "Summary: total={} changed={} added={} removed={}",
            s.total, s.changed, s.added, s.removed
        ));
    }
    Ok(lines.join("\n"))
}

fn format_dyff_github(args: &crate::cli::DyffArgs, entries: &[DiffEntry]) -> Result<String, Error> {
    let _ = color_policy(&args.color, false)?;
    let from_label = args.label_from.as_deref().unwrap_or(&args.from);
    let to_label = args.label_to.as_deref().unwrap_or(&args.to);
    let mut lines = Vec::new();
    lines.push(format!("::notice::dyff compare {from_label} -> {to_label}"));
    if !args.summary_only {
        for e in entries {
            let (lvl, t) = match e.kind {
                DiffKind::Added => ("notice", "added"),
                DiffKind::Removed => ("warning", "removed"),
                DiffKind::Changed => ("error", "changed"),
            };
            lines.push(format!("::{lvl}::{t} {}", e.path));
        }
    }
    let s = diff_summary(entries);
    if args.stats || args.summary_only {
        lines.push(format!(
            "::notice::summary total={} changed={} added={} removed={}",
            s.total, s.changed, s.added, s.removed
        ));
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{
        BatchArgs, Command, CompletionArgs, DyffArgs, ImportArgs, ImportSharedArgs, InspectArgs,
        LibraryArgs, LibraryCommand, LibraryExtractArgs, QueryArgs, ValidateArgs,
    };
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn dyff_fail_on_diff_returns_error() {
        let cli = Cli {
            web: false,
            studio: false,
            web_stdin: false,
            web_addr: "127.0.0.1:8088".to_string(),
            web_open_browser: true,
            command: Some(Command::Dyff(DyffArgs {
                from: "-".into(),
                to: "-".into(),
                ignore_order: false,
                ignore_whitespace: false,
                quiet: true,
                fail_on_diff: true,
                format: "text".into(),
                color: "auto".into(),
                stats: false,
                summary_only: false,
                label_from: None,
                label_to: None,
                output: None,
            })),
        };
        let _ = cli; // runtime stdin case intentionally not executed here
    }

    #[test]
    fn dyff_parse_entries_and_summary() {
        let entries = parse_diff_entries("added: a\nremoved: b\nchanged: c\n");
        assert_eq!(entries.len(), 3);
        let s = diff_summary(&entries);
        assert_eq!(s.total, 3);
        assert_eq!(s.changed, 1);
        assert_eq!(s.added, 1);
        assert_eq!(s.removed, 1);
    }

    #[test]
    fn dyff_text_output_has_labels_and_stats() {
        let args = DyffArgs {
            from: "a.yaml".into(),
            to: "b.yaml".into(),
            ignore_order: false,
            ignore_whitespace: false,
            quiet: false,
            fail_on_diff: false,
            format: "text".into(),
            color: "never".into(),
            stats: true,
            summary_only: false,
            label_from: Some("source".into()),
            label_to: Some("generated".into()),
            output: None,
        };
        let out = format_dyff_output(
            &args,
            &[DiffEntry {
                kind: DiffKind::Changed,
                path: "doc[0].spec".into(),
            }],
            false,
        )
        .expect("format");
        assert!(out.contains("Compare: source -> generated"));
        assert!(out.contains("changed"));
        assert!(out.contains("Summary: total=1 changed=1 added=0 removed=0"));
    }

    #[test]
    fn dyff_json_output_machine_readable() {
        let args = DyffArgs {
            from: "a.yaml".into(),
            to: "b.yaml".into(),
            ignore_order: false,
            ignore_whitespace: false,
            quiet: false,
            fail_on_diff: false,
            format: "json".into(),
            color: "never".into(),
            stats: false,
            summary_only: false,
            label_from: None,
            label_to: None,
            output: None,
        };
        let out = format_dyff_output(
            &args,
            &[
                DiffEntry {
                    kind: DiffKind::Added,
                    path: "doc[0].a".into(),
                },
                DiffEntry {
                    kind: DiffKind::Removed,
                    path: "doc[0].b".into(),
                },
            ],
            false,
        )
        .expect("format");
        let v: serde_json::Value = serde_json::from_str(&out).expect("json");
        assert_eq!(v.get("equal").and_then(|x| x.as_bool()), Some(false));
        assert_eq!(
            v.get("summary")
                .and_then(|s| s.get("total"))
                .and_then(|x| x.as_u64()),
            Some(2)
        );
        assert_eq!(
            v.get("entries").and_then(|x| x.as_array()).map(|x| x.len()),
            Some(2)
        );
    }

    #[test]
    fn dyff_invalid_color_rejected() {
        let err = color_policy("rainbow", true).expect_err("must fail");
        assert!(matches!(err, Error::DyffColor(_)));
    }

    #[test]
    fn dyff_github_output_has_annotations() {
        let args = DyffArgs {
            from: "a.yaml".into(),
            to: "b.yaml".into(),
            ignore_order: false,
            ignore_whitespace: false,
            quiet: false,
            fail_on_diff: false,
            format: "github".into(),
            color: "never".into(),
            stats: false,
            summary_only: false,
            label_from: None,
            label_to: None,
            output: None,
        };
        let out = format_dyff_output(
            &args,
            &[
                DiffEntry {
                    kind: DiffKind::Changed,
                    path: "doc[0].a".into(),
                },
                DiffEntry {
                    kind: DiffKind::Added,
                    path: "doc[0].b".into(),
                },
            ],
            false,
        )
        .expect("format");
        assert!(out.contains("::error::changed"));
        assert!(out.contains("::notice::added"));
    }

    #[test]
    fn dyff_summary_only_hides_entries() {
        let args = DyffArgs {
            from: "a.yaml".into(),
            to: "b.yaml".into(),
            ignore_order: false,
            ignore_whitespace: false,
            quiet: false,
            fail_on_diff: false,
            format: "text".into(),
            color: "never".into(),
            stats: true,
            summary_only: true,
            label_from: None,
            label_to: None,
            output: None,
        };
        let out = format_dyff_output(
            &args,
            &[DiffEntry {
                kind: DiffKind::Changed,
                path: "doc[0].spec".into(),
            }],
            false,
        )
        .expect("format");
        assert!(out.contains("Summary: total=1 changed=1 added=0 removed=0"));
        assert!(!out.contains("doc[0].spec"));
    }

    #[test]
    fn inspect_command_is_not_stubbed() {
        let cli = Cli {
            web: false,
            studio: false,
            web_stdin: false,
            web_addr: "127.0.0.1:8088".to_string(),
            web_open_browser: true,
            command: Some(Command::Inspect(InspectArgs {
                path: "/definitely/missing/chart".to_string(),
                release_name: "inspect".to_string(),
                namespace: None,
                values_files: Vec::new(),
                set_values: Vec::new(),
                set_string_values: Vec::new(),
                set_file_values: Vec::new(),
                set_json_values: Vec::new(),
                kube_version: None,
                api_versions: Vec::new(),
                include_crds: false,
                web: false,
                addr: "127.0.0.1:8088".to_string(),
            })),
        };
        let result = run_with(cli);
        assert!(
            matches!(result, Err(Error::Source(_))),
            "inspect must execute implementation path and fail only on source/render stage for missing chart"
        );
    }

    #[test]
    fn validate_command_succeeds_on_valid_yaml() {
        let td = TempDir::new().expect("tmp");
        let p = td.path().join("values.yaml");
        fs::write(&p, "global:\n  env: dev\n").expect("write");
        let cli = Cli {
            web: false,
            studio: false,
            web_stdin: false,
            web_addr: "127.0.0.1:8088".to_string(),
            web_open_browser: true,
            command: Some(Command::Validate(ValidateArgs {
                values: p.to_string_lossy().to_string(),
            })),
        };
        let result = run_with(cli);
        assert!(
            result.is_ok(),
            "validate should pass for valid yaml: {result:?}"
        );
    }

    #[test]
    fn validate_command_fails_on_invalid_yaml() {
        let td = TempDir::new().expect("tmp");
        let p = td.path().join("values.yaml");
        fs::write(&p, "global:\n  env: [dev\n").expect("write");
        let cli = Cli {
            web: false,
            studio: false,
            web_stdin: false,
            web_addr: "127.0.0.1:8088".to_string(),
            web_open_browser: true,
            command: Some(Command::Validate(ValidateArgs {
                values: p.to_string_lossy().to_string(),
            })),
        };
        let result = run_with(cli);
        assert!(
            matches!(result, Err(Error::Source(crate::source::Error::Yaml(_)))),
            "expected yaml parse error, got: {result:?}"
        );
    }

    #[test]
    fn completion_command_writes_shell_script() {
        let td = TempDir::new().expect("tmp");
        let out = td.path().join("happ.bash");
        let cli = Cli {
            web: false,
            studio: false,
            web_stdin: false,
            web_addr: "127.0.0.1:8088".to_string(),
            web_open_browser: true,
            command: Some(Command::Completion(CompletionArgs {
                shell: Some("bash".to_string()),
                shell_flag: None,
                output: Some(out.to_string_lossy().to_string()),
            })),
        };
        let result = run_with(cli);
        assert!(result.is_ok(), "completion should succeed: {result:?}");
        let script = fs::read_to_string(&out).expect("read completion");
        assert!(!script.trim().is_empty());
        assert!(script.contains("happ"));
    }

    #[test]
    fn library_extract_command_writes_embedded_chart() {
        let td = TempDir::new().expect("tmp");
        let out_dir = td.path().join("helm-apps");
        let cli = Cli {
            web: false,
            studio: false,
            web_stdin: false,
            web_addr: "127.0.0.1:8088".to_string(),
            web_open_browser: true,
            command: Some(Command::Library(LibraryArgs {
                command: LibraryCommand::Extract(LibraryExtractArgs {
                    out_dir: out_dir.to_string_lossy().to_string(),
                }),
            })),
        };
        let result = run_with(cli);
        assert!(result.is_ok(), "library extract should succeed: {result:?}");
        let chart_yaml = out_dir.join("Chart.yaml");
        assert!(
            chart_yaml.exists(),
            "embedded chart Chart.yaml not extracted"
        );
    }

    #[test]
    fn parse_doc_selection_modes() {
        let first = QueryArgs {
            query: ".".to_string(),
            input: "-".to_string(),
            doc_mode: "first".to_string(),
            doc_index: None,
            compact: false,
            raw_output: false,
        };
        assert!(matches!(
            parse_doc_selection(&first).expect("mode"),
            DocSelection::First
        ));

        let all = QueryArgs {
            doc_mode: "all".to_string(),
            ..first.clone()
        };
        assert!(matches!(
            parse_doc_selection(&all).expect("mode"),
            DocSelection::All
        ));

        let idx = QueryArgs {
            doc_mode: "index".to_string(),
            doc_index: Some(2),
            ..first.clone()
        };
        assert!(matches!(
            parse_doc_selection(&idx).expect("mode"),
            DocSelection::Index(2)
        ));
    }

    #[test]
    fn parse_doc_selection_index_requires_value() {
        let args = QueryArgs {
            query: ".".to_string(),
            input: "-".to_string(),
            doc_mode: "index".to_string(),
            doc_index: None,
            compact: false,
            raw_output: false,
        };
        let err = parse_doc_selection(&args).expect_err("must fail");
        assert!(err.to_string().contains("--doc-index is required"));
    }

    #[test]
    fn select_docs_modes() {
        let docs = vec![
            serde_json::json!({"a":1}),
            serde_json::json!({"a":2}),
            serde_json::json!({"a":3}),
        ];
        let first = select_docs(docs.clone(), DocSelection::First, "yq").expect("first");
        assert_eq!(first, vec![serde_json::json!({"a":1})]);
        let all = select_docs(docs.clone(), DocSelection::All, "yq").expect("all");
        assert_eq!(all.len(), 3);
        let one = select_docs(docs, DocSelection::Index(1), "yq").expect("idx");
        assert_eq!(one, vec![serde_json::json!({"a":2})]);
    }

    #[test]
    fn format_query_error_adds_input_context_when_line_col_present() {
        let input = "a: 1\nb: [\n";
        let err = crate::query::parse_input_docs_prefer_yaml(input).expect_err("must fail");
        let msg = format_query_error("yq", ".", input, &err);
        assert!(msg.contains("input context:"));
        assert!(msg.contains("| b: ["));
    }

    #[test]
    fn format_query_error_for_compile_errors_points_to_query() {
        let query = ".items[\n";
        let wrapped = format_query_error(
            "yq",
            query,
            "a: 1\n",
            &crate::query::Error::Unsupported("parse failed at line 1, column 7".to_string()),
        );
        assert!(wrapped.contains("--> <query>:1:7"));
        assert!(wrapped.contains(".items["));
    }

    #[test]
    fn verify_equivalence_rejected_for_manifests_mode() {
        let cli = Cli {
            web: false,
            studio: false,
            web_stdin: false,
            web_addr: "127.0.0.1:8088".to_string(),
            web_open_browser: true,
            command: Some(Command::Manifests(import_args_with_verify())),
        };
        let result = run_with(cli).expect_err("must fail");
        assert!(
            matches!(result, Error::Convert(ref msg) if msg.contains("--verify-equivalence is supported only for chart source")),
            "unexpected error: {result:?}"
        );
    }

    #[test]
    fn verify_equivalence_rejected_for_compose_mode() {
        let cli = Cli {
            web: false,
            studio: false,
            web_stdin: false,
            web_addr: "127.0.0.1:8088".to_string(),
            web_open_browser: true,
            command: Some(Command::Compose(import_args_with_verify())),
        };
        let result = run_with(cli).expect_err("must fail");
        assert!(
            matches!(result, Error::Convert(ref msg) if msg.contains("--verify-equivalence is supported only for chart source")),
            "unexpected error: {result:?}"
        );
    }

    #[test]
    fn studio_with_subcommand_is_rejected() {
        let cli = Cli {
            web: false,
            studio: true,
            web_stdin: false,
            web_addr: "127.0.0.1:8088".to_string(),
            web_open_browser: true,
            command: Some(Command::Validate(ValidateArgs {
                values: "/tmp/values.yaml".to_string(),
            })),
        };
        let err = run_with(cli).expect_err("must fail");
        assert!(
            matches!(err, Error::Convert(ref msg) if msg.contains("--studio cannot be combined with subcommands")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn no_command_without_modes_is_rejected() {
        let cli = Cli {
            web: false,
            studio: false,
            web_stdin: false,
            web_addr: "127.0.0.1:8088".to_string(),
            web_open_browser: true,
            command: None,
        };
        let err = run_with(cli).expect_err("must fail");
        assert!(
            matches!(err, Error::Convert(ref msg) if msg.contains("no command provided")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn completion_command_without_shell_is_rejected() {
        let cli = Cli {
            web: false,
            studio: false,
            web_stdin: false,
            web_addr: "127.0.0.1:8088".to_string(),
            web_open_browser: true,
            command: Some(Command::Completion(CompletionArgs {
                shell: None,
                shell_flag: None,
                output: None,
            })),
        };
        let err = run_with(cli).expect_err("must fail");
        assert!(
            matches!(err, Error::Convert(ref msg) if msg.contains("missing shell")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn batch_command_fails_for_missing_input_dir() {
        let td = TempDir::new().expect("tmp");
        let out = td.path().join("out");
        let cli = Cli {
            web: false,
            studio: false,
            web_stdin: false,
            web_addr: "127.0.0.1:8088".to_string(),
            web_open_browser: true,
            command: Some(Command::Batch(BatchArgs {
                charts_dir: td.path().join("missing").to_string_lossy().to_string(),
                out_dir: out.to_string_lossy().to_string(),
                keep_going: true,
                import: ImportSharedArgs::default(),
            })),
        };
        let err = run_with(cli).expect_err("must fail");
        assert!(
            matches!(err, Error::Convert(ref msg) if msg.contains("does not exist")),
            "unexpected error: {err:?}"
        );
    }

    fn import_args_with_verify() -> ImportArgs {
        ImportArgs {
            path: "/tmp/input".to_string(),
            env: "dev".to_string(),
            group_name: "apps-k8s-manifests".to_string(),
            group_type: "apps-k8s-manifests".to_string(),
            min_include_bytes: 24,
            include_status: false,
            output: None,
            out_chart_dir: None,
            chart_name: None,
            library_chart_path: None,
            import_strategy: "helpers".to_string(),
            allow_template_includes: Vec::new(),
            unsupported_template_mode: "error".into(),
            verify_equivalence: true,
            release_name: "imported".to_string(),
            namespace: None,
            values_files: Vec::new(),
            set_values: Vec::new(),
            set_string_values: Vec::new(),
            set_file_values: Vec::new(),
            set_json_values: Vec::new(),
            kube_version: None,
            api_versions: Vec::new(),
            include_crds: false,
            write_rendered_output: None,
        }
    }
}
