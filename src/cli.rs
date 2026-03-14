use clap::{ArgAction, Parser, Subcommand};

pub const DEFAULT_WEB_ADDR: &str = "127.0.0.1:18088";
pub const DEFAULT_STUDIO_ADDR: &str = "127.0.0.1:18089";
pub const DEFAULT_COMPOSE_STUDIO_ADDR: &str = "127.0.0.1:18090";

#[derive(Parser, Debug)]
#[command(
    name = "happ",
    version = env!("CARGO_PKG_VERSION"),
    about = "happ imports Helm chart render output or raw manifests into a helm-apps-based consumer chart"
)]
pub struct Cli {
    #[arg(long, global = true, default_value_t = false, action = ArgAction::SetTrue, help = "Start web utilities UI")]
    pub web: bool,
    #[arg(
        long,
        global = true,
        default_value_t = false,
        action = ArgAction::SetTrue,
        conflicts_with_all = ["web", "web_stdin", "web_addr", "web_open_browser"],
        help = "Start studio backend over stdio (no HTTP port)"
    )]
    pub studio: bool,
    #[arg(
        long = "web-stdin",
        global = true,
        default_value_t = false,
        action = ArgAction::SetTrue,
        help = "Read stdin and pass it into --web mode (opt-in to avoid blocking on open pipes)"
    )]
    pub web_stdin: bool,
    #[arg(
        long = "web-addr",
        global = true,
        default_value = DEFAULT_WEB_ADDR,
        help = "Address for --web mode"
    )]
    pub web_addr: String,
    #[arg(
        long = "web-open-browser",
        global = true,
        default_value_t = true,
        action = ArgAction::Set,
        help = "Open browser automatically in --web mode"
    )]
    pub web_open_browser: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    Chart(ImportArgs),
    #[command(about = "Read or extract the embedded helm-apps library chart")]
    Library(LibraryArgs),
    #[command(about = "Batch-convert chart directory into library-format charts")]
    Batch(BatchArgs),
    Manifests(ImportArgs),
    Compose(ImportArgs),
    Validate(ValidateArgs),
    Lsp(LspArgs),
    Completion(CompletionArgs),
    #[command(
        about = "Run jq-like query syntax on JSON or YAML input",
        long_about = "Run jq-like query syntax on input data.\nInput may be JSON or YAML; parsing is automatic."
    )]
    Jq(QueryArgs),
    #[command(
        about = "Run yq-like query syntax on YAML or JSON input",
        long_about = "Run yq-like query syntax on input data.\nInput may be YAML or JSON; parsing is automatic."
    )]
    Yq(QueryArgs),
    Inspect(InspectArgs),
    #[command(name = "compose-inspect")]
    ComposeInspect(ComposeInspectArgs),
    Dyff(DyffArgs),
}

#[derive(clap::Args, Debug, Clone)]
pub struct QueryArgs {
    #[arg(long = "query", help = "Query expression in jq/yq language syntax")]
    pub query: String,
    #[arg(
        long = "input",
        default_value = "-",
        help = "Input file path or '-' for stdin. Supports both JSON and YAML."
    )]
    pub input: String,
    #[arg(
        long = "doc-mode",
        default_value = "first",
        help = "Document selection for YAML streams: first|all|index"
    )]
    pub doc_mode: String,
    #[arg(
        long = "doc-index",
        help = "Zero-based document index when --doc-mode=index"
    )]
    pub doc_index: Option<usize>,
    #[arg(long, default_value_t = false, action = ArgAction::SetTrue)]
    pub compact: bool,
    #[arg(long, default_value_t = false, action = ArgAction::SetTrue)]
    pub raw_output: bool,
}

#[derive(clap::Args, Debug, Clone)]
pub struct ValidateArgs {
    #[arg(long)]
    pub values: String,
}

#[derive(clap::Args, Debug, Clone)]
pub struct LspArgs {
    #[arg(
        long,
        default_value_t = true,
        action = ArgAction::Set,
        num_args = 0..=1,
        default_missing_value = "true",
        help = "Use stdio transport for Language Server Protocol"
    )]
    pub stdio: bool,
    #[arg(
        long = "parent-pid",
        help = "Parent process PID to monitor; exit LSP when parent is gone"
    )]
    pub parent_pid: Option<u32>,
}

#[derive(clap::Args, Debug, Clone)]
pub struct CompletionArgs {
    #[arg(
        value_name = "SHELL",
        value_parser = ["bash", "zsh", "fish", "powershell", "elvish"],
        required_unless_present = "shell_flag",
        help = "Target shell (kubectl style: happ completion zsh)"
    )]
    pub shell: Option<String>,
    #[arg(
        long = "shell",
        id = "shell_flag",
        value_name = "SHELL",
        value_parser = ["bash", "zsh", "fish", "powershell", "elvish"],
        required_unless_present = "shell",
        help = "Target shell"
    )]
    pub shell_flag: Option<String>,
    #[arg(long, help = "Write completion script to file (stdout by default)")]
    pub output: Option<String>,
}

#[derive(clap::Args, Debug, Clone)]
pub struct LibraryArgs {
    #[command(subcommand)]
    pub command: LibraryCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum LibraryCommand {
    #[command(about = "Print embedded helm-apps chart version")]
    Version,
    #[command(about = "Extract embedded helm-apps chart into directory")]
    Extract(LibraryExtractArgs),
}

#[derive(clap::Args, Debug, Clone)]
pub struct LibraryExtractArgs {
    #[arg(long = "out-dir", help = "Destination directory for embedded helm-apps chart")]
    pub out_dir: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    #[test]
    fn parses_validate_subcommand() {
        let cli = Cli::try_parse_from(["happ", "validate", "--values", "/tmp/values.yaml"])
            .expect("parse validate");
        match cli.command.expect("command") {
            Command::Validate(args) => assert_eq!(args.values, "/tmp/values.yaml"),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_lsp_subcommand() {
        let cli = Cli::try_parse_from(["happ", "lsp", "--stdio=true"]).expect("parse lsp");
        match cli.command.expect("command") {
            Command::Lsp(args) => {
                assert!(args.stdio);
                assert_eq!(args.parent_pid, None);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_lsp_subcommand_with_flag_only() {
        let cli = Cli::try_parse_from(["happ", "lsp", "--stdio"]).expect("parse lsp");
        match cli.command.expect("command") {
            Command::Lsp(args) => {
                assert!(args.stdio);
                assert_eq!(args.parent_pid, None);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_lsp_subcommand_with_parent_pid() {
        let cli = Cli::try_parse_from(["happ", "lsp", "--parent-pid", "12345"])
            .expect("parse lsp parent pid");
        match cli.command.expect("command") {
            Command::Lsp(args) => {
                assert!(args.stdio);
                assert_eq!(args.parent_pid, Some(12345));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_completion_subcommand() {
        let cli = Cli::try_parse_from(["happ", "completion", "--shell", "zsh"])
            .expect("parse completion");
        match cli.command.expect("command") {
            Command::Completion(args) => {
                assert_eq!(args.shell.as_deref(), None);
                assert_eq!(args.shell_flag.as_deref(), Some("zsh"));
                assert_eq!(args.output, None);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_completion_subcommand_kubectl_style() {
        let cli = Cli::try_parse_from(["happ", "completion", "zsh"]).expect("parse completion");
        match cli.command.expect("command") {
            Command::Completion(args) => {
                assert_eq!(args.shell.as_deref(), Some("zsh"));
                assert_eq!(args.shell_flag.as_deref(), None);
                assert_eq!(args.output, None);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_library_version_subcommand() {
        let cli = Cli::try_parse_from(["happ", "library", "version"]).expect("parse library version");
        match cli.command.expect("command") {
            Command::Library(args) => match args.command {
                LibraryCommand::Version => {}
                other => panic!("unexpected library command: {other:?}"),
            },
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_library_extract_subcommand() {
        let cli = Cli::try_parse_from([
            "happ",
            "library",
            "extract",
            "--out-dir",
            "/tmp/helm-apps",
        ])
        .expect("parse library extract");
        match cli.command.expect("command") {
            Command::Library(args) => match args.command {
                LibraryCommand::Extract(extract) => {
                    assert_eq!(extract.out_dir, "/tmp/helm-apps");
                }
                other => panic!("unexpected library command: {other:?}"),
            },
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn completion_help_mentions_shells() {
        let mut cmd = Cli::command();
        let mut buf = Vec::new();
        cmd.find_subcommand_mut("completion")
            .expect("completion subcommand")
            .write_long_help(&mut buf)
            .expect("write help");
        let help = String::from_utf8(buf).expect("utf8");
        assert!(help.contains("Target shell"));
        assert!(help.contains("bash"));
        assert!(help.contains("zsh"));
        assert!(help.contains("fish"));
        assert!(help.contains("powershell"));
        assert!(help.contains("elvish"));
    }

    #[test]
    fn parses_compose_inspect_web_flags() {
        let cli = Cli::try_parse_from([
            "happ",
            "compose-inspect",
            "--path",
            "/tmp/compose.yaml",
            "--web=true",
            "--addr",
            "127.0.0.1:9900",
            "--open-browser=false",
        ])
        .expect("parse compose-inspect");
        match cli.command.expect("command") {
            Command::ComposeInspect(args) => {
                assert!(args.web);
                assert_eq!(args.addr, "127.0.0.1:9900");
                assert!(!args.open_browser);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_dyff_extended_flags() {
        let cli = Cli::try_parse_from([
            "happ",
            "dyff",
            "--from",
            "a.yaml",
            "--to",
            "b.yaml",
            "--format",
            "json",
            "--color",
            "never",
            "--stats",
            "--label-from",
            "source",
            "--label-to",
            "generated",
        ])
        .expect("parse dyff");
        match cli.command.expect("command") {
            Command::Dyff(args) => {
                assert_eq!(args.format, "json");
                assert_eq!(args.color, "never");
                assert!(args.stats);
                assert_eq!(args.label_from.as_deref(), Some("source"));
                assert_eq!(args.label_to.as_deref(), Some("generated"));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_dyff_github_summary_only_flags() {
        let cli = Cli::try_parse_from([
            "happ",
            "dyff",
            "--from",
            "a.yaml",
            "--to",
            "b.yaml",
            "--format",
            "github",
            "--summary-only",
        ])
        .expect("parse dyff github");
        match cli.command.expect("command") {
            Command::Dyff(args) => {
                assert_eq!(args.format, "github");
                assert!(args.summary_only);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_jq_subcommand() {
        let cli = Cli::try_parse_from([
            "happ",
            "jq",
            "--query",
            ".a",
            "--input",
            "in.json",
            "--compact",
            "--raw-output",
        ])
        .expect("parse jq");
        match cli.command.expect("command") {
            Command::Jq(args) => {
                assert_eq!(args.query, ".a");
                assert_eq!(args.input, "in.json");
                assert_eq!(args.doc_mode, "first");
                assert_eq!(args.doc_index, None);
                assert!(args.compact);
                assert!(args.raw_output);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_yq_subcommand() {
        let cli = Cli::try_parse_from(["happ", "yq", "--query", ".a", "--input", "in.yaml"])
            .expect("parse yq");
        match cli.command.expect("command") {
            Command::Yq(args) => {
                assert_eq!(args.query, ".a");
                assert_eq!(args.input, "in.yaml");
                assert_eq!(args.doc_mode, "first");
                assert_eq!(args.doc_index, None);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_query_doc_mode_and_index() {
        let cli = Cli::try_parse_from([
            "happ",
            "yq",
            "--query",
            ".a",
            "--input",
            "multi.yaml",
            "--doc-mode",
            "index",
            "--doc-index",
            "2",
        ])
        .expect("parse yq");
        match cli.command.expect("command") {
            Command::Yq(args) => {
                assert_eq!(args.doc_mode, "index");
                assert_eq!(args.doc_index, Some(2));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn jq_help_mentions_json_and_yaml_input() {
        let mut cmd = Cli::command();
        let mut buf = Vec::new();
        cmd.find_subcommand_mut("jq")
            .expect("jq subcommand")
            .write_long_help(&mut buf)
            .expect("write help");
        let help = String::from_utf8(buf).expect("utf8");
        assert!(help.contains("Input may be JSON or YAML"));
        assert!(help.contains("Supports both JSON and YAML."));
    }

    #[test]
    fn yq_help_mentions_yaml_and_json_input() {
        let mut cmd = Cli::command();
        let mut buf = Vec::new();
        cmd.find_subcommand_mut("yq")
            .expect("yq subcommand")
            .write_long_help(&mut buf)
            .expect("write help");
        let help = String::from_utf8(buf).expect("utf8");
        assert!(help.contains("Input may be YAML or JSON"));
        assert!(help.contains("Supports both JSON and YAML."));
    }

    #[test]
    fn parses_top_level_web_mode_without_subcommand() {
        let cli = Cli::try_parse_from(["happ", "--web", "--web-addr", "127.0.0.1:9999"])
            .expect("parse web");
        assert!(cli.web);
        assert!(!cli.studio);
        assert!(!cli.web_stdin);
        assert_eq!(cli.web_addr, "127.0.0.1:9999");
        assert!(cli.web_open_browser);
        assert!(cli.command.is_none());
    }

    #[test]
    fn parses_top_level_web_mode_without_open_browser() {
        let cli = Cli::try_parse_from([
            "happ",
            "--web",
            "--web-addr",
            "127.0.0.1:9999",
            "--web-open-browser=false",
        ])
        .expect("parse web no open");
        assert!(cli.web);
        assert!(!cli.studio);
        assert!(!cli.web_stdin);
        assert_eq!(cli.web_addr, "127.0.0.1:9999");
        assert!(!cli.web_open_browser);
        assert!(cli.command.is_none());
    }

    #[test]
    fn parses_top_level_web_mode_with_stdin_opt_in() {
        let cli = Cli::try_parse_from(["happ", "--web", "--web-stdin"]).expect("parse web stdin");
        assert!(cli.web);
        assert!(!cli.studio);
        assert!(cli.web_stdin);
        assert!(cli.command.is_none());
    }

    #[test]
    fn parses_top_level_studio_mode_without_subcommand() {
        let cli = Cli::try_parse_from(["happ", "--studio"]).expect("parse studio");
        assert!(!cli.web);
        assert!(cli.studio);
        assert!(cli.command.is_none());
    }

    #[test]
    fn rejects_top_level_web_and_studio_combination() {
        let err = Cli::try_parse_from(["happ", "--web", "--studio"]).expect_err("arg conflict");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn default_ports_for_web_and_studio_are_stable_and_distinct() {
        let cli = Cli::try_parse_from(["happ"]).expect("parse defaults");
        assert_eq!(cli.web_addr, DEFAULT_WEB_ADDR);
        assert!(!cli.studio);
        assert_ne!(DEFAULT_WEB_ADDR, DEFAULT_STUDIO_ADDR);
        assert_ne!(DEFAULT_STUDIO_ADDR, DEFAULT_COMPOSE_STUDIO_ADDR);
    }

    #[test]
    fn supports_top_level_version_flag() {
        let err = Cli::try_parse_from(["happ", "--version"]).expect_err("display version");
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
        assert!(err.to_string().contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn parses_chart_verify_equivalence_flag() {
        let cli = Cli::try_parse_from([
            "happ",
            "chart",
            "--path",
            "/tmp/chart",
            "--verify-equivalence",
        ])
        .expect("parse chart verify");
        match cli.command.expect("command") {
            Command::Chart(args) => assert!(args.verify_equivalence),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_chart_allow_template_include_flags() {
        let cli = Cli::try_parse_from([
            "happ",
            "chart",
            "--path",
            "/tmp/chart",
            "--allow-template-include",
            "opensearch-cluster.*",
            "--allow-template-include",
            "my-helper",
        ])
        .expect("parse chart allow template includes");
        match cli.command.expect("command") {
            Command::Chart(args) => assert_eq!(
                args.allow_template_includes,
                vec!["opensearch-cluster.*", "my-helper"]
            ),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_chart_unsupported_template_mode_flag() {
        let cli = Cli::try_parse_from([
            "happ",
            "chart",
            "--path",
            "/tmp/chart",
            "--unsupported-template-mode",
            "escape",
        ])
        .expect("parse chart unsupported template mode");
        match cli.command.expect("command") {
            Command::Chart(args) => assert_eq!(args.unsupported_template_mode, "escape"),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_batch_command_with_required_dirs() {
        let cli = Cli::try_parse_from([
            "happ",
            "batch",
            "--charts-dir",
            "/tmp/charts",
            "--out-dir",
            "/tmp/out",
        ])
        .expect("parse batch");
        match cli.command.expect("command") {
            Command::Batch(args) => {
                assert_eq!(args.charts_dir, "/tmp/charts");
                assert_eq!(args.out_dir, "/tmp/out");
                assert!(!args.keep_going);
                assert_eq!(args.import.import_strategy, "raw");
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn import_args_from_shared_copies_fields() {
        let mut shared = ImportSharedArgs::default();
        shared.env = "prod".to_string();
        shared.import_strategy = "helpers".to_string();
        shared.allow_template_includes = vec!["foo.*".to_string()];
        shared.verify_equivalence = true;
        shared.release_name = "rel".to_string();
        shared.namespace = Some("ns".to_string());
        shared.include_crds = true;
        shared.values_files = vec!["values.yaml".to_string()];
        shared.set_values = vec!["a=b".to_string()];
        shared.library_chart_path = Some("/tmp/lib".to_string());

        let args = ImportArgs::from_shared("/tmp/chart".to_string(), &shared);
        assert_eq!(args.path, "/tmp/chart");
        assert_eq!(args.env, "prod");
        assert_eq!(args.import_strategy, "helpers");
        assert_eq!(args.allow_template_includes, vec!["foo.*"]);
        assert!(args.verify_equivalence);
        assert_eq!(args.release_name, "rel");
        assert_eq!(args.namespace.as_deref(), Some("ns"));
        assert!(args.include_crds);
        assert_eq!(args.values_files, vec!["values.yaml"]);
        assert_eq!(args.set_values, vec!["a=b"]);
        assert_eq!(args.library_chart_path.as_deref(), Some("/tmp/lib"));
        assert!(args.out_chart_dir.is_none());
        assert!(args.output.is_none());
    }
}

#[derive(clap::Args, Debug, Clone)]
pub struct ImportArgs {
    #[arg(long)]
    pub path: String,
    #[arg(long, default_value = "dev")]
    pub env: String,
    #[arg(long, default_value = "apps-k8s-manifests")]
    pub group_name: String,
    #[arg(long, default_value = "apps-k8s-manifests")]
    pub group_type: String,
    #[arg(long, default_value_t = 24)]
    pub min_include_bytes: usize,
    #[arg(long, action = ArgAction::SetTrue)]
    pub include_status: bool,
    #[arg(long)]
    pub output: Option<String>,
    #[arg(long)]
    pub out_chart_dir: Option<String>,
    #[arg(long)]
    pub chart_name: Option<String>,
    #[arg(long)]
    pub library_chart_path: Option<String>,
    #[arg(long, default_value = "raw")]
    pub import_strategy: String,
    #[arg(
        long = "allow-template-include",
        value_name = "NAME|PREFIX*",
        help = "Keep extra include helpers as templated values during import (repeatable; supports '*' suffix wildcard)"
    )]
    pub allow_template_includes: Vec<String>,
    #[arg(
        long = "unsupported-template-mode",
        default_value = "error",
        value_parser = ["error", "escape"],
        help = "How to handle source includes not supported by library chart: error or escape as literal template"
    )]
    pub unsupported_template_mode: String,
    #[arg(
        long,
        action = ArgAction::SetTrue,
        help = "Verify source chart render against generated library chart render (chart source only)"
    )]
    pub verify_equivalence: bool,

    #[arg(long, default_value = "imported")]
    pub release_name: String,
    #[arg(long)]
    pub namespace: Option<String>,
    #[arg(long = "values")]
    pub values_files: Vec<String>,
    #[arg(long = "set")]
    pub set_values: Vec<String>,
    #[arg(long = "set-string")]
    pub set_string_values: Vec<String>,
    #[arg(long = "set-file")]
    pub set_file_values: Vec<String>,
    #[arg(long = "set-json")]
    pub set_json_values: Vec<String>,
    #[arg(long)]
    pub kube_version: Option<String>,
    #[arg(long = "api-version")]
    pub api_versions: Vec<String>,
    #[arg(long, action = ArgAction::SetTrue)]
    pub include_crds: bool,
    #[arg(long)]
    pub write_rendered_output: Option<String>,
}

#[derive(clap::Args, Debug, Clone)]
pub struct ImportSharedArgs {
    #[arg(long, default_value = "dev")]
    pub env: String,
    #[arg(long, default_value = "apps-k8s-manifests")]
    pub group_name: String,
    #[arg(long, default_value = "apps-k8s-manifests")]
    pub group_type: String,
    #[arg(long, default_value_t = 24)]
    pub min_include_bytes: usize,
    #[arg(long, action = ArgAction::SetTrue)]
    pub include_status: bool,
    #[arg(long, default_value = "raw")]
    pub import_strategy: String,
    #[arg(
        long = "allow-template-include",
        value_name = "NAME|PREFIX*",
        help = "Keep extra include helpers as templated values during import (repeatable; supports '*' suffix wildcard)"
    )]
    pub allow_template_includes: Vec<String>,
    #[arg(
        long = "unsupported-template-mode",
        default_value = "error",
        value_parser = ["error", "escape"],
        help = "How to handle source includes not supported by library chart: error or escape as literal template"
    )]
    pub unsupported_template_mode: String,
    #[arg(
        long,
        action = ArgAction::SetTrue,
        help = "Verify source chart render against generated library chart render (chart source only)"
    )]
    pub verify_equivalence: bool,
    #[arg(long, default_value = "imported")]
    pub release_name: String,
    #[arg(long)]
    pub namespace: Option<String>,
    #[arg(long = "values")]
    pub values_files: Vec<String>,
    #[arg(long = "set")]
    pub set_values: Vec<String>,
    #[arg(long = "set-string")]
    pub set_string_values: Vec<String>,
    #[arg(long = "set-file")]
    pub set_file_values: Vec<String>,
    #[arg(long = "set-json")]
    pub set_json_values: Vec<String>,
    #[arg(long)]
    pub kube_version: Option<String>,
    #[arg(long = "api-version")]
    pub api_versions: Vec<String>,
    #[arg(long, action = ArgAction::SetTrue)]
    pub include_crds: bool,
    #[arg(long)]
    pub library_chart_path: Option<String>,
}

impl Default for ImportSharedArgs {
    fn default() -> Self {
        Self {
            env: "dev".to_string(),
            group_name: "apps-k8s-manifests".to_string(),
            group_type: "apps-k8s-manifests".to_string(),
            min_include_bytes: 24,
            include_status: false,
            import_strategy: "raw".to_string(),
            allow_template_includes: Vec::new(),
            unsupported_template_mode: "error".to_string(),
            verify_equivalence: false,
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
            library_chart_path: None,
        }
    }
}

impl ImportArgs {
    pub fn from_shared(path: String, shared: &ImportSharedArgs) -> Self {
        Self {
            path,
            env: shared.env.clone(),
            group_name: shared.group_name.clone(),
            group_type: shared.group_type.clone(),
            min_include_bytes: shared.min_include_bytes,
            include_status: shared.include_status,
            output: None,
            out_chart_dir: None,
            chart_name: None,
            library_chart_path: shared.library_chart_path.clone(),
            import_strategy: shared.import_strategy.clone(),
            allow_template_includes: shared.allow_template_includes.clone(),
            unsupported_template_mode: shared.unsupported_template_mode.clone(),
            verify_equivalence: shared.verify_equivalence,
            release_name: shared.release_name.clone(),
            namespace: shared.namespace.clone(),
            values_files: shared.values_files.clone(),
            set_values: shared.set_values.clone(),
            set_string_values: shared.set_string_values.clone(),
            set_file_values: shared.set_file_values.clone(),
            set_json_values: shared.set_json_values.clone(),
            kube_version: shared.kube_version.clone(),
            api_versions: shared.api_versions.clone(),
            include_crds: shared.include_crds,
            write_rendered_output: None,
        }
    }
}

#[derive(clap::Args, Debug, Clone)]
pub struct BatchArgs {
    #[arg(long, value_name = "DIR", help = "Directory with source charts")]
    pub charts_dir: String,
    #[arg(
        long,
        value_name = "DIR",
        help = "Directory where converted library-format charts will be written"
    )]
    pub out_dir: String,
    #[arg(
        long,
        action = ArgAction::SetTrue,
        help = "Continue conversion after per-chart errors and report all failures"
    )]
    pub keep_going: bool,
    #[command(flatten)]
    pub import: ImportSharedArgs,
}

#[derive(clap::Args, Debug, Clone)]
pub struct InspectArgs {
    #[arg(long)]
    pub path: String,
    #[arg(long, default_value = "inspect")]
    pub release_name: String,
    #[arg(long)]
    pub namespace: Option<String>,
    #[arg(long = "values")]
    pub values_files: Vec<String>,
    #[arg(long = "set")]
    pub set_values: Vec<String>,
    #[arg(long = "set-string")]
    pub set_string_values: Vec<String>,
    #[arg(long = "set-file")]
    pub set_file_values: Vec<String>,
    #[arg(long = "set-json")]
    pub set_json_values: Vec<String>,
    #[arg(long)]
    pub kube_version: Option<String>,
    #[arg(long = "api-version")]
    pub api_versions: Vec<String>,
    #[arg(long, action = ArgAction::SetTrue)]
    pub include_crds: bool,
    #[arg(long, default_value_t = true, action = ArgAction::Set)]
    pub web: bool,
    #[arg(long, default_value = DEFAULT_STUDIO_ADDR)]
    pub addr: String,
}

#[derive(clap::Args, Debug, Clone)]
pub struct ComposeInspectArgs {
    #[arg(long)]
    pub path: String,
    #[arg(long, default_value = "yaml")]
    pub format: String,
    #[arg(long)]
    pub output: Option<String>,
    #[arg(long, default_value_t = false, action = ArgAction::Set)]
    pub web: bool,
    #[arg(long, default_value = DEFAULT_COMPOSE_STUDIO_ADDR)]
    pub addr: String,
    #[arg(long, default_value_t = true, action = ArgAction::Set)]
    pub open_browser: bool,
}

#[derive(clap::Args, Debug, Clone)]
pub struct DyffArgs {
    #[arg(long)]
    pub from: String,
    #[arg(long)]
    pub to: String,
    #[arg(long, action = ArgAction::SetTrue)]
    pub ignore_order: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    pub ignore_whitespace: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    pub quiet: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    pub fail_on_diff: bool,
    #[arg(long, default_value = "text")]
    pub format: String,
    #[arg(long, default_value = "auto")]
    pub color: String,
    #[arg(long, default_value_t = false, action = ArgAction::SetTrue)]
    pub stats: bool,
    #[arg(long, default_value_t = false, action = ArgAction::SetTrue)]
    pub summary_only: bool,
    #[arg(long)]
    pub label_from: Option<String>,
    #[arg(long)]
    pub label_to: Option<String>,
    #[arg(long)]
    pub output: Option<String>,
}
