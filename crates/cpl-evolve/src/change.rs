//! Facet-addressed optimistic edits, authority, change contracts, and semantic compatibility reports.
use crate::{
    error,
    verify::{self, Evidence, VerifyConfig},
    Snapshot,
};
use cpl_diag::{Diagnostic, Facet, FileId, Span, StableId};
use cpl_hir::{ImportTarget, RefKind};
use cpl_project::LoadOptions;
use cpl_syntax::{
    ast, format_fragment,
    lexer::{lex, TokenKind},
    parse_module, Fragment, FragmentMode,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edit {
    pub symbol: String,
    /// Every facet this operation can overwrite must have an expected hash.
    pub expect: BTreeMap<Facet, String>,
    pub operation: Operation,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    ReplaceBody {
        body: String,
    },
    Rename {
        name: String,
    },
    ReplaceDeclaration {
        source: String,
    },
    AddDeclaration {
        file: String,
        module: String,
        source: String,
    },
    /// Delete the declaration, its doc comments, annotations, and leading comments.
    RemoveDeclaration,
    /// Replace the typed hole `?hole` of the target with one expression.
    FillHole {
        hole: String,
        expression: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub symbol: String,
    pub facets: BTreeSet<Facet>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ChangeContract {
    pub preserve: Vec<Selection>,
    pub behavior_samples: Vec<BehaviorSamples>,
    /// Old and new functions compared on generated inputs; agreement is test evidence only.
    pub differential: Vec<crate::differential::Differential>,
    /// These are universal obligations. Tests alone never discharge them.
    pub preserve_behavior: bool,
    /// Measured improvements: a measure name (`"time_complexity"`) for every function the change
    /// modifies, or `{symbol, measure}` for one.
    pub improve: Vec<Improvement>,
    pub require_migrations: bool,
    /// `@preserve` statements the actor attests that this change keeps.
    pub acknowledge: Vec<Acknowledgement>,
}
/// One `improve` requirement.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Improvement {
    /// The measure, for every function the change modifies.
    Named(String),
    /// The measure of one function.
    Target { symbol: String, measure: String },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Acknowledgement {
    pub symbol: String,
    pub statement: String,
}
impl Default for ChangeContract {
    fn default() -> Self {
        Self {
            preserve: vec![],
            behavior_samples: vec![],
            differential: vec![],
            preserve_behavior: false,
            improve: vec![],
            require_migrations: true,
            acknowledge: vec![],
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BehaviorSamples {
    pub symbol: String,
    pub arguments: Vec<Vec<Value>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub name: String,
    pub actor: String,
    pub edits: Vec<Edit>,
    #[serde(default)]
    pub reads: Vec<ReadGuard>,
    #[serde(default)]
    pub contract: ChangeContract,
    #[serde(default)]
    pub verification: VerifyConfig,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadGuard {
    pub symbol: String,
    pub facet: Facet,
    pub expect: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub actor: String,
    pub write: Vec<Selection>,
    #[serde(default)]
    pub forbid: Vec<Selection>,
    #[serde(default)]
    pub add: Vec<String>,
    /// Namespaces in which the actor may remove declarations.
    #[serde(default)]
    pub remove: Vec<String>,
    /// When present, the declarations the actor's changed code may depend on: a stable identity or
    /// name, `module.*`, or `dependencies(NAME)` (the transitive dependency closure of NAME, NAME
    /// included). Standard-library declarations and the change's own declarations are always readable.
    #[serde(default)]
    pub read: Option<Vec<String>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub namespace: String,
    pub owner: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub struct Policy {
    pub grants: Vec<Grant>,
    pub regions: Vec<Region>,
}

#[derive(Debug, Serialize)]
pub struct ChangeReport {
    pub name: String,
    pub actor: String,
    pub request: Change,
    pub base: String,
    pub candidate: String,
    pub authority_digest: Option<String>,
    pub changes: Value,
    pub behavior: Vec<Value>,
    pub obligations: Vec<Value>,
    /// `@preserve` statements the actor attested (unverified) or revised with the change.
    pub attestations: Vec<Value>,
    pub evidence: Evidence,
    pub accepted: bool,
}
pub struct Candidate {
    pub snapshot: Snapshot,
    pub report: ChangeReport,
    seal: String,
}
impl Candidate {
    pub(crate) fn accepted_for(&self, base: &Snapshot) -> bool {
        self.report.accepted
            && self.report.base == base.digest()
            && self.report.candidate == self.snapshot.digest()
            && self.seal
                == blake3::hash(&serde_json::to_vec(&self.report).unwrap())
                    .to_hex()
                    .as_str()
    }
}

fn hole<'s>(
    base: &'s Snapshot,
    id: cpl_hir::DefId,
    name: &str,
) -> Result<&'s cpl_sema::HoleReport, Diagnostic> {
    let mut found = base
        .checked
        .holes
        .iter()
        .filter(|h| h.def == id && h.name == name);
    match (found.next(), found.next()) {
        (Some(h), None) => Ok(h),
        (None, _) => Err(error(
            "change_hole_missing",
            format!("the target has no hole ?{name}"),
        )),
        _ => Err(error(
            "change_hole_ambiguous",
            format!("?{name} occurs more than once in the target"),
        )),
    }
}

/// The facets an operation overwrites: each needs an expected hash in the edit.
pub fn operation_facets(
    base: &Snapshot,
    id: cpl_hir::DefId,
    operation: &Operation,
) -> Result<Vec<Facet>, Diagnostic> {
    Ok(match operation {
        Operation::ReplaceBody { .. } => vec![Facet::Body],
        Operation::Rename { .. } => vec![Facet::Name],
        Operation::ReplaceDeclaration { .. } | Operation::RemoveDeclaration => {
            base.index().version(id).facets.keys().copied().collect()
        }
        Operation::AddDeclaration { .. } => vec![],
        Operation::FillHole { hole: name, .. } => {
            let span = hole(base, id, name)?.span;
            let body = match &base.checked.program.ast(id).kind {
                ast::DeclKind::Fn(f) => f.body.as_ref().map(|b| b.span),
                ast::DeclKind::Test(t) => Some(t.body.span),
                ast::DeclKind::Property(t) => Some(t.body.span),
                _ => None,
            };
            if body.is_some_and(|b| b.start <= span.start && span.end <= b.end) {
                vec![Facet::Body]
            } else {
                vec![Facet::Contract]
            }
        }
    })
}

pub fn prepare(
    base: &Snapshot,
    change: &Change,
    policy: Option<&Policy>,
    options: &LoadOptions,
) -> Result<Candidate, Diagnostic> {
    if change.name.trim().is_empty() || change.actor.trim().is_empty() || change.edits.is_empty() {
        return Err(error(
            "change_invalid",
            "a change needs a name, actor, and at least one edit",
        ));
    }
    let index = base.index();
    for read in &change.reads {
        let id = base.resolve(&read.symbol)?;
        if index
            .version(id)
            .facets
            .get(&read.facet)
            .map(|x| x.as_str())
            != Some(read.expect.as_str())
        {
            return Err(error(
                "change_read_stale",
                format!("a dependency changed: {}.{}", read.symbol, read.facet),
            ));
        }
    }
    // Edits are based on the same snapshot. Overlapping byte ranges are rejected before applying any.
    let mut replacements: BTreeMap<String, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut added_files = BTreeMap::new();
    let mut touched = BTreeSet::new();
    for edit in &change.edits {
        if let Operation::AddDeclaration {
            file,
            module,
            source,
        } = &edit.operation
        {
            crate::storage::safe_relative(file)?;
            if !edit.expect.is_empty() {
                return Err(error(
                    "change_invalid",
                    "an added declaration has no prior facets",
                ));
            }
            if !file.ends_with(".cpl") {
                return Err(error(
                    "change_invalid",
                    "declarations must be added to a .cpl file",
                ));
            }
            let parsed = parse_one(module, source)?;
            let stable = cpl_index::declaration_identity(
                &module.split('.').map(str::to_string).collect::<Vec<_>>(),
                &parsed,
            );
            if stable.as_str() != edit.symbol
                || base.checked.program.by_stable_id(&stable).is_some()
                || !touched.insert((stable.clone(), Facet::Identity))
            {
                return Err(error(
                    "change_identity",
                    "an addition needs a new explicit stable id matching symbol",
                ));
            }
            if stable.is_provisional() {
                return Err(error(
                    "change_identity",
                    "assign an @id to the added declaration",
                ));
            }
            let block = format!("\n{source}\n");
            if let Some(bytes) = base.tree.files.get(file) {
                if cpl_work::module_header(bytes)
                    != Some(module.split('.').map(str::to_string).collect())
                {
                    return Err(error(
                        "change_module",
                        "the destination module differs from the requested module",
                    ));
                }
                replacements.entry(file.clone()).or_default().push((
                    bytes.len(),
                    bytes.len(),
                    block,
                ));
            } else {
                let entry = added_files
                    .entry(file.clone())
                    .or_insert_with(|| format!("module {module}\n"));
                if cpl_work::module_header(entry.as_bytes())
                    != Some(module.split('.').map(str::to_string).collect())
                {
                    return Err(error(
                        "change_module",
                        "two additions disagree on the destination module",
                    ));
                }
                entry.push_str(&block);
            }
            continue;
        }
        let id = base.resolve(&edit.symbol)?;
        let p = &base.checked.program;
        let def = p.def(id);
        if def.provisional {
            return Err(error(
                "change_identity",
                "assign a stable @id before submitting semantic edits",
            ));
        }
        if !base
            .tree
            .files
            .contains_key(&p.source_map.file(def.file).path)
        {
            return Err(error(
                "change_readonly",
                "standard library declarations cannot be edited",
            ));
        }
        for (facet, expected) in &edit.expect {
            if index.version(id).facets.get(facet).map(|h| h.as_str()) != Some(expected.as_str()) {
                return Err(error(
                    "change_stale",
                    format!(
                        "{}.{} no longer matches the expected facet hash",
                        edit.symbol, facet
                    ),
                ));
            }
        }
        let required = operation_facets(base, id, &edit.operation)?;
        if required
            .iter()
            .any(|f| !edit.expect.contains_key(f) || !touched.insert((def.stable_id.clone(), *f)))
        {
            return Err(error(
                "change_precondition",
                "every overwritten facet needs a hash and may be edited only once per transaction",
            ));
        }
        let mut replace = |span: Span, text: String| {
            replacements
                .entry(p.source_map.file(span.file).path.clone())
                .or_default()
                .push((span.start as usize, span.end as usize, text));
        };
        let decl = p.ast(id);
        match &edit.operation {
            Operation::ReplaceBody { body } => {
                let block = match &decl.kind {
                    ast::DeclKind::Fn(f) => f.body.as_ref(),
                    ast::DeclKind::Test(t) => Some(&t.body),
                    ast::DeclKind::Property(t) => Some(&t.body),
                    _ => None,
                }
                .ok_or_else(|| error("change_no_body", "the target has no body"))?;
                let replacement =
                    parse_one("candidate", &format!("fn candidate() -> Unit {body}"))?;
                let ast::DeclKind::Fn(f) = replacement.kind else {
                    unreachable!()
                };
                replace(
                    block.span,
                    format_fragment(
                        Fragment::Block(f.body.as_ref().unwrap()),
                        FragmentMode::Source,
                    ),
                );
            }
            Operation::Rename { name } => {
                let old = decl.name().ok_or_else(|| {
                    error("change_rename", "rename requires an identifier declaration")
                })?;
                let (tokens, diagnostics) = lex(name, FileId(0));
                let expected_kind = if old.text.starts_with(char::is_uppercase) {
                    TokenKind::UpperIdent
                } else {
                    TokenKind::LowerIdent
                };
                if !diagnostics.is_empty()
                    || tokens.len() != 2
                    || tokens[0].kind != expected_kind
                    || tokens[0].text != *name
                {
                    return Err(error(
                        "change_rename",
                        "the new name must be one identifier of the same case",
                    ));
                }
                replace(old.span, name.clone());
                let mut spans = BTreeSet::new();
                for r in p.references_to(id) {
                    if r.kind == RefKind::FieldAccess || r.kind == RefKind::CapabilityOp {
                        continue;
                    }
                    spans.insert(r.span);
                }
                for import in &p.imports {
                    if import.target == ImportTarget::Def(id) {
                        spans.insert(import.span);
                    }
                }
                for span in spans {
                    let text = p.source_map.file(span.file).text();
                    // References delimit the resolved name, not a whole calling expression. Match the
                    // final old-name identifier in that span; strings, comments, and other locals are untouched.
                    let (tokens, _) = lex(&text[span.start as usize..span.end as usize], span.file);
                    if let Some(token) = tokens.iter().rev().find(|t| {
                        matches!(t.kind, TokenKind::LowerIdent | TokenKind::UpperIdent)
                            && t.text == old.text
                    }) {
                        replace(
                            Span {
                                file: span.file,
                                start: span.start + token.span.start,
                                end: span.start + token.span.end,
                            },
                            name.clone(),
                        );
                    }
                }
            }
            Operation::ReplaceDeclaration { source } => {
                let module = &p.module(def.module).path;
                let replacement = parse_one(&module.join("."), source)?;
                if cpl_index::declaration_identity(module, &replacement) != def.stable_id {
                    return Err(error(
                        "change_identity",
                        "replacement declarations must retain the original @id",
                    ));
                }
                replace(decl.span, source.clone());
            }
            Operation::RemoveDeclaration => {
                let file = decl.span.file;
                let (start, end) = crate::merge::deletion(p.source_map.file(file).text(), decl);
                replace(Span { file, start, end }, String::new());
            }
            Operation::FillHole {
                hole: name,
                expression,
            } => {
                let span = hole(base, id, name)?.span;
                let parsed = cpl_syntax::parse_expr(expression, FileId(0));
                let Some(expr) = parsed.value.filter(|_| parsed.diagnostics.is_empty()) else {
                    return Err(error(
                        "change_syntax",
                        "a hole is filled with exactly one valid expression",
                    )
                    .with_data(
                        "parse_errors",
                        json!(parsed
                            .diagnostics
                            .iter()
                            .map(|d| &d.message)
                            .collect::<Vec<_>>()),
                    ));
                };
                // A hole is a primary expression; keep the filled expression's precedence.
                use ast::ExprKind as K;
                let atom = matches!(
                    expr.kind,
                    K::Literal(_)
                        | K::Name(_)
                        | K::Hole(_)
                        | K::Member(_)
                        | K::Call(_)
                        | K::Propagate(_)
                        | K::Paren(_)
                        | K::List(_)
                        | K::Struct(_)
                );
                let text = expression.trim();
                replace(
                    span,
                    if atom {
                        text.to_string()
                    } else {
                        format!("({text})")
                    },
                );
            }
            Operation::AddDeclaration { .. } => unreachable!(),
        }
    }
    let mut tree = base.tree.clone();
    for (path, mut edits) in replacements {
        edits.sort_by_key(|e| (e.0, e.1));
        edits.dedup();
        if edits.windows(2).any(|w| w[0].1 > w[1].0) {
            return Err(error(
                "change_overlap",
                format!("overlapping edits in {path}"),
            ));
        }
        let mut bytes =
            tree.files.get(&path).cloned().ok_or_else(|| {
                error("change_readonly", "cannot edit a file outside the project")
            })?;
        for (start, end, replacement) in edits.into_iter().rev() {
            bytes.splice(start..end, replacement.bytes());
        }
        tree.files.insert(path, bytes);
    }
    for (path, text) in added_files {
        tree.files.insert(path, text.into_bytes());
    }
    let candidate = Snapshot::new(tree, options)?;
    enforce(base, &candidate, change, policy)?;
    let attestations = acknowledgements(base, &candidate, change)?;
    let mut behavior = Vec::new();
    let mut samples_ok = true;
    for sample in &change.contract.behavior_samples {
        if sample.arguments.is_empty() {
            return Err(error(
                "change_empty_evidence",
                "behavior sample sets must not be empty",
            ));
        }
        let id = base
            .checked
            .program
            .def(base.resolve(&sample.symbol)?)
            .stable_id
            .clone();
        let mut passed = 0;
        for args in &sample.arguments {
            let old = call(base, &id, args, &change.verification);
            let new = call(&candidate, &id, args, &change.verification);
            match (old, new) {
                (Ok(a), Ok(b)) if a == b => {
                    passed += 1;
                }
                (a, b) => {
                    samples_ok = false;
                    behavior.push(json!({"symbol": id, "status": "failed", "arguments": args, "before": outcome(a), "after": outcome(b)}));
                }
            }
        }
        behavior.push(json!({"symbol": id, "status": "tested", "samples_passed": passed, "samples_total": sample.arguments.len(), "equivalence": "unknown"}));
    }
    for spec in &change.contract.differential {
        let (records, ok) =
            crate::differential::compare(base, &candidate, spec, &change.verification)?;
        samples_ok &= ok;
        behavior.extend(records);
    }
    let mut obligations = Vec::new();
    if change.contract.preserve_behavior {
        obligations.push(crate::equivalence::preserve_behavior(
            base,
            &candidate,
            &change.verification,
        )?);
    }
    for improvement in &change.contract.improve {
        let (targets, measure) = match improvement {
            Improvement::Named(measure) => (
                crate::equivalence::changed_functions(base, &candidate),
                measure,
            ),
            Improvement::Target { symbol, measure } => {
                let def = base.resolve(symbol)?;
                (
                    vec![base.checked.program.def(def).stable_id.clone()],
                    measure,
                )
            }
        };
        if targets.is_empty() {
            obligations.push(json!({"requirement": measure, "status": "unknown", "reason": "no_changed_function"}));
        }
        for id in targets {
            obligations.push(crate::equivalence::measure(
                base,
                &candidate,
                &id,
                measure,
                &change.verification,
            )?);
        }
    }
    // The contract decides whether schema compatibility is required; evidence records it either way.
    let verification = VerifyConfig {
        require_migrations: change.contract.require_migrations,
        ..change.verification.clone()
    };
    let evidence = verify::verify(&candidate, Some(base), &verification)?;
    // An improvement is established by measurement; every other requirement by proof.
    let accepted = samples_ok
        && evidence.accepted
        && obligations
            .iter()
            .all(|o| o["status"] == "proved" || o["status"] == "measured");
    let changes = json!(cpl_index::semantic_diff(
        base.index().facet_map(),
        candidate.index().facet_map()
    ));
    let report = ChangeReport {
        name: change.name.clone(),
        actor: change.actor.clone(),
        request: change.clone(),
        base: base.digest(),
        candidate: candidate.digest(),
        authority_digest: policy.map(|p| {
            blake3::hash(&serde_json::to_vec(p).unwrap())
                .to_hex()
                .to_string()
        }),
        changes,
        behavior,
        obligations,
        attestations,
        evidence,
        accepted,
    };
    let seal = blake3::hash(&serde_json::to_vec(&report).unwrap())
        .to_hex()
        .to_string();
    Ok(Candidate {
        snapshot: candidate,
        report,
        seal,
    })
}

/// A body or declaration replacement of a declaration carrying `@preserve` must either revise that
/// docs metadata or acknowledge each statement. Acknowledgements are the actor's attestation, recorded
/// as such; they are never treated as verified. Unmatched acknowledgements are errors.
fn acknowledgements(
    base: &Snapshot,
    candidate: &Snapshot,
    change: &Change,
) -> Result<Vec<Value>, Diagnostic> {
    let p = &base.checked.program;
    let (old, new) = (base.index(), candidate.index());
    let mut needed = BTreeSet::new();
    let mut out = Vec::new();
    for edit in &change.edits {
        if !matches!(
            edit.operation,
            Operation::ReplaceBody { .. } | Operation::ReplaceDeclaration { .. }
        ) {
            continue;
        }
        let def = base.resolve(&edit.symbol)?;
        let id = p.def(def).stable_id.clone();
        let Some(statement) = p
            .annotation_texts(def, "preserve")
            .first()
            .map(|s| s.to_string())
        else {
            continue;
        };
        let docs = |m: &cpl_index::Index| {
            m.facet_map()
                .get(&id)
                .and_then(|e| e.facet(Facet::Docs))
                .cloned()
        };
        if docs(&old) != docs(&new) {
            let after = candidate.checked.program.by_stable_id(&id).and_then(|d| {
                candidate
                    .checked
                    .program
                    .annotation_texts(d, "preserve")
                    .first()
                    .map(|s| s.to_string())
            });
            out.push(json!({"symbol": id, "statement": statement, "status": "revised", "revision": after}));
        } else {
            needed.insert((id, statement));
        }
    }
    let mut given = BTreeSet::new();
    for a in &change.contract.acknowledge {
        let id = p.def(base.resolve(&a.symbol)?).stable_id.clone();
        if !needed.contains(&(id.clone(), a.statement.clone())) {
            return Err(error(
                "change_acknowledgement_invalid",
                format!(
                    "{} does not require acknowledging {:?}",
                    a.symbol, a.statement
                ),
            ));
        }
        given.insert((id, a.statement.clone()));
    }
    let missing: Vec<_> = needed
        .difference(&given)
        .map(|(id, statement)| json!({"symbol": id, "statement": statement}))
        .collect();
    if !missing.is_empty() {
        return Err(error(
            "change_preserve_unacknowledged",
            "acknowledge each @preserve statement of a replaced declaration, or revise it",
        )
        .with_data("statements", json!(missing)));
    }
    out.extend(needed.into_iter().map(|(id, statement)| {
        json!({"symbol": id, "statement": statement, "status": "attested", "by": change.actor})
    }));
    Ok(out)
}

pub(crate) fn parse_one(module: &str, source: &str) -> Result<ast::Declaration, Diagnostic> {
    let parsed = parse_module(&format!("module {module}\n{source}\n"), FileId(0));
    match parsed.value {
        Some(mut m) if m.declarations.len() == 1 && m.uses.is_empty() => {
            Ok(m.declarations.remove(0))
        }
        _ => Err(error(
            "change_syntax",
            "replacement must be exactly one valid declaration, without imports or a module header",
        )
        .with_data(
            "parse_errors",
            json!(parsed
                .diagnostics
                .iter()
                .map(|d| &d.message)
                .collect::<Vec<_>>()),
        )),
    }
}

fn outcome(r: Result<Value, Diagnostic>) -> Value {
    match r {
        Ok(v) => json!({"value":v}),
        Err(d) => json!({"error":d.message}),
    }
}
fn call(
    s: &Snapshot,
    id: &StableId,
    args: &[Value],
    cfg: &VerifyConfig,
) -> Result<Value, Diagnostic> {
    let p = &s.checked.program;
    let def = p
        .by_stable_id(id)
        .ok_or_else(|| error("change_target_missing", id.to_string()))?;
    let result = cpl_eval::run_function(
        p,
        &p.symbol_path(def),
        &args
            .iter()
            .cloned()
            .map(cpl_eval::RunArg::Json)
            .collect::<Vec<_>>(),
        &cfg.eval(),
    )
    .map_err(|e| error("change_sample_error", e.to_string()))?;
    result
        .value
        .map(|v| cpl_eval::value_to_json(p, &v, &result.return_ty))
        .map_err(|f| f.to_diagnostic(p))
}
fn covers(namespace: &str, module: &str) -> bool {
    namespace == "*"
        || module == namespace
        || module
            .strip_prefix(namespace)
            .is_some_and(|s| s.starts_with('.'))
}
fn selected(s: &Snapshot, selection: &Selection, id: &StableId, facet: Facet) -> bool {
    if !selection.facets.contains(&facet) {
        return false;
    }
    if let Some(ns) = selection.symbol.strip_suffix(".*") {
        return s
            .index()
            .facet_map()
            .get(id)
            .is_some_and(|e| covers(ns, &e.module));
    }
    s.resolve(&selection.symbol)
        .is_ok_and(|d| &s.checked.program.def(d).stable_id == id)
}

/// What `actor` may change under `policy`: the same grant, forbid, and region rules `prepare`
/// enforces, evaluated for every project declaration. Identity is never writable.
pub fn permissions(s: &Snapshot, policy: &Policy, actor: &str) -> Value {
    let grants: Vec<_> = policy.grants.iter().filter(|g| g.actor == actor).collect();
    let index = s.index();
    let mut symbols = Vec::new();
    for (id, entry) in &index.facet_map().entries {
        let owned_elsewhere = policy
            .regions
            .iter()
            .any(|r| covers(&r.namespace, &entry.module) && r.owner != actor);
        let forbidden = |facet: Facet| {
            grants
                .iter()
                .any(|g| g.forbid.iter().any(|sel| selected(s, sel, id, facet)))
        };
        let writable: Vec<_> = if owned_elsewhere {
            vec![]
        } else {
            entry
                .version
                .facets
                .keys()
                .copied()
                .filter(|f| *f != Facet::Identity && !forbidden(*f))
                .filter(|f| {
                    grants
                        .iter()
                        .any(|g| g.write.iter().any(|sel| selected(s, sel, id, *f)))
                })
                .collect()
        };
        let removable = !owned_elsewhere
            && !Facet::ALL.iter().any(|f| forbidden(*f))
            && grants
                .iter()
                .any(|g| g.remove.iter().any(|ns| covers(ns, &entry.module)));
        if !writable.is_empty() || removable {
            symbols.push(json!({"symbol": id, "path": entry.version.path, "writable": writable, "removable": removable}));
        }
    }
    let add: BTreeSet<_> = grants
        .iter()
        .flat_map(|g| g.add.iter())
        .filter(|ns| {
            !policy
                .regions
                .iter()
                .any(|r| r.owner != actor && (covers(&r.namespace, ns) || covers(ns, &r.namespace)))
        })
        .collect();
    let readable = match readable(s, policy, actor) {
        Ok(None) => json!("unrestricted"),
        Ok(Some(ids)) => json!(ids),
        Err(d) => json!({"error": d.message}),
    };
    json!({"actor": actor, "symbols": symbols, "add": add, "readable": readable,
        "regions": policy.regions.iter().filter(|r| r.owner == actor).map(|r| &r.namespace).collect::<Vec<_>>()})
}

fn enforce(
    base: &Snapshot,
    after: &Snapshot,
    change: &Change,
    policy: Option<&Policy>,
) -> Result<(), Diagnostic> {
    let old = base.index();
    let new = after.index();
    for selection in &change.contract.preserve {
        // A misspelled contract is an error, never an empty promise.
        let ids: Vec<_> = if let Some(ns) = selection.symbol.strip_suffix(".*") {
            old.facet_map()
                .entries
                .iter()
                .filter(|(_, e)| covers(ns, &e.module))
                .map(|(id, _)| id.clone())
                .collect()
        } else {
            vec![base
                .checked
                .program
                .def(base.resolve(&selection.symbol)?)
                .stable_id
                .clone()]
        };
        if ids.is_empty() || selection.facets.is_empty() {
            return Err(error(
                "change_empty_preserve",
                "preserve selectors must match at least one facet",
            ));
        }
        for id in ids {
            for facet in &selection.facets {
                let expected = old.facet_map().get(&id).and_then(|e| e.facet(*facet));
                if expected.is_none()
                    || expected != new.facet_map().get(&id).and_then(|e| e.facet(*facet))
                {
                    return Err(error(
                        "change_preserve_violation",
                        format!("preserved facet {id}.{facet} changed or is absent"),
                    ));
                }
            }
        }
    }
    let ids: BTreeSet<_> = old
        .facet_map()
        .entries
        .keys()
        .chain(new.facet_map().entries.keys())
        .cloned()
        .collect();
    for id in ids {
        let a = old.facet_map().get(&id);
        let b = new.facet_map().get(&id);
        if a == b {
            continue;
        }
        if let Some(policy) = policy {
            let module = b.or(a).unwrap().module.as_str();
            if policy
                .regions
                .iter()
                .any(|r| covers(&r.namespace, module) && r.owner != change.actor)
            {
                return Err(error(
                    "change_region_denied",
                    format!("{} does not own region containing {module}", change.actor),
                ));
            }
            let grants: Vec<_> = policy
                .grants
                .iter()
                .filter(|g| g.actor == change.actor)
                .collect();
            if b.is_none() {
                // A removal needs a remove namespace and no forbid on any facet of the symbol.
                if grants.iter().any(|g| {
                    g.forbid.iter().any(|s| {
                        Facet::ALL
                            .iter()
                            .any(|facet| selected(base, s, &id, *facet))
                    })
                }) || !grants
                    .iter()
                    .any(|g| g.remove.iter().any(|ns| covers(ns, module)))
                {
                    return Err(error(
                        "change_remove_denied",
                        format!("{} cannot remove {id} from {module}", change.actor),
                    ));
                }
            } else if a.is_none() {
                if grants.iter().any(|g| {
                    g.forbid.iter().any(|s| {
                        Facet::ALL
                            .iter()
                            .any(|facet| selected(after, s, &id, *facet))
                    })
                }) {
                    return Err(error(
                        "change_write_denied",
                        format!("{} is forbidden to add {id}", change.actor),
                    ));
                }
                if !grants
                    .iter()
                    .any(|g| g.add.iter().any(|ns| covers(ns, module)))
                {
                    return Err(error(
                        "change_add_denied",
                        format!("{} cannot add declarations in {module}", change.actor),
                    ));
                }
            } else {
                for facet in Facet::ALL {
                    if a.and_then(|e| e.facet(facet)) == b.and_then(|e| e.facet(facet)) {
                        continue;
                    }
                    if facet == Facet::Identity
                        || grants
                            .iter()
                            .any(|g| g.forbid.iter().any(|s| selected(base, s, &id, facet)))
                        || !grants
                            .iter()
                            .any(|g| g.write.iter().any(|s| selected(base, s, &id, facet)))
                    {
                        return Err(error(
                            "change_write_denied",
                            format!("{} cannot modify {id}.{facet}", change.actor),
                        ));
                    }
                }
            }
        }
    }
    if let Some(policy) = policy {
        enforce_reads(base, after, change, policy)?;
    }
    if change.contract.require_migrations {
        crate::migration::require_migrations(base, after)?;
    }
    Ok(())
}

/// The declarations an actor may depend on, or `None` when no grant restricts reads.
pub fn readable(
    s: &Snapshot,
    policy: &Policy,
    actor: &str,
) -> Result<Option<BTreeSet<StableId>>, Diagnostic> {
    let lists: Vec<_> = policy
        .grants
        .iter()
        .filter(|g| g.actor == actor)
        .filter_map(|g| g.read.as_ref())
        .collect();
    if lists.is_empty() {
        return Ok(None);
    }
    let p = &s.checked.program;
    let index = s.index();
    let mut out = BTreeSet::new();
    for selector in lists.into_iter().flatten() {
        let before = out.len();
        if let Some(target) = selector
            .strip_prefix("dependencies(")
            .and_then(|r| r.strip_suffix(')'))
        {
            let root = s.resolve(target.trim())?;
            let mut todo = vec![root];
            let mut seen = BTreeSet::from([root]);
            while let Some(id) = todo.pop() {
                if p.module(p.def(id).module).origin != cpl_diag::FileOrigin::Std {
                    out.insert(p.def(id).stable_id.clone());
                }
                for r in p.references_from(id) {
                    if seen.insert(r.to) {
                        todo.push(r.to);
                    }
                }
            }
        } else if let Some(ns) = selector.strip_suffix(".*") {
            out.extend(
                index
                    .facet_map()
                    .entries
                    .iter()
                    .filter(|(_, e)| covers(ns, &e.module))
                    .map(|(id, _)| id.clone()),
            );
        } else {
            out.insert(p.def(s.resolve(selector)?).stable_id.clone());
        }
        if out.len() == before && !selector.starts_with("dependencies(") {
            return Err(error(
                "change_read_selector_empty",
                format!("read selector {selector:?} names no declaration"),
            ));
        }
    }
    Ok(Some(out))
}

/// Every reference from a declaration the change wrote must target a readable declaration.
fn enforce_reads(
    base: &Snapshot,
    after: &Snapshot,
    change: &Change,
    policy: &Policy,
) -> Result<(), Diagnostic> {
    let Some(allowed) = readable(base, policy, &change.actor)? else {
        return Ok(());
    };
    for guard in &change.reads {
        let id = &base
            .checked
            .program
            .def(base.resolve(&guard.symbol)?)
            .stable_id;
        if !allowed.contains(id) {
            return Err(error(
                "change_read_denied",
                format!("{} may not read {id}", change.actor),
            ));
        }
    }
    let (old, new) = (base.index(), after.index());
    let written: BTreeSet<StableId> = new
        .facet_map()
        .entries
        .iter()
        .filter(|(id, entry)| old.facet_map().get(id) != Some(*entry))
        .map(|(id, _)| id.clone())
        .collect();
    let p = &after.checked.program;
    for id in &written {
        let Some(def) = p.by_stable_id(id) else {
            continue;
        };
        for r in p.references_from(def) {
            let target = p.def(r.to);
            if p.module(target.module).origin == cpl_diag::FileOrigin::Std
                || written.contains(&target.stable_id)
                || allowed.contains(&target.stable_id)
            {
                continue;
            }
            return Err(error(
                "change_read_denied",
                format!(
                    "{} may not depend on {}: {id} references it outside the actor's read grant",
                    change.actor, target.stable_id
                ),
            )
            .with_data("declaration", json!(id))
            .with_data("target", json!(target.stable_id)));
        }
    }
    Ok(())
}
