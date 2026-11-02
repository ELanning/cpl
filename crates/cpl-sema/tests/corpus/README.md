# cpl-sema typecheck corpus

The test corpus for `crates/cpl-sema`, written from `spec/semantics.md`,
`spec/grammar.md`, `spec/toolchain.md` (Section 12), and `spec/stdlib.md`
before the checker existed. Every file is a complete module; every
expectation uses the marker forms of toolchain 12.2 (decision T38):

```text
let n: Text = 7 // expect-error: type_mismatch          trailing: the line where the primary span starts
// expect-error@+1: unresolved_name                      the line N lines below
// expect-error(global): reserved_module_path            a diagnostic without a span
```

`expect-warning` and `expect-info` take the same forms. The harness,
`crates/cpl-sema/tests/corpus.rs`, compares the exact multiset of (file,
line, severity, code) per case; an unexpected diagnostic or an unmatched
marker fails the case, and so does an error in a positive (`ok/`) case,
which toolchain 12's positive row requires to check with zero errors, and
a diagnostic whose code is in no code table (semantics 9.4, toolchain
Appendix B, the `parse` codes) or whose category is not that table's.

## Layout

| Path | Contents | Checked with |
| - | - | - |
| `ok/*.cpl` | positive single-file cases: zero errors; deliberate warnings and infos are marked | `check` (development mode) |
| `ok/<case>/` | positive directory cases (`cpl.toml` + `src/`): `variant_resolution/`, `multi_file_module/` | `check` |
| `errors/<category>/*.cpl` | negative single-file cases, one directory per 9.4 category (`resolve`, `type`, `effect`, `contract`, `hole`, `test`) | `check` |
| `errors/project/<case>/` | negative directory cases needing their own `cpl.toml` or several files | `check` |
| `errors/project/c_target/<code>/` | Milestone 2: one negative directory case per `project_*` code of the C target that `cpl check` reports (toolchain 11.1, T74), grouped one level down so that `crates/cpl-project/tests/corpus.rs`, a Milestone 1 test that counts the direct cases of `errors/project/`, stays unchanged; the harness recurses into the group | `check` |
| `errors/project/python_target/<code>/` | Milestone 3: one negative directory case per `project_*` code of the Python target and the test fixtures (`spec/python.md`, toolchain 11.1), grouped like `c_target/` | `check` |
| `release/<case>/` | directory cases checked in release mode; see `release/README.md` | `check_with(release = true)` |

toolchain 12 names the negative tree `crates/cpl-sema/tests/corpus/errors/**`
and the positive tree `ok/**`; this corpus uses exactly those names.

### How the harness must load a case

1. **Single file.** Wrap it in a `VirtualTree` with the synthetic manifest
   toolchain 12.2 describes (`name = "errors"` or `"ok"`, `edition = "0.1"`),
   placing the file at the conventional path of its `module` header
   (`module negative.x` -> `src/negative/x.cpl`), so that no
   `module_file_misplaced` warning is produced for the wrapper itself.
2. **Directory.** Load the directory as the project. Markers may appear in
   any `.cpl` file of the case. A diagnostic whose span lies in a file that is
   not `.cpl` (the manifest) is matched by a `(global)` marker in any `.cpl`
   file of the case.
3. **Release.** `release/<case>/` is loaded as a directory and checked with
   `CheckConfig { release: true, .. }`. No case carries a `cpl.lock`, so the
   span-less warning `project_lock_missing` is expected in every release
   case.
4. Items 1-3 are now the normative harness conventions of toolchain 12.2
   (T38, amended by the rulings of 2026-10-28). The former `ambiguous/`
   trees were dissolved by those rulings (last section); every case is in
   the main tree and states its ruling in a header comment beginning
   `Ruling`.

### Span conventions the markers encode

Primary spans are fixed by semantics S63 [D-230]; the bullets below restate
that decision as the corpus applies it.

- A diagnostic about an expression starts at that expression (the operand
  that fails a check, the call, the constructor, the hole). Offending
  expressions are kept on one line with their marker.
- A diagnostic about a clause (`requires`, `ensures`, `invariant`,
  `decreases`, `effects`, `where`, `for`) starts at the clause line:
  `unused_decreases`, `effect_in_contract`, `recursive_contract`,
  `private_in_public_contract`, `statement_in_contract` (the statement line),
  `result_outside_ensures`, `type_mismatch` on a clause.
- A diagnostic about a declaration starts at its header line (`fn`, `pub
  struct`, `type`, `test`): `duplicate_declaration`, `duplicate_identity`
  (S63 (3): the later declaration's name token, not its `@id` line),
  `missing_body`, `prelude_name_redefined`, `unused_type_parameter`, `infinite_type`,
  `capability_in_value_position` on a return type, `using_param_not_capability`,
  `unused_capability`, `self_bound` on a parameter. Headers that carry such
  a marker are written on one line; annotation-level diagnostics
  (`unknown_annotation`, `duplicate_annotation`, `invalid_annotation_argument`,
  `annotation_misplaced`) start at the annotation line.
- Two-location diagnostics (`duplicate_declaration`, `duplicate_identity`,
  `duplicate_import`, `duplicate_test_name`, `duplicate_field`, `duplicate_variant`,
  ...) have one diagnostic whose primary span is the *later* occurrence in
  (file path, item/statement order) and whose `notes` carry the earlier one,
  as 9.4's field list (`name`, `notes (other location)`) implies.
  Exceptions where the subject is unambiguous: `import_conflicts_with_declaration`
  starts at the import, `shadowed_binding` at the shadowing binding,
  `assign_to_immutable` at the assignment.
- Span-less (`(global)`): only diagnostics without a source location, here
  every `project_*` manifest diagnostic and `project_lock_missing` (S63
  (6)). Declaration-level diagnostics span the declaration's name token, so
  `verification_result`, `verification_unsatisfied`, and the function form
  of `missing_decreases` are marked on the `fn` line and
  `reserved_module_path` on the `module` line (S63 (3)).
- A `use` item that is itself diagnosed (`unresolved_module`,
  `unresolved_name`, `private_item`, `duplicate_import`, `generator_import`,
  `import_conflicts_with_declaration`) produces no `unused_import` warning
  (S62); the corpus never uses such a name afterwards.
- An expression whose type is erroneous takes part in no further check (no
  cascades, S62 [D-229]; a check on a *position*, such as a generator
  outside a `for` range, still fires); bindings that would otherwise be
  reported `unused_binding` are `_`-prefixed in negative files.

## Coverage

Codes of semantics 9.4 in the categories `resolve`, `type`, `effect`,
`contract`, `hole`, `test`, `release_policy`, plus the `project` codes of
toolchain Appendix B that a single project can trigger. "F" marks a runtime
fault that `cpl check` never emits.

### resolve (46 codes; 44 covered)

| Code | Files |
| - | - |
| `unsupported_edition` | `errors/project/unsupported_edition/` |
| `reserved_module_path` | `errors/resolve/reserved_module_path.cpl` |
| `module_file_misplaced` (W) | `errors/project/module_file_misplaced/` |
| `duplicate_declaration` | `errors/resolve/duplicate_declaration.cpl`, `duplicate_declaration_type_vs_fn.cpl`, `errors/project/duplicate_across_files/` |
| `duplicate_identity` | `errors/resolve/duplicate_identity.cpl` (the `@id`-equals-qualified-name form is impossible: toolchain 4.3, an accepted `@id` never contains `.`) |
| `duplicate_import` | `errors/resolve/duplicate_import.cpl`, `duplicate_import_single_form.cpl` |
| `import_conflicts_with_declaration` | `errors/resolve/import_conflicts_with_declaration.cpl` |
| `prelude_name_redefined` | `errors/resolve/prelude_name_redefined_fn.cpl`, `prelude_name_redefined_reserved.cpl` |
| `generator_import` | `errors/resolve/generator_import.cpl` |
| `missing_body` | `errors/resolve/missing_body.cpl` (grammar 3.9's bodiless `fn_tail` and 3.5's `opaque_decl`, G1/G2 of stdlib 10.2; the current parser implements both, so the file parses) |
| `unresolved_module` | `errors/resolve/unresolved_module_use.cpl`, `unresolved_module_chain.cpl` |
| `unresolved_name` | `errors/resolve/unresolved_name_local.cpl`, `unresolved_name_import.cpl`, `unresolved_name_qualified.cpl`, `unresolved_name_constructor.cpl`, `unresolved_name_property_binder_in_range.cpl`, `assign_to_immutable.cpl` |
| `unresolved_type` | `errors/resolve/unresolved_type.cpl` |
| `private_item` | `errors/project/private_item/` (by `use` and by qualified path) |
| `private_type_in_public_signature` | `errors/resolve/private_type_in_public_signature.cpl` (fn params/return, struct, enum, wrapper, refinement base), `private_type_in_public_signature_capability.cpl` (`using` type, operation types) |
| `private_in_public_contract` | `errors/resolve/private_in_public_contract.cpl` |
| `unused_import` (W) | `errors/resolve/unused_import.cpl`, `ok/warnings.cpl` |
| `unused_binding` (W) | `errors/resolve/unused_binding.cpl`, `ok/warnings.cpl`, `ok/unused_var_assigned_only.cpl` (write-only `var`, S67); no warning for parameters or `for` binders: `ok/property_binder_unused.cpl` |
| `shadowed_binding` | `errors/resolve/shadowed_binding_parameter.cpl`, `shadowed_binding_nested.cpl` |
| `duplicate_pattern_binding` | `errors/resolve/duplicate_pattern_binding.cpl` |
| `ambiguous_constructor` | `errors/resolve/ambiguous_constructor.cpl`, `ambiguous_constructor_prelude.cpl` |
| `not_a_constructor` | `errors/resolve/not_a_constructor.cpl` |
| `not_a_value` | `errors/resolve/not_a_value.cpl` |
| `unknown_annotation` | `errors/resolve/unknown_annotation.cpl` |
| `annotation_misplaced` | `errors/resolve/annotation_misplaced.cpl` |
| `duplicate_annotation` | `errors/resolve/duplicate_annotation.cpl` |
| `invalid_annotation_argument` | `errors/resolve/invalid_annotation_argument.cpl` |
| `result_outside_ensures` | `errors/resolve/result_outside_ensures.cpl` |
| `result_name_reserved` | `errors/resolve/result_name_reserved.cpl` |
| `self_outside_where` | `errors/resolve/self_outside_where.cpl` |
| `self_bound` (W) | `errors/resolve/self_bound.cpl`, `ok/warnings.cpl` |
| `assign_to_immutable` | `errors/resolve/assign_to_immutable.cpl` |
| `return_outside_function` | `errors/resolve/return_outside_function.cpl` |
| `propagate_outside_function` | `errors/resolve/propagate_outside_function.cpl` |
| `duplicate_field` | `errors/resolve/duplicate_field.cpl` (declaration and literal) |
| `unknown_field` | `errors/resolve/unknown_field.cpl` |
| `missing_field` | `errors/resolve/missing_field.cpl` |
| `duplicate_variant`, `duplicate_operation`, `duplicate_parameter`, `duplicate_type_parameter` | `errors/resolve/duplicate_members.cpl` |
| `unused_type_parameter` | `errors/resolve/unused_type_parameter.cpl` (error on struct/enum, warning on fn), `ok/warnings.cpl` |
| `cyclic_type_definition` | `errors/resolve/cyclic_type_definition.cpl`, `cyclic_type_definition_mutual.cpl` (every declaration of the cycle, S66) |
| `infinite_type` | `errors/resolve/infinite_type.cpl` |
| `cannot_run_effectful`, `cannot_run_generic` | **not covered**: emitted only by `cpl run` (S39), never by `cpl check` |

### type (29 codes; 28 covered, 1 not triggerable)

| Code | Files |
| - | - |
| `type_mismatch` | `errors/type/type_mismatch_let_annotation.cpl`, `type_mismatch_if_branches.cpl`, `type_mismatch_match_arms.cpl`, `type_mismatch_list_elements.cpl`, `type_mismatch_struct_fields.cpl`, `type_mismatch_call_arguments.cpl`, `type_mismatch_return.cpl`, `type_mismatch_operators.cpl`, `type_mismatch_equality.cpl`, `type_mismatch_assignment.cpl`, `type_mismatch_refinement_invariance.cpl`, `type_mismatch_wrapper.cpl`, `type_mismatch_contract_clause.cpl`, `type_mismatch_using_argument.cpl`, `type_mismatch_test_tail.cpl`, `type_mismatch_missing_tail.cpl` (the block's span, S63), `if_statement_non_unit_branches.cpl` (S68) |
| `cannot_infer_type` | `errors/type/cannot_infer_type.cpl` |
| `cannot_prove_refinement` | `errors/type/cannot_prove_refinement.cpl` (names, calls, arithmetic over names, `let`-bound literal), `cannot_prove_refinement_predicate_call.cpl` (predicate outside the evaluable subset), `cannot_prove_refinement_distinct_refinement.cpl` (two refinements of one root base, S65), `cannot_prove_refinement_propagate.cpl` (`?` in a function whose return type is a refinement, S73) |
| `refinement_false` | `errors/type/refinement_false.cpl` (Int, list, option, chain, test, and the `Unit` of a bare `return` and of a missing tail at a refinement of `Unit`, 3.10, 3.11) |
| `constant_evaluation_fault` | `errors/type/constant_evaluation_fault.cpl` (fault in the constant and in the predicate) |
| `check_requires_type_argument` | `errors/type/check_requires_type_argument.cpl` |
| `check_requires_refinement` | `errors/type/check_requires_refinement.cpl` |
| `type_argument_count_mismatch` | `errors/type/type_argument_count_mismatch.cpl` |
| `not_a_value_type` | **not triggerable** in 0.1: a capability in any value position is `capability_in_value_position` (semantics 1.8 as amended, ruling 2) and `Gen` cannot be written |
| `arity_mismatch` | `errors/type/arity_mismatch.cpl` |
| `using_arity_mismatch` | `errors/type/using_arity_mismatch.cpl` |
| `not_callable` | `errors/type/not_callable.cpl` |
| `unknown_member` | `errors/type/unknown_member.cpl` |
| `property_called` | `errors/type/property_called.cpl` (`xs.len()`, `page.text.len()`, `gen.int()`) |
| `method_not_called` | `errors/type/method_not_called.cpl` (`xs.is_empty`, `xs.push`, `n.abs`, `t.is_empty`) |
| `unused_value` | `errors/type/unused_value.cpl` |
| `non_exhaustive_match` | `errors/type/non_exhaustive_match_enum.cpl`, `non_exhaustive_match_option_result.cpl`, `non_exhaustive_match_nested.cpl`, `non_exhaustive_match_literals.cpl` (Bool, Int, Text, no arms) |
| `unreachable_arm` (W) | `errors/type/unreachable_arm.cpl`, `ok/warnings.cpl` |
| `pattern_type_mismatch` | `errors/type/pattern_type_mismatch.cpl` |
| `pattern_arity_mismatch` | `errors/type/pattern_arity_mismatch.cpl` |
| `propagate_not_result` | `errors/type/propagate_not_result.cpl` |
| `propagate_return_mismatch` | `errors/type/propagate_return_mismatch.cpl` |
| `propagate_error_mismatch` | `errors/type/propagate_error_mismatch.cpl` |
| `return_value_missing` | `errors/type/return_value_missing.cpl` |
| `comparison_unsupported` | `errors/type/comparison_unsupported.cpl` |
| `forall_range_not_list` | `errors/type/forall_range_not_list.cpl` |
| `for_range_not_generator` | `errors/type/for_range_not_generator.cpl` |
| `generator_outside_for` | `errors/type/generator_outside_for.cpl`, `generator_equality.cpl` (both operands, S62) |
| `unreachable_statement` (W) | `errors/type/unreachable_statement.cpl` (statements and tails, nested blocks), `ok/warnings.cpl` |

### effect (11 codes; 11 covered)

| Code | Files |
| - | - |
| `using_param_not_capability` | `errors/effect/using_param_not_capability.cpl` |
| `effect_param_not_capability` | `errors/effect/effect_param_not_capability.cpl` |
| `unknown_effect_operation` | `errors/effect/unknown_effect_operation.cpl` |
| `duplicate_effect` (W) | `errors/effect/duplicate_effect.cpl`, `ok/warnings.cpl` |
| `unused_capability` (W) | `errors/effect/unused_capability.cpl`, `ok/warnings.cpl` |
| `undeclared_effect` | `errors/effect/undeclared_effect_direct.cpl` (no clause, empty clause, other operation), `undeclared_effect_forwarding.cpl` (forwarding, partial bound, two handles) |
| `effect_in_contract` | `errors/effect/effect_in_contract.cpl` (requires, ensures, invariant) |
| `capability_as_value` | `errors/effect/capability_as_value.cpl` (let, list, argument, payload, tail, scrutinee, assert, callee, return), `contract_call_with_using_argument.cpl` (a `using` argument in a contract, also as `(db)`, where it is the only report: no `using_argument_not_capability` and no `undeclared_effect` beside it, 5.1, S62) |
| `capability_equality` | `errors/effect/capability_equality.cpl` |
| `capability_in_value_position` | `errors/effect/capability_in_value_position.cpl` (parameter, return, field, payload), `capability_in_value_position_operation.cpl` (operation parameter and return) |
| `using_argument_not_capability` | `errors/effect/using_argument_not_capability.cpl` |

### contract (9 codes; 7 covered, 2 runtime)

| Code | Files |
| - | - |
| `hole_in_contract` | `errors/contract/hole_in_contract.cpl` (the error alone, no `hole` info, S64) |
| `propagate_in_contract` | `errors/contract/propagate_in_contract.cpl` |
| `statement_in_contract` | `errors/contract/statement_in_contract.cpl` (var, assert, return, while) |
| `recursive_contract` | `errors/contract/recursive_contract.cpl` (requires, ensures, decreases) |
| `unused_decreases` (W) | `errors/contract/unused_decreases.cpl`, `ok/warnings.cpl` |
| `missing_decreases` (W) | `errors/contract/missing_decreases.cpl` (direct recursion, mutual recursion, loop) |
| `contract_violation` (F), `contract_counterexample` | **not covered**: raised by `cpl-eval` at run or test time |
| `verification_result` (I) | `errors/contract/missing_decreases.cpl`, `ok/annotations_and_docs.cpl`, `release/verification_unsatisfied/` |

### hole (3 codes; 2 covered, 1 runtime)

| Code | Files |
| - | - |
| `hole` (I; W in test bodies under release) | `ok/holes.cpl` (every check-mode position of S56), `release/hole_in_release/` |
| `hole_type_unknown` | `errors/hole/hole_type_unknown.cpl` (every infer-only position of S56; the error alone, S64) |
| `unresolved_hole` (F) | **not covered**: runtime fault |

### test (5 codes; 2 covered)

| Code | Files |
| - | - |
| `empty_test_name` | `errors/test/empty_test_name.cpl` |
| `duplicate_test_name` | `errors/test/duplicate_test_name.cpl`, `errors/project/duplicate_test_across_files/` |
| `empty_test_selection` | **not covered**: a `cpl test` configuration error |
| `test_result`, `assertion_failed` (F) | **not covered**: produced by the test runner |

### release_policy (2 codes; 2 covered)

| Code | Files |
| - | - |
| `hole_in_release` | `release/hole_in_release/` |
| `verification_unsatisfied` | `release/verification_unsatisfied/` |

### project (toolchain Appendix B; 26 covered)

| Code | Files |
| - | - |
| `project_manifest_unknown_key` | `errors/project/project_manifest_unknown_key/` |
| `project_source_dir_missing` | `errors/project/project_source_dir_missing/` |
| `project_binding_unknown_capability` | `errors/project/project_binding_unknown_capability/` |
| `project_binding_target_not_enabled` | `errors/project/project_binding_target_not_enabled/` |
| `project_binding_bad_isolation` | `errors/project/project_binding_bad_isolation/` |
| `project_binding_bad_backend` | `errors/project/project_binding_bad_backend/` (alone, without `project_binding_target_not_enabled`, T48) |
| `project_lock_missing` (W) | every `release/` case |
| `project_manifest_invalid`, `project_lock_invalid` | **not covered**: fatal `LoadError`s (exit 2), no diagnostic list to match |
| `project_no_sources` | **not covered**: a project with no `.cpl` file has no file to carry a marker |
| `project_duplicate_file_module` | not in this corpus: a file reachable from two overlapping `[build] sources` directories is loaded once (toolchain 11.1), so it duplicates no declaration and the case is the one span-less error; covered by `crates/cpl-project/tests/load.rs` (loading) and `crates/cpl-sema/tests/driver.rs` (`check`) |
| `project_lock_stale`, `project_lock_toolchain_mismatch` | **not covered**: need a `cpl.lock` with toolchain-computed digests (the C build-input half of `project_lock_stale`, T75, is covered by `crates/cpl-sema/tests/interop.rs`) |
| `project_c_target_not_enabled` | `errors/project/c_target/project_c_target_not_enabled/` (Milestone 2; `[interop.c]` without `"c"` in `targets`) |
| `project_binding_name_invalid` | `errors/project/c_target/project_binding_name_invalid/` (Milestone 2) |
| `project_binding_isolation_unsupported` | retired from Milestone 3 (a C adapter may run in a worker, `spec/python.md` 6.6), so it has no case |
| `project_binding_adapter_kind` | `errors/project/c_target/project_binding_adapter_kind/` (Milestone 2; a `.h` adapter) |
| `project_binding_adapter_missing` | `errors/project/c_target/project_binding_adapter_missing/` (Milestone 2; the harness loads directory cases from disk, so the file checks of `check_c_bindings` run) |
| `project_binding_path_missing` | `errors/project/c_target/project_binding_path_missing/` (Milestone 2; a missing `sources` file) |
| `project_binding_c_options_misplaced` | `errors/project/c_target/project_binding_c_options_misplaced/` (Milestone 2; `[bindings.codec.c]` on a python binding) |
| `project_binding_duplicate_capability` | `errors/project/c_target/project_binding_duplicate_capability/` (Milestone 2) |
| `project_cbind_invalid` | `errors/project/c_target/project_cbind_invalid/` (Milestone 2; a `.cbind.toml` that is not UTF-8, rejected before the schema parser; the schema's own rules have their negative cases in `crates/cpl-project/tests/cbind.rs` and `crates/cpl-sema/tests/cbind.rs`) |
| `project_export_unknown`, `project_export_ambiguous` | `errors/project/c_target/project_export_unknown/`, `errors/project/c_target/project_export_ambiguous/` (Milestone 2; the bare name `size` of two modules) |
| `project_export_not_function`, `project_export_not_public`, `project_export_not_project` | `errors/project/c_target/project_export_not_function/` (a struct), `errors/project/c_target/project_export_not_public/`, `errors/project/c_target/project_export_not_project/` (`std.text.trim`) (Milestone 2) |
| `project_export_generic` | `errors/project/c_target/project_export_generic/` (Milestone 2; the note names the monomorphic caller) |
| `project_export_symbol_invalid`, `project_export_symbol_collision` | `errors/project/c_target/project_export_symbol_invalid/` (the reserved family `cases_type_*`), `errors/project/c_target/project_export_symbol_collision/` (two default symbols `cases_size`) (Milestone 2) |
| `project_export_int_unused` (W) | `errors/project/c_target/project_export_int_unused/` (Milestone 2) |
| `project_type_name_unknown` | `errors/project/c_target/project_type_name_unknown/` (Milestone 2) |
| `project_export_skipped_generic` (I) | **not covered**: `cpl build` reports it, never `cpl check` (toolchain 11.1, T74), so the corpus results of semantics Section 11 do not change; covered by `crates/cpl-sema/tests/interop.rs` (`InteropPlan::skipped_diagnostics`) |
| `project_python_target_not_enabled` | `errors/project/python_target/project_python_target_not_enabled/` (Milestone 3) |
| `project_python_option_invalid` | `errors/project/python_target/project_python_option_invalid/` (Milestone 3; a package named like a standard-library module, and a version that is no release segment) |
| `project_binding_adapter_invalid` | `errors/project/python_target/project_binding_adapter_invalid/` (Milestone 3) |
| `project_binding_timeout_invalid` | `errors/project/python_target/project_binding_timeout_invalid/` (Milestone 3; `timeout_ms = 0`, and one on an in-process C binding) |
| `project_fixture_binding_unknown`, `project_fixture_capability_mismatch` | `errors/project/python_target/project_fixture_binding_unknown/`, `errors/project/python_target/project_fixture_capability_mismatch/` (Milestone 3) |
| `id_kind_mismatch` (W), `id_missing` (lint) | not in scope; every hand-written `@id` in the corpus avoids the generated format so that `id_kind_mismatch` cannot fire |

The Milestone 2 cases (the C target, toolchain 11.1, T74), under
`errors/project/c_target/`, that enable
`"c"` set `[interop.c] prefix = "cases"`: with the C target enabled the
effective prefix must be a C prefix of at most 31 characters (a project
name such as `project_binding_isolation_unsupported` is not, and would be
the fatal `project_manifest_invalid`). Their adapter files (`adapters/*.c`)
are build inputs, never part of the source set, and `cpl check` reads only
their existence (and a declarative binding's text).

Summary: 105 codes in the seven semantics categories; 96 covered in the
main tree, 1 not triggerable in 0.1 (`not_a_value_type`, ruling 2), 8 not
triggerable by `cpl check` (`cannot_run_effectful`, `cannot_run_generic`,
`contract_violation`, `contract_counterexample`, `unresolved_hole`,
`empty_test_selection`, `test_result`, `assertion_failed`). The parser
does not have `project_*` or `parse` codes in this corpus's scope beyond
those listed.

## Positive coverage (`ok/`)

| File | Rules exercised |
| - | - |
| `literals_and_operators.cpl` | every literal and operator row of 3.8, precedence, methods on literals (S11), `++` on lists and texts (S21) |
| `list_members.cpl`, `int_methods.cpl`, `text_and_bytes.cpl`, `option_result_members.cpl` | the closed member tables of 3.9 and stdlib 2-5, `len` as a property, `ok_or(e)?` as the Option bridge, std.text/std.bytes by `use` and by qualified path |
| `generics_inference.cpl` | S19 argument-first calls and expected-first literals, explicit `generic_args`, empty-list contexts, generic structs and enums (S12), `==` on `T` (S13) |
| `refinements.cpl`, `refinement_same_class.cpl` | S16 constant proofs in every literal-like form, 3.4 rules 1/2/2', `check<R>` over chains, S20 first-element typing, erased equality (S17), member lookup through `strip`, predicates from the evaluable subset, S55 identity of a user `Positive` with `gen.positive_int` (`gen.Positive`, `std.gen.Positive` as type paths), the `Unit` of a bare `return` and of a missing tail proved against refinements of `Unit` (3.10, 3.11) |
| `wrappers_and_structs.cpl`, `enums_and_matching.cpl` | wrappers (S23), spread (D29), zero-field structs, S10 constructor resolution in all three forms, 3.12 patterns, S25 exhaustiveness for Bool/Unit/wrapper/nested/literal matches, `match` as statement and operand |
| `if_expressions.cpl`, `control_flow.cpl`, `divergence_and_recursion.cpl` | `if` forms incl. D41, statements of 3.11, `while` with invariants and `decreases`, S24 divergence (nested returns, diverging arms, empty enum), S1 order independence and mutual recursion |
| `contracts.cpl`, `quantifiers.cpl` | 5.1 clauses, S32 `let` sub-blocks, D66 clause order, operation contracts (S31), `forall` scoping and D45 extent |
| `capabilities.cpl` | 4.1-4.4: bounds, forwarding by position, two handles of one type, wide bounds without warning (S29), `effects {}` |
| `tests_and_properties.cpl` | every generator of stdlib 8.1, nested combinators, pure calls in ranges (S30), `std.gen` spelling |
| `holes.cpl` | the `hole` info in every check-mode position of S56 |
| `annotations_and_docs.cpl` | doc comments (module, declaration, member, interleaved with annotations), `////`, hand-written `@id`, `@prove` with measures (`verification_result`) |
| `contextual_names.cpl`, `prelude_and_qualification.cpl`, `equality_erasure.cpl`, `std_foreign.cpl`, `constraint_error.cpl`, `question_mark.cpl`, `layout_edge_cases.cpl` | S9 `result`/`self`, locals hiding universe names, redundant `use std.prelude`, fully qualified std paths, S17 erasure, stdlib 7 and 1.5 types, `?` in every position, grammar 2 layout forms |
| `warnings.cpl` | one of each warning a positive file may carry, all marked |
| `else_if_chain_diverges.cpl` | a body that diverges through an `else if` chain (S24 as amended, ruling 16) |
| `property_binder_unused.cpl` | no `unused_binding` for an unused parameter or property `for` binder (S67, ruling 15) |
| `unused_var_assigned_only.cpl` | `unused_binding` for a `var` that is only assigned (S67, ruling 15) |
| `variant_resolution/` | directory: constructors resolved by expected type with no import of the enum or wrapper, qualified forms through the module path |
| `multi_file_module/` | directory: one module in two files at the conventional paths, private cross-file visibility, file-scoped imports (S5), a client module |

Design-doc examples (`examples/reader`, `examples/holes`) are covered by
toolchain 12's positive row directly and are not duplicated here except
where a rule needed them as context.

## Rulings applied (2026-10-28)

Every ambiguity this corpus reported was ruled on by the specification
editor; the ids are those of `spec/decisions.md`, and each ruling is stated
in exactly one place in `spec/`. The former `ok/ambiguous/` and
`errors/ambiguous/` trees are dissolved: every case is in the main tree
with markers that match its ruling, and its header comment names the
ruling. Changes to cases that were never ambiguous are marked **changed**.

1. **Distinct refinements of one base** → `cannot_prove_refinement`, with a
   note quoting both predicates [D-232] (semantics S65; S55 and stdlib S29
   amended). `errors/type/cannot_prove_refinement_distinct_refinement.cpl`
   (was `errors/ambiguous/type_mismatch_distinct_refinement.cpl`, markers
   unchanged). **Changed**: the stale main-tree copy
   `errors/type/type_mismatch_distinct_refinement.cpl`, which expected
   `type_mismatch`, was removed.
2. **`not_a_value_type` vs `capability_in_value_position`** → the specific
   code in every value position, operation parameters and returns included
   (semantics 1.8 amended to follow S13/S27; no new decision).
   `errors/effect/capability_in_value_position_operation.cpl` (was
   `errors/ambiguous/not_a_value_type.cpl`; markers changed).
   `not_a_value_type` is not reachable in 0.1.
3. **`hole` beside `hole_type_unknown` / `hole_in_contract`** → the error
   only [D-231] (semantics S64). `errors/hole/hole_type_unknown.cpl`,
   `errors/contract/hole_in_contract.cpl` (markers unchanged).
4. **`hole` beside `hole_in_release`** → `hole_in_release` replaces `hole`
   [D-231] (S64; S47 amended). `release/hole_in_release/` unchanged.
5. **Spans of `verification_result` / `verification_unsatisfied`** → the
   function's name token [D-230] (semantics S63 (3); toolchain 7.2's
   example gained a `span`). **Changed**: markers moved from `(global)` to
   the `fn` line in `errors/contract/missing_decreases.cpl`,
   `ok/annotations_and_docs.cpl`, and
   `release/verification_unsatisfied/src/cases/proofs.cpl`;
   `release/README.md` updated.
6. **Span of `missing_decreases` on a function** → the function's name
   token [D-230] (5.4 amended); the loop form is unchanged. **Changed**:
   `errors/contract/missing_decreases.cpl` markers moved to the `fn` lines.
7. **Span of clause diagnostics** → the clause, from its keyword [D-230]
   (S63 (2)). Main tree unchanged.
8. **Two-location diagnostics** → one diagnostic at the later occurrence
   with the earlier in `notes` [D-230] (S63 (4); toolchain 3.7 amended).
   Main tree unchanged.
9. **`cyclic_type_definition` on a two-type cycle** → every declaration of
   the cycle [D-233] (S66).
   `errors/resolve/cyclic_type_definition_mutual.cpl` (unchanged).
10. **`gen.int == gen.int`** → two `generator_outside_for`, one per operand
    [D-229] (S62: a check on a position is not suppressed by an error
    elsewhere). `errors/type/generator_equality.cpl` (marker now lists the
    code twice).
11. **`if` statement with non-Unit branches** → one `type_mismatch` per
    branch tail [D-235] (S68).
    `errors/type/if_statement_non_unit_branches.cpl` (unchanged).
12. **Span of "missing tail expression"** → the body block [D-230] (S63
    (1)). `errors/type/type_mismatch_missing_tail.cpl` (unchanged).
13. **A call with `using` inside a contract** → `capability_as_value` only
    [D-229] (S62; 4.5 amended).
    `errors/effect/contract_call_with_using_argument.cpl` (2026-10-29: also
    `using (db)`, which 5.1 now names: `capability_as_value` alone, whatever
    the argument's form).
14. **`project_binding_bad_backend` vs `project_binding_target_not_enabled`**
    → the bad-backend error only [D-236] (toolchain T48).
    `errors/project/project_binding_bad_backend/` (unchanged).
15. **`unused_binding` for a write-only `var`, binders, parameters** →
    warns for the `var`; never for parameters or `for`/`forall` binders
    [D-234] (S67). `ok/unused_var_assigned_only.cpl`,
    `ok/property_binder_unused.cpl` (unchanged).
16. **Divergence through an `else if` chain** → the nested `if` counts (S24
    amended, D-101). `ok/else_if_chain_diverges.cpl` (unchanged).
17. **Failed imports and `unused_import`** → a diagnosed `use` item binds
    an erroneous name and is never `unused_import` [D-229] (S62). Main
    tree unchanged.
18. **`reserved_module_path` span** → the module path [D-230] (S63 (3)).
    **Changed**: `errors/resolve/reserved_module_path.cpl` now carries a
    trailing marker on the `module` line; toolchain 12.2's example uses
    `project_lock_missing` for the `(global)` form instead.
19. **`self_bound` for a parameter named `self`** → warns (S9 amended: a
    parameter is a binding). Main tree unchanged.
20. **`property_called` on a `for` range** → the property's type is kept, no
    `for_range_not_generator` [D-229] (S62). Main tree unchanged.
21. **`duplicate_parameter` counting `using` parameters**: no ruling
    needed; the positional form remains the tested one.
22. **`?` in a `where` predicate** → `propagate_in_contract` takes precedence
    over `propagate_outside_function` (semantics 5.1 amended). Not
    exercised here.
23. **`@property` cannot be lexed** → an annotation name may be any keyword
    token [D-227] (grammar D71, 3.3, 1.4); the current parser already
    accepts `@property`. `errors/resolve/unknown_annotation.cpl` keeps
    `@method` and marks `@property` as well (since 2026-10-29; its header
    comment no longer says that `@property` cannot lex).
24. **Bodiless `fn` / definition-less `type` (G1/G2)**: the grammar stands
    (this was an implementation gap, not a spec question); the current
    parser implements both forms and `errors/resolve/missing_body.cpl`
    parses.

Every `.cpl` file of this corpus was re-parsed with the current parser
(`cpl_syntax::parse_module`) after the re-homing of 2026-10-28: all 188
files of the sema and eval corpora parse with zero diagnostics,
`missing_body.cpl` included. The re-homed files differ from their earlier
versions only in their header comments and, where a ruling required it,
their markers.
