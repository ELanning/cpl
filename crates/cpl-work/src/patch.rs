//! Semantic patches (`spec/toolchain.md` 10.5 and 5.5; grammar 4.5, D39):
//! resolving the selectors of `patch S expect base(S')` ([`resolve_patches`]),
//! the checks of a patch against a task's baseline and grant
//! ([`validate_patch`]), and body replacement ([`apply_patch`]).
//!
//! A patch form names its target and expected facets by selectors, which
//! are resolved against a checked program in the scope of the submitting
//! task's `.work` file, then as fully qualified paths (grammar 4.5):
//! [`resolve_patches`] turns each form into a [`ResolvedPatch`], whose
//! target and expected facets are `(stable id, facet)` pairs, and
//! everything else works on those.
//!
//! | Code | Raised by | When |
//! | - | - | - |
//! | `patch_facet_unsupported` | [`resolve_patches`], [`validate_patch`], [`apply_patch`] | a selector names a namespace or a whole declaration ([`resolve_patches`]); the target and expected pairs differ, the facet is not `body` (D39), or the declaration has no `body` facet |
//! | `patch_unknown_symbol` | [`resolve_patches`], [`validate_patch`] | a selector names no declaration of the program; the target is not in the baseline (5.5 step 2) |
//! | `patch_target_missing` | [`validate_patch`], [`apply_patch`] | the target is not in the tree being patched (5.5 step 3), or a bodiless `fn` has no block to replace |
//! | `patch_stale` | [`validate_patch`] | the current `body` hash differs from the baseline's (5.5 step 4), with both hashes |
//! | `patch_out_of_scope` | [`validate_patch`] | the target `body` is not in the grant's `write` set |
//! | `patch_preserved_facet` | [`validate_patch`] | the target `body` is preserved and the patch body differs from the baseline's |
//! | `patch_syntax` | [`parse_submitted_patch`] | a submitted `.patch` file does not parse; the note carries the parse diagnostic |
//!
//! Every diagnostic of [`validate_patch`] and [`apply_patch`] spans the
//! patch form and blames the target facet together with the target's
//! `symbol` and `qualified_name`, the only way semantics S49 allows a
//! `facet`. A target whose qualified name is unknown (an `@id` that neither
//! facet map has, or, for [`apply_patch`], that no file of the tree
//! declares) cannot be blamed: its id is the payload's `target` instead.

use crate::codes;
use crate::model::PatchSummary;
use crate::scheduler::Grant;
use crate::validate::selector_text;
use cpl_diag::json::FacetHash;
use cpl_diag::{Category, Diagnostic, Facet, FacetSelector, FileId, FileOrigin, Span, StableId};
use cpl_index::{declaration_identity, facet_hash, symbol_path, FacetMap};
use cpl_project::{ParsedFile, Project, VirtualTree};
use cpl_sema::hir::Program;
use cpl_sema::{resolve_selector, SelectorScope};
use cpl_syntax::ast::{self, DeclKind, SelectorKind};
use cpl_syntax::{format_fragment, parse_patch, Fragment, FragmentMode};
use serde_json::json;

/// One patch form with its selectors resolved (Section 1.9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPatch {
    /// What `patch S` names.
    pub target: (StableId, Facet),
    /// What `expect base(S')` names.
    pub expect: (StableId, Facet),
    /// The replacement block.
    pub body: ast::Block,
    /// The patch form in its `.patch` file.
    pub span: Span,
}

impl ResolvedPatch {
    /// The replacement text: the block as the formatter prints it at
    /// indentation level 0, comments kept (`FragmentMode::Source`).
    pub fn body_text(&self) -> String {
        format_fragment(Fragment::Block(&self.body), FragmentMode::Source)
    }

    /// The `body` facet hash the patch produces: the hash of the block's
    /// canonical text (Section 5.2), which is what the patched
    /// declaration's `body` facet hashes to.
    pub fn body_hash(&self) -> FacetHash {
        facet_hash(&format_fragment(
            Fragment::Block(&self.body),
            FragmentMode::Canonical,
        ))
    }

    /// The provenance record of the applied patch (`Candidate::patches`):
    /// the target, the baseline's `body` hash, and [`ResolvedPatch::body_hash`].
    /// `None` when the baseline has no `body` facet for the target.
    pub fn summary(&self, baseline: &FacetMap) -> Option<PatchSummary> {
        let expected = baseline.get(&self.target.0)?.facet(Facet::Body)?.clone();
        Some(PatchSummary {
            target: self.target.clone(),
            expected,
            applied: self.body_hash(),
        })
    }
}

/// Resolve the two selectors of every patch form of a `.patch` file against
/// a checked program (Section 1.9, grammar 4.5): `scope` is the submitting
/// task's `.work` file (its `use` lines, `module: None`), and a selector is
/// otherwise read as a fully qualified path, so a patch submitted outside
/// any task names `reader.pages.paginate.body`. Each selector must name one
/// facet of one declaration: a selector that names no declaration of the
/// program (the baseline the patch was written against) is
/// `patch_unknown_symbol`, and a namespace wildcard or a whole declaration
/// (`paginate`, `paginate.definition`) is `patch_facet_unsupported` (D39).
/// The stable identity of a target is the program's (`Def::stable_id`),
/// the key of the facet maps [`validate_patch`] compares. Whether both
/// selectors name the same `body` facet is [`validate_patch`]'s first
/// check.
///
/// `Ok` holds one [`ResolvedPatch`] per form, in file order; `Err` holds
/// every diagnostic, in form order (target before expected), each spanning
/// the offending selector.
pub fn resolve_patches(
    file: &ast::PatchFile,
    program: &Program,
    scope: &SelectorScope,
) -> Result<Vec<ResolvedPatch>, Vec<Diagnostic>> {
    let mut patches = Vec::new();
    let mut diagnostics = Vec::new();
    for form in &file.forms {
        let target = resolve_patch_selector(program, &form.target, scope);
        let expect = resolve_patch_selector(program, &form.expect.base, scope);
        match (target, expect) {
            (Ok(target), Ok(expect)) => patches.push(ResolvedPatch {
                target,
                expect,
                body: form.body.clone(),
                span: form.span,
            }),
            (target, expect) => {
                diagnostics.extend(target.err());
                diagnostics.extend(expect.err());
            }
        }
    }
    if diagnostics.is_empty() {
        Ok(patches)
    } else {
        Err(diagnostics)
    }
}

/// One selector of a patch form as `(stable id, facet)`.
fn resolve_patch_selector(
    program: &Program,
    sel: &ast::Selector,
    scope: &SelectorScope,
) -> Result<(StableId, Facet), Diagnostic> {
    let text = selector_text(sel);
    if matches!(sel.kind, SelectorKind::Namespace) {
        return Err(Diagnostic::error(
            codes::PATCH_FACET_UNSUPPORTED,
            codes::PATCH,
            format!(
                "`{text}` names a namespace; a patch replaces the body of one declaration (`<declaration>.body`, D39)"
            ),
        )
        .with_span(sel.span)
        .with_data("selector", json!(text)));
    }
    let def = match resolve_selector(program, sel, scope) {
        Ok(selected) if selected.len() == 1 => selected[0].def,
        resolved => {
            let mut diagnostic = Diagnostic::error(
                codes::PATCH_UNKNOWN_SYMBOL,
                codes::PATCH,
                format!("`{text}` names no declaration of the patched program"),
            )
            .with_span(sel.span)
            .with_data("selector", json!(text));
            if let Err(cause) = resolved {
                diagnostic = diagnostic.with_note(cause.message, cause.span);
            }
            return Err(diagnostic);
        }
    };
    let declaration = program.def(def);
    let facet = match &sel.kind {
        SelectorKind::Facet(f) => match FacetSelector::parse(f.kind.as_str()) {
            Some(FacetSelector::Facet(facet)) => Some(facet),
            _ => None,
        },
        _ => None,
    };
    match facet {
        Some(facet) => Ok((declaration.stable_id.clone(), facet)),
        None => Err(Diagnostic::error(
            codes::PATCH_FACET_UNSUPPORTED,
            codes::PATCH,
            format!(
                "`{text}` names the whole declaration `{}`; a patch replaces its `body` facet only (`{}.body`, D39)",
                program.display_path(def),
                sel.path.dotted()
            ),
        )
        .with_span(sel.span)
        .with_symbol(
            declaration.stable_id.clone(),
            program.display_path(def),
            None,
        )
        .with_data("selector", json!(text))),
    }
}

/// The checks of Section 5.5 and 10.5 for one patch, in this order:
///
/// 1. the target and expected pairs are equal and name a `body` facet,
///    else `patch_facet_unsupported` (D39) and nothing else is checked;
/// 2. `expected = baseline[id].body`: an id absent from the baseline is
///    `patch_unknown_symbol`, and a declaration without a `body` facet
///    `patch_facet_unsupported`, and nothing else is checked;
/// 3. `current = current[id].body`: absent is `patch_target_missing`;
/// 4. `current != expected` is `patch_stale` with both hashes (`expected`
///    and `actual` in the payload);
/// 5. with a grant: `(id, body)` outside `grant.write` is
///    `patch_out_of_scope` (the patch escapes the write set, design line
///    415), and `(id, body)` in `grant.preserve` with a patch body that
///    differs from the baseline's is `patch_preserved_facet`.
///
/// `current` is the facet map of the tree being patched: the baseline at
/// task-level validation, the target head at integration (5.5). Without a
/// grant (a patch submitted outside any task) only steps 1-4 run. Every
/// diagnostic spans the patch form and blames the target facet, with the
/// target's `symbol` and `qualified_name` (S49); a target that is in
/// neither map and not provisional has no qualified name to give, so its
/// id is the payload's `target` and no facet is set.
pub fn validate_patch(
    patch: &ResolvedPatch,
    baseline: &FacetMap,
    current: &FacetMap,
    grant: Option<&Grant>,
) -> Vec<Diagnostic> {
    let (id, facet) = &patch.target;
    let blamed = |d: Diagnostic| blame_target(d, patch, &[baseline, current]);
    if patch.target != patch.expect {
        return vec![blamed(Diagnostic::error(
            codes::PATCH_FACET_UNSUPPORTED,
            codes::PATCH,
            format!(
                "the patch replaces the {} of `{}` but its `expect base` names the {} of `{}`; both must name the same `body` facet (D39)",
                facet,
                display(id, &[baseline, current]),
                patch.expect.1,
                display(&patch.expect.0, &[baseline, current]),
            ),
        ))];
    }
    if *facet != Facet::Body {
        return vec![blamed(facet_unsupported(
            *facet,
            &display(id, &[baseline, current]),
        ))];
    }
    let Some(entry) = baseline.get(id) else {
        return vec![blamed(
            Diagnostic::error(
                codes::PATCH_UNKNOWN_SYMBOL,
                codes::PATCH,
                format!("`{}` is not in the task's baseline", id),
            )
            .with_data("target", json!(id)),
        )];
    };
    let Some(expected) = entry.facet(Facet::Body) else {
        return vec![blamed(Diagnostic::error(
            codes::PATCH_FACET_UNSUPPORTED,
            codes::PATCH,
            format!(
                "`{}` is a `{}` declaration, which has no `body` facet; a patch replaces a body (D39)",
                entry.version.path,
                entry.kind.prefix()
            ),
        ))];
    };
    let path = entry.version.path.as_str();
    let mut diagnostics = Vec::new();
    match current.get(id).and_then(|e| e.facet(Facet::Body)) {
        None => diagnostics.push(blamed(Diagnostic::error(
            codes::PATCH_TARGET_MISSING,
            codes::PATCH,
            format!("`{path}` is not in the tree being patched"),
        ))),
        Some(actual) if actual != expected => diagnostics.push(blamed(
            Diagnostic::error(
                codes::PATCH_STALE,
                codes::PATCH,
                format!(
                    "the body of `{path}` is not the one the patch expects: `expect base` names body {expected}, the tree being patched has body {actual}"
                ),
            )
            .with_data("expected", json!(expected))
            .with_data("actual", json!(actual)),
        )),
        Some(_) => {}
    }
    if let Some(grant) = grant {
        if !grant.may_write(id, Facet::Body) {
            diagnostics.push(blamed(Diagnostic::error(
                codes::PATCH_OUT_OF_SCOPE,
                codes::PATCH,
                format!("the patch replaces the body of `{path}`, which is outside the task's write set"),
            )));
        }
        let applied = patch.body_hash();
        if grant.preserve.contains(&(id.clone(), Facet::Body)) && applied != *expected {
            diagnostics.push(blamed(
                Diagnostic::error(
                    codes::PATCH_PRESERVED_FACET,
                    codes::PATCH,
                    format!("the patch changes the body of `{path}`, which the task must preserve"),
                )
                .with_data("expected", json!(expected))
                .with_data("actual", json!(applied)),
            ));
        }
    }
    diagnostics
}

/// Replace the target's body in `tree` with the patch block (Section 10.5):
/// locate the target declaration (the first declaration with the target's
/// stable identity, in path order, of the `.cpl` files of `project`, which
/// must be the project loaded from `tree`), replace the bytes of its block,
/// `{` to `}`, with [`ResolvedPatch::body_text`] re-indented to the
/// declaration's level, and leave every other byte of the tree unchanged.
///
/// Re-indentation prefixes every line after the first with the whitespace
/// that starts the declaration's line (nothing, for the top-level
/// declarations of a formatted file), and blank lines stay empty. The
/// block's line terminators follow the file: CRLF when the line on which
/// the block starts ends with CRLF, else LF (the formatter's output, rule
/// 1). The body of a `test` or `property` is replaced the same way: they
/// have a `body` facet too (Section 5.1).
///
/// Errors: a target facet other than `body`, or a declaration kind without
/// a body, is `patch_facet_unsupported`, blaming the target's facet with its
/// `symbol` and `qualified_name` when the tree declares it (S49), else
/// carrying its id as `target`; a target that is not declared, or
/// a bodiless `fn`, is `patch_target_missing`; a `project` whose text of
/// the target's file differs from `tree` is `internal_error`. Type-checking
/// the patched body in the target's scope is the caller's next step:
/// loading the patched tree.
pub fn apply_patch(
    tree: &VirtualTree,
    project: &Project,
    patch: &ResolvedPatch,
) -> Result<VirtualTree, Diagnostic> {
    let (id, facet) = &patch.target;
    let found = find_declaration(project, id);
    if *facet != Facet::Body {
        // Blame the target facet with its declaration (S49: `facet` only
        // together with `symbol` and `qualified_name`); an undeclared target
        // has no qualified name, so its id is the payload's `target`.
        let diagnostic = match &found {
            Some((_, _, name)) => {
                facet_unsupported(*facet, name).with_symbol(id.clone(), name.clone(), Some(*facet))
            }
            None => facet_unsupported(*facet, id.as_str()).with_data("target", json!(id)),
        };
        return Err(diagnostic.with_span(patch.span));
    }
    let Some((file, decl, qualified_name)) = found else {
        return Err(Diagnostic::error(
            codes::PATCH_TARGET_MISSING,
            codes::PATCH,
            format!("`{id}` is not declared in the tree being patched"),
        )
        .with_span(patch.span)
        .with_data("target", json!(id)));
    };
    let source = project.source_map.file(file);
    let blamed = |d: Diagnostic| {
        d.with_span(patch.span)
            .with_symbol(id.clone(), qualified_name.clone(), Some(Facet::Body))
    };
    let (block, keyword) = match &decl.kind {
        DeclKind::Fn(f) => match &f.body {
            Some(body) => (body, f.span.start),
            None => {
                return Err(blamed(Diagnostic::error(
                    codes::PATCH_TARGET_MISSING,
                    codes::PATCH,
                    format!(
                        "`{}` is a bodiless `fn`: there is no block to replace",
                        decl.display_name()
                    ),
                )))
            }
        },
        DeclKind::Test(t) => (&t.body, t.span.start),
        DeclKind::Property(p) => (&p.body, p.span.start),
        _ => {
            return Err(blamed(Diagnostic::error(
                codes::PATCH_FACET_UNSUPPORTED,
                codes::PATCH,
                format!(
                    "`{}` has no `body` facet; a patch replaces a body (D39)",
                    decl.display_name()
                ),
            )))
        }
    };
    let text = source.text();
    let path = &source.path;
    if tree.files.get(path).map(Vec::as_slice) != Some(text.as_bytes()) {
        return Err(internal(format!(
            "`{path}` of the project is not the file of the tree being patched; load the project from that tree"
        )));
    }
    let (start, end) = (block.span.start as usize, block.span.end as usize);
    let spliced = text
        .get(start..end)
        .is_some_and(|b| b.starts_with('{') && b.ends_with('}'));
    if !spliced {
        return Err(internal(format!(
            "the body span {start}..{end} of `{path}` is not a block"
        )));
    }
    let indent = line_indent(text, keyword as usize);
    let terminator = line_terminator(text, start);
    let replacement = reindent(&patch.body_text(), indent, terminator);
    let mut patched_text = String::with_capacity(text.len() - (end - start) + replacement.len());
    patched_text.push_str(&text[..start]);
    patched_text.push_str(&replacement);
    patched_text.push_str(&text[end..]);
    let mut patched = tree.clone();
    patched
        .files
        .insert(path.clone(), patched_text.into_bytes());
    Ok(patched)
}

/// Parse a submitted `.patch` file (Section 10.5): its patch forms, or
/// `patch_syntax` when it does not parse. The parser stops at the first
/// error (T6), so nothing from such a file is applied; the diagnostic spans
/// that error and its note carries the parse diagnostic (`<code>: <message>`
/// at its span).
pub fn parse_submitted_patch(text: &str, file: FileId) -> Result<ast::PatchFile, Diagnostic> {
    let result = parse_patch(text, file);
    if let Some(patch_file) = result.value {
        return Ok(patch_file);
    }
    let mut diagnostic = Diagnostic::error(
        codes::PATCH_SYNTAX,
        codes::PATCH,
        "the submitted patch file does not parse; nothing from it is applied",
    );
    if let Some(parse) = result.diagnostics.into_iter().next() {
        if let Some(span) = parse.span {
            diagnostic = diagnostic.with_span(span);
        }
        diagnostic = diagnostic.with_note(format!("{}: {}", parse.code, parse.message), parse.span);
    }
    Err(diagnostic)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The qualified name of `id`: the path of the first map that has it, else
/// the id itself when it is provisional (a provisional identity is the
/// qualified name, semantics S2), else unknown.
fn qualified_name(id: &StableId, maps: &[&FacetMap]) -> Option<String> {
    maps.iter()
        .find_map(|map| map.get(id).map(|e| e.version.path.clone()))
        .or_else(|| id.is_provisional().then(|| id.to_string()))
}

/// The display path of `id`: its qualified name, else the id.
fn display(id: &StableId, maps: &[&FacetMap]) -> String {
    qualified_name(id, maps).unwrap_or_else(|| id.to_string())
}

/// Span the patch form and blame the target facet (1.9): `symbol`,
/// `qualified_name`, and `facet` together. S49 allows `facet` only with
/// `symbol` and `symbol` only with `qualified_name`, so a target whose
/// qualified name neither map knows (and that is not provisional) is named
/// by the payload's `target`, its id, and nothing is blamed.
fn blame_target(diagnostic: Diagnostic, patch: &ResolvedPatch, maps: &[&FacetMap]) -> Diagnostic {
    let id = &patch.target.0;
    let diagnostic = diagnostic.with_span(patch.span);
    match qualified_name(id, maps) {
        Some(path) => diagnostic.with_symbol(id.clone(), path, Some(patch.target.1)),
        None => diagnostic.with_data("target", json!(id)),
    }
}

/// `patch_facet_unsupported` for a target facet other than `body` of the
/// declaration `name`; the caller blames the target.
fn facet_unsupported(facet: Facet, name: &str) -> Diagnostic {
    Diagnostic::error(
        codes::PATCH_FACET_UNSUPPORTED,
        codes::PATCH,
        format!(
            "a patch replaces a `body` facet only (D39); this one targets the `{facet}` facet of `{name}`"
        ),
    )
}

fn internal(message: String) -> Diagnostic {
    Diagnostic::error(codes::INTERNAL_ERROR, Category::Internal, message)
}

/// The first declaration of the project's source-set `.cpl` files (path
/// order, T15) whose stable identity is `id`, the occurrence a facet map
/// keeps (Section 5.4), with its file and qualified name.
fn find_declaration<'p>(
    project: &'p Project,
    id: &StableId,
) -> Option<(FileId, &'p ast::Declaration, String)> {
    project
        .files
        .iter()
        .zip(&project.parsed)
        .filter(|(entry, _)| entry.origin != FileOrigin::Std)
        .find_map(|(entry, parsed)| {
            let ParsedFile::Module(module) = parsed else {
                return None;
            };
            let module_path: Vec<String> = module
                .header
                .path
                .segments
                .iter()
                .map(|s| s.text.clone())
                .collect();
            module
                .declarations
                .iter()
                .find(|decl| declaration_identity(&module_path, decl) == *id)
                .map(|decl| {
                    let name = symbol_path(&module_path, decl).display();
                    (entry.file, decl, name)
                })
        })
}

/// The spaces and tabs that start the line containing `offset`.
fn line_indent(text: &str, offset: usize) -> &str {
    let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let line = &text[line_start..];
    let width = line.len() - line.trim_start_matches([' ', '\t']).len();
    &line[..width.min(offset - line_start)]
}

/// The line terminator of the line containing `offset` (CRLF or LF), else
/// of the line before it, else LF.
fn line_terminator(text: &str, offset: usize) -> &'static str {
    let bytes = text.as_bytes();
    let crlf = |newline: usize| newline > 0 && bytes[newline - 1] == b'\r';
    let newline = text[offset..]
        .find('\n')
        .map(|i| offset + i)
        .or_else(|| text[..offset].rfind('\n'));
    match newline {
        Some(i) if crlf(i) => "\r\n",
        _ => "\n",
    }
}

/// `text` (LF-separated) with `terminator` between its lines and `indent`
/// before every line but the first, blank lines left empty.
fn reindent(text: &str, indent: &str, terminator: &str) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push_str(terminator);
            if !line.is_empty() {
                out.push_str(indent);
            }
        }
        out.push_str(line);
    }
    out
}
