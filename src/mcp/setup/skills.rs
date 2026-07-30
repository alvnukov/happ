//! The skills happ installs alongside its server.
//!
//! A skill is a `SKILL.md` under a directory named for it, in the [Agent
//! Skills](https://agentskills.io) format that Claude Code and OpenCode both
//! read. It differs from the instructions block in when it is paid for: the
//! block is loaded into every session, so it has to stay short, while a skill's
//! body loads only once the client decides it is relevant. That is where the
//! step-by-step workflows belong.
//!
//! The two skills split along the two tools, because that is how the questions
//! arrive: a chart question and a source question have nothing to say to each
//! other, and a client picks between them on `description` alone.

/// One skill, as the file it becomes.
pub(super) struct Skill {
    /// Directory name, which is also what the client uses to invoke it, and
    /// which the frontmatter `name` has to repeat.
    pub(super) name: &'static str,
    pub(super) body: &'static str,
}

pub(super) const SKILLS: &[Skill] = &[
    Skill {
        name: "happ-charts",
        body: CHARTS,
    },
    Skill {
        name: "happ-code",
        body: CODE,
    },
];

const CHARTS: &str = r#"---
name: happ-charts
description: Read, resolve, render and diff a Helm chart built on the helm-apps library chart, where apps are values entries under apps-* groups rather than templates. Use when a chart has no per-app templates of its own, when values.yaml alone does not explain what will be deployed, or when a question involves _include profiles, _includeFile references, env maps or per-environment overrides.
---

# helm-apps charts

A chart built on the helm-apps library has no per-app templates. Every app is a
values entry under an `apps-*` group -- `apps-stateless.api`, `apps-stateful.db`
-- and the library chart renders it.

This is why reading `values.yaml` and answering from it is wrong here. Profiles
pulled in by `_include`, files pulled in by `_includeFile`, env maps and
per-environment overrides all resolve at render time, so the file shows
fragments of an answer rather than the answer.

## Order of work

1. `helm_apps op='overview'` -- what the chart contains: groups, apps,
   environments. Start here even when the question sounds specific; it is the
   cheapest way to learn which names are real.
2. `helm_apps op='apps'` -- the apps in one group, when the overview is large.
3. `helm_apps op='resolve'` -- one app's values after everything has resolved.
   This is the answer to "what is actually set", and it is not the file.
4. `helm_apps op='render'` -- one app's Kubernetes manifests. Use when the
   question is about the object rather than the values that produced it.
5. `helm_apps op='query_manifests'` -- jq over manifests from every enabled app.
   Use `kind` or `resource` to discard unrelated objects before the query runs.
6. `helm_apps op='diff'` with `from_env` and `to_env` -- what differs between two
   environments. Better than resolving both and comparing by eye.

## The rest of the surface

- `op='query'` runs a jq expression over the resolved values, for questions that
  are a filter rather than a lookup -- which apps are enabled, which set a
  particular key.
- `op='lint'` reports what the library chart would reject, before a render does.
- `op='contract'` and `op='template'` return the library's own template source.
  Reach for them when the question is what the library *guarantees*, and answer
  from that source rather than from inference about it.

The `happ://helm-apps/...` resources carry the same source if you would rather
read it directly.
"#;

const CODE: &str = r#"---
name: happ-code
description: Answer questions about source code through a real language server rather than text search - where a symbol is defined, everything that references it, its type and documentation, its callers and callees, and the compiler diagnostics for a file. Use for Rust, Go, TypeScript, Python and C/C++ whenever the question is who calls this, where does this come from, what type is this, or did my edit compile.
---

# Code navigation

`code` answers through the language server for the file's language, started by
happ and kept warm. That is the difference from a text search: a search matches
a name, while the language server resolves it. It tells a shadowed local apart
from the import it shadows, follows a re-export to what it re-exports, and finds
a caller that reaches the symbol under a different spelling.

Address a symbol by `symbol` name rather than by position. The name only has to
*appear* in `file` -- asking about a function that is called there but declared
elsewhere is the normal way to find out where it comes from, and the answer says
which position it resolved from.

## The questions and the ops

| Question | Call |
|---|---|
| Where does this come from? | `op='definition'` with `file` + `symbol` |
| What would I break by changing it? | `op='calls'` -- callers, or callees with `direction='outgoing'` |
| Everywhere it is used, not just calls | `op='references'` |
| What is its type, what does it document? | `op='hover'` |
| What is in this file? | `op='symbols'` with `file` |
| Where in the project is this name? | `op='symbols'` with `query`, no `file` needed |
| Did my edit compile? | `op='diagnostics'` with `file` |

## Working on a change

Before editing a function, run `op='calls'` on it and read the callers. Report
what depends on it before changing its signature or its contract, and say so
when the list is long enough that the change is no longer local.

After editing, run `op='diagnostics'` on each file touched. It is the language
server's own answer and it arrives without a build, so there is no reason to
guess at whether an edit type-checks.

`op='languages'` reports which servers are installed and running, and what to
install when one is missing.
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// The format is shared, but the two clients that read it enforce slightly
    /// different things. These are the tighter bounds of the two, so a skill
    /// that passes here loads in both.
    #[test]
    fn every_skill_satisfies_the_agent_skills_frontmatter_contract() {
        for skill in SKILLS {
            assert!(
                (1..=64).contains(&skill.name.len())
                    && skill
                        .name
                        .chars()
                        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
                    && !skill.name.starts_with('-')
                    && !skill.name.ends_with('-')
                    && !skill.name.contains("--"),
                "'{}' is not a valid skill name",
                skill.name
            );

            let front = skill
                .body
                .strip_prefix("---\n")
                .and_then(|rest| rest.split_once("\n---\n"))
                .map(|(front, _)| front)
                .unwrap_or_else(|| panic!("{} has no frontmatter", skill.name));

            let named = front
                .lines()
                .find_map(|line| line.strip_prefix("name: "))
                .unwrap_or_else(|| panic!("{} declares no name", skill.name));
            assert_eq!(
                named, skill.name,
                "the frontmatter name must repeat the directory name"
            );

            let description = front
                .lines()
                .find_map(|line| line.strip_prefix("description: "))
                .unwrap_or_else(|| panic!("{} declares no description", skill.name));
            assert!(
                (1..=1024).contains(&description.len()),
                "{}: a description is capped at 1024 characters, this one is {}",
                skill.name,
                description.len()
            );
        }
    }

    #[test]
    fn the_skills_have_distinct_names() {
        for (index, skill) in SKILLS.iter().enumerate() {
            assert!(
                !SKILLS[..index].iter().any(|other| other.name == skill.name),
                "two skills both called {}",
                skill.name
            );
        }
    }

    #[test]
    fn no_skill_names_an_op_the_tools_do_not_have() {
        // A skill that promises an op that does not exist sends the model into
        // an error it cannot recover from, so the two lists are pinned together.
        let charts = [
            "overview",
            "apps",
            "resolve",
            "render",
            "query_manifests",
            "lint",
            "diff",
            "query",
            "contract",
            "template",
        ];
        let code = [
            "languages",
            "diagnostics",
            "definition",
            "references",
            "hover",
            "symbols",
            "calls",
        ];
        for skill in SKILLS {
            let known: &[&str] = if skill.name == "happ-charts" {
                &charts
            } else {
                &code
            };
            for (offset, _) in skill.body.match_indices("op='") {
                let rest = &skill.body[offset + "op='".len()..];
                let op = rest.split('\'').next().unwrap_or_default();
                assert!(
                    known.contains(&op),
                    "{} mentions op='{op}', which is not one of {known:?}",
                    skill.name
                );
            }
        }
    }
}
