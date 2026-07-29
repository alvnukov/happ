//! The block happ keeps in the file its client reads for instructions.
//!
//! Registering the server only makes the tools *available*. What decides
//! whether a model reaches for them, rather than falling back to grep, is
//! whether the project's instruction file says they are worth reaching for.
//! That file is `CLAUDE.md` for Claude Code and `AGENTS.md` for Codex and
//! OpenCode; the caller picks, this module only edits.
//!
//! The block is delimited by HTML comments, which is what makes it removable:
//! happ can find its own section in a file a human otherwise owns, and replace
//! or excise exactly that. Claude Code strips block-level HTML comments before
//! the file enters the model's context, so the markers themselves cost nothing
//! to keep.

use crate::mcp::Error;

const START: &str = "<!-- happ:start -->";
const END: &str = "<!-- happ:end -->";

/// What happ asks an agent to know about itself.
///
/// Deliberately short. This text is loaded in full at the start of every
/// session, so it earns its place only by answering the question a model
/// actually has -- *should I use this instead of what I would have done?* --
/// rather than by restating the tool schemas, which the client already has.
const BODY: &str = "\
## happ

This project is served by the `happ` MCP server. Two tools, each taking an `op`.

`code` -- source questions answered through a real language server (Rust, Go,
TypeScript, Python, C/C++). Ops: `definition`, `references`, `hover`, `calls`,
`symbols`, `diagnostics`, `languages`.

Prefer it over a text search for *who calls this*, *where does this come from*
and *what type is this*. A search matches names; `code` resolves them, so it
tells apart a shadowed local from the import it shadows, and finds a caller that
spells the name differently. Address a symbol by name -- it only has to appear
in the file, so asking about a function that is called there but declared
elsewhere is the normal way to find where it lives.

Before changing a function, run `code` with `op='calls'` on it and read the
callers. Before finishing, run `op='diagnostics'` on the files you touched.

`helm_apps` -- charts built on the helm-apps library chart. Such a chart has no
per-app templates: every app is a values entry under an `apps-*` group, and the
library renders it. Reading `values.yaml` directly is misleading, because
`_include` profiles, `_includeFile` references and env maps all resolve at
render time. Start at `op='overview'`; use `op='resolve'` for what a value
actually becomes and `op='render'` for the manifest.";

/// The block as it should appear, markers included.
pub(super) fn block() -> String {
    format!("{START}\n{BODY}\n{END}\n")
}

/// `existing` with happ's block present and current.
///
/// An existing block is replaced where it stands rather than moved to the end,
/// so upgrading happ does not shuffle a file somebody else maintains.
pub(super) fn with_block(existing: &str) -> Result<String, Error> {
    let wanted = block();
    let Some((start, end)) = span(existing)? else {
        if existing.trim().is_empty() {
            return Ok(wanted);
        }
        return Ok(format!("{}\n\n{wanted}", existing.trim_end()));
    };
    Ok(format!(
        "{}{wanted}{}",
        &existing[..start],
        &existing[end..]
    ))
}

/// `existing` without happ's block, or `None` when there was no block to take
/// out -- the caller then leaves the file entirely alone.
pub(super) fn without_block(existing: &str) -> Result<Option<String>, Error> {
    let Some((start, end)) = span(existing)? else {
        return Ok(None);
    };

    let before = existing[..start].trim_end();
    let after = existing[end..].trim_start_matches('\n');
    Ok(Some(match (before.is_empty(), after.is_empty()) {
        (true, true) => String::new(),
        (true, false) => after.to_string(),
        (false, true) => format!("{before}\n"),
        (false, false) => format!("{before}\n\n{after}"),
    }))
}

/// Where happ's block sits, as a byte range covering the markers and the
/// newline after the closing one.
///
/// A file with one marker and not the other has been edited by hand into a
/// state where any guess about the intended boundary could destroy text. That
/// is refused rather than guessed at.
fn span(text: &str) -> Result<Option<(usize, usize)>, Error> {
    match (text.find(START), text.find(END)) {
        (None, None) => Ok(None),
        (Some(start), Some(end)) if start < end => {
            let end = end + END.len();
            let end = end + usize::from(text[end..].starts_with('\n'));
            Ok(Some((start, end)))
        }
        _ => Err(Error::Setup(format!(
            "the instructions file has a damaged happ block: it must contain \
             '{START}' followed by '{END}', or neither -- fix it by hand and run this again"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_becomes_just_the_block() {
        assert_eq!(with_block("").expect("insert"), block());
    }

    #[test]
    fn an_existing_file_keeps_its_own_text_above_the_block() {
        let written = with_block("# My rules\n\n1. Be brief.\n").expect("insert");
        assert!(written.starts_with("# My rules\n\n1. Be brief.\n\n"));
        assert!(written.ends_with(&block()));
    }

    #[test]
    fn installing_twice_is_a_fixed_point() {
        let once = with_block("# My rules\n").expect("insert");
        let twice = with_block(&once).expect("insert");
        assert_eq!(once, twice, "a second setup must change nothing");
    }

    #[test]
    fn an_out_of_date_block_is_replaced_where_it_stands() {
        let stale = format!("# Top\n\n{START}\nold happ text\n{END}\n\n# Bottom\n");
        let written = with_block(&stale).expect("insert");
        assert!(!written.contains("old happ text"));
        assert!(
            written.find("# Top").expect("top") < written.find(START).expect("block"),
            "the block must not migrate to the end of the file"
        );
        assert!(
            written.find(END).expect("block") < written.find("# Bottom").expect("bottom"),
            "text below the block must stay below it"
        );
        assert_eq!(written.matches(START).count(), 1, "no duplicate block");
    }

    #[test]
    fn removing_leaves_the_surrounding_text_alone() {
        let installed = with_block("# My rules\n\n1. Be brief.\n").expect("insert");
        let removed = without_block(&installed)
            .expect("remove")
            .expect("a block was there");
        assert_eq!(removed, "# My rules\n\n1. Be brief.\n");
    }

    #[test]
    fn removing_keeps_text_that_came_after_the_block() {
        let text = format!("# Top\n\n{START}\nhapp\n{END}\n\n# Bottom\n");
        let removed = without_block(&text)
            .expect("remove")
            .expect("a block was there");
        assert_eq!(removed, "# Top\n\n# Bottom\n");
    }

    #[test]
    fn a_file_happ_never_touched_is_left_untouched() {
        assert!(without_block("# My rules\n").expect("remove").is_none());
        assert!(without_block("").expect("remove").is_none());
    }

    #[test]
    fn a_file_that_was_only_the_block_ends_up_empty() {
        let removed = without_block(&block())
            .expect("remove")
            .expect("a block was there");
        assert_eq!(removed, "");
    }

    #[test]
    fn a_half_deleted_block_is_refused_rather_than_guessed_at() {
        // Either marker alone leaves no honest way to tell happ's text from the
        // author's, so both commands stop instead of eating somebody's file.
        assert!(with_block(&format!("# Top\n{START}\nhapp\n")).is_err());
        assert!(with_block(&format!("# Top\nhapp\n{END}\n")).is_err());
        assert!(without_block(&format!("{END}\n{START}\n")).is_err());
    }
}
