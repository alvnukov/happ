//! `happ mcp setup -c claude,codex,opencode` and its inverse, `happ mcp remove`.
//!
//! Registering happ with a client is three separate things, and each client
//! spells all three differently:
//!
//! * the MCP server entry, so the tools exist at all -- [`config`];
//! * a block in the file the client reads for instructions, so a model knows
//!   the tools are worth reaching for -- [`instructions`];
//! * skills, which carry the step-by-step workflows without occupying context
//!   until they are needed -- [`skills`].
//!
//! Getting any of them wrong fails quietly: the client simply never offers the
//! tools, or offers them and never reaches for them.
//!
//! Both commands are idempotent by construction rather than by care. Every
//! write goes through [`apply`], which compares what it is about to write
//! against what is on disk and does nothing when they match. One comparison
//! covers all three formats -- JSON, TOML and markdown -- so there is no
//! per-format cleverness to get wrong, and a second run reports that everything
//! is already current instead of rewriting files to their own contents.

mod config;
mod instructions;
mod skills;

use std::path::{Path, PathBuf};

use super::Error;

/// A client happ knows how to register itself with.
struct ClientSpec {
    id: &'static str,
    label: &'static str,
    format: ConfigFormat,
    /// Where this client looks for skills, relative to the project or the home
    /// directory. `None` for a client with no skill support to speak of.
    skills: Option<Scoped>,
    /// The file this client reads for instructions.
    guidance: Scoped,
}

enum ConfigFormat {
    /// `{"mcpServers": {"happ": {"command": ..., "args": [...]}}}`
    ClaudeJson,
    /// `[mcp_servers.happ]` in TOML.
    CodexToml,
    /// `{"mcp": {"happ": {"type": "local", "command": [...], "enabled": true}}}`
    OpenCodeJson,
}

/// A path that differs between the project-scoped and the user-wide install.
struct Scoped {
    project: &'static str,
    global: &'static str,
}

impl Scoped {
    fn resolve(&self, global: bool) -> Result<PathBuf, Error> {
        if global {
            Ok(home()?.join(self.global))
        } else {
            Ok(cwd()?.join(self.project))
        }
    }
}

/// Where each client reads what, all of it from the clients' own documentation.
///
/// Two details are worth keeping in view. Claude Code reads `CLAUDE.md` and not
/// `AGENTS.md`, so the two files are not interchangeable. And OpenCode also
/// searches `.claude/skills`, but happ writes its skills to OpenCode's own
/// directory anyway: sharing one directory would mean removing happ from Claude
/// Code silently broke it for OpenCode.
const CLIENTS: &[ClientSpec] = &[
    ClientSpec {
        id: "claude",
        label: "Claude Code",
        format: ConfigFormat::ClaudeJson,
        skills: Some(Scoped {
            project: ".claude/skills",
            global: ".claude/skills",
        }),
        guidance: Scoped {
            project: "CLAUDE.md",
            global: ".claude/CLAUDE.md",
        },
    },
    ClientSpec {
        id: "codex",
        label: "Codex CLI",
        format: ConfigFormat::CodexToml,
        // Codex documents AGENTS.md but no skill directory, and guessing at one
        // would scatter files it never reads.
        skills: None,
        guidance: Scoped {
            project: "AGENTS.md",
            global: ".codex/AGENTS.md",
        },
    },
    ClientSpec {
        id: "opencode",
        label: "OpenCode",
        format: ConfigFormat::OpenCodeJson,
        skills: Some(Scoped {
            project: ".opencode/skills",
            global: ".config/opencode/skills",
        }),
        guidance: Scoped {
            project: "AGENTS.md",
            global: ".config/opencode/AGENTS.md",
        },
    },
];

/// What one file ended up doing.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Outcome {
    /// The file was changed.
    Done,
    /// It already said exactly this. Nothing was written.
    Current,
    /// There was nothing here to do.
    Nothing,
    /// A dry run: this is what would have changed.
    Would,
}

impl Outcome {
    fn word(self, done: &str, nothing: &str) -> String {
        match self {
            Outcome::Done => done.to_string(),
            Outcome::Current => "already up to date".to_string(),
            Outcome::Nothing => nothing.to_string(),
            Outcome::Would => format!("would be {done}"),
        }
    }
}

pub(crate) fn run(
    setup: &crate::cli::McpSetupArgs,
    args: &crate::cli::McpArgs,
) -> Result<(), Error> {
    let command = happ_command()?;
    let launch_args = launch_args(args);

    let mut report = Vec::new();
    for requested in &setup.clients {
        let spec = client(requested)?;
        let mut lines = vec![spec.label.to_string()];

        let path = config_path(spec, setup.global)?;
        let existing = read_config(&path)?;
        let entry = match spec.format {
            ConfigFormat::ClaudeJson => config::merge_json(
                &existing,
                "mcpServers",
                config::claude_entry(&command, &launch_args),
            )?,
            ConfigFormat::OpenCodeJson => config::merge_json(
                &existing,
                "mcp",
                config::opencode_entry(&command, &launch_args),
            )?,
            ConfigFormat::CodexToml => config::merge_codex_toml(&existing, &command, &launch_args)?,
        };
        lines.push(note(
            "server",
            &path,
            apply(&path, Some(&entry), setup.dry_run)?.word("registered", "nothing to do"),
        ));

        if !setup.no_instructions {
            let path = spec.guidance.resolve(setup.global)?;
            let wanted = instructions::with_block(&read_config(&path)?)?;
            lines.push(note(
                "instructions",
                &path,
                apply(&path, Some(&wanted), setup.dry_run)?.word("added", "nothing to do"),
            ));
        }

        if !setup.no_skills {
            lines.push(install_skills(spec, setup.global, setup.dry_run)?);
        }

        report.push(lines.join("\n"));
    }

    println!("{}", report.join("\n\n"));
    if !setup.dry_run {
        println!("\nRestart the client to pick up the new server.");
    }
    Ok(())
}

pub(crate) fn remove(args: &crate::cli::McpRemoveArgs) -> Result<(), Error> {
    let mut report = Vec::new();
    let mut changed = false;
    for requested in &args.clients {
        let spec = client(requested)?;
        let mut lines = vec![spec.label.to_string()];

        let path = config_path(spec, args.global)?;
        let existing = read_config(&path)?;
        let entry = match spec.format {
            ConfigFormat::ClaudeJson => config::without_json(&existing, "mcpServers")?,
            ConfigFormat::OpenCodeJson => config::without_json(&existing, "mcp")?,
            ConfigFormat::CodexToml => config::without_codex_toml(&existing)?,
        };
        let outcome = apply(&path, entry.as_deref(), args.dry_run)?;
        changed |= outcome == Outcome::Done;
        lines.push(note(
            "server",
            &path,
            outcome.word("removed", "not registered"),
        ));

        if !args.no_instructions {
            let path = spec.guidance.resolve(args.global)?;
            let wanted = instructions::without_block(&read_config(&path)?)?;
            let outcome = apply(&path, wanted.as_deref(), args.dry_run)?;
            changed |= outcome == Outcome::Done;
            lines.push(note(
                "instructions",
                &path,
                outcome.word("removed", "no happ block"),
            ));
        }

        if !args.no_skills {
            let (line, touched) = uninstall_skills(spec, args.global, args.dry_run)?;
            changed |= touched;
            lines.push(line);
        }

        report.push(lines.join("\n"));
    }

    println!("{}", report.join("\n\n"));
    if changed {
        println!("\nRestart the client to drop the server.");
    }
    Ok(())
}

fn install_skills(spec: &ClientSpec, global: bool, dry_run: bool) -> Result<String, Error> {
    let Some(scoped) = spec.skills.as_ref() else {
        return Ok(format!(
            "  {:<13} skipped -- {} documents no skill directory",
            "skills", spec.label
        ));
    };
    let dir = scoped.resolve(global)?;

    let mut written = Vec::new();
    let mut current = Vec::new();
    for skill in skills::SKILLS {
        let path = dir.join(skill.name).join("SKILL.md");
        match apply(&path, Some(skill.body), dry_run)? {
            Outcome::Done | Outcome::Would => written.push(skill.name),
            _ => current.push(skill.name),
        }
    }

    let detail = match (written.is_empty(), current.is_empty()) {
        (true, _) => "already up to date".to_string(),
        (false, true) if dry_run => format!("{} would be written", written.join(", ")),
        (false, true) => format!("{} written", written.join(", ")),
        (false, false) => format!(
            "{} written, {} already current",
            written.join(", "),
            current.join(", ")
        ),
    };
    Ok(note("skills", &dir, detail))
}

/// Takes out the `SKILL.md` files happ put there, then the directories they
/// leave behind -- but only while those are empty, so a skill somebody else
/// added under the same root survives happ's uninstall.
fn uninstall_skills(
    spec: &ClientSpec,
    global: bool,
    dry_run: bool,
) -> Result<(String, bool), Error> {
    let Some(scoped) = spec.skills.as_ref() else {
        return Ok((
            format!("  {:<13} skipped -- none were installed", "skills"),
            false,
        ));
    };
    let dir = scoped.resolve(global)?;

    let mut removed = Vec::new();
    for skill in skills::SKILLS {
        let home = dir.join(skill.name);
        let file = home.join("SKILL.md");
        if !file.exists() {
            continue;
        }
        removed.push(skill.name);
        if dry_run {
            continue;
        }
        std::fs::remove_file(&file)
            .map_err(|err| Error::Setup(format!("remove {}: {err}", file.display())))?;
        // `remove_dir` refuses a directory that still holds something, which is
        // exactly the check wanted here -- so its failure is the answer, not an
        // error to report.
        std::fs::remove_dir(&home).ok();
    }
    if !dry_run && !removed.is_empty() {
        // Then the directories the skills sat in: the skills directory itself
        // and the client directory holding it, each only while it is empty.
        // Two levels is as far as happ ever created anything, and stopping
        // there is what keeps an empty project root out of reach.
        let mut candidate = dir.clone();
        for _ in 0..2 {
            if std::fs::remove_dir(&candidate).is_err() {
                break;
            }
            match candidate.parent() {
                Some(parent) => candidate = parent.to_path_buf(),
                None => break,
            }
        }
    }

    let detail = if removed.is_empty() {
        "none installed".to_string()
    } else if dry_run {
        format!("{} would be removed", removed.join(", "))
    } else {
        format!("{} removed", removed.join(", "))
    };
    Ok((
        note("skills", &dir, detail),
        !removed.is_empty() && !dry_run,
    ))
}

fn note(what: &str, path: &Path, detail: String) -> String {
    format!("  {what:<13} {} -- {detail}", path.display())
}

/// Writes `wanted` to `path`, unless it is already exactly that.
///
/// This is the whole of happ's idempotence. Because the comparison is against
/// the finished text, it holds for every format the two commands touch without
/// any of them knowing about it, and it means a re-run cannot reformat a file,
/// bump its mtime, or duplicate a block.
///
/// `wanted` of `None` means there was nothing to do in the first place, and
/// `wanted` of nothing at all means the file should go: a config left holding
/// `{}`, or an instructions file left holding one blank line, is a husk happ
/// created and should take away again.
fn apply(path: &Path, wanted: Option<&str>, dry_run: bool) -> Result<Outcome, Error> {
    let Some(wanted) = wanted else {
        return Ok(Outcome::Nothing);
    };
    if read_config(path)? == wanted {
        return Ok(Outcome::Current);
    }
    if dry_run {
        return Ok(Outcome::Would);
    }
    if wanted.trim().is_empty() {
        std::fs::remove_file(path)
            .map_err(|err| Error::Setup(format!("remove {}: {err}", path.display())))?;
        return Ok(Outcome::Done);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| Error::Setup(format!("create {}: {err}", parent.display())))?;
    }
    std::fs::write(path, wanted)
        .map_err(|err| Error::Setup(format!("write {}: {err}", path.display())))?;
    Ok(Outcome::Done)
}

/// The file as it stands, treating one that is not there as empty: registering
/// on a machine with no config yet has to work, and removing from a file that
/// does not exist is already the outcome asked for.
///
/// Every other read failure is fatal. A file that exists but cannot be read
/// must never be mistaken for an absent one, because both commands write the
/// merged result straight back -- so treating an unreadable file as empty would
/// replace somebody's whole server list with a file holding only happ.
fn read_config(path: &Path) -> Result<String, Error> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(Error::Setup(format!("read {}: {err}", path.display()))),
    }
}

/// The client `id` names, or an error listing the ones happ knows.
fn client(id: &str) -> Result<&'static ClientSpec, Error> {
    CLIENTS
        .iter()
        .find(|candidate| candidate.id == id)
        .ok_or_else(|| {
            Error::Setup(format!(
                "unknown client '{id}' -- happ knows: {}",
                CLIENTS
                    .iter()
                    .map(|client| client.id)
                    .collect::<Vec<&str>>()
                    .join(", ")
            ))
        })
}

/// The command a client should run. An absolute path to this very binary beats
/// the bare name: a client started from a GUI often has a PATH that does not
/// include wherever happ was installed.
fn happ_command() -> Result<String, Error> {
    std::env::current_exe()
        .map(|path| path.display().to_string())
        .or_else(|_| Ok::<String, Error>("happ".to_string()))
}

fn launch_args(args: &crate::cli::McpArgs) -> Vec<String> {
    let mut out = vec!["mcp".to_string(), "--stdio".to_string()];
    if let Some(chart) = args.chart.as_deref() {
        out.push("--chart".to_string());
        out.push(chart.to_string());
    }
    for override_spec in &args.language_servers {
        out.push("--language-server".to_string());
        out.push(override_spec.clone());
    }
    out
}

fn home() -> Result<PathBuf, Error> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| Error::Setup("HOME is not set".to_string()))
}

fn cwd() -> Result<PathBuf, Error> {
    std::env::current_dir().map_err(|err| Error::Setup(format!("resolve current directory: {err}")))
}

fn config_path(spec: &ClientSpec, global: bool) -> Result<PathBuf, Error> {
    Ok(match (spec.id, global) {
        // Claude Code reads a project-scoped .mcp.json, which is the one worth
        // committing; --global falls back to the user-wide config.
        ("claude", false) => cwd()?.join(".mcp.json"),
        ("claude", true) => home()?.join(".claude.json"),
        // Codex has no project scope: it is always the user config.
        ("codex", _) => home()?.join(".codex/config.toml"),
        ("opencode", false) => cwd()?.join("opencode.json"),
        ("opencode", true) => home()?.join(".config/opencode/opencode.json"),
        (other, _) => return Err(Error::Setup(format!("unknown client '{other}'"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_known_client_resolves_all_of_its_paths() {
        for client in CLIENTS {
            for global in [false, true] {
                assert!(
                    config_path(client, global).is_ok(),
                    "no server config path for {}",
                    client.id
                );
                assert!(
                    client.guidance.resolve(global).is_ok(),
                    "no instructions path for {}",
                    client.id
                );
                if let Some(skills) = client.skills.as_ref() {
                    assert!(
                        skills.resolve(global).is_ok(),
                        "no skills path for {}",
                        client.id
                    );
                }
            }
        }
    }

    #[test]
    fn claude_and_codex_do_not_share_an_instructions_file() {
        // Claude Code reads CLAUDE.md and not AGENTS.md, so pointing both at one
        // file would leave one of them with no instructions at all.
        let claude = client("claude").expect("claude");
        let codex = client("codex").expect("codex");
        assert_ne!(claude.guidance.project, codex.guidance.project);
    }

    #[test]
    fn an_unknown_client_names_the_ones_that_exist() {
        let err = client("cursor").err().expect("unknown client");
        let message = err.to_string();
        for known in CLIENTS {
            assert!(
                message.contains(known.id),
                "'{message}' should mention {}",
                known.id
            );
        }
    }

    #[test]
    fn a_missing_file_reads_as_an_empty_one() {
        let tmp = tempfile::tempdir().expect("temp dir");
        assert_eq!(
            read_config(&tmp.path().join("nothing-here.json")).ok(),
            Some(String::new())
        );
    }

    #[test]
    fn a_file_that_cannot_be_read_is_not_mistaken_for_an_empty_one() {
        // A directory standing where the file should be is a read failure that
        // is not NotFound, and unlike a permission bit it behaves the same
        // whichever user runs the tests.
        let tmp = tempfile::tempdir().expect("temp dir");
        let err = read_config(tmp.path())
            .err()
            .expect("a directory is not readable as a config");
        assert!(
            err.to_string().contains("read "),
            "the failure must name the file it could not read: {err}"
        );
    }

    #[test]
    fn writing_the_same_text_twice_touches_the_file_once() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let path = tmp.path().join("nested").join("config.json");

        assert!(apply(&path, Some("hello\n"), false).expect("write") == Outcome::Done);
        let first = std::fs::metadata(&path).expect("stat").modified().ok();

        assert!(
            apply(&path, Some("hello\n"), false).expect("write") == Outcome::Current,
            "a second identical write must be skipped"
        );
        assert_eq!(
            std::fs::metadata(&path).expect("stat").modified().ok(),
            first,
            "the file must not even be touched"
        );

        assert!(apply(&path, Some("goodbye\n"), false).expect("write") == Outcome::Done);
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "goodbye\n",
            "different content must still be written"
        );
    }

    #[test]
    fn a_dry_run_reports_without_writing() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let path = tmp.path().join("config.json");
        assert!(apply(&path, Some("hello\n"), true).expect("dry run") == Outcome::Would);
        assert!(!path.exists(), "a dry run must not create the file");
    }

    #[test]
    fn a_file_left_holding_nothing_is_taken_away() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let path = tmp.path().join("config.json");
        std::fs::write(&path, "{\n  \"mcpServers\": {}\n}\n").expect("seed");

        assert!(apply(&path, Some(""), false).expect("empty") == Outcome::Done);
        assert!(
            !path.exists(),
            "an emptied config is a husk, not a configuration"
        );
        // And asking again is quiet, because absent already reads as empty.
        assert!(apply(&path, Some(""), false).expect("empty") == Outcome::Current);
    }

    #[test]
    fn nothing_to_write_is_not_a_write() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let path = tmp.path().join("config.json");
        assert!(apply(&path, None, false).expect("nothing") == Outcome::Nothing);
        assert!(!path.exists());
    }

    #[test]
    fn launch_args_carry_the_servers_own_options_forward() {
        let args = crate::cli::McpArgs {
            stdio: true,
            chart: Some("./charts/app".to_string()),
            parent_pid: None,
            language_servers: vec!["go=/opt/gopls".to_string()],
            command: None,
        };
        assert_eq!(
            launch_args(&args),
            vec![
                "mcp",
                "--stdio",
                "--chart",
                "./charts/app",
                "--language-server",
                "go=/opt/gopls",
            ]
        );
    }
}
