//! Which language server answers for which language.
//!
//! Adding a language is one entry here. Nothing else in the bridge, and no tool
//! schema, changes.

use std::path::{Path, PathBuf};

pub(crate) struct LanguageSpec {
    /// Identifier a caller passes as `lang`.
    pub(crate) id: &'static str,
    /// Human-facing name for listings.
    pub(crate) label: &'static str,
    /// File extensions that select this language, without the dot.
    pub(crate) extensions: &'static [&'static str],
    /// Exact file names that select it, for languages addressed by convention.
    pub(crate) filenames: &'static [&'static str],
    /// `textDocument/didOpen` language id the server expects.
    pub(crate) language_id: &'static str,
    /// Command and arguments to spawn, when not served in-process.
    pub(crate) command: &'static [&'static str],
    /// Files that mark the root of a project for this language, nearest first.
    pub(crate) root_markers: &'static [&'static str],
    /// What to tell the caller when the server is not installed.
    pub(crate) install_hint: &'static str,
    /// Whether happ answers this language itself rather than spawning anything.
    pub(crate) in_process: bool,
}

impl LanguageSpec {
    pub(crate) fn default_command(&self) -> Vec<String> {
        self.command
            .iter()
            .map(|part| (*part).to_string())
            .collect()
    }

    /// The project root to hand the server, found by walking up from the file
    /// until a root marker appears. Falls back to the file's directory, which
    /// is what a single-file scratch project effectively is.
    pub(crate) fn workspace_root(&self, file: &Path) -> PathBuf {
        let start = if file.is_dir() {
            Some(file.to_path_buf())
        } else {
            file.parent().map(Path::to_path_buf)
        };
        let Some(mut current) = start else {
            return PathBuf::from(".");
        };
        let first = current.clone();
        loop {
            if self
                .root_markers
                .iter()
                .any(|marker| current.join(marker).exists())
            {
                return current;
            }
            if !current.pop() {
                return first;
            }
        }
    }
}

pub(crate) const LANGUAGES: &[LanguageSpec] = &[
    LanguageSpec {
        id: "helm-apps",
        label: "helm-apps values (served by happ itself)",
        extensions: &[],
        filenames: &["values.yaml", "values.yml", "secret-values.yaml"],
        language_id: "yaml",
        command: &[],
        root_markers: &["Chart.yaml"],
        install_hint: "Built into happ; nothing to install.",
        in_process: true,
    },
    LanguageSpec {
        id: "go",
        label: "Go",
        extensions: &["go"],
        filenames: &[],
        language_id: "go",
        command: &["gopls"],
        root_markers: &["go.work", "go.mod"],
        install_hint: "Install with: go install golang.org/x/tools/gopls@latest",
        in_process: false,
    },
    LanguageSpec {
        id: "rust",
        label: "Rust",
        extensions: &["rs"],
        filenames: &[],
        language_id: "rust",
        command: &["rust-analyzer"],
        root_markers: &["Cargo.toml"],
        install_hint: "Install with: rustup component add rust-analyzer",
        in_process: false,
    },
    LanguageSpec {
        id: "typescript",
        label: "TypeScript and JavaScript",
        extensions: &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"],
        filenames: &[],
        language_id: "typescript",
        command: &["typescript-language-server", "--stdio"],
        root_markers: &["tsconfig.json", "jsconfig.json", "package.json"],
        install_hint: "Install with: npm i -g typescript typescript-language-server",
        in_process: false,
    },
    LanguageSpec {
        id: "python",
        label: "Python",
        extensions: &["py", "pyi"],
        filenames: &[],
        language_id: "python",
        command: &["pyright-langserver", "--stdio"],
        root_markers: &[
            "pyproject.toml",
            "setup.py",
            "setup.cfg",
            "requirements.txt",
        ],
        install_hint:
            "Install with: npm i -g pyright (or override with --language-server python=<cmd>)",
        in_process: false,
    },
    LanguageSpec {
        id: "c",
        label: "C and C++",
        extensions: &["c", "h", "cc", "cpp", "cxx", "hpp", "hh"],
        filenames: &[],
        language_id: "cpp",
        command: &["clangd"],
        root_markers: &["compile_commands.json", "CMakeLists.txt", "Makefile"],
        install_hint: "Install clangd from your distribution or from LLVM releases.",
        in_process: false,
    },
];

pub(crate) fn spec_for_language(id: &str) -> Option<&'static LanguageSpec> {
    LANGUAGES.iter().find(|spec| spec.id == id)
}

/// The languages whose project markers sit directly in `dir`, for a question
/// that names no file -- searching the whole project for a symbol.
///
/// Only `dir` itself is examined, not its ancestors: a directory somewhere below
/// a Makefile is not thereby a C project, and a caller who meant a different
/// root can always name a file in it. In-process languages are skipped because
/// they answer per file and have no project-wide index to search.
pub(crate) fn specs_rooted_at(dir: &Path) -> Vec<&'static LanguageSpec> {
    LANGUAGES
        .iter()
        .filter(|spec| !spec.in_process)
        .filter(|spec| {
            spec.root_markers
                .iter()
                .any(|marker| dir.join(marker).exists())
        })
        .collect()
}

/// Finds `program`, looking where each toolchain actually installs its server
/// as well as on PATH.
///
/// `go install` puts gopls in `$GOPATH/bin` and rustup puts rust-analyzer in
/// `~/.cargo/bin`; neither is necessarily on the PATH of an editor or MCP host
/// started from a desktop session. Reporting "not installed" for a server
/// sitting in its own default location is the wrong answer.
pub(crate) fn resolve_program(language: &str, program: &str) -> Option<PathBuf> {
    if program.contains(std::path::MAIN_SEPARATOR) {
        let path = PathBuf::from(program);
        return path.is_file().then_some(path);
    }

    let on_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let candidate = dir.join(program);
            candidate.is_file().then_some(candidate)
        })
    });
    if on_path.is_some() {
        return on_path;
    }

    conventional_dirs(language).into_iter().find_map(|dir| {
        let candidate = dir.join(program);
        candidate.is_file().then_some(candidate)
    })
}

/// Where a toolchain installs its binaries by convention.
fn conventional_dirs(language: &str) -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let from_env = |key: &str| std::env::var_os(key).map(PathBuf::from);

    match language {
        "go" => [
            from_env("GOBIN"),
            from_env("GOPATH").map(|gopath| gopath.join("bin")),
            home.map(|home| home.join("go/bin")),
        ]
        .into_iter()
        .flatten()
        .collect(),
        "rust" => [
            from_env("CARGO_HOME").map(|cargo| cargo.join("bin")),
            home.map(|home| home.join(".cargo/bin")),
        ]
        .into_iter()
        .flatten()
        .collect(),
        "typescript" | "python" => [
            from_env("npm_config_prefix").map(|prefix| prefix.join("bin")),
            home.map(|home| home.join(".npm-global/bin")),
            Some(PathBuf::from("/usr/local/bin")),
            Some(PathBuf::from("/opt/homebrew/bin")),
        ]
        .into_iter()
        .flatten()
        .collect(),
        _ => Vec::new(),
    }
}

/// Picks the language for a file from its name, so a caller never has to say
/// what `main.go` obviously is.
///
/// helm-apps wins over plain YAML only for files that sit in a chart, because
/// `values.yaml` outside one is just YAML.
pub(crate) fn detect_language(file: &Path) -> Option<&'static LanguageSpec> {
    let name = file.file_name()?.to_str()?;

    if let Some(spec) = LANGUAGES.iter().find(|spec| {
        spec.filenames
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(name))
    }) {
        if spec.workspace_root(file).join("Chart.yaml").exists() {
            return Some(spec);
        }
    }

    let extension = file.extension()?.to_str()?.to_ascii_lowercase();
    LANGUAGES.iter().find(|spec| {
        spec.extensions
            .iter()
            .any(|candidate| *candidate == extension)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_ids_are_unique() {
        let mut ids: Vec<&str> = LANGUAGES.iter().map(|spec| spec.id).collect();
        let before = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), before, "duplicate language id in the registry");
    }

    #[test]
    fn only_the_built_in_language_runs_in_process() {
        for spec in LANGUAGES {
            if spec.in_process {
                assert!(spec.command.is_empty(), "{} must spawn nothing", spec.id);
            } else {
                assert!(!spec.command.is_empty(), "{} needs a command", spec.id);
            }
        }
    }

    #[test]
    fn a_project_root_is_recognised_by_its_marker() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(
            specs_rooted_at(dir.path()).is_empty(),
            "an empty directory is no project"
        );

        std::fs::write(dir.path().join("Cargo.toml"), "[package]\n").expect("write");
        let found = specs_rooted_at(dir.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "rust");

        // A chart alongside is answered per file, so it must not make the root
        // ambiguous for a project-wide search.
        std::fs::write(dir.path().join("Chart.yaml"), "name: demo\n").expect("write");
        assert_eq!(specs_rooted_at(dir.path()).len(), 1);
    }

    #[test]
    fn a_marker_in_a_parent_does_not_claim_a_subdirectory() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\n").expect("write");
        let nested = dir.path().join("src");
        std::fs::create_dir(&nested).expect("mkdir");
        assert!(specs_rooted_at(&nested).is_empty());
    }

    #[test]
    fn files_are_matched_by_extension() {
        assert_eq!(
            detect_language(Path::new("/src/main.go")).map(|spec| spec.id),
            Some("go")
        );
        assert_eq!(
            detect_language(Path::new("/src/lib.rs")).map(|spec| spec.id),
            Some("rust")
        );
        assert_eq!(
            detect_language(Path::new("/src/notes.txt")).map(|s| s.id),
            None
        );
    }

    #[test]
    fn values_yaml_is_helm_apps_only_inside_a_chart() {
        let chart = tempfile::tempdir().expect("tempdir");
        std::fs::write(chart.path().join("Chart.yaml"), "name: demo\n").expect("chart");
        let inside = chart.path().join("values.yaml");
        std::fs::write(&inside, "global: {}\n").expect("values");
        assert_eq!(
            detect_language(&inside).map(|spec| spec.id),
            Some("helm-apps")
        );

        let loose = tempfile::tempdir().expect("tempdir");
        let outside = loose.path().join("values.yaml");
        std::fs::write(&outside, "global: {}\n").expect("values");
        assert_eq!(
            detect_language(&outside).map(|spec| spec.id),
            None,
            "values.yaml outside a chart is not helm-apps"
        );
    }

    /// `go install` drops gopls in `$GOPATH/bin`, which is routinely absent
    /// from the PATH of a desktop-launched MCP host. Reporting it as missing
    /// would be wrong, and refusing to start it worse.
    #[test]
    fn a_server_in_its_toolchains_default_location_is_found_off_path() {
        let gopath = tempfile::tempdir().expect("tempdir");
        let bin = gopath.path().join("bin");
        std::fs::create_dir_all(&bin).expect("bin dir");
        let gopls = bin.join("gopls");
        std::fs::write(&gopls, "#!/bin/sh\n").expect("write");

        let restore = std::env::var_os("GOPATH");
        // SAFETY: single-threaded test, restored before returning.
        unsafe { std::env::set_var("GOPATH", gopath.path()) };
        let found = resolve_program("go", "gopls");
        match restore {
            Some(value) => unsafe { std::env::set_var("GOPATH", value) },
            None => unsafe { std::env::remove_var("GOPATH") },
        }

        assert_eq!(found.as_deref(), Some(gopls.as_path()));
    }

    #[test]
    fn an_absolute_command_is_taken_as_given() {
        assert_eq!(resolve_program("go", "/definitely/not/here/gopls"), None);
        assert!(resolve_program("go", "/bin/sh").is_some());
    }

    #[test]
    fn a_server_that_exists_nowhere_is_not_invented() {
        assert_eq!(resolve_program("go", "happ-no-such-server-binary"), None);
    }

    #[test]
    fn the_workspace_root_is_the_nearest_marker() {
        let project = tempfile::tempdir().expect("tempdir");
        std::fs::write(project.path().join("go.mod"), "module demo\n").expect("go.mod");
        let nested = project.path().join("internal/api");
        std::fs::create_dir_all(&nested).expect("nested dirs");
        let file = nested.join("handler.go");
        std::fs::write(&file, "package api\n").expect("source");

        let spec = spec_for_language("go").expect("go spec");
        assert_eq!(spec.workspace_root(&file), project.path());
    }

    #[test]
    fn a_file_with_no_marker_above_it_roots_at_its_own_directory() {
        let loose = tempfile::tempdir().expect("tempdir");
        let file = loose.path().join("scratch.go");
        std::fs::write(&file, "package main\n").expect("source");
        let spec = spec_for_language("go").expect("go spec");
        assert_eq!(spec.workspace_root(&file), loose.path());
    }
}
