//! `cpl diff <range> [--semantic] [--exit-code] [--json]`
//! (`spec/toolchain.md` 7.2, Section 9; T31, T32).
//!
//! The semantic diff between two revisions of the project
//! (`cpl_index::semantic_diff` of their facet maps). `--semantic` is the
//! design's spelling of the only mode there is.
//!
//! | `<range>` | Old revision | New revision |
//! | - | - | - |
//! | `A...B` | `git merge-base A B` | `B` |
//! | `A..B` | `A` | `B` |
//! | `A` | `A` | the working tree |
//!
//! The range splits at its first `...`, else at its first `..` (no ref name
//! contains `..`, `git check-ref-format`). Each named revision is resolved
//! with `cpl_index::resolve_rev`, the names in the order written, before any
//! tree is read; a commit's project tree is materialized with
//! `cpl_index::tree_at` and loaded from memory (Section 9.3,
//! [`crate::project::load_revision`]) at the revision `{ commit, state:
//! clean, digest }`; the working tree is the located project at its
//! current revision (Section 6.3).
//!
//! | Outcome | Code | Exit |
//! | - | - | - |
//! | the diff was computed | the `DiffOutput` of 9.2 | 0, or 1 under `--exit-code` when there is a statement |
//! | a name that does not resolve, is empty, or starts with `-` | `git_unknown_revision` (`revision`) | 2 |
//! | two commits without a common ancestor (`A...B`), or another git failure | `git_command_failed` | 2 |
//! | a commit without `cpl.toml` at the project's path | `git_no_project_at_revision` (`revision`, `commit`) | 2 |
//! | a commit whose `cpl.toml` or `cpl.lock` does not load | `project_manifest_invalid`, `project_lock_invalid` (with `revision`, `commit`) | 2 |
//! | no repository, or no `git` | `git_not_a_repository`, `git_command_failed` | 2 |
//!
//! Human output (stdout): one statement per line, `<statement>  <path>
//! (<id>)`, in the order of 9.2 (path, then the statement order of 9.1),
//! with the detail of a rename (`was <old path>`) or a move (`from <old
//! module>`), the effects an effect statement adds or removes, and `kind
//! changed` on the two statements of a kind change (with `, was <old
//! path>` on its removal when the declaration was renamed too: both
//! statements are at the new path, Section 9.1); then one note per id
//! declared more than once in either revision. An empty diff prints
//! nothing, as `git diff` does.
//!
//! Reading: the envelope's `revision` is the working tree's, as for every
//! command run in a project (Section 6.3); the two sides are `old` and
//! `new`.

use super::Ctx;
use crate::cli::DiffArgs;
use crate::output::{spelling, Failure, Report};
use crate::project::load_revision;
use cpl_diag::json::{
    DiffOutput, DiffStatement, DiffStatementKind, Revision, RevisionState, Status,
};
use cpl_diag::Diagnostic;
use cpl_index::{facet_map, git_error_diagnostic, FacetMap, GitRepo};
use serde_json::Value;
use std::fmt::Write as _;

/// The two sides a range names (Section 7.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Range<'a> {
    /// `A...B`: from the merge base of `A` and `B` to `B`.
    Symmetric(&'a str, &'a str),
    /// `A..B`: from `A` to `B`.
    Between(&'a str, &'a str),
    /// `A`: from `A` to the working tree.
    Since(&'a str),
}

impl<'a> Range<'a> {
    /// Split a range at its first `...`, else at its first `..`. The names
    /// are kept as written (an empty one is `git_unknown_revision` when it
    /// is resolved, as Section 7.2 rules for every empty name).
    pub fn parse(range: &'a str) -> Range<'a> {
        if let Some((a, b)) = range.split_once("...") {
            Range::Symmetric(a, b)
        } else if let Some((a, b)) = range.split_once("..") {
            Range::Between(a, b)
        } else {
            Range::Since(range)
        }
    }
}

pub fn run(ctx: &Ctx, args: &DiffArgs) -> Result<Report<DiffOutput>, Failure> {
    let opened = ctx.session.open(false)?;
    let revision = Some(opened.revision.clone());
    let fail = |d: Diagnostic| Failure::new(Status::Error, revision.clone(), &[d], None);
    let repo = opened.repo.required("`cpl diff`").map_err(fail)?;
    let resolve =
        |name: &str| cpl_index::resolve_rev(repo, name).map_err(|e| git_error_diagnostic(&e));
    let (old, new) = match Range::parse(&args.range) {
        Range::Symmetric(a, b) => {
            let base =
                cpl_index::merge_base(repo, a, b).map_err(|e| fail(git_error_diagnostic(&e)))?;
            let b_commit = resolve(b).map_err(fail)?;
            let old = revision_map(repo, &base, &base).map_err(fail)?;
            let new = revision_map(repo, b, &b_commit).map_err(fail)?;
            (old, new)
        }
        Range::Between(a, b) => {
            let a_commit = resolve(a).map_err(fail)?;
            let b_commit = resolve(b).map_err(fail)?;
            let old = revision_map(repo, a, &a_commit).map_err(fail)?;
            let new = revision_map(repo, b, &b_commit).map_err(fail)?;
            (old, new)
        }
        Range::Since(a) => {
            let a_commit = resolve(a).map_err(fail)?;
            let old = revision_map(repo, a, &a_commit).map_err(fail)?;
            let new = facet_map(&opened.project, opened.revision.clone());
            (old, new)
        }
    };
    let output = cpl_index::semantic_diff(&old, &new);
    let status = if args.exit_code && !output.statements.is_empty() {
        Status::Unsuccessful
    } else {
        Status::Ok
    };
    let stdout = human(&output);
    Ok(Report::new("diff", revision, status, Vec::new(), output).with_stdout(stdout))
}

/// The facet map of the project at `commit` (Section 9.3), which the range
/// named `name`.
fn revision_map(repo: &GitRepo, name: &str, commit: &str) -> Result<FacetMap, Diagnostic> {
    let project = load_revision(repo, name, commit, false)?;
    let revision = Revision {
        commit: Some(commit.to_string()),
        state: RevisionState::Clean,
        digest: cpl_index::source_digest(&project),
    };
    Ok(facet_map(&project, revision))
}

/// The human form of a diff (module docs).
fn human(output: &DiffOutput) -> String {
    let mut out = String::new();
    for statement in &output.statements {
        let _ = writeln!(out, "{}", statement_line(statement));
    }
    for (side, duplicates) in [
        ("old", &output.duplicates.old),
        ("new", &output.duplicates.new),
    ] {
        for duplicate in duplicates {
            let _ = writeln!(
                out,
                "note: {} is declared more than once in the {side} revision ({}); its statements describe the first",
                duplicate.id,
                duplicate.files.join(", ")
            );
        }
    }
    out
}

/// One statement in human form: `<statement>  <path>  (<id>)` with the
/// detail of a rename, a move, an effect statement, or a kind change.
pub fn statement_line(statement: &DiffStatement) -> String {
    let mut line = format!(
        "{}  {}  ({})",
        spelling(&statement.statement),
        statement.path,
        statement.id
    );
    let detail = &statement.detail;
    let text = |key: &str| detail.get(key).and_then(Value::as_str);
    let list = |key: &str| {
        detail
            .get(key)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    };
    match statement.statement {
        DiffStatementKind::IdentityRenamed => {
            if let Some(from) = text("from") {
                let _ = write!(line, "  was {from}");
            }
        }
        DiffStatementKind::Moved => {
            if let Some(from) = text("from_module") {
                let _ = write!(line, "  from {from}");
            }
        }
        DiffStatementKind::EffectBoundWidened => {
            let _ = write!(line, "  added {}", list("added"));
        }
        DiffStatementKind::EffectBoundNarrowed => {
            let _ = write!(line, "  removed {}", list("removed"));
        }
        DiffStatementKind::IdentityAdded | DiffStatementKind::IdentityRemoved
            if text("note") == Some("kind_changed") =>
        {
            line.push_str("  kind changed");
            // The removed side keeps its old version: a rename's detail.
            let was = detail.pointer("/version/path").and_then(Value::as_str);
            if let Some(old) = was.filter(|old| *old != statement.path) {
                let _ = write!(line, ", was {old}");
            }
        }
        _ => {}
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpl_diag::{Facet, StableId};
    use serde_json::json;

    #[test]
    fn ranges_split_at_the_first_separator() {
        assert_eq!(
            Range::parse("main...HEAD"),
            Range::Symmetric("main", "HEAD")
        );
        assert_eq!(Range::parse("a..b"), Range::Between("a", "b"));
        assert_eq!(Range::parse("HEAD~1"), Range::Since("HEAD~1"));
        assert_eq!(Range::parse("v1.0..v1.1"), Range::Between("v1.0", "v1.1"));
        assert_eq!(Range::parse("a....b"), Range::Symmetric("a", ".b"));
        assert_eq!(Range::parse("main.."), Range::Between("main", ""));
        assert_eq!(Range::parse("...b"), Range::Symmetric("", "b"));
        assert_eq!(Range::parse(""), Range::Since(""));
    }

    fn statement(kind: DiffStatementKind, detail: Value) -> DiffStatement {
        DiffStatement {
            statement: kind,
            id: StableId::parse("pgn_7c2d").expect("an id"),
            path: "reader.pages.paginate".into(),
            kind: "fn".into(),
            facet: Some(Facet::Contract),
            detail,
        }
    }

    #[test]
    fn statement_lines() {
        assert_eq!(
            statement_line(&statement(
                DiffStatementKind::ContractChanged,
                json!({ "old": "a", "new": "b" })
            )),
            "contract_changed  reader.pages.paginate  (pgn_7c2d)"
        );
        assert_eq!(
            statement_line(&statement(
                DiffStatementKind::IdentityRenamed,
                json!({ "from": "reader.pages.pages_of", "to": "reader.pages.paginate" })
            )),
            "identity_renamed  reader.pages.paginate  (pgn_7c2d)  was reader.pages.pages_of"
        );
        assert_eq!(
            statement_line(&statement(
                DiffStatementKind::Moved,
                json!({ "from_module": "reader.old", "to_module": "reader.pages" })
            )),
            "moved  reader.pages.paginate  (pgn_7c2d)  from reader.old"
        );
        assert_eq!(
            statement_line(&statement(
                DiffStatementKind::EffectBoundWidened,
                json!({ "added": ["db.get", "db.put"] })
            )),
            "effect_bound_widened  reader.pages.paginate  (pgn_7c2d)  added db.get, db.put"
        );
        assert_eq!(
            statement_line(&statement(
                DiffStatementKind::IdentityAdded,
                json!({ "version": {}, "note": "kind_changed" })
            )),
            "identity_added  reader.pages.paginate  (pgn_7c2d)  kind changed"
        );
        // A kind change that renamed: the removal is at the new path and
        // names the old one.
        assert_eq!(
            statement_line(&statement(
                DiffStatementKind::IdentityRemoved,
                json!({ "version": { "path": "reader.pages.pages_of" }, "note": "kind_changed" })
            )),
            "identity_removed  reader.pages.paginate  (pgn_7c2d)  kind changed, was reader.pages.pages_of"
        );
        assert_eq!(
            statement_line(&statement(
                DiffStatementKind::IdentityRemoved,
                json!({ "version": { "path": "reader.pages.paginate" }, "note": "kind_changed" })
            )),
            "identity_removed  reader.pages.paginate  (pgn_7c2d)  kind changed"
        );
    }
}
