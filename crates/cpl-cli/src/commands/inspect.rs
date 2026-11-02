//! `cpl inspect <symbol> [--include LIST] [--json] [--since REV]`
//! (`spec/toolchain.md` 7.2, Section 6).
//!
//! Reads the include list strictly (`cpl_index::IncludeSet::parse`: an
//! unknown or empty item, a space, or `interface_dependencies` is
//! `usage_bad_argument`, exit 2; without `--include`, the default set of
//! 6.2), loads and checks the project, builds the semantic index
//! (`cpl_index::Index::from_check`), and answers with
//! `cpl_index::Index::inspect`: the target's symbol block and one list per
//! included item, every version from the one revision the envelope
//! reports. With `changes` included, the base is the facet map of `--since`
//! (default `HEAD~1`, `cpl_index::changes_base`).
//!
//! | Outcome | Code | Exit |
//! | - | - | - |
//! | the target resolves | the response; `index_partial` (warning) when the project does not check without errors | 0 |
//! | it names nothing, or a bare name names several declarations | `index_unknown_symbol`, `index_ambiguous_symbol` | 1 |
//! | a bad `--include` | `usage_bad_argument` | 2 |
//! | `changes` without a repository, without `git`, or with a `--since` that does not resolve or has no project | `git_not_a_repository`, `git_command_failed`, `git_unknown_revision`, `git_no_project_at_revision` | 2 |
//!
//! Human output (stdout) prints the information of the JSON in labelled
//! sections: `revision`; `symbol <path>` with the target's fields and its
//! facet hash table; then `signature`, `contract`, `effects`, `docs`,
//! `implementation`, `holes` (each hole's report: expected type, bindings,
//! capabilities, contract clauses, dependencies), `interface dependencies`,
//! `dependencies`, `callers` (with call sites), `obligations`, `tests`, and
//! `changes since <commit>`, for what the include set has. Each nested
//! symbol carries its own fields and facet hash table. Diagnostics go to
//! stderr.
//!
//! Readings: the target is resolved before the `changes` base is read, so
//! a target that names nothing is reported (exit 1) without consulting
//! git; `--since` without `changes` in the include set has nothing to set
//! and is ignored (the response's `include` shows the effective set);
//! inspect checks in development mode and does not report the check's own
//! diagnostics, which are `cpl check`'s, only `index_partial` (7.2).

use super::Ctx;
use crate::cli::InspectArgs;
use crate::codes;
use crate::commands::diff::statement_line;
use crate::output::{compact, location_text, spelling, Failure, Report};
use crate::project::RepoState;
use cpl_diag::json::{HoleReportJson, InspectOutput, Obligation, Revision, Status, SymbolBlock};
use cpl_diag::{Category, Diagnostic};
use cpl_index::{changes_base, Include, IncludeSet, Index, Target, DEFAULT_SINCE};
use cpl_sema::CheckConfig;
use serde_json::json;
use std::fmt::Write as _;

pub fn run(ctx: &Ctx, args: &InspectArgs) -> Result<Report<InspectOutput>, Failure> {
    let include = match &args.include {
        Some(csv) => IncludeSet::parse(csv).map_err(|message| {
            ctx.session.refuse(
                codes::bad_argument(format!("--include {csv}: {message}"))
                    .with_data("include", json!(csv)),
            )
        })?,
        None => IncludeSet::default_set(),
    };
    let opened = ctx.session.open(false)?;
    let project = &opened.project;
    let sm = project.source_map.clone();
    let revision = opened.revision.clone();
    let checked = cpl_sema::check_with(project, &CheckConfig::default());
    let mut index = Index::from_check(&checked, project, revision.clone());
    let mut diagnostics: Vec<Diagnostic> = index.partial().into_iter().collect();
    let target = Target::parse(&args.symbol);
    let stop = |diagnostics: &[Diagnostic], status| {
        Failure::new(
            status,
            Some(revision.clone()),
            diagnostics,
            Some(sm.clone()),
        )
    };

    if let Err(d) = index.resolve(&target) {
        diagnostics.push(d);
        return Err(stop(&diagnostics, Status::Unsuccessful));
    }
    if include.contains(Include::Changes) {
        let since = args.since.as_deref().unwrap_or(DEFAULT_SINCE);
        let base = match &opened.repo {
            // The index's own `git_not_a_repository` for `changes`.
            RepoState::NoRepository => changes_base(None, since),
            state => state
                .required("`changes`")
                .and_then(|repo| changes_base(Some(repo), since)),
        };
        match base {
            Ok(base) => index = index.with_changes_base(base),
            Err(d) => {
                diagnostics.push(d);
                return Err(stop(&diagnostics, Status::Error));
            }
        }
    }
    let output = match index.inspect(&target, &include) {
        Ok(output) => output,
        Err(d) => {
            // The resolution's codes are answers (exit 1); anything else
            // (git) is an error.
            let status = if d.category == Category::Index {
                Status::Unsuccessful
            } else {
                Status::Error
            };
            diagnostics.push(d);
            return Err(stop(&diagnostics, status));
        }
    };
    let stdout = human(&revision, &output);
    let json = diagnostics.iter().map(|d| d.to_json(&sm)).collect();
    Ok(
        Report::new("inspect", Some(revision.clone()), Status::Ok, json, output)
            .with_stdout(stdout)
            .with_sources(sm.clone()),
    )
}

/// Two spaces per level.
fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

/// One `label value` line at `depth`, the label padded to 12 columns.
fn field(out: &mut String, depth: usize, label: &str, value: impl std::fmt::Display) {
    let _ = writeln!(out, "{}{label:<12}{value}", indent(depth));
}

/// A labelled list at `depth`, its lines one level deeper; `(none)` for an
/// empty one.
fn section(out: &mut String, depth: usize, label: &str, lines: &[String]) {
    let pad = indent(depth);
    let _ = writeln!(out, "{pad}{label}");
    if lines.is_empty() {
        let _ = writeln!(out, "{pad}  (none)");
    }
    for line in lines {
        let _ = writeln!(out, "{pad}  {line}");
    }
}

fn split(text: &str) -> Vec<String> {
    text.lines().map(str::to_string).collect()
}

/// The human form of a response at `revision` (module docs).
pub fn human(revision: &Revision, output: &InspectOutput) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "revision");
    field(
        &mut out,
        1,
        "commit",
        revision.commit.as_deref().unwrap_or("none"),
    );
    field(&mut out, 1, "state", spelling(&revision.state));
    field(&mut out, 1, "digest", &revision.digest);
    let symbol = &output.symbol;
    let _ = writeln!(out, "symbol {}", symbol.version.path);
    fields(&mut out, symbol, 1);
    texts(&mut out, symbol, 0);
    if let Some(list) = &output.interface_dependencies {
        blocks(&mut out, "interface dependencies", list);
    }
    if let Some(list) = &output.dependencies {
        blocks(&mut out, "dependencies", list);
    }
    if let Some(list) = &output.callers {
        blocks(&mut out, "callers", list);
    }
    if let Some(obligations) = &output.obligations {
        obligation_lines(&mut out, obligations);
    }
    if let Some(list) = &output.tests {
        blocks(&mut out, "tests", list);
    }
    if let Some(changes) = &output.changes {
        let since = changes
            .since
            .commit
            .as_deref()
            .unwrap_or("the working tree");
        let lines: Vec<String> = changes.statements.iter().map(statement_line).collect();
        section(&mut out, 0, &format!("changes since {since}"), &lines);
    }
    out
}

/// A block's fields and its facet hash table, at `depth`.
fn fields(out: &mut String, block: &SymbolBlock, depth: usize) {
    let version = &block.version;
    let provisional = if version.provisional {
        "  (provisional)"
    } else {
        ""
    };
    field(out, depth, "id", format!("{}{provisional}", version.id));
    field(out, depth, "kind", &block.kind);
    field(out, depth, "module", &block.module);
    field(out, depth, "public", block.public);
    field(out, depth, "location", location_text(&block.location));
    if let Some(policy) = &block.verification_policy {
        field(out, depth, "prove", policy.prove);
    }
    let pad = indent(depth);
    let _ = writeln!(out, "{pad}facets");
    for (facet, hash) in &version.facets {
        let _ = writeln!(out, "{pad}  {:<21}{hash}", facet.as_str());
    }
    let _ = writeln!(out, "{pad}  {:<21}{}", "definition", version.definition);
}

/// The facet texts a block carries, each a labelled section at `depth`.
fn texts(out: &mut String, block: &SymbolBlock, depth: usize) {
    if let Some(signature) = &block.signature {
        section(out, depth, "signature", &split(signature));
    }
    if let Some(contract) = &block.contract {
        section(out, depth, "contract", contract);
    }
    if let Some(effects) = &block.effects {
        section(out, depth, "effects", effects);
    }
    if let Some(docs) = &block.docs {
        section(
            out,
            depth,
            "docs",
            &docs.as_deref().map(split).unwrap_or_default(),
        );
    }
    if let Some(implementation) = &block.implementation {
        section(out, depth, "implementation", &split(implementation));
    }
    if let Some(holes) = &block.holes {
        let pad = indent(depth);
        let _ = writeln!(out, "{pad}holes");
        if holes.is_empty() {
            let _ = writeln!(out, "{pad}  (none)");
        }
        for hole in holes {
            hole_report(out, hole, depth + 1);
        }
    }
}

/// One hole report (semantics S48) at `depth`: `?name: type`, then its
/// bindings, capabilities, contract clauses, and dependencies.
fn hole_report(out: &mut String, hole: &HoleReportJson, depth: usize) {
    let _ = writeln!(
        out,
        "{}?{}: {}",
        indent(depth),
        hole.hole,
        hole.expected_type
    );
    let inner = depth + 1;
    let bindings: Vec<String> = hole
        .bindings
        .iter()
        .map(|b| {
            let mutable = if b.mutable { ", mutable" } else { "" };
            format!("{}: {}  ({}{mutable})", b.name, b.ty, b.kind.as_str())
        })
        .collect();
    section(out, inner, "bindings", &bindings);
    let mut capabilities = Vec::new();
    for c in &hole.capabilities {
        capabilities.push(format!("{}: {}", c.parameter, c.capability));
        for op in &c.operations {
            let bound = if op.in_bound {
                ""
            } else {
                "  (outside the bound)"
            };
            capabilities.push(format!("  {}{bound}", op.signature));
        }
    }
    section(out, inner, "capabilities", &capabilities);
    section(out, inner, "requires", &hole.requires);
    section(out, inner, "ensures", &hole.ensures);
    section(out, inner, "invariants", &hole.invariants);
    let deps = &hole.dependencies;
    let pad = indent(inner);
    let _ = writeln!(out, "{pad}dependencies");
    let dirty = if deps.dirty == Some(true) {
        "  (dirty)"
    } else {
        ""
    };
    field(
        out,
        inner + 1,
        "source",
        format!("{}{dirty}", deps.source_revision),
    );
    field(out, inner + 1, "std", &deps.toolchain.std);
    let modules: Vec<String> = deps
        .modules
        .iter()
        .map(|m| format!("{}  {}", m.module, m.content_hash))
        .collect();
    section(out, inner + 1, "modules", &modules);
    let symbols: Vec<String> = deps
        .symbols
        .iter()
        .map(|s| format!("{}  {}", s.path, s.definition))
        .collect();
    section(out, inner + 1, "symbols", &symbols);
}

/// A list of symbol blocks under its label: each block's heading (its
/// path, with its relation), fields, facet texts, and call sites.
fn blocks(out: &mut String, label: &str, list: &[SymbolBlock]) {
    let _ = writeln!(out, "{label}");
    if list.is_empty() {
        let _ = writeln!(out, "  (none)");
    }
    for block in list {
        let mut heading = block.version.path.clone();
        let mut tags: Vec<String> = block.relation.iter().map(spelling).collect();
        match block.interface {
            Some(true) => tags.push("interface".to_string()),
            Some(false) => tags.push("body".to_string()),
            None => {}
        }
        if !tags.is_empty() {
            let _ = write!(heading, "  ({})", tags.join(", "));
        }
        let _ = writeln!(out, "  {heading}");
        fields(out, block, 2);
        texts(out, block, 2);
        if let Some(sites) = &block.call_sites {
            let sites: Vec<String> = sites.iter().map(location_text).collect();
            section(out, 2, "call sites", &sites);
        }
    }
}

/// `obligations`: one entry per obligation, `<id>  <kind>  <status>`
/// (with the reason of an `unknown` one), then its clause, location,
/// evidence, and assumptions.
fn obligation_lines(out: &mut String, obligations: &[Obligation]) {
    let _ = writeln!(out, "obligations");
    if obligations.is_empty() {
        let _ = writeln!(out, "  (none)");
    }
    for o in obligations {
        let status = spelling(&o.status);
        let status = match &o.reason {
            Some(reason) => format!("{status} ({reason})"),
            None => status,
        };
        let _ = writeln!(out, "  {}  {}  {status}", o.id, o.kind);
        if !o.clause.is_empty() {
            let _ = writeln!(out, "    {:<10}{}", "clause", o.clause);
        }
        let _ = writeln!(out, "    {:<10}{}", "location", location_text(&o.location));
        if let Some(evidence) = &o.evidence {
            let _ = writeln!(out, "    {:<10}{}", "evidence", compact(evidence));
        }
        if !o.assumptions.is_empty() {
            let _ = writeln!(out, "    {:<10}{}", "assumes", o.assumptions.join(", "));
        }
    }
}
