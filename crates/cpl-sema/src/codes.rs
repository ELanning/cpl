//! Every diagnostic code `cpl-sema` emits, with its category and default
//! severity: the language codes of `spec/semantics.md` 9.4 (categories
//! `resolve`, `type`, `effect`, `contract`, `hole`, `test`,
//! `release_policy`, `runtime`, `internal`) and the toolchain codes of
//! `spec/toolchain.md` Appendix B whose producer is `sema`
//! (`id_kind_mismatch`, `id_missing`, `std_builtin_missing`, and the
//! `project` codes of the release lock check, which [`cpl_project::codes`]
//! spells).
//!
//! The `parse` codes are `cpl_syntax::codes`; project loading spells its
//! own in [`cpl_project::codes`]. Every stage of the checker takes its codes
//! from here, so a code is spelled exactly once; [`category`] and
//! [`default_severity`] give the table's values.

use cpl_diag::{Category, DiagCode, Severity};

macro_rules! codes {
    ($( $(#[$meta:meta])* $name:ident = $code:literal, $category:ident, $severity:ident; )*) => {
        $( $(#[$meta])* pub const $name: DiagCode = DiagCode($code); )*
        /// Every code of this module: (code, category, default severity),
        /// in the order of semantics 9.4 and then toolchain Appendix B.
        pub const ALL: &[(DiagCode, Category, Severity)] = &[
            $( ($name, Category::$category, Severity::$severity), )*
        ];
    };
}

codes! {
    PARALLEL_RANGE_NOT_LIST = "parallel_range_not_list", Type, Error;
    PARALLEL_UNSAFE = "parallel_unsafe", Effect, Error;
    // State machines and explicit function roles (spec/state-machines.md).
    PURE_CALLS_MUTATE = "pure_calls_mutate", Effect, Error;
    MUTATE_IN_CONTRACT = "mutate_in_contract", Effect, Error;
    PURE_DECLARES_EFFECTS = "pure_declares_effects", Effect, Error;
    MUTATION_DEPTH = "mutation_depth", Effect, Warning;
    MACHINE_SIGNATURE = "machine_signature", Type, Error;
    PROGRAM_MACHINE_COUNT = "program_machine_count", Type, Error;
    PROGRAM_GENERIC = "program_generic", Type, Error;
    STATE_CONSTRUCTION_OUTSIDE_MACHINE = "state_construction_outside_machine", Effect, Error;
    INITIAL_SIGNATURE = "initial_signature", Type, Error;
    CONSTRUCTOR_INVARIANT_FALSE = "constructor_invariant_false", Contract, Error;
    // ---- resolve (semantics 9.4) ------------------------------------------
    /// `[project] edition` other than `"0.1"` (raised by project loading).
    UNSUPPORTED_EDITION = "unsupported_edition", Resolve, Error;
    /// A project module path beginning with `std` or `gen` (raised by project loading).
    RESERVED_MODULE_PATH = "reserved_module_path", Resolve, Error;
    /// A module file outside its conventional location (raised by project loading).
    MODULE_FILE_MISPLACED = "module_file_misplaced", Resolve, Warning;
    /// Two item-namespace declarations of one name in one module (1.4, S3).
    DUPLICATE_DECLARATION = "duplicate_declaration", Resolve, Error;
    /// Two equal `@id` arguments (S2).
    DUPLICATE_IDENTITY = "duplicate_identity", Resolve, Error;
    /// The same name imported twice in one file (1.6).
    DUPLICATE_IMPORT = "duplicate_import", Resolve, Error;
    /// An imported name equal to a declaration of the importing module (1.6).
    IMPORT_CONFLICTS_WITH_DECLARATION = "import_conflicts_with_declaration", Resolve, Error;
    /// A module-level declaration or import named like a prelude name (S6).
    PRELUDE_NAME_REDEFINED = "prelude_name_redefined", Resolve, Error;
    /// An import of an item of `std.gen` (1.6, stdlib S28).
    GENERATOR_IMPORT = "generator_import", Resolve, Error;
    /// A bodiless `fn` or definition-less `type` outside the standard library's `@builtin` stubs (1.8).
    MISSING_BODY = "missing_body", Resolve, Error;
    /// A module path that names no module (1.6, 2.4).
    UNRESOLVED_MODULE = "unresolved_module", Resolve, Error;
    /// A name no scope binds (2.2-2.5).
    UNRESOLVED_NAME = "unresolved_name", Resolve, Error;
    /// A type position naming no type, a reserved type name, or `Gen` (S14, stdlib S11).
    UNRESOLVED_TYPE = "unresolved_type", Resolve, Error;
    /// A `use` of, or qualified path to, a private item of another module (1.5).
    PRIVATE_ITEM = "private_item", Resolve, Error;
    /// A private type in a public signature (1.5).
    PRIVATE_TYPE_IN_PUBLIC_SIGNATURE = "private_type_in_public_signature", Resolve, Error;
    /// A public declaration's clause calling a private function (S4).
    PRIVATE_IN_PUBLIC_CONTRACT = "private_in_public_contract", Resolve, Error;
    /// An import never used in its file (1.6).
    UNUSED_IMPORT = "unused_import", Resolve, Warning;
    /// A `let`, `var`, or pattern binding never read (S67).
    UNUSED_BINDING = "unused_binding", Resolve, Warning;
    /// A local binding reusing a name bound in the same or an enclosing scope (S8).
    SHADOWED_BINDING = "shadowed_binding", Resolve, Error;
    /// A name bound twice in one pattern (3.12).
    DUPLICATE_PATTERN_BINDING = "duplicate_pattern_binding", Resolve, Error;
    /// Two or more lexical constructor candidates (S10 step 2).
    AMBIGUOUS_CONSTRUCTOR = "ambiguous_constructor", Resolve, Error;
    /// A type that is not a constructor in a constructor position (S10 step 3).
    NOT_A_CONSTRUCTOR = "not_a_constructor", Resolve, Error;
    /// A module, alias, or function in value position (S11).
    NOT_A_VALUE = "not_a_value", Resolve, Error;
    /// An annotation outside the vocabulary of 1.8.
    UNKNOWN_ANNOTATION = "unknown_annotation", Resolve, Error;
    /// A known annotation in the wrong position (1.8, stdlib 10.2).
    ANNOTATION_MISPLACED = "annotation_misplaced", Resolve, Error;
    /// An annotation given twice on one declaration (1.8).
    DUPLICATE_ANNOTATION = "duplicate_annotation", Resolve, Error;
    /// An annotation argument outside its accepted syntax (1.8, toolchain 4.1).
    INVALID_ANNOTATION_ARGUMENT = "invalid_annotation_argument", Resolve, Error;
    /// `result` outside an `ensures` clause with no binding of that name (S9).
    RESULT_OUTSIDE_ENSURES = "result_outside_ensures", Resolve, Error;
    /// A parameter or local named `result` in a declaration with `ensures` (S9).
    RESULT_NAME_RESERVED = "result_name_reserved", Resolve, Error;
    /// `self` outside a `where` predicate with no binding of that name (S9).
    SELF_OUTSIDE_WHERE = "self_outside_where", Resolve, Error;
    /// A binding named `self` (S9).
    SELF_BOUND = "self_bound", Resolve, Warning;
    /// An assignment to something other than a `var` (3.11).
    ASSIGN_TO_IMMUTABLE = "assign_to_immutable", Resolve, Error;
    /// `return` outside a `fn` body (3.11).
    RETURN_OUTSIDE_FUNCTION = "return_outside_function", Resolve, Error;
    /// `?` outside a `fn` body (3.8).
    PROPAGATE_OUTSIDE_FUNCTION = "propagate_outside_function", Resolve, Error;
    /// A field declared twice, or listed twice in a struct literal (1.4, 3.8).
    DUPLICATE_FIELD = "duplicate_field", Resolve, Error;
    /// A struct literal naming a field its struct does not have (3.8).
    UNKNOWN_FIELD = "unknown_field", Resolve, Error;
    /// A struct literal without spread that omits fields (3.8).
    MISSING_FIELD = "missing_field", Resolve, Error;
    /// A variant declared twice in one enum (1.4).
    DUPLICATE_VARIANT = "duplicate_variant", Resolve, Error;
    /// An operation declared twice in one capability (1.4).
    DUPLICATE_OPERATION = "duplicate_operation", Resolve, Error;
    /// A parameter name used twice in one function or operation (1.4).
    DUPLICATE_PARAMETER = "duplicate_parameter", Resolve, Error;
    /// A type parameter name used twice in one declaration (1.4).
    DUPLICATE_TYPE_PARAMETER = "duplicate_type_parameter", Resolve, Error;
    /// A type parameter no field, payload, or signature type uses (1.8; a warning on `fn`).
    UNUSED_TYPE_PARAMETER = "unused_type_parameter", Resolve, Error;
    /// A refinement whose base chain reaches itself (1.8, S66).
    CYCLIC_TYPE_DEFINITION = "cyclic_type_definition", Resolve, Error;
    /// A containment cycle not broken by `List`, `Option`, or `Result` (S7).
    INFINITE_TYPE = "infinite_type", Resolve, Error;
    /// `cpl run` of an effectful function (S39).
    CANNOT_RUN_EFFECTFUL = "cannot_run_effectful", Resolve, Error;
    /// `cpl run` of a generic function (S39).
    CANNOT_RUN_GENERIC = "cannot_run_generic", Resolve, Error;

    // ---- type --------------------------------------------------------------
    TYPE_MISMATCH = "type_mismatch", Type, Error;
    CANNOT_INFER_TYPE = "cannot_infer_type", Type, Error;
    CANNOT_PROVE_REFINEMENT = "cannot_prove_refinement", Type, Error;
    REFINEMENT_FALSE = "refinement_false", Type, Error;
    CONSTANT_EVALUATION_FAULT = "constant_evaluation_fault", Type, Error;
    CHECK_REQUIRES_TYPE_ARGUMENT = "check_requires_type_argument", Type, Error;
    CHECK_REQUIRES_REFINEMENT = "check_requires_refinement", Type, Error;
    /// A generic type or callee given the wrong number of type arguments (S12, 3.7).
    TYPE_ARGUMENT_COUNT_MISMATCH = "type_argument_count_mismatch", Type, Error;
    /// A non-value type in a value position (S13; not reachable in 0.1).
    NOT_A_VALUE_TYPE = "not_a_value_type", Type, Error;
    ARITY_MISMATCH = "arity_mismatch", Type, Error;
    USING_ARITY_MISMATCH = "using_arity_mismatch", Type, Error;
    NOT_CALLABLE = "not_callable", Type, Error;
    UNKNOWN_MEMBER = "unknown_member", Type, Error;
    PROPERTY_CALLED = "property_called", Type, Error;
    METHOD_NOT_CALLED = "method_not_called", Type, Error;
    UNUSED_VALUE = "unused_value", Type, Error;
    NON_EXHAUSTIVE_MATCH = "non_exhaustive_match", Type, Error;
    UNREACHABLE_ARM = "unreachable_arm", Type, Warning;
    PATTERN_TYPE_MISMATCH = "pattern_type_mismatch", Type, Error;
    PATTERN_ARITY_MISMATCH = "pattern_arity_mismatch", Type, Error;
    PROPAGATE_NOT_RESULT = "propagate_not_result", Type, Error;
    PROPAGATE_RETURN_MISMATCH = "propagate_return_mismatch", Type, Error;
    PROPAGATE_ERROR_MISMATCH = "propagate_error_mismatch", Type, Error;
    RETURN_VALUE_MISSING = "return_value_missing", Type, Error;
    COMPARISON_UNSUPPORTED = "comparison_unsupported", Type, Error;
    FORALL_RANGE_NOT_LIST = "forall_range_not_list", Type, Error;
    FOR_RANGE_NOT_GENERATOR = "for_range_not_generator", Type, Error;
    GENERATOR_OUTSIDE_FOR = "generator_outside_for", Type, Error;
    UNREACHABLE_STATEMENT = "unreachable_statement", Type, Warning;

    // ---- effect ------------------------------------------------------------
    /// A `using` parameter whose type is not a capability (1.8, 4.1).
    USING_PARAM_NOT_CAPABILITY = "using_param_not_capability", Effect, Error;
    /// An `effect_ref` whose left part is not a `using` parameter (4.1).
    EFFECT_PARAM_NOT_CAPABILITY = "effect_param_not_capability", Effect, Error;
    /// An `effect_ref` naming an operation the capability lacks (4.1).
    UNKNOWN_EFFECT_OPERATION = "unknown_effect_operation", Effect, Error;
    /// The same `effect_ref` listed twice (4.4).
    DUPLICATE_EFFECT = "duplicate_effect", Effect, Warning;
    UNUSED_CAPABILITY = "unused_capability", Effect, Warning;
    UNDECLARED_EFFECT = "undeclared_effect", Effect, Error;
    EFFECT_IN_CONTRACT = "effect_in_contract", Effect, Error;
    CAPABILITY_AS_VALUE = "capability_as_value", Effect, Error;
    CAPABILITY_EQUALITY = "capability_equality", Effect, Error;
    /// A capability type in a value position (S13, S27, 1.8).
    CAPABILITY_IN_VALUE_POSITION = "capability_in_value_position", Effect, Error;
    USING_ARGUMENT_NOT_CAPABILITY = "using_argument_not_capability", Effect, Error;

    // ---- contract ----------------------------------------------------------
    HOLE_IN_CONTRACT = "hole_in_contract", Contract, Error;
    PROPAGATE_IN_CONTRACT = "propagate_in_contract", Contract, Error;
    STATEMENT_IN_CONTRACT = "statement_in_contract", Contract, Error;
    RECURSIVE_CONTRACT = "recursive_contract", Contract, Error;
    UNUSED_DECREASES = "unused_decreases", Contract, Warning;
    MISSING_DECREASES = "missing_decreases", Contract, Warning;
    CONTRACT_VIOLATION = "contract_violation", Contract, Error;
    CONTRACT_COUNTEREXAMPLE = "contract_counterexample", Contract, Error;
    VERIFICATION_RESULT = "verification_result", Contract, Info;

    // ---- hole --------------------------------------------------------------
    HOLE = "hole", Hole, Info;
    HOLE_TYPE_UNKNOWN = "hole_type_unknown", Hole, Error;
    UNRESOLVED_HOLE = "unresolved_hole", Hole, Error;

    // ---- test --------------------------------------------------------------
    /// A test or property named `""` (S42).
    EMPTY_TEST_NAME = "empty_test_name", Test, Error;
    /// Two tests or properties of one name in one module (S3, S42).
    DUPLICATE_TEST_NAME = "duplicate_test_name", Test, Error;
    EMPTY_TEST_SELECTION = "empty_test_selection", Test, Error;
    TEST_RESULT = "test_result", Test, Info;
    ASSERTION_FAILED = "assertion_failed", Test, Error;

    // ---- release_policy ----------------------------------------------------
    HOLE_IN_RELEASE = "hole_in_release", ReleasePolicy, Error;
    VERIFICATION_UNSATISFIED = "verification_unsatisfied", ReleasePolicy, Error;

    // ---- runtime and internal ----------------------------------------------
    DIVISION_BY_ZERO = "division_by_zero", Runtime, Error;
    REFINEMENT_VIOLATION = "refinement_violation", Runtime, Error;
    INTERNAL_ERROR = "internal_error", Internal, Error;
    RESOURCE_EXHAUSTION = "resource_exhaustion", Internal, Error;

    // ---- toolchain Appendix B, producer `sema` ------------------------------
    /// A generated-format `@id` whose kind prefix disagrees with the declaration (T21).
    ID_KIND_MISMATCH = "id_kind_mismatch", Resolve, Warning;
    /// A public declaration without `@id` (the opt-in lint of toolchain 4.3).
    ID_MISSING = "id_missing", Resolve, Warning;
    /// A std stub naming a `@builtin` id outside the registry of stdlib 10.6.
    STD_BUILTIN_MISSING = "std_builtin_missing", Internal, Error;
    /// Under `--release`: a present `cpl.lock` that differs from the tree (T37).
    PROJECT_LOCK_STALE = "project_lock_stale", Project, Error;
    /// Under `--release`: no `cpl.lock` (T37).
    PROJECT_LOCK_MISSING = "project_lock_missing", Project, Warning;
}

/// The category of a code: this module's table, then the `project` codes of
/// [`cpl_project::codes`] and the `parse` codes of `cpl_syntax::codes`;
/// `Internal` for a code no table lists.
pub fn category(code: DiagCode) -> Category {
    if let Some((_, category, _)) = ALL.iter().find(|(c, _, _)| *c == code) {
        return *category;
    }
    if cpl_project::codes::PROJECT.contains(&code) {
        return cpl_project::codes::CATEGORY;
    }
    if cpl_syntax::codes::ALL.contains(&code) {
        return cpl_syntax::codes::CATEGORY;
    }
    Category::Internal
}

/// The default severity of a code (9.4's `(W)`/`(I)` marks): `Error` for a
/// code this module does not list.
pub fn default_severity(code: DiagCode) -> Severity {
    ALL.iter()
        .find(|(c, _, _)| *c == code)
        .map_or(Severity::Error, |(_, _, s)| *s)
}

/// The `@builtin` registry ids of stdlib 10.6, sorted: a std stub naming any
/// other id is `std_builtin_missing`. `cpl-sema` cannot depend on `cpl-eval`
/// (toolchain 1.2); from Milestone 2 both read the one list of `cpl-rtspec`,
/// whose ids the evaluator's registry is tested equal to (toolchain 1.6, T70;
/// stdlib S47): the same name, type, and contents as in Milestone 1.
pub const BUILTIN_IDS: &[&str] = &cpl_rtspec::builtins::IDS;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_builtins_sorted() {
        let mut seen = std::collections::BTreeSet::new();
        for (code, _, _) in ALL {
            assert!(seen.insert(code.0), "duplicate code {}", code.0);
        }
        let mut sorted = BUILTIN_IDS.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, BUILTIN_IDS);
        assert_eq!(BUILTIN_IDS.len(), 59);
    }

    #[test]
    fn categories() {
        assert_eq!(category(UNRESOLVED_NAME), Category::Resolve);
        assert_eq!(category(CAPABILITY_IN_VALUE_POSITION), Category::Effect);
        assert_eq!(category(PROJECT_LOCK_MISSING), Category::Project);
        assert_eq!(
            category(cpl_project::codes::PROJECT_BINDING_BAD_BACKEND),
            Category::Project
        );
        assert_eq!(default_severity(UNUSED_IMPORT), Severity::Warning);
        assert_eq!(default_severity(HOLE), Severity::Info);
    }
}
