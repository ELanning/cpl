use super::Ctx;
use crate::{
    cli::*,
    output::{Failure, Report},
    project::load_options,
};
use cpl_diag::{json::Status, Diagnostic};
use cpl_evolve::{
    change::{Acknowledgement, Change, ChangeContract, Edit, Operation, Policy},
    verify::{Evidence, VerifyConfig},
    Snapshot,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path, sync::Arc};

fn failure(ctx: &Ctx, d: Diagnostic) -> Failure {
    Failure::new(Status::Unsuccessful, ctx.session.revision(), &[d], None)
}
fn current(ctx: &Ctx) -> Result<Snapshot, Failure> {
    Snapshot::read(&ctx.session.locate()?, &load_options(false)).map_err(|d| failure(ctx, d))
}
fn historical(ctx: &Ctx, revision: &str) -> Result<Snapshot, Failure> {
    if revision == "worktree" {
        return current(ctx);
    }
    let root = ctx.session.locate()?;
    let repo = crate::project::discover(&root)?;
    let repo = repo
        .required("evolution history")
        .map_err(|d| failure(ctx, d))?;
    let commit = cpl_index::resolve_rev(repo, revision)
        .map_err(|e| failure(ctx, cpl_index::git_error_diagnostic(&e)))?;
    let tree = cpl_index::tree_at(repo, &commit)
        .map_err(|e| failure(ctx, cpl_index::git_error_diagnostic(&e)))?;
    Snapshot::new(tree, &load_options(false)).map_err(|d| failure(ctx, d))
}
fn read<T: DeserializeOwned>(ctx: &Ctx, path: &Path) -> Result<T, Failure> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        ctx.session.refuse(crate::codes::bad_argument(format!(
            "{}: {e}",
            path.display()
        )))
    })?;
    let value = crate::json_arg::read(&text).map_err(|e| {
        ctx.session
            .refuse(crate::codes::bad_argument(e.to_string()))
    })?;
    serde_json::from_value(value).map_err(|e| {
        ctx.session.refuse(crate::codes::bad_argument(format!(
            "{}: {e}",
            path.display()
        )))
    })
}
fn literal(ctx: &Ctx, text: &str) -> Result<Value, Failure> {
    crate::json_arg::read(text).map_err(|e| {
        ctx.session
            .refuse(crate::codes::bad_argument(e.to_string()))
    })
}
fn report(ctx: &Ctx, command: &str, value: Value, ok: bool) -> Result<Report<Value>, Failure> {
    let stdout = format!("{}\n", crate::output::pretty(&value));
    Ok(Report::new(
        command,
        ctx.session.revision(),
        if ok { Status::Ok } else { Status::Unsuccessful },
        vec![],
        value,
    )
    .with_stdout(stdout))
}
/// Write an output artifact, refusing the files of `source` (the snapshot the command loaded)
/// and the authority policy.
fn save(
    ctx: &Ctx,
    source: &Snapshot,
    path: &Path,
    value: &impl serde::Serialize,
) -> Result<(), Failure> {
    let absolute = crate::project::absolute(&ctx.session.cwd, path);
    let root = ctx.session.locate()?;
    if source
        .tree
        .files
        .keys()
        .any(|f| crate::project::real_path(&root.join(f)) == crate::project::real_path(&absolute))
        || crate::project::real_path(&absolute)
            == crate::project::real_path(&root.join(".cpl/authority.json"))
    {
        return Err(ctx.session.refuse(crate::codes::bad_argument(
            "an output artifact cannot overwrite project inputs or authority policy",
        )));
    }
    cpl_evolve::storage::atomic_write(&absolute, &serde_json::to_vec_pretty(value).unwrap())
        .map_err(|d| failure(ctx, d))
}
fn policy(ctx: &Ctx, path: Option<&Path>) -> Result<Option<Policy>, Failure> {
    let default = ctx.session.locate()?.join(".cpl/authority.json");
    let path = path.or_else(|| default.is_file().then_some(default.as_path()));
    path.map(|p| read(ctx, p)).transpose()
}

pub fn query(ctx: &Ctx, args: &QueryArgs) -> Result<Report<Value>, Failure> {
    let s = current(ctx)?;
    if let Some(actor) = args.expression.trim().strip_prefix("writable by ") {
        let authority = policy(ctx, args.policy.as_deref())?.ok_or_else(|| {
            failure(
                ctx,
                cpl_evolve::error(
                    "query_policy_required",
                    "writable by ACTOR needs an authority policy (--policy or .cpl/authority.json)",
                ),
            )
        })?;
        let value = cpl_evolve::change::permissions(&s, &authority, actor.trim());
        return report(ctx, "query", json!({"permissions": value}), true);
    }
    let base = args
        .since
        .as_deref()
        .map(|r| historical(ctx, r))
        .transpose()?;
    report(
        ctx,
        "query",
        cpl_evolve::query::query(&s, &args.expression, base.as_ref())
            .map_err(|d| failure(ctx, d))?,
        true,
    )
}
pub fn verify(ctx: &Ctx, args: &VerifyArgs) -> Result<Report<Value>, Failure> {
    let s = current(ctx)?;
    let base = args
        .changed
        .then(|| historical(ctx, &args.since))
        .transpose()?;
    let config = VerifyConfig {
        seed: args.seed,
        cases: args.cases,
        timeout_ms: args.timeout_ms,
        require_proofs: args.require_proofs,
        require_migrations: !args.allow_schema_changes,
        ..VerifyConfig::default()
    };
    let evidence = if let Some(path) = &args.evidence {
        let old: Evidence = read(ctx, path)?;
        cpl_evolve::verify::recheck(&s, base.as_ref(), &old)
    } else {
        cpl_evolve::verify::verify(&s, base.as_ref(), &config)
    }
    .map_err(|d| failure(ctx, d))?;
    if let Some(path) = &args.receipt {
        save(ctx, &s, path, &evidence)?;
    }
    report(
        ctx,
        "verify",
        json!({"verification":evidence}),
        evidence.accepted,
    )
}
pub fn edit(ctx: &Ctx, args: &EditArgs) -> Result<Report<Value>, Failure> {
    let (symbol, flags, operation) = match &args.command {
        EditCommand::ReplaceBody(a) | EditCommand::ReplaceDeclaration(a) => {
            let source = std::fs::read_to_string(&a.input).map_err(|e| {
                ctx.session
                    .refuse(crate::codes::bad_argument(e.to_string()))
            })?;
            let op = if matches!(&args.command, EditCommand::ReplaceBody(_)) {
                Operation::ReplaceBody { body: source }
            } else {
                Operation::ReplaceDeclaration { source }
            };
            (&a.symbol, &a.flags, op)
        }
        EditCommand::Rename(a) => (
            &a.symbol,
            &a.flags,
            Operation::Rename {
                name: a.name.clone(),
            },
        ),
        EditCommand::Remove(a) => (&a.symbol, &a.flags, Operation::RemoveDeclaration),
        EditCommand::FillHole(a) => (
            &a.symbol,
            &a.flags,
            Operation::FillHole {
                hole: a.hole.clone(),
                expression: a.expression.clone(),
            },
        ),
    };
    let s = current(ctx)?;
    let id = s.resolve(symbol).map_err(|d| failure(ctx, d))?;
    let index = s.index();
    let version = index.version(id);
    let required =
        cpl_evolve::change::operation_facets(&s, id, &operation).map_err(|d| failure(ctx, d))?;
    let expect = required
        .into_iter()
        .map(|f| {
            version
                .facets
                .get(&f)
                .map(|h| (f, h.to_string()))
                .ok_or_else(|| {
                    ctx.session.refuse(crate::codes::bad_argument(
                        "target does not have the requested facet",
                    ))
                })
        })
        .collect::<Result<_, _>>()?;
    let edit = Edit {
        symbol: version.id.to_string(),
        expect,
        operation,
    };
    let acknowledgements = flags.acknowledge.iter().map(|statement| Acknowledgement {
        symbol: version.id.to_string(),
        statement: statement.clone(),
    });
    // --append stages this edit into an existing change file; all edits are validated together.
    let staged = match &flags.patch {
        Some(path)
            if flags.append && crate::project::absolute(&ctx.session.cwd, path).is_file() =>
        {
            Some(read::<Change>(ctx, path)?)
        }
        _ => None,
    };
    let change = match staged {
        Some(mut change) => {
            if change.actor != flags.actor {
                return Err(failure(
                    ctx,
                    cpl_evolve::error(
                        "change_actor_mismatch",
                        "the staged change belongs to another actor",
                    ),
                ));
            }
            change.edits.push(edit);
            change.contract.acknowledge.extend(acknowledgements);
            change
        }
        None => Change {
            name: format!("edit-{symbol}"),
            actor: flags.actor.clone(),
            edits: vec![edit],
            reads: vec![],
            contract: ChangeContract {
                require_migrations: true,
                acknowledge: acknowledgements.collect(),
                ..ChangeContract::default()
            },
            verification: VerifyConfig::default(),
        },
    };
    let authority = policy(ctx, flags.policy.as_deref())?;
    let candidate =
        cpl_evolve::change::prepare(&s, &change, authority.as_ref(), &load_options(false))
            .map_err(|d| failure(ctx, d))?;
    if let Some(path) = &flags.patch {
        save(ctx, &s, path, &change)?;
    }
    let files = if flags.apply && candidate.report.accepted {
        let install = candidate.install(&s).map_err(|d| failure(ctx, d))?;
        cpl_evolve::storage::apply(&ctx.session.locate()?, &s, &install, &load_options(false))
            .map_err(|d| failure(ctx, d))?
    } else {
        vec![]
    };
    report(
        ctx,
        "edit",
        json!({"change":candidate.report,"patch":change,"applied":files}),
        candidate.report.accepted,
    )
}
pub fn transaction(ctx: &Ctx, args: &TransactionArgs) -> Result<Report<Value>, Failure> {
    let root = ctx.session.locate()?;
    if args.recover {
        let count = cpl_evolve::storage::recover(&root).map_err(|d| failure(ctx, d))?;
        return report(ctx, "transaction", json!({"recovered_files":count}), true);
    }
    let change: Change = read(ctx, args.file.as_ref().unwrap())?;
    if args.actor.as_ref().is_some_and(|a| a != &change.actor) {
        return Err(failure(
            ctx,
            cpl_evolve::error(
                "change_actor_mismatch",
                "caller identity differs from submitted actor",
            ),
        ));
    }
    let authority = policy(ctx, args.policy.as_deref())?;
    if authority.is_some() && args.actor.is_none() {
        return Err(failure(
            ctx,
            cpl_evolve::error(
                "change_actor_required",
                "a policy-controlled transaction requires --actor to identify the caller",
            ),
        ));
    }
    let s = current(ctx)?;
    let candidate =
        cpl_evolve::change::prepare(&s, &change, authority.as_ref(), &load_options(false))
            .map_err(|d| failure(ctx, d))?;
    let install = || candidate.install(&s).map_err(|d| failure(ctx, d));
    let files = if args.apply && candidate.report.accepted {
        cpl_evolve::storage::apply(&root, &s, &install()?, &load_options(false))
            .map_err(|d| failure(ctx, d))?
    } else {
        vec![]
    };
    let commit = match &args.publish {
        Some(r) if candidate.report.accepted => Some(
            cpl_evolve::storage::publish(&root, &s, &install()?, r, &load_options(false))
                .map_err(|d| failure(ctx, d))?,
        ),
        _ => None,
    };
    report(
        ctx,
        "transaction",
        json!({"change":candidate.report,"applied":files,"commit":commit,"reference":args.publish}),
        candidate.report.accepted,
    )
}
/// `conflicts` previews; `merge` can also install over the worktree or publish a merge commit.
pub fn merge(
    ctx: &Ctx,
    args: &ConflictsArgs,
    write: Option<&MergeArgs>,
) -> Result<Report<Value>, Failure> {
    let command = if write.is_some() {
        "merge"
    } else {
        "conflicts"
    };
    let base_rev = match &args.base {
        Some(base) => base.clone(),
        None if args.left != "worktree" && args.right != "worktree" => {
            let root = ctx.session.locate()?;
            let repo = crate::project::discover(&root)?;
            let repo = repo.required("merge base").map_err(|d| failure(ctx, d))?;
            let commit = |rev: &str| {
                cpl_index::resolve_rev(repo, rev)
                    .map_err(|e| failure(ctx, cpl_index::git_error_diagnostic(&e)))
            };
            let (left, right) = (commit(&args.left)?, commit(&args.right)?);
            cpl_index::merge_base(repo, &left, &right)
                .map_err(|e| failure(ctx, cpl_index::git_error_diagnostic(&e)))?
        }
        None => "HEAD".into(),
    };
    let base = historical(ctx, &base_rev)?;
    let left = historical(ctx, &args.left)?;
    let right = historical(ctx, &args.right)?;
    let config = VerifyConfig {
        seed: args.seed,
        cases: args.cases,
        timeout_ms: args.timeout_ms,
        ..VerifyConfig::default()
    };
    let options = load_options(false);
    let merged = cpl_evolve::merge::merge(&base, &left, &right, &config, &options)
        .map_err(|d| failure(ctx, d))?;
    let root = ctx.session.locate()?;
    let mut applied = vec![];
    let mut commit = None;
    if let Some(write) = write.filter(|_| merged.report.accepted) {
        let install = merged.install(&left).map_err(|d| failure(ctx, d))?;
        if write.apply {
            if args.left != "worktree" && current(ctx)?.digest() != left.digest() {
                return Err(failure(
                    ctx,
                    cpl_evolve::error(
                        "merge_apply_left",
                        "--apply installs over the worktree, so the left side must be the worktree",
                    ),
                ));
            }
            applied = cpl_evolve::storage::apply(&root, &left, &install, &options)
                .map_err(|d| failure(ctx, d))?;
        }
        if let Some(reference) = &write.publish {
            if args.left == "worktree" || args.right == "worktree" {
                return Err(ctx.session.refuse(crate::codes::bad_argument(
                    "--publish merges two committed revisions; commit or publish the worktree first",
                )));
            }
            commit = Some(
                cpl_evolve::storage::publish_merge(
                    &root,
                    &args.left,
                    &args.right,
                    &install,
                    reference,
                    &options,
                )
                .map_err(|d| failure(ctx, d))?,
            );
        }
    }
    let accepted = merged.report.accepted;
    report(
        ctx,
        command,
        json!({"merge": merged.report, "base_revision": base_rev, "applied": applied, "commit": commit,
            "reference": write.and_then(|w| w.publish.clone())}),
        accepted,
    )
}
pub fn explore(ctx: &Ctx, args: &ExploreArgs) -> Result<Report<Value>, Failure> {
    let s = current(ctx)?;
    let events = args
        .events
        .as_deref()
        .map(|e| literal(ctx, e))
        .transpose()?;
    let events = events
        .as_ref()
        .map(|e| {
            e.as_array().cloned().ok_or_else(|| {
                ctx.session
                    .refuse(crate::codes::bad_argument("--events must be an array"))
            })
        })
        .transpose()?;
    let state = args.state.as_deref().map(|e| literal(ctx, e)).transpose()?;
    let config = cpl_evolve::explore::ExploreConfig {
        depth: args.depth,
        max_states: args.max_states,
        max_transitions: args.max_transitions,
        verification: VerifyConfig {
            timeout_ms: args.timeout_ms,
            ..VerifyConfig::default()
        },
        invariants: args.invariants.clone(),
        goal: args.goal.clone(),
    };
    let result = cpl_evolve::explore::explore(
        &s,
        &args.machine,
        state.as_ref(),
        events.as_deref(),
        &config,
    )
    .map_err(|d| failure(ctx, d))?;
    let ok = matches!(
        result.result.as_str(),
        "exhaustive" | "bounded_pass" | "witness" | "unreachable"
    );
    report(ctx, "explore", json!({"exploration":result}), ok)
}
pub fn schema(ctx: &Ctx, args: &SchemaArgs) -> Result<Report<Value>, Failure> {
    let s = current(ctx)?;
    let value = if let Some(v) = &args.value {
        json!({"stored":cpl_evolve::migration::pack(&s,&args.symbol,&literal(ctx,v)?,&VerifyConfig::default()).map_err(|d|failure(ctx,d))?})
    } else {
        let id = s.resolve(&args.symbol).map_err(|d| failure(ctx, d))?;
        if s.checked.program.annotation_texts(id, "schema").is_empty() {
            return Err(failure(
                ctx,
                cpl_evolve::error("schema_annotation_required", "select a @schema type"),
            ));
        }
        json!({"symbol":cpl_evolve::query::symbol(&s,id),"schema_digest":cpl_evolve::migration::fingerprint(&s,id)})
    };
    report(ctx, "schema", value, true)
}
pub fn migrate(ctx: &Ctx, args: &MigrateArgs) -> Result<Report<Value>, Failure> {
    let s = current(ctx)?;
    let value = read(ctx, &args.input)?;
    let config = VerifyConfig::default();
    let (stored, steps) = match (&args.function, &args.to) {
        (Some(function), _) => (
            cpl_evolve::migration::migrate(&s, function, &value, &config),
            vec![function.clone()],
        ),
        (None, Some(version)) => {
            match cpl_evolve::migration::migrate_to(&s, &value, version, &config) {
                Ok((stored, steps)) => (Ok(stored), steps),
                Err(d) => (Err(d), vec![]),
            }
        }
        (None, None) => unreachable!("clap requires a function or --to"),
    };
    let stored = stored.map_err(|d| failure(ctx, d))?;
    report(
        ctx,
        "migrate",
        json!({"stored": stored, "steps": steps}),
        true,
    )
}
struct Fixtures(BTreeMap<String, BTreeMap<String, Value>>);
impl cpl_evolve::concurrent::EffectHost for Fixtures {
    fn call(&self, resource: &str, operation: &str, _arguments: &[Value]) -> Result<Value, String> {
        self.0
            .get(resource)
            .and_then(|r| r.get(operation))
            .cloned()
            .ok_or_else(|| format!("no fixture for {resource}.{operation}"))
    }
}
pub fn concurrent(ctx: &Ctx, args: &ConcurrentArgs) -> Result<Report<Value>, Failure> {
    let s = current(ctx)?;
    let scope = read(ctx, &args.file)?;
    let result = if let Some(path) = &args.replay {
        Some(cpl_evolve::concurrent::replay(
            &s,
            &scope,
            &read(ctx, path)?,
            args.workers,
        ))
    } else if let Some(path) = &args.responses {
        Some(cpl_evolve::concurrent::execute(
            &s,
            &scope,
            Arc::new(Fixtures(read(ctx, path)?)),
            args.workers,
        ))
    } else {
        None
    };
    if let Some(result) = result {
        let result = result.map_err(|d| failure(ctx, d))?;
        if let Some(path) = &args.receipt {
            save(ctx, &s, path, &result)?;
        }
        report(
            ctx,
            "concurrent",
            json!({"execution":result}),
            result.completed,
        )
    } else {
        let plan = cpl_evolve::concurrent::plan(&s, &scope).map_err(|d| failure(ctx, d))?;
        if args.receipt.is_some() {
            return Err(ctx.session.refuse(crate::codes::bad_argument(
                "--receipt requires --responses or --replay",
            )));
        }
        report(ctx, "concurrent", json!({"plan":plan}), true)
    }
}
