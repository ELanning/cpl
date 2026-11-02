//! Adversarial parse-diagnostic tests, round 2 (`spec/grammar.md`).
//!
//! The lens is what the parser reports rather than the trees it builds:
//! every `parse` code of `spec/semantics.md` 9.4 with the message (a prefix,
//! grammar D72 [D-238]), the span (D59) and the data fields the grammar
//! fixes; the earliest-error rule with lexer resynchronisation (D70
//! [D-226]) and the single diagnostic per file (D59); the implementation
//! limits of D73 [D-239] (`MAX_DEPTH` 64, `MAX_EXPR_OPS` 1000) at their exact
//! boundaries; keywords and reserved words as names and annotation names
//! (D3, D4, D53, D56, D71 [D-227]); malformed literals (1.5, D6-D8, D62);
//! constructs left open at the end of a file; `.work` and `.patch` syntax
//! errors (Section 4); `parse_expr` (`spec/toolchain.md` 1.5); and that no
//! input makes the parser panic or recurse without bound.
//!
//! Every expectation is the specification's answer, not the current
//! behaviour. A `‸` in a test input marks the byte at which the reported
//! diagnostic's span must start; it is removed before parsing.

use cpl_diag::{DiagCode, Diagnostic, FileId, FileOrigin, Severity, SourceMap};
use cpl_syntax::ast::{DeclKind, Expr, ExprKind, FnDecl, LiteralKind, Module};
use cpl_syntax::parser::{MAX_DEPTH, MAX_EXPR_OPS};
use cpl_syntax::{codes, parse_expr, parse_module, parse_patch, parse_work};
use std::time::{Duration, Instant};

// ===========================================================================
// Harness
// ===========================================================================

const FILE: FileId = FileId(0);

/// Marks the byte at which a diagnostic's span must start (removed before parsing).
const MARK: char = '‸';

/// The 33 reserved keywords (grammar 1.4).
const KEYWORDS: [&str; 33] = [
    "and",
    "assert",
    "capability",
    "decreases",
    "effects",
    "else",
    "ensures",
    "enum",
    "false",
    "fn",
    "for",
    "forall",
    "if",
    "in",
    "invariant",
    "let",
    "match",
    "module",
    "not",
    "or",
    "property",
    "pub",
    "requires",
    "return",
    "struct",
    "test",
    "true",
    "type",
    "use",
    "using",
    "var",
    "where",
    "while",
];

/// The eight words reserved for a future edition (grammar D4).
const RESERVED: [&str; 8] = [
    "as", "async", "await", "break", "continue", "ghost", "lemma", "loop",
];

/// D73 with this implementation's limits (`MAX_DEPTH`, `MAX_EXPR_OPS`).
const NESTING: &str = "nesting is too deep (more than 64 levels)";
const TOO_MANY_OPERATORS: &str =
    "an expression has too many operators (more than 1000); split it into bindings";
/// D36.
const NO_REQUIREMENT: &str = "expected a requirement (typecheck, no_holes, no_unresolved_assumptions, prove(selector), test(selector), or review(task))";
const SIX_FORMS: &str = "; expected typecheck, no_holes, no_unresolved_assumptions, prove(selector), test(selector), or review(task)";
/// Grammar 2.4 rule 4.
const ELSE_MESSAGE: &str =
    "`else` must follow the closing `}` of the preceding block on the same line";
/// Grammar 1.2.
const MISPLACED_DOC: &str = "doc comment must precede a declaration";
const DOC_AFTER_CODE: &str = "doc comment must begin a line";
/// Grammar 3.12 rule 2.
const VARIANT_STRUCT: &str = "a struct literal names a type; variants have positional payloads";
/// Grammar D64.
const TYPE_EQUATION: &str =
    "a type declaration is a wrapper, type A(T), or a refinement, type A = T where ...";

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Form {
    Cpl,
    Work,
    Patch,
    Expression,
}

use Form::{Cpl, Expression, Patch, Work};

const FORMS: [Form; 4] = [Cpl, Work, Patch, Expression];

/// A readable excerpt of a possibly huge input for assertion messages.
fn short(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    if chars.len() <= 160 {
        return src.to_string();
    }
    let head: String = chars[..80].iter().collect();
    let tail: String = chars[chars.len() - 60..].iter().collect();
    format!("{head}…{tail} ({} bytes)", src.len())
}

/// Remove the single marker, returning the text and the marker's byte offset.
fn mark(marked: &str) -> (String, u32) {
    let at = marked
        .find(MARK)
        .unwrap_or_else(|| panic!("no marker in {:?}", short(marked)));
    let src = format!("{}{}", &marked[..at], &marked[at + MARK.len_utf8()..]);
    assert!(
        !src.contains(MARK),
        "one marker per input: {:?}",
        short(marked)
    );
    (src, at as u32)
}

/// `stmts` (already indented) as the body of `fn f() -> Int` in module `m`;
/// the body braces are nesting level 1 (D73).
fn body(stmts: &str) -> String {
    format!("module m\n\nfn f() -> Int {{\n{stmts}\n}}\n")
}

fn run(form: Form, src: &str) -> (bool, Vec<Diagnostic>) {
    match form {
        Cpl => {
            let r = parse_module(src, FILE);
            (r.value.is_some(), r.diagnostics)
        }
        Work => {
            let r = parse_work(src, FILE);
            (r.value.is_some(), r.diagnostics)
        }
        Patch => {
            let r = parse_patch(src, FILE);
            (r.value.is_some(), r.diagnostics)
        }
        Expression => {
            let r = parse_expr(src, FILE);
            (r.value.is_some(), r.diagnostics)
        }
    }
}

/// The envelope every parse diagnostic carries (D59, toolchain 1.5 and T6).
#[track_caller]
fn check_envelope(form: Form, src: &str, d: &Diagnostic) {
    let ctx = || format!("{form:?} {:?}: {d:#?}", short(src));
    assert_eq!(d.category, codes::CATEGORY, "category parse: {}", ctx());
    assert_eq!(d.severity, Severity::Error, "{}", ctx());
    assert!(
        codes::ALL.contains(&d.code),
        "not a parse code of semantics 9.4: {}",
        ctx()
    );
    assert_ne!(
        d.code,
        codes::INVALID_UTF8,
        "invalid_utf8 is raised when a file is read, never by the parser (D70): {}",
        ctx()
    );
    assert!(!d.message.is_empty(), "{}", ctx());
    let span = d
        .span
        .unwrap_or_else(|| panic!("D59: a parse diagnostic has a span: {}", ctx()));
    assert_eq!(span.file, FILE, "{}", ctx());
    assert!(
        span.start <= span.end && span.end as usize <= src.len(),
        "span out of the text: {}",
        ctx()
    );
    assert!(
        src.is_char_boundary(span.start as usize) && src.is_char_boundary(span.end as usize),
        "span off a character boundary: {}",
        ctx()
    );
}

/// The `ParseResult` contract (toolchain 1.5, grammar D59 and D70): a value
/// and no error, or no value and exactly one well-formed diagnostic.
#[track_caller]
fn contract(form: Form, src: &str) -> Option<Diagnostic> {
    let (ok, diagnostics) = run(form, src);
    if ok {
        assert!(
            !diagnostics.iter().any(Diagnostic::is_error),
            "{form:?} {:?}: a tree comes without errors: {diagnostics:#?}",
            short(src)
        );
        None
    } else {
        assert_eq!(
            diagnostics.len(),
            1,
            "D59: exactly one diagnostic for {form:?} {:?}: {diagnostics:#?}",
            short(src)
        );
        let d = diagnostics.into_iter().next().expect("one diagnostic");
        check_envelope(form, src, &d);
        Some(d)
    }
}

#[track_caller]
fn reject(form: Form, src: &str) -> Diagnostic {
    contract(form, src).unwrap_or_else(|| panic!("{form:?} {:?} must be rejected", short(src)))
}

#[track_caller]
fn accept(form: Form, src: &str) {
    if let Some(d) = contract(form, src) {
        panic!("{form:?} {:?} must parse, got {d:#?}", short(src));
    }
}

/// Reject a marked input with `code`, the span starting at the marker.
#[track_caller]
fn reject_at(form: Form, marked: &str, code: DiagCode) -> (String, Diagnostic) {
    let (src, at) = mark(marked);
    let d = reject(form, &src);
    assert_eq!(d.code, code, "{form:?} {:?}: {d:#?}", short(&src));
    let start = d.span.expect("span").start;
    assert_eq!(
        start,
        at,
        "{form:?} {:?}: the span must start at byte {at} ({:?}), not {start}: {d:#?}",
        short(&src),
        short(&src[at as usize..])
    );
    (src, d)
}

fn field<'a>(d: &'a Diagnostic, key: &str) -> Option<&'a str> {
    d.data.get(key).and_then(|v| v.as_str())
}

fn span_text<'a>(src: &'a str, d: &Diagnostic) -> &'a str {
    let span = d.span.expect("span");
    &src[span.start as usize..span.end as usize]
}

/// D72: a message of the shape `expected <description>, found <token>`
/// beginning with `prefix`, with the `expected` and `found` fields.
#[track_caller]
fn assert_generic(d: &Diagnostic, prefix: &str, found: &str) {
    assert_eq!(d.code, codes::SYNTAX_ERROR, "{d:#?}");
    assert!(
        d.message.starts_with(prefix),
        "D72: {:?} must begin with {prefix:?}",
        d.message
    );
    let expected = field(d, "expected")
        .unwrap_or_else(|| panic!("D72: a generic message carries `expected`: {d:#?}"));
    assert_eq!(
        field(d, "found"),
        Some(found),
        "D72: the `found` field: {d:#?}"
    );
    assert_eq!(
        d.message,
        format!("expected {expected}, found {found}"),
        "D72: the generic shape"
    );
}

/// Semantics 9.4: `expected` and `found` only on generic messages.
#[track_caller]
fn assert_no_generic_fields(d: &Diagnostic) {
    assert!(
        !d.data.contains_key("expected") && !d.data.contains_key("found"),
        "9.4: a message that is not of the generic shape carries no `expected`/`found`: {d:#?}"
    );
}

#[track_caller]
fn assert_prefix(d: &Diagnostic, prefix: &str) {
    assert!(
        d.message.starts_with(prefix),
        "D72: {:?} must begin with {prefix:?}",
        d.message
    );
}

#[track_caller]
fn module_ok(src: &str) -> Module {
    let r = parse_module(src, FILE);
    match r.value {
        Some(m) if !r.diagnostics.iter().any(Diagnostic::is_error) => m,
        _ => panic!("{:?} must parse: {:#?}", short(src), r.diagnostics),
    }
}

#[track_caller]
fn expr_ok(src: &str) -> Expr {
    let r = parse_expr(src, FILE);
    match r.value {
        Some(e) if !r.diagnostics.iter().any(Diagnostic::is_error) => e,
        _ => panic!("{:?} must parse: {:#?}", short(src), r.diagnostics),
    }
}

fn first_fn(m: &Module) -> &FnDecl {
    match &m.declarations[0].kind {
        DeclKind::Fn(f) => f,
        other => panic!("not a fn: {other:?}"),
    }
}

fn fn_tail(m: &Module) -> &Expr {
    &first_fn(m)
        .body
        .as_ref()
        .expect("a body")
        .tail
        .as_ref()
        .expect("a tail expression")
        .expr
}

/// D73: `nesting is too deep`, reported at `token`, without generic fields.
#[track_caller]
fn nesting_rejected(form: Form, marked: &str, token: &str) {
    let (src, d) = reject_at(form, marked, codes::SYNTAX_ERROR);
    assert_prefix(&d, NESTING);
    assert_no_generic_fields(&d);
    assert_eq!(span_text(&src, &d), token, "D73: reported at the token");
}

/// D73: `an expression has too many operators`, reported at the operator.
#[track_caller]
fn operators_rejected(form: Form, marked: &str, operator: &str) {
    let (src, d) = reject_at(form, marked, codes::SYNTAX_ERROR);
    assert_prefix(&d, TOO_MANY_OPERATORS);
    assert_no_generic_fields(&d);
    assert_eq!(
        span_text(&src, &d),
        operator,
        "D73: reported at the operator"
    );
}

/// `inner` inside `n` nested `while` loops in a function body.
fn in_whiles(n: usize, inner: &str) -> String {
    body(&format!(
        "{}{inner}\n{}",
        "while c {\n".repeat(n),
        "}\n".repeat(n)
    ))
}

/// Run `f` on a thread with the 1 MiB stack D73's rationale budgets.
fn with_one_mebibyte_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(f)
        .expect("spawn")
        .join()
        .expect("the parser stays within a 1 MiB stack");
}

/// Run `work` on a 1 MiB thread in a child process that runs only
/// `test_name`: a stack overflow aborts the process it happens in, so the
/// parent reports it as this test's failure instead of losing the binary.
/// The child marks itself non-dumpable first, so an abort leaves no core
/// dump behind (WSL, for one, captures them into the Windows temp folder).
fn in_child_process_with_one_mebibyte_stack(test_name: &str, work: fn()) {
    const CHILD: &str = "CPL_SYNTAX_R2_STACK_CHILD";
    if std::env::var_os(CHILD).is_some() {
        no_core_dump();
        with_one_mebibyte_stack(work);
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().expect("the test binary"))
        .args([test_name, "--exact", "--nocapture", "--test-threads", "1"])
        .env(CHILD, "1")
        .output()
        .expect("run the test binary");
    assert!(
        output.status.success(),
        "{test_name} with a 1 MiB parser thread (child process {}):\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
fn no_core_dump() {
    extern "C" {
        fn prctl(option: std::os::raw::c_int, ...) -> std::os::raw::c_int;
    }
    const PR_SET_DUMPABLE: std::os::raw::c_int = 4;
    let not_dumpable: std::os::raw::c_ulong = 0;
    // SAFETY: `prctl(PR_SET_DUMPABLE, 0)` only clears this process's dumpable
    // flag; it touches no memory of ours.
    let status = unsafe { prctl(PR_SET_DUMPABLE, not_dumpable) };
    assert_eq!(status, 0, "prctl(PR_SET_DUMPABLE, 0)");
}

#[cfg(not(target_os = "linux"))]
fn no_core_dump() {}

// ===========================================================================
// D73: the nesting limit
// ===========================================================================

#[test]
fn d73_the_limits_are_the_minimum_the_grammar_guarantees() {
    // toolchain 1.5: "The implementation limits of grammar D73, at the
    // minimum the grammar guarantees".
    assert_eq!(MAX_DEPTH, 64);
    assert_eq!(MAX_EXPR_OPS, 1000);
}

#[test]
fn d73_parentheses_count_from_the_braces_of_the_function_body() {
    // "levels are counted from the top level of the file, so the braces of a
    // function body are its first level": 63 parentheses reach level 64.
    accept(
        Cpl,
        &body(&format!("    {}1{}", "(".repeat(63), ")".repeat(63))),
    );
    // The 64th parenthesis opens level 65; the first token after it is `1`.
    nesting_rejected(
        Cpl,
        &body(&format!("    {}‸1{}", "(".repeat(64), ")".repeat(64))),
        "1",
    );
}

#[test]
fn d73_parse_expr_counts_levels_from_its_own_top_level() {
    accept(
        Expression,
        &format!("{}1{}", "(".repeat(64), ")".repeat(64)),
    );
    nesting_rejected(
        Expression,
        &format!("{}‸1{}", "(".repeat(65), ")".repeat(65)),
        "1",
    );
    accept(Expression, &format!("{}{}", "[".repeat(64), "]".repeat(64)));
    nesting_rejected(
        Expression,
        &format!("{}‸]{}", "[".repeat(65), "]".repeat(64)),
        "]",
    );
}

#[test]
fn d73_clause_and_refinement_expressions_start_at_the_top_level() {
    let clause = |n: usize, m: &str| {
        format!(
            "module m\n\nfn f(x: Int) -> Int\n    requires {}{m}x{} > 0\n{{\n    x\n}}\n",
            "(".repeat(n),
            ")".repeat(n)
        )
    };
    accept(Cpl, &clause(64, ""));
    nesting_rejected(Cpl, &clause(65, "‸"), "x");
    let refinement = |n: usize, m: &str| {
        format!(
            "module m\n\ntype A = Int where {}{m}self{} > 0\n",
            "(".repeat(n),
            ")".repeat(n)
        )
    };
    accept(Cpl, &refinement(64, ""));
    nesting_rejected(Cpl, &refinement(65, "‸"), "self");
}

#[test]
fn d73_brackets_struct_literal_braces_and_call_parentheses_are_levels() {
    accept(
        Cpl,
        &body(&format!("    {}1{}", "[".repeat(63), "]".repeat(63))),
    );
    nesting_rejected(
        Cpl,
        &body(&format!("    {}‸1{}", "[".repeat(64), "]".repeat(64))),
        "1",
    );
    accept(
        Cpl,
        &body(&format!("    {}1{}", "A { a: ".repeat(63), " }".repeat(63))),
    );
    nesting_rejected(
        Cpl,
        &body(&format!(
            "    {}A {{ ‸a: 1{}",
            "A { a: ".repeat(63),
            " }".repeat(64)
        )),
        "a",
    );
    accept(
        Cpl,
        &body(&format!("    {}1{}", "f(".repeat(63), ")".repeat(63))),
    );
    nesting_rejected(
        Cpl,
        &body(&format!("    {}‸1{}", "f(".repeat(64), ")".repeat(64))),
        "1",
    );
}

#[test]
fn d73_each_prefix_operator_is_a_level_reported_at_the_next_token() {
    accept(Cpl, &body(&format!("    {}x", "not ".repeat(63))));
    nesting_rejected(Cpl, &body(&format!("    {}‸x", "not ".repeat(64))), "x");
    accept(Cpl, &body(&format!("    {}x", "-".repeat(63))));
    nesting_rejected(Cpl, &body(&format!("    {}‸x", "-".repeat(64))), "x");
    nesting_rejected(Cpl, &body(&format!("    {}‸x", "not -".repeat(32))), "x");
    // The 64th `not` opens the excess level; the token after it is the 65th.
    nesting_rejected(
        Cpl,
        &body(&format!("    {}‸not x", "not ".repeat(64))),
        "not",
    );
}

#[test]
fn d73_if_and_match_are_levels_reported_at_their_keyword() {
    // `while` is a statement, not a level; its block is one. The body braces
    // and 63 loop blocks reach level 64.
    accept(Cpl, &in_whiles(63, "1"));
    nesting_rejected(Cpl, &in_whiles(63, "‸if c { 1 }"), "if");
    nesting_rejected(Cpl, &in_whiles(63, "‸match x {}"), "match");
    nesting_rejected(Cpl, &in_whiles(63, "(‸1)"), "1");
    // An `if` is a level and its block another; so are a `match` and its body.
    accept(Cpl, &in_whiles(61, "if c { 1 }"));
    nesting_rejected(Cpl, &in_whiles(62, "if c { ‸1 }"), "1");
    accept(Cpl, &in_whiles(61, "match x { _ => 1 }"));
    nesting_rejected(Cpl, &in_whiles(62, "match x { ‸_ => 1 }"), "_");
}

#[test]
fn d73_each_if_of_an_else_if_chain_counts_one_level() {
    // "(each `if` of an `else if` chain counts one)": the k-th `if` of a chain
    // in a function body is at level k + 1 and its blocks at level k + 2.
    let chain = |last: &str| {
        body(&format!(
            "    if c {{ 1 }}{}{last}",
            " else if c { 1 }".repeat(61)
        ))
    };
    accept(Cpl, &chain(" else { 2 }"));
    nesting_rejected(Cpl, &chain(" else if c { ‸1 }"), "1");
}

#[test]
fn d73_generic_argument_lists_of_types_are_levels() {
    let ret = |n: usize, m: &str| {
        format!(
            "module m\n\nfn f() -> {}{m}Int{} {{\n    1\n}}\n",
            "List<".repeat(n),
            ">".repeat(n)
        )
    };
    accept(Cpl, &ret(64, ""));
    nesting_rejected(Cpl, &ret(65, "‸"), "Int");
    let param = |n: usize, m: &str| {
        format!(
            "module m\n\nfn f(x: {}{m}Int{}) -> Int {{\n    1\n}}\n",
            "List<".repeat(n),
            ">".repeat(n)
        )
    };
    accept(Cpl, &param(63, ""));
    nesting_rejected(Cpl, &param(64, "‸"), "Int");
    let annotation = |n: usize, m: &str| {
        body(&format!(
            "    let x: {}{m}Int{} = y\n    x",
            "List<".repeat(n),
            ">".repeat(n)
        ))
    };
    accept(Cpl, &annotation(63, ""));
    nesting_rejected(Cpl, &annotation(64, "‸"), "Int");
}

#[test]
fn d73_generic_call_argument_lists_are_levels() {
    // `f<List^62<Int>>(1)`: 63 generic argument lists at levels 2..64.
    let src = body(&format!(
        "    f<{}Int{}>(1)",
        "List<".repeat(62),
        ">".repeat(62)
    ));
    let m = module_ok(&src);
    let ExprKind::Call(call) = &fn_tail(&m).kind else {
        panic!("a generic call (D42)")
    };
    assert!(call.generic_args.is_some(), "the generic-call reading");
}

#[test]
fn d73_a_generic_call_beyond_the_nesting_limit_is_the_nesting_error() {
    // D73: "an input beyond its limits is the syntax error `nesting is too
    // deep ...`, reported at ... the first token after the ... `<` ... that
    // opens the excess level". Here 64 generic argument lists inside the
    // body braces need level 65; the generic-call reading (D42) is the only
    // one under which the input is a program at all, so its excess level is
    // the error, not a fallback to a chain of comparisons.
    nesting_rejected(
        Cpl,
        &body(&format!(
            "    f<{}‸Int{}>(1)",
            "List<".repeat(63),
            ">".repeat(63)
        )),
        "Int",
    );
}

#[test]
fn d73_each_forall_predicate_is_a_level() {
    let src = |n: usize, m: &str| {
        format!(
            "module m\n\nfn f(xs: List<Int>) -> Bool\n    requires {}{m}true\n{{\n    true\n}}\n",
            "forall x in xs: ".repeat(n)
        )
    };
    accept(Cpl, &src(64, ""));
    // The 65th predicate's `:` opens the excess level.
    nesting_rejected(Cpl, &src(65, "‸"), "true");
}

#[test]
fn d73_constructor_pattern_payloads_are_levels() {
    // The body braces, the `match`, and its body are levels 1-3, so 61
    // payload lists reach level 64.
    let src = |n: usize, m: &str| {
        body(&format!(
            "    match x {{\n        {}{m}y{} => 1\n    }}",
            "Some(".repeat(n),
            ")".repeat(n)
        ))
    };
    accept(Cpl, &src(61, ""));
    nesting_rejected(Cpl, &src(62, "‸"), "y");
}

#[test]
fn d73_levels_of_different_kinds_add_up_and_closed_levels_are_released() {
    // 21 x (paren, bracket, prefix minus) = 63 levels inside the body braces.
    accept(
        Cpl,
        &body(&format!("    {}x{}", "([-".repeat(21), "])".repeat(21))),
    );
    nesting_rejected(
        Cpl,
        &body(&format!("    ({}‸x{})", "([-".repeat(21), "])".repeat(21))),
        "x",
    );
    // Siblings at level 64 never add up.
    let deep = format!("{}1{}", "(".repeat(63), ")".repeat(63));
    accept(
        Cpl,
        &body(&format!(
            "    let a = {deep} + {deep}\n    let b = f({}) + {deep}\n    a",
            &deep[1..deep.len() - 1]
        )),
    );
    // A failed generic-call speculation (D42) at level 63 releases the level
    // it tried, so a later expression may still reach level 64.
    accept(
        Cpl,
        &body(&format!(
            "    let a = {}x < B{}\n    let b = {deep}\n    a",
            "(".repeat(62),
            ")".repeat(62)
        )),
    );
}

#[test]
fn d73_the_token_after_an_excess_bracket_is_read_in_the_bracket_s_own_mode() {
    // Grammar 2.2: inside `(`, `[`, and a struct literal's `{` newlines are
    // discarded before the parser sees them, so the first token after such a
    // bracket (where D73 reports the excess level) is the one after the line
    // break. The body braces, 31 `if`s with their blocks, and a loop block
    // reach level 64, a significant-newline context; the bracket opens 65.
    let at_64 = |stmt: &str| {
        body(&format!(
            "{}while c {{\n{stmt}\n}}\n{}",
            "if c {\n".repeat(31),
            "}\n".repeat(31)
        ))
    };
    accept(Cpl, &at_64("1"));
    nesting_rejected(Cpl, &at_64("f(\n‸1)"), "1");
    nesting_rejected(Cpl, &at_64("[\n‸1]"), "1");
    nesting_rejected(Cpl, &at_64("A {\n‸a: 1}"), "a");
}

#[test]
fn d73_test_property_and_patch_bodies_are_first_levels() {
    let deep = |n: usize, m: &str| format!("{}{m}1{}", "(".repeat(n), ")".repeat(n));
    for (form, head, tail) in [
        (Cpl, "module m\n\ntest \"t\" {\n    ", "\n}\n"),
        (
            Cpl,
            "module m\n\nproperty \"p\"\n    for x in g\n{\n    ",
            "\n}\n",
        ),
        (
            Patch,
            "patch a.body\n    expect base(a.body)\n{\n    ",
            "\n}\n",
        ),
    ] {
        accept(form, &format!("{head}{}{tail}", deep(63, "")));
        nesting_rejected(form, &format!("{head}{}{tail}", deep(64, "‸")), "1");
    }
}

// ===========================================================================
// D73: the operator limit
// ===========================================================================

#[test]
fn d73_one_thousand_binary_operators_fit_in_one_expression() {
    accept(Cpl, &body(&format!("    1{}", " + 1".repeat(1000))));
    operators_rejected(
        Cpl,
        &body(&format!("    1{} ‸+ 1", " + 1".repeat(1000))),
        "+",
    );
    accept(Expression, &format!("1{}", " * 1".repeat(1000)));
    operators_rejected(Expression, &format!("1{} ‸% 1", " * 1".repeat(1000)), "%");
    accept(Cpl, &body(&format!("    a{}", " or a".repeat(1000))));
    operators_rejected(
        Cpl,
        &body(&format!("    a{} ‸and a", " or a".repeat(1000))),
        "and",
    );
}

#[test]
fn d73_each_postfix_operator_counts_one() {
    accept(Cpl, &body(&format!("    a{}", ".b".repeat(1000))));
    operators_rejected(Cpl, &body(&format!("    a{}‸.b", ".b".repeat(1000))), ".");
    accept(Cpl, &body(&format!("    f{}", "()".repeat(1000))));
    operators_rejected(Cpl, &body(&format!("    f{}‸()", "()".repeat(1000))), "(");
    accept(Cpl, &body(&format!("    x{}", "?".repeat(1000))));
    operators_rejected(Cpl, &body(&format!("    x{}‸?", "?".repeat(1000))), "?");
    // A struct literal is one postfix operator, and so is a generic call.
    accept(Cpl, &body(&format!("    A {{}}{}", " + 1".repeat(999))));
    operators_rejected(
        Cpl,
        &body(&format!("    A {{}}{} ‸+ 1", " + 1".repeat(999))),
        "+",
    );
    accept(Cpl, &body(&format!("    f<Int>(1){}", " + 1".repeat(999))));
    operators_rejected(
        Cpl,
        &body(&format!("    f<Int>(1){} ‸+ 1", " + 1".repeat(999))),
        "+",
    );
    // `a.b(c)` is a member access and a call: two operators.
    accept(Cpl, &body(&format!("    a.b(c){}", " + 1".repeat(998))));
    operators_rejected(
        Cpl,
        &body(&format!("    a.b(c){} ‸+ 1", " + 1".repeat(998))),
        "+",
    );
}

#[test]
fn d73_a_comparison_after_a_name_is_one_operator() {
    // The 1001st operator is rejected whatever it is...
    operators_rejected(
        Cpl,
        &body(&format!("    a{} ‸< b", " + a".repeat(1000))),
        "<",
    );
    // ...but 999 additions and one comparison are 1000 operators. After a
    // name, `<` first starts the speculative generic-call parse of D42; when
    // that fails the `<` is one binary operator, the comparison.
    accept(Cpl, &body(&format!("    a{} < b", " + a".repeat(999))));
    // 500 comparisons and 499 conjunctions: 999 operators.
    accept(
        Cpl,
        &body(&format!("    {}a < b", "a < b and ".repeat(499))),
    );
}

#[test]
fn d73_operators_inside_blocks_and_arms_count_toward_the_enclosing_expression() {
    // "counted over an outermost expression ... together with every expression
    // nested inside it, the statements of a block inside it included".
    let if_value = |extra: &str| {
        body(&format!(
            "    let x = if c {{\n        let y = 1{}\n        y{}{extra}\n    }} else {{\n        0\n    }}\n    x",
            " + 1".repeat(600),
            " + 1".repeat(400)
        ))
    };
    accept(Cpl, &if_value(""));
    operators_rejected(Cpl, &if_value(" ‸+ 1"), "+");
    let arms = |extra: &str| {
        body(&format!(
            "    match x {{\n        A => 1{}\n        B => 1{}{extra}\n    }}",
            " + 1".repeat(500),
            " + 1".repeat(500)
        ))
    };
    accept(Cpl, &arms(""));
    operators_rejected(Cpl, &arms(" ‸+ 1"), "+");
    // So do the statements of a statement-level `if`: the `if` is the
    // statement's expression and its blocks are inside it.
    let if_statement = |extra: &str| {
        body(&format!(
            "    if c {{\n        let a = 1{}\n        let b = 1{}{extra}\n    }}",
            " + 1".repeat(500),
            " + 1".repeat(500)
        ))
    };
    accept(Cpl, &if_statement(""));
    operators_rejected(Cpl, &if_statement(" ‸+ 1"), "+");
}

#[test]
fn d73_each_statement_clause_and_predicate_has_its_own_operator_budget() {
    let chain = " + 1".repeat(1000);
    // The `while` condition is the loop statement's expression; the statements
    // of its body are statements of their own.
    accept(
        Cpl,
        &body(&format!(
            "    let a = 1{chain}\n    let b = 1{chain}\n    while c{chain} {{\n        let d = 1{chain}\n        d = 1{chain}\n    }}\n    a{chain}"
        )),
    );
    let clauses = format!(
        "module m\n\nfn f(x: Int) -> Int\n    requires x{} > 0\n    ensures result{} > 0\n    decreases x{chain}\n{{\n    x{chain}\n}}\n",
        " + 1".repeat(999),
        " + 1".repeat(999)
    );
    accept(Cpl, &clauses);
    operators_rejected(
        Cpl,
        &format!("module m\n\nfn f(x: Int) -> Int\n    requires x{chain} ‸> 0\n{{\n    x\n}}\n"),
        ">",
    );
    accept(
        Cpl,
        &format!(
            "module m\n\ntype A = Int where self{} > 0\n",
            " + 1".repeat(999)
        ),
    );
    operators_rejected(
        Cpl,
        &format!("module m\n\ntype A = Int where self{chain} ‸> 0\n"),
        ">",
    );
}

#[test]
fn d73_prefix_operators_parentheses_and_lists_are_not_operators() {
    accept(
        Cpl,
        &body(&format!(
            "    {}x{}",
            "not ".repeat(60),
            " or y".repeat(1000)
        )),
    );
    accept(
        Cpl,
        &body(&format!(
            "    [{}(1)]{}",
            "(1), ".repeat(3000),
            " ++ [1]".repeat(1000)
        )),
    );
    accept(
        Cpl,
        &body(&format!(
            "    f({}1){}",
            "-1, ".repeat(3000),
            " + 1".repeat(999)
        )),
    );
}

// ===========================================================================
// D59 and D70: one diagnostic, the earliest
// ===========================================================================

#[test]
fn d70_a_lexical_error_before_a_syntax_error_is_the_one_reported() {
    let (_, d) = reject_at(Cpl, &body("    ‸# x y"), codes::INVALID_CHARACTER);
    assert_prefix(&d, "unexpected character `#`");
    assert!(
        field(&d, "found").is_some_and(|f| f.contains('#')),
        "{d:#?}"
    );
    reject_at(
        Work,
        "change c {\n    ‸$ base\n}\n",
        codes::INVALID_CHARACTER,
    );
    reject_at(
        Patch,
        "patch a.body\n    expect base(a.body)\n{\n    ‸~ x y\n}\n",
        codes::INVALID_CHARACTER,
    );
    reject_at(Expression, "‸^ )", codes::INVALID_CHARACTER);
}

#[test]
fn d70_a_syntax_error_before_a_lexical_error_is_the_one_reported() {
    let (_, d) = reject_at(
        Cpl,
        "module m\n\nfn f() ‸Int {\n    let x = \"unterminated\n    #\n}\n",
        codes::SYNTAX_ERROR,
    );
    assert_prefix(&d, "expected `->`");
    reject_at(
        Work,
        "change c {\n    target ‸\"main\"\n    workers 007\n}\n",
        codes::SYNTAX_ERROR,
    );
    reject_at(
        Patch,
        "patch a.body\n    expect base(a.body)‸\nlet x = 1\n{\n    \"\\q\"\n}\n",
        codes::SYNTAX_ERROR,
    );
    reject_at(Expression, "1 ‸2 \"\\q\"", codes::SYNTAX_ERROR);
}

#[test]
fn d70_a_lexical_and_a_syntax_error_at_one_offset_report_the_lexical_one() {
    // The parser's error at a token and the lexer's error about that same
    // token start at the same offset.
    let (_, d) = reject_at(
        Cpl,
        "module m\n\nfn f() -> ‸007 {\n    1\n}\n",
        codes::INVALID_INT_LITERAL,
    );
    assert_prefix(&d, "leading zeros are not permitted");
    reject_at(
        Cpl,
        "module m\n\nfn f() -> ‸\"Int\n",
        codes::UNTERMINATED_TEXT_LITERAL,
    );
    reject_at(
        Cpl,
        "module m\n\nuse a.‸2x\n",
        codes::LITERAL_ADJACENT_IDENTIFIER,
    );
    reject_at(
        Work,
        "change c {\n    ‸1.5\n}\n",
        codes::FLOAT_LITERAL_UNSUPPORTED,
    );
    reject_at(Patch, "‸b\"x\"\n", codes::PREFIXED_LITERAL_RESERVED);
    reject_at(Expression, "1 ‸?Foo", codes::HOLE_NAME_CASE);
}

#[test]
fn d70_a_lexical_error_fails_a_file_whose_tokens_parse() {
    reject_at(
        Cpl,
        "module m\n\n‸test\"t\" {\n}\n",
        codes::PREFIXED_LITERAL_RESERVED,
    );
    reject_at(Cpl, &body("    1‸;"), codes::INVALID_CHARACTER);
    // A lone CR is an error even inside a comment (grammar 1.1).
    reject_at(
        Cpl,
        "module m\n\n// a‸\r// b\n",
        codes::INVALID_LINE_TERMINATOR,
    );
    // Inside a speculative generic call too (D42 reports nothing, D70 does).
    reject_at(Cpl, &body("    f<Int ‸# >(1)"), codes::INVALID_CHARACTER);
    reject_at(
        Work,
        "change c {\n    task t {\n        ‸goal\"x\"\n    }\n}\n",
        codes::PREFIXED_LITERAL_RESERVED,
    );
    let (_, d) = reject_at(
        Patch,
        "patch a.body\n    expect base(a.body)\n{\n    x ‸! = y\n}\n",
        codes::INVALID_CHARACTER,
    );
    assert_prefix(&d, "use not");
    let (_, d) = reject_at(Expression, "‸\u{1}1", codes::INVALID_CHARACTER);
    assert_prefix(&d, "unexpected character U+0001");
}

#[test]
fn d70_the_earliest_lexical_error_wins_whatever_order_the_lexer_found_them() {
    // `00_`: the misplaced separator is found first, the leading zero (which
    // starts at the literal) is earlier.
    let (_, d) = reject_at(Cpl, &body("    let x = ‸00_"), codes::INVALID_INT_LITERAL);
    assert_prefix(&d, "leading zeros are not permitted");
    // A bad escape is found before the literal turns out to be unterminated;
    // the unterminated literal starts at its quote.
    reject_at(
        Cpl,
        &body("    let x = ‸\"\\u{41\n    1"),
        codes::UNTERMINATED_TEXT_LITERAL,
    );
    reject_at(Expression, "‸\"\\q", codes::UNTERMINATED_TEXT_LITERAL);
}

#[test]
fn d70_nothing_is_reported_during_a_speculative_generic_call_parse() {
    // The speculation from the first `<` fails at `c`; what is reported is
    // the chained comparison at the second `<`, a generic syntax error.
    let (_, d) = reject_at(Cpl, &body("    a < B ‸< c"), codes::SYNTAX_ERROR);
    assert_eq!(field(&d, "found"), Some("`<`"));
    let (_, d) = reject_at(Cpl, &body("    let x = a < B ‸> c"), codes::SYNTAX_ERROR);
    assert_eq!(field(&d, "found"), Some("`>`"));
    accept(Cpl, &body("    let x = a < B\n    x"));
    accept(Cpl, &body("    f(a < B, c)"));
    // Inside brackets the speculation may span lines (D42).
    let m = module_ok(&body("    f(a <\n        B > (c))"));
    let ExprKind::Call(outer) = &fn_tail(&m).kind else {
        panic!("a call")
    };
    assert_eq!(outer.args.positional.len(), 1, "one argument, a<B>(c)");
}

#[test]
fn d59_a_file_full_of_errors_carries_exactly_one_diagnostic() {
    let src = "module m\n\n# ; $\nfn f( -> {\n    let = = =\n    \"unterminated\n    007 1__0 2x 1.5\n}\nuse a.B.{}\n/// dangling\n";
    let d = reject(Cpl, src);
    assert_eq!(d.code, codes::INVALID_CHARACTER);
    assert_eq!(d.span.expect("span").start, src.find('#').unwrap() as u32);
    let src = "change c {\n    base\n    base\n    workers x\n}\nuse a.b\n#\n";
    let d = reject(Work, src);
    assert_generic(&d, "expected `git`", "end of line");
    assert_eq!(
        d.span.expect("span").start,
        src.find('\n').unwrap() as u32 + 9
    );
    let src = "patch\npatch\n#\n";
    let d = reject(Patch, src);
    assert_eq!(d.code, codes::SYNTAX_ERROR);
    assert_eq!(field(&d, "found"), Some("end of line"));
    assert_eq!(d.span.expect("span").start, 5);
}

#[test]
fn d70_a_lone_carriage_return_inside_a_doc_comment_is_the_earliest_error() {
    // Grammar 1.1: a line ends at LF or CRLF; a CR not followed by LF is a
    // lexical error, not a line end. A doc comment therefore runs past a lone
    // CR to the end of its line and still documents what follows it, and the
    // CR is the file's only error. D70: the lexer "skips an offending
    // character", and resynchronisation is invisible in the result; ending
    // the comment at the CR instead turns the rest of the line into tokens
    // and makes the doc comment look misplaced, an error at an earlier
    // offset that the input does not contain.
    reject_at(Cpl, "/// a‸\rb\nmodule m\n", codes::INVALID_LINE_TERMINATOR);
    reject_at(
        Cpl,
        "module m\n\n/// d‸\rx\nfn f() -> Int {\n    1\n}\n",
        codes::INVALID_LINE_TERMINATOR,
    );
    reject_at(
        Work,
        "/// d‸\rx\nchange c {\n}\n",
        codes::INVALID_LINE_TERMINATOR,
    );
    reject_at(
        Patch,
        "/// d‸\rx\npatch a.body\n    expect base(a.body)\n{\n}\n",
        codes::INVALID_LINE_TERMINATOR,
    );
}

#[test]
fn d70_a_misplaced_doc_comment_is_reported_at_the_doc_comment_before_later_lexical_errors() {
    // The doc comment documents nothing (it is followed by `#`, then `}`): its
    // error starts before the lexical one.
    let (_, d) = reject_at(
        Cpl,
        "module m\n\nstruct S {\n    a: Int\n    ‸/// d\n    #\n}\n",
        codes::DOC_COMMENT_POSITION,
    );
    assert_prefix(&d, MISPLACED_DOC);
    // With a member after the bad character the doc comment is fine; only the
    // lexical error remains.
    reject_at(
        Cpl,
        "module m\n\nstruct S {\n    /// d\n    ‸#\n    a: Int\n}\n",
        codes::INVALID_CHARACTER,
    );
}

// ===========================================================================
// D71, D53, D4, D3, D56, D5, D10: keywords and reserved words as names
// ===========================================================================

#[test]
fn d71_every_reserved_keyword_is_an_annotation_name() {
    for kw in KEYWORDS {
        let m = module_ok(&format!(
            "module m\n\n@{kw}\n@{kw}(\"x\", 1, true)\nfn f() -> Int {{\n    1\n}}\n"
        ));
        let annotations = &m.declarations[0].annotations;
        assert_eq!(annotations.len(), 2, "@{kw}");
        assert_eq!(annotations[0].name.text, kw, "the AST stores the spelling");
        assert!(!annotations[0].parens);
        assert_eq!(annotations[1].name.text, kw);
        assert_eq!(annotations[1].args.len(), 3);
    }
    // Whitespace between `@` and the name is not significant (2.6 rule 5).
    let m = module_ok("module m\n\n@ property\nfn f() -> Int {\n    1\n}\n");
    assert_eq!(m.declarations[0].annotations[0].name.text, "property");
    // The standard library's stub shape (stdlib 10.2) in a project file parses.
    module_ok("module m\n\n@builtin(\"text.len\")\n@property\npub fn len(t: Text) -> Int\n");
}

#[test]
fn d71_reserved_future_words_after_at_are_reserved_word() {
    for w in RESERVED {
        let (src, d) = reject_at(
            Cpl,
            &format!("module m\n\n@‸{w}\nfn f() -> Int {{\n    1\n}}\n"),
            codes::RESERVED_WORD,
        );
        assert_eq!(field(&d, "name"), Some(w), "9.4: `name`");
        assert_prefix(&d, &format!("`{w}` is reserved for a future edition"));
        assert_eq!(span_text(&src, &d), w);
    }
}

#[test]
fn d71_other_tokens_after_at_are_the_generic_syntax_error() {
    for (marked, found) in [
        ("@‸Id(\"x\")", Some("identifier `Id`")),
        ("@‸_", Some("`_`")),
        ("@‸1", None),
        ("@‸\"id\"", None),
        ("@‸@id", Some("`@`")),
        ("@‸?h", None),
        ("@‸(\"x\")", Some("`(`")),
    ] {
        let src = format!("module m\n\n{marked}\nfn f() -> Int {{\n    1\n}}\n");
        let (_, d) = reject_at(Cpl, &src, codes::SYNTAX_ERROR);
        assert!(d.data.contains_key("expected"), "{d:#?}");
        if let Some(found) = found {
            assert_eq!(field(&d, "found"), Some(found));
        }
    }
    let (_, d) = reject_at(
        Cpl,
        "module m\n\n@‸\nfn f() -> Int {\n    1\n}\n",
        codes::SYNTAX_ERROR,
    );
    assert_eq!(field(&d, "found"), Some("end of line"));
    // An annotation occupies its own line.
    reject_at(
        Cpl,
        "module m\n\n@prove ‸fn f() -> Int {\n    1\n}\n",
        codes::SYNTAX_ERROR,
    );
    // Arguments are literals only (D61): no unary minus, names or calls.
    for marked in ["@id(‸-1)", "@id(‸x)", "@id(\"a\" ‸+ \"b\")", "@id(‸[1])"] {
        let src = format!("module m\n\n{marked}\nfn f() -> Int {{\n    1\n}}\n");
        reject_at(Cpl, &src, codes::SYNTAX_ERROR);
    }
}

#[test]
fn d53_a_keyword_is_never_a_member_name() {
    for kw in KEYWORDS {
        let (_, d) = reject_at(Expression, &format!("x.‸{kw}"), codes::SYNTAX_ERROR);
        assert_generic(&d, "expected a member name", &format!("`{kw}`"));
    }
    for w in RESERVED {
        let (_, d) = reject_at(Expression, &format!("x.‸{w}"), codes::RESERVED_WORD);
        assert_eq!(field(&d, "name"), Some(w));
    }
    // After any postfix operator, and in every file form.
    reject_at(Cpl, &body("    f(x)?.‸type"), codes::SYNTAX_ERROR);
    reject_at(
        Patch,
        "patch a.body\n    expect base(a.body)\n{\n    x.‸effects\n}\n",
        codes::SYNTAX_ERROR,
    );
    reject_at(
        Cpl,
        "module m\n\nfn f(p: Page) -> Int\n    ensures result == p.‸match\n{\n    1\n}\n",
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn d4_a_reserved_word_is_reserved_word_in_every_name_position() {
    let cases: Vec<(Form, String)> = vec![
        (Cpl, "module ‸loop\n".into()),
        (Cpl, "module a.‸as\n".into()),
        (Cpl, "module m\n\nuse ‸async.b\n".into()),
        (Cpl, "module m\n\nuse a.‸await.c\n".into()),
        (Cpl, "module m\n\nuse a.b.‸break\n".into()),
        (Cpl, "module m\n\nuse a.b.{c, ‸continue}\n".into()),
        (Cpl, "module m\n\nuse a.b ‸as c\n".into()),
        (Cpl, "module m\n\nfn ‸ghost() -> Int {\n    1\n}\n".into()),
        (
            Cpl,
            "module m\n\nfn f(‸lemma: Int) -> Int {\n    1\n}\n".into(),
        ),
        (
            Cpl,
            "module m\n\nfn f(x: ‸loop) -> Int {\n    1\n}\n".into(),
        ),
        (Cpl, "module m\n\nfn f<‸as>() -> Int {\n    1\n}\n".into()),
        (Cpl, "module m\n\nfn f() -> ‸async {\n    1\n}\n".into()),
        (Cpl, "module m\n\nstruct S {\n    ‸await: Int\n}\n".into()),
        (Cpl, "module m\n\nenum E {\n    ‸break\n}\n".into()),
        (
            Cpl,
            "module m\n\ncapability C {\n    fn ‸continue() -> Int\n}\n".into(),
        ),
        (Cpl, "module m\n\ntype ‸ghost(Int)\n".into()),
        (
            Cpl,
            "module m\n\npub ‸lemma fn f() -> Int {\n    1\n}\n".into(),
        ),
        (Cpl, "module m\n\n‸loop\n".into()),
        (
            Cpl,
            "module m\n\nfn f(using db: Db) -> Int\n    effects { db.‸as }\n{\n    1\n}\n".into(),
        ),
        (
            Cpl,
            "module m\n\nproperty \"p\"\n    for ‸async in g\n{\n}\n".into(),
        ),
        (Cpl, body("    let ‸await = 1")),
        (Cpl, body("    var ‸break = 1")),
        (Cpl, body("    ‸continue")),
        (Cpl, body("    x.‸ghost")),
        (Cpl, body("    f(1, ‸lemma)")),
        (Cpl, body("    let x = 1 ‸loop")),
        (Cpl, body("    match x {\n        ‸as => 1\n    }")),
        (Cpl, body("    match x {\n        Some(‸async) => 1\n    }")),
        (Cpl, body("    forall ‸await in xs: true")),
        (Work, "change ‸break {\n}\n".into()),
        (Work, "use ‸continue.b\nchange c {\n}\n".into()),
        (Work, "change c {\n    ‸ghost git(\"x\")\n}\n".into()),
        (Work, "change c {\n    task ‸lemma {\n    }\n}\n".into()),
        (
            Work,
            "change c {\n    task t after ‸loop {\n    }\n}\n".into(),
        ),
        (
            Work,
            "change c {\n    task t {\n        ‸as { a }\n    }\n}\n".into(),
        ),
        (
            Work,
            "change c {\n    task t {\n        write { a.‸async }\n    }\n}\n".into(),
        ),
        (
            Work,
            "change c {\n    integrate {\n        require ‸await\n    }\n}\n".into(),
        ),
        (
            Work,
            "change c {\n    integrate {\n        require review(‸break)\n    }\n}\n".into(),
        ),
        (
            Patch,
            "patch ‸continue.body\n    expect base(a.body)\n{\n}\n".into(),
        ),
        (
            Patch,
            "patch a.body\n    expect base(‸ghost.body)\n{\n}\n".into(),
        ),
        (
            Patch,
            "patch a.body\n    expect ‸lemma(a.body)\n{\n}\n".into(),
        ),
        (Expression, "‸loop".into()),
        (Expression, "1 + ‸as".into()),
    ];
    for (form, marked) in cases {
        let (src, d) = reject_at(form, &marked, codes::RESERVED_WORD);
        let word = span_text(&src, &d).to_string();
        assert!(
            RESERVED.contains(&word.as_str()),
            "{form:?} {src:?}: {d:#?}"
        );
        assert_eq!(field(&d, "name"), Some(word.as_str()), "9.4: `name`");
        assert_prefix(&d, &format!("`{word}` is reserved for a future edition"));
    }
}

#[test]
fn d3_a_keyword_in_a_name_position_is_the_generic_syntax_error() {
    let cases: Vec<(Form, String)> = vec![
        (Cpl, "module ‸module\n".into()),
        (Cpl, "module m\n\nuse ‸fn.b\n".into()),
        (Cpl, "module m\n\nuse a.‸if.b\n".into()),
        (Cpl, "module m\n\nuse a.b.{‸struct}\n".into()),
        (Cpl, "module m\n\nfn ‸type() -> Int {\n    1\n}\n".into()),
        (
            Cpl,
            "module m\n\nfn f(‸in: Int) -> Int {\n    1\n}\n".into(),
        ),
        (Cpl, "module m\n\nfn f<‸pub>() -> Int {\n    1\n}\n".into()),
        (Cpl, "module m\n\nstruct S {\n    ‸type: Int\n}\n".into()),
        (Cpl, "module m\n\nenum E {\n    ‸true\n}\n".into()),
        (
            Cpl,
            "module m\n\ncapability C {\n    fn ‸match() -> Int\n}\n".into(),
        ),
        (Cpl, "module m\n\ntype ‸enum(Int)\n".into()),
        (
            Cpl,
            "module m\n\nfn f(using db: Db) -> Int\n    effects { db.‸use }\n{\n    1\n}\n".into(),
        ),
        (Cpl, body("    let ‸if = 1")),
        (Cpl, body("    var ‸for = 1")),
        (Cpl, body("    forall ‸in in xs: true")),
        (Work, "change ‸test {\n}\n".into()),
        (Work, "change c {\n    task ‸fn {\n    }\n}\n".into()),
        (
            Work,
            "change c {\n    task t after ‸if {\n    }\n}\n".into(),
        ),
        (
            Work,
            "change c {\n    task t {\n        write { ‸effects }\n    }\n}\n".into(),
        ),
        (
            Work,
            "change c {\n    task t {\n        write { a.‸type }\n    }\n}\n".into(),
        ),
        (
            Work,
            "change c {\n    integrate {\n        require review(‸let)\n    }\n}\n".into(),
        ),
        (
            Patch,
            "patch ‸fn.body\n    expect base(a.body)\n{\n}\n".into(),
        ),
    ];
    for (form, marked) in cases {
        let (src, d) = reject_at(form, &marked, codes::SYNTAX_ERROR);
        let word = span_text(&src, &d).to_string();
        assert!(
            KEYWORDS.contains(&word.as_str()),
            "{form:?} {src:?}: {d:#?}"
        );
        assert_eq!(field(&d, "found"), Some(format!("`{word}`").as_str()));
        assert!(d.data.contains_key("expected"));
    }
}

#[test]
fn d56_tests_is_a_module_name_and_test_is_not() {
    module_ok("module tests.pagination\n");
    module_ok("module a.tests.testing.test_cases\n");
    let (_, d) = reject_at(Cpl, "module ‸test\n", codes::SYNTAX_ERROR);
    assert_eq!(field(&d, "found"), Some("`test`"));
    reject_at(Cpl, "module tests.‸test\n", codes::SYNTAX_ERROR);
    reject_at(
        Cpl,
        "module m\n\nfn ‸test() -> Int {\n    1\n}\n",
        codes::SYNTAX_ERROR,
    );
}

#[test]
fn d5_contextual_keywords_name_changes_tasks_and_selector_segments() {
    // In `.work` positions that take a name, the contextual keywords are the
    // LOWER_IDENT tokens they always are (grammar 1.4, D5).
    accept(
        Work,
        "change change {\n    task task {\n        write { write.body, base.target }\n    }\n    task base after task {\n    }\n    task after {\n    }\n    task t after after {\n    }\n    integrate {\n        require review(base)\n    }\n}\n",
    );
}

#[test]
fn keyword_and_reserved_word_prefixes_make_ordinary_names() {
    // A spelling is classified as a whole (grammar 1.8 step 4).
    accept(
        Cpl,
        &body("    let loop_count = 1\n    let asx = loop_count\n    let breaks = asx\n    let iff = breaks\n    let types = iff\n    let testing = types\n    let async_x = ?awaiting\n    let lemmas = ?iffy + ?testing + ?types\n    async_x"),
    );
    module_ok(
        "module tests.types.looping\n\nfn fnx(ifs: Int, ass: Int) -> Int {\n    ifs + ass\n}\n",
    );
    accept(
        Work,
        "change loops {\n    task asynchronous {\n        write { types.body, uses.* }\n    }\n}\n",
    );
}

#[test]
fn comment_markers_inside_text_literals_are_text() {
    let m = module_ok(&body(
        "    let x = \"// not a comment\"\n    let y = \"/// nor a doc ///\"\n    let z = \"a\\\"// b\"\n    x",
    ));
    let statements = &first_fn(&m).body.as_ref().expect("a body").statements;
    let texts: Vec<String> = statements
        .iter()
        .map(|s| match s {
            cpl_syntax::ast::Statement::Let(l) => match &l.value.kind {
                ExprKind::Literal(lit) => match &lit.kind {
                    LiteralKind::Text(t) => t.clone(),
                    other => panic!("{other:?}"),
                },
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(texts, ["// not a comment", "/// nor a doc ///", "a\"// b"]);
}

#[test]
fn d5_contextual_keywords_are_ordinary_names_outside_their_positions() {
    module_ok(
        "module m\n\nfn add(base: Int, target: Int, workers: Int) -> Int {\n    let write = base + target\n    let patch = write * workers\n    let change = patch\n    var expect = change\n    expect = expect + 1\n    let task = expect\n    let goal = task\n    let require = goal\n    let typecheck = require\n    let integrate = typecheck\n    let git = integrate\n    let after = git\n    after\n}\n",
    );
    // Inside a patch block `patch`, `expect`, and `base` are identifiers (4.5).
    accept(
        Patch,
        "patch a.body\n    expect base(a.body)\n{\n    let patch = 1\n    let expect = 2\n    let base = patch + expect\n    base\n}\n",
    );
    // And `add`, `write`, `goal` name ordinary declarations.
    module_ok(
        "module m\n\nstruct S {\n    add: Int\n    write: Text\n    goal: Int\n}\n\nfn base(target: Int) -> Int {\n    target\n}\n",
    );
}

#[test]
fn d10_the_consequences_grammar_1_7_spells_out() {
    // "`x? y` is propagation followed by a syntax error; `x?y` lexes as `x`
    // `HOLE(y)` and is a syntax error; `?build_pages` is a hole."
    reject_at(Cpl, &body("    x? ‸y"), codes::SYNTAX_ERROR);
    reject_at(Cpl, &body("    x‸?y"), codes::SYNTAX_ERROR);
    accept(Cpl, &body("    x?\n    ?build_pages"));
    // A `?` before a digit or after a space is the `Question` token.
    let (_, d) = reject_at(Cpl, &body("    let x = ‸?1"), codes::SYNTAX_ERROR);
    assert_eq!(field(&d, "found"), Some("`?`"));
    accept(Cpl, &body("    let x = y ?\n    x"));
}

#[test]
fn d10_holes_named_by_keywords_are_hole_name_keyword() {
    for name in ["if", "loop", "test", "as", "effects", "true"] {
        let (_, d) = reject_at(
            Cpl,
            &body(&format!("    ‸?{name}")),
            codes::HOLE_NAME_KEYWORD,
        );
        assert_prefix(&d, "a hole name cannot be a keyword");
        assert_eq!(field(&d, "name"), Some(name));
    }
    for name in ["Foo", "_", "X1"] {
        let (_, d) = reject_at(Cpl, &body(&format!("    ‸?{name}")), codes::HOLE_NAME_CASE);
        assert_prefix(&d, "hole names are snake_case");
        assert_eq!(field(&d, "name"), Some(name));
    }
    // `self`, `result` (D50), and `_`-prefixed names are holes.
    for name in ["self", "result", "_x", "__", "a1"] {
        accept(Cpl, &body(&format!("    ?{name}")));
    }
}

// ===========================================================================
// Grammar 1.5, D6-D8, D62: malformed literals
// ===========================================================================

#[test]
fn d6_int_literal_errors_in_every_literal_position() {
    let cases: Vec<(Form, String, DiagCode, Option<&str>)> = vec![
        (
            Cpl,
            body("    let x = ‸007"),
            codes::INVALID_INT_LITERAL,
            Some("leading zeros are not permitted"),
        ),
        (
            Cpl,
            body("    let x = ‸0_7"),
            codes::INVALID_INT_LITERAL,
            Some("leading zeros are not permitted"),
        ),
        (
            Cpl,
            body("    let x = ‸00"),
            codes::INVALID_INT_LITERAL,
            Some("leading zeros are not permitted"),
        ),
        (
            Cpl,
            body("    let x = ‸1.5"),
            codes::FLOAT_LITERAL_UNSUPPORTED,
            Some("floating-point literals are not supported in edition 0.1"),
        ),
        (
            Cpl,
            body("    let x = ‸0.0"),
            codes::FLOAT_LITERAL_UNSUPPORTED,
            Some("floating-point literals are not supported in edition 0.1"),
        ),
        (
            Cpl,
            body("    let x = ‸1e5"),
            codes::LITERAL_ADJACENT_IDENTIFIER,
            Some("a literal must be separated from an identifier"),
        ),
        (
            Cpl,
            body("    let x = ‸2x"),
            codes::LITERAL_ADJACENT_IDENTIFIER,
            Some("a literal must be separated from an identifier"),
        ),
        (
            Cpl,
            body("    let x = ‸0x1F"),
            codes::LITERAL_ADJACENT_IDENTIFIER,
            None,
        ),
        (
            Cpl,
            body("    let x = ‸1_000abc"),
            codes::LITERAL_ADJACENT_IDENTIFIER,
            None,
        ),
        (
            Cpl,
            body("    match x {\n        ‸007 => 1\n    }"),
            codes::INVALID_INT_LITERAL,
            None,
        ),
        (
            Cpl,
            body("    match x {\n        -‸1.5 => 1\n    }"),
            codes::FLOAT_LITERAL_UNSUPPORTED,
            None,
        ),
        (
            Cpl,
            "module m\n\n@id(‸1.5)\nfn f() -> Int {\n    1\n}\n".into(),
            codes::FLOAT_LITERAL_UNSUPPORTED,
            None,
        ),
        (
            Cpl,
            "module m\n\n@id(‸08)\nfn f() -> Int {\n    1\n}\n".into(),
            codes::INVALID_INT_LITERAL,
            None,
        ),
        (
            Work,
            "change c {\n    workers ‸007\n}\n".into(),
            codes::INVALID_INT_LITERAL,
            None,
        ),
        (
            Work,
            "change c {\n    workers ‸2x\n}\n".into(),
            codes::LITERAL_ADJACENT_IDENTIFIER,
            None,
        ),
        (
            Work,
            "change c {\n    workers ‸3.0\n}\n".into(),
            codes::FLOAT_LITERAL_UNSUPPORTED,
            None,
        ),
        (Expression, "‸01".into(), codes::INVALID_INT_LITERAL, None),
    ];
    for (form, marked, code, prefix) in cases {
        let (_, d) = reject_at(form, &marked, code);
        if let Some(prefix) = prefix {
            assert_prefix(&d, prefix);
        }
    }
    // A misplaced separator (`_` only between two digits): the grammar quotes
    // no message for it, and whether its span is the separator or the whole
    // literal is not fixed either (D59 says "the offending token"); the
    // diagnostic lies within the literal.
    for (form, src, literal) in [
        (Cpl, body("    let x = 1__0"), "1__0"),
        (Cpl, body("    let x = 1_000_"), "1_000_"),
        (Cpl, body("    let x = 3_"), "3_"),
        (
            Work,
            "change c {\n    workers 1__2\n}\n".to_string(),
            "1__2",
        ),
    ] {
        let d = reject(form, &src);
        assert_eq!(d.code, codes::INVALID_INT_LITERAL, "{src:?}: {d:#?}");
        let start = src.find(literal).expect("literal") as u32;
        let span = d.span.expect("span");
        assert!(
            span.start >= start && span.end <= start + literal.len() as u32,
            "{src:?}: {d:#?}"
        );
    }
}

#[test]
fn d6_valid_int_literals_and_lookalikes_that_are_not_floats() {
    let m = module_ok(&body(
        "    let a = 0\n    let b = 10\n    let c = 1_000_000\n    let d = -0\n    let e = 1.len\n    let f = 1.e5\n    let g = 99999999999999999999999999999999999999\n    a",
    ));
    let statements = &first_fn(&m).body.as_ref().unwrap().statements;
    let digits = |i: usize| match &statements[i] {
        cpl_syntax::ast::Statement::Let(l) => match &l.value.kind {
            ExprKind::Literal(lit) => match &lit.kind {
                LiteralKind::Int(d) => d.clone(),
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    };
    assert_eq!(digits(2), "1000000", "the digit string without separators");
    assert_eq!(digits(6), "99999999999999999999999999999999999999");
    // `1.e5` is a member access: a `.` followed by a digit makes a float, a
    // `.` followed by a letter does not (1.5).
    let ExprKind::Member(_) = &(match &statements[5] {
        cpl_syntax::ast::Statement::Let(l) => &l.value,
        other => panic!("{other:?}"),
    })
    .kind
    else {
        panic!("`1.e5` is a member access")
    };
    // With a space the `.` is a member access whose name is missing.
    let (_, d) = reject_at(Cpl, &body("    let x = 1 .‸5"), codes::SYNTAX_ERROR);
    assert_prefix(&d, "expected a member name");
    let (_, d) = reject_at(Cpl, &body("    let x = 1. ‸5"), codes::SYNTAX_ERROR);
    assert_prefix(&d, "expected a member name");
    // `..` is the spread token, never a range (D54).
    let (_, d) = reject_at(Cpl, &body("    let x = 1‸..2"), codes::SYNTAX_ERROR);
    assert_eq!(field(&d, "found"), Some("`..`"));
    // `_1` is an identifier, not a literal.
    accept(Cpl, &body("    let x = _1\n    x"));
}

#[test]
fn d62_control_characters_in_text_literals() {
    for b in (0u8..=0x1f).chain([0x7f]) {
        if matches!(b, b'\t' | b'\n' | b'\r') {
            continue;
        }
        let c = b as char;
        let (_, d) = reject_at(
            Cpl,
            &body(&format!("    let x = \"a‸{c}b\"\n    1")),
            codes::INVALID_CHARACTER,
        );
        assert_prefix(&d, "control character in text literal");
        assert!(d.data.contains_key("found"), "9.4: `found`: {d:#?}");
    }
    // Tab, the C1 range, and every other non-ASCII scalar value are permitted raw.
    let e = expr_ok("\"a\tb\u{85}\u{9f}\u{a0}é漢\u{feff}\u{10ffff}\"");
    let ExprKind::Literal(lit) = &e.kind else {
        panic!("a literal")
    };
    assert_eq!(
        lit.kind,
        LiteralKind::Text("a\tb\u{85}\u{9f}\u{a0}é漢\u{feff}\u{10ffff}".into())
    );
}

#[test]
fn d7_a_raw_line_terminator_ends_a_text_literal() {
    for marked in [
        body("    let x = ‸\"a\rb\"\n    x"),
        body("    let x = ‸\"a\r\nb\"\n    x"),
        body("    let x = ‸\"a\nb\"\n    x"),
        body("    let x = ‸\"a\\\nb\"\n    x"),
    ] {
        let (_, d) = reject_at(Cpl, &marked, codes::UNTERMINATED_TEXT_LITERAL);
        assert_prefix(&d, "unterminated text literal");
    }
    reject_at(Expression, "‸\"abc", codes::UNTERMINATED_TEXT_LITERAL);
    reject_at(
        Work,
        "change c {\n    task t {\n        goal ‸\"open\n    }\n}\n",
        codes::UNTERMINATED_TEXT_LITERAL,
    );
    // The lexer closes the literal at the line end (D70), so the next line is
    // read normally and a later error does not replace the earlier one.
    reject_at(
        Cpl,
        &body("    let x = ‸\"open\n    let y = #"),
        codes::UNTERMINATED_TEXT_LITERAL,
    );
}

#[test]
fn text_escape_boundaries_decode_exactly() {
    let e = expr_ok(
        r#""\\ \" \n \r \t \u{41} \u{0} \u{10FFFF} \u{D7FF} \u{E000} \u{000041} \u{fffd} \u{1F600}""#,
    );
    let ExprKind::Literal(lit) = &e.kind else {
        panic!("a literal")
    };
    assert_eq!(
        lit.kind,
        LiteralKind::Text(
            "\\ \" \n \r \t A \u{0} \u{10FFFF} \u{D7FF} \u{E000} A \u{fffd} \u{1F600}".into()
        )
    );
}

#[test]
fn invalid_escapes_in_every_text_position() {
    for escape in [
        r"\q",
        r"\0",
        r"\x41",
        r"\'",
        r"\U{41}",
        r"\u41",
        r"\u{}",
        r"\u{g}",
        r"\u{41",
        r"\u{110000}",
        r"\u{D800}",
        r"\u{DFFF}",
        r"\u{0000041}",
        r"\ ",
        r"\é",
    ] {
        let src = format!("\"a{escape}b\"");
        let d = reject(Expression, &src);
        assert_eq!(d.code, codes::INVALID_ESCAPE, "{src:?}: {d:#?}");
        assert!(d.data.contains_key("found"), "9.4: `found`: {d:#?}");
        if escape == r"\u{0000041}" {
            assert_prefix(&d, "unicode escape has more than six hex digits");
        }
    }
    for (form, src) in [
        (
            Work,
            "change c {\n    task t {\n        goal \"a\\qb\"\n    }\n}\n".to_string(),
        ),
        (
            Work,
            "change c {\n    base git(\"\\u{D800}\")\n}\n".to_string(),
        ),
        (
            Cpl,
            "module m\n\n@id(\"\\x\")\nfn f() -> Int {\n    1\n}\n".to_string(),
        ),
        (Cpl, body("    match x {\n        \"\\q\" => 1\n    }")),
        (
            Patch,
            "patch a.body\n    expect base(a.body)\n{\n    \"\\u{110000}\"\n}\n".to_string(),
        ),
    ] {
        assert_eq!(reject(form, &src).code, codes::INVALID_ESCAPE, "{src:?}");
    }
}

#[test]
fn d8_prefixed_text_literals_are_reserved_in_every_position() {
    let cases: Vec<(Form, String)> = vec![
        (Expression, "‸b\"x\"".into()),
        (Expression, "‸B\"x\"".into()),
        (Expression, "‸r\"x\"".into()),
        (Expression, "‸_\"x\"".into()),
        (Expression, "‸if\"x\"".into()),
        (Expression, "‸loop\"x\"".into()),
        (Expression, "f(‸bytes\"x\")".into()),
        (Cpl, "module m\n\n‸test\"t\" {\n}\n".into()),
        (Cpl, body("    match x {\n        ‸b\"x\" => 1\n    }")),
        (
            Cpl,
            "module m\n\n@id(‸b\"x\")\nfn f() -> Int {\n    1\n}\n".into(),
        ),
        (
            Work,
            "change c {\n    task t {\n        ‸goal\"x\"\n    }\n}\n".into(),
        ),
        (Work, "change c {\n    base ‸git\"main\"\n}\n".into()),
        (Patch, "‸patch\"x\"\n".into()),
    ];
    for (form, marked) in cases {
        let (_, d) = reject_at(form, &marked, codes::PREFIXED_LITERAL_RESERVED);
        assert_prefix(&d, "prefixed string literals are reserved");
    }
    // With a space between them, or after a literal, it is not D8.
    reject_at(Expression, "b ‸\"x\"", codes::SYNTAX_ERROR);
    reject_at(Expression, "1‸\"x\"", codes::SYNTAX_ERROR);
    reject_at(Expression, "\"x\"‸b", codes::SYNTAX_ERROR);
}

#[test]
fn non_ascii_outside_text_literals_and_comments() {
    let cases: Vec<(Form, String)> = vec![
        (Cpl, body("    let caf‸é = 1")),
        (Cpl, body("    let x = 1 ‸– 2")),
        (Cpl, body("    let x = 1‸\u{a0}+ 2")),
        (Cpl, "module m\n‸\u{feff}\n".into()),
        (
            Work,
            "change c {\n    task t {\n        write { caf‸é }\n    }\n}\n".into(),
        ),
        (Patch, "patch a.b‸ö\n".into()),
        (Expression, "‸λ".into()),
    ];
    for (form, marked) in cases {
        let (src, d) = reject_at(form, &marked, codes::NON_ASCII_OUTSIDE_TEXT);
        let found = field(&d, "found").expect("9.4: `found`");
        assert!(found.contains(span_text(&src, &d)), "{found:?} {d:#?}");
    }
    // Comments (D1), doc comments, and text literals may hold any of it; a
    // leading byte-order mark is skipped.
    module_ok(
        "\u{feff}/// Dokumentation für Ü\nmodule m // 漢字\n\n// ∀x ∈ S → y\nfn f() -> Text {\n    \"ü\" // é\n}\n",
    );
    accept(Work, "\u{feff}change c {\n}\n");
    accept(
        Patch,
        "\u{feff}patch a.body\n    expect base(a.body)\n{\n}\n",
    );
    // Offsets keep counting the mark's bytes; only a leading mark is skipped.
    reject_at(Cpl, "\u{feff}‸fn f() -> Int {\n}\n", codes::SYNTAX_ERROR);
    reject_at(
        Cpl,
        "\u{feff}‸\u{feff}module m\n",
        codes::NON_ASCII_OUTSIDE_TEXT,
    );
    open_at_eof(Work, "\u{feff}", Some("expected `change`"));
}

#[test]
fn invalid_characters_carry_the_grammar_messages() {
    for (ch, prefix) in [
        (";", "semicolons are not part of CPL"),
        ("!", "use not"),
        ("&", "use and / or"),
        ("&&", "use and / or"),
        ("|", "use and / or"),
        ("||", "use and / or"),
        ("#", "unexpected character `#`"),
        ("$", "unexpected character `$`"),
        ("'", "unexpected character `'`"),
        ("`", "unexpected character `"),
        ("^", "unexpected character `^`"),
        ("~", "unexpected character `~`"),
        ("\\", "unexpected character `\\`"),
        ("\u{0}", "unexpected character U+0000"),
        ("\u{1}", "unexpected character U+0001"),
        ("\u{b}", "unexpected character U+000B"),
        ("\u{c}", "unexpected character U+000C"),
        ("\u{7f}", "unexpected character U+007F"),
    ] {
        let (_, d) = reject_at(
            Cpl,
            &body(&format!("    x ‸{ch} y")),
            codes::INVALID_CHARACTER,
        );
        assert_prefix(&d, prefix);
        assert!(d.data.contains_key("found"), "9.4: `found`: {d:#?}");
    }
    // `!=` is an operator; only a `!` alone is an error.
    accept(Cpl, &body("    x != y"));
    let (_, d) = reject_at(Cpl, &body("    x ‸! = y"), codes::INVALID_CHARACTER);
    assert_prefix(&d, "use not");
}

#[test]
fn a_lone_carriage_return_is_invalid_line_terminator() {
    reject_at(
        Cpl,
        &body("    let x = 1‸\r    let y = 2\n    x"),
        codes::INVALID_LINE_TERMINATOR,
    );
    reject_at(
        Cpl,
        &body("    let x = 1‸\r\r\n    x"),
        codes::INVALID_LINE_TERMINATOR,
    );
    reject_at(Work, "change c {‸\r}\n", codes::INVALID_LINE_TERMINATOR);
    reject_at(Expression, "1‸\r", codes::INVALID_LINE_TERMINATOR);
    // CRLF is one terminator everywhere.
    module_ok("module m\r\n\r\n/// d\r\nfn f() -> Int\r\n    requires true\r\n{\r\n    1\r\n}\r\n");
    accept(Work, "change c {\r\n    base git(\"main\")\r\n}\r\n");
    accept(
        Patch,
        "patch a.body\r\n    expect base(a.body)\r\n{\r\n}\r\n",
    );
}

#[test]
fn invalid_utf8_is_the_load_time_parse_code() {
    // D70: raised when the bytes are read (toolchain 1.4 `add_bytes`), with
    // the `parse` code the syntax crate's catalogue names.
    let mut sm = SourceMap::new();
    let (_, d) = sm.add_bytes("src/m.cpl".into(), FileOrigin::Project, b"module m\n\xff\n");
    let d = d.expect("invalid_utf8");
    assert_eq!(d.code, codes::INVALID_UTF8);
    assert_eq!(d.category, codes::CATEGORY);
    assert_eq!(codes::ALL[0], codes::INVALID_UTF8, "9.4 table order");
    assert_eq!(codes::ALL.len(), 21);
}

// ===========================================================================
// Constructs open at the end of the file
// ===========================================================================

/// Every input ends inside a construct: the single diagnostic is the generic
/// syntax error with `found` = `end of file`, at the end of the text; a
/// quoted message is checked as a prefix (D72).
#[track_caller]
fn open_at_eof(form: Form, src: &str, prefix: Option<&str>) {
    let d = reject(form, src);
    assert_eq!(d.code, codes::SYNTAX_ERROR, "{form:?} {src:?}: {d:#?}");
    assert_generic(&d, prefix.unwrap_or("expected "), "end of file");
    let span = d.span.expect("span");
    assert_eq!(
        (span.start, span.end),
        (src.len() as u32, src.len() as u32),
        "{form:?} {src:?}: at the end of the file"
    );
}

#[test]
fn constructs_open_at_the_end_of_an_cpl_file_report_end_of_file() {
    for (src, prefix) in [
        ("", None),
        ("// only a comment\n", None),
        ("\u{feff}", None),
        ("module", None),
        ("module a.", None),
        ("module m\n\nuse", None),
        ("module m\n\nuse a", None),
        ("module m\n\nuse a.b.{c,", None),
        ("module m\n\n@id(", None),
        ("module m\n\n@id(\"x\"", None),
        ("module m\n\n@prove", None),
        ("module m\n\n@prove\n", None),
        ("module m\n\npub", None),
        ("module m\n\ntype", None),
        ("module m\n\ntype A =", None),
        ("module m\n\ntype A = Int where", None),
        ("module m\n\ntype A(", None),
        ("module m\n\nstruct S", Some("expected `{`")),
        ("module m\n\nstruct S {", None),
        ("module m\n\nstruct S {\n    a:", None),
        ("module m\n\nstruct S<", None),
        ("module m\n\nenum E {\n    A(", None),
        ("module m\n\nenum E {\n    A(Int,", None),
        ("module m\n\ncapability C", Some("expected `{`")),
        ("module m\n\ncapability C {\n    fn get(", None),
        ("module m\n\ncapability C {\n    fn get() ->", None),
        ("module m\n\nfn", None),
        ("module m\n\nfn f(", None),
        ("module m\n\nfn f()", Some("expected `->`")),
        ("module m\n\nfn f() ->", None),
        ("module m\n\nfn f() -> List<", None),
        ("module m\n\nfn f() -> List<Int", None),
        ("module m\n\nfn f() -> Int {", None),
        ("module m\n\nfn f() -> Int\n    requires", None),
        ("module m\n\nfn f() -> Int\n    effects {", None),
        ("module m\n\nfn f() -> Int\n    effects { db.", None),
        ("module m\n\ntest", None),
        ("module m\n\ntest \"t\"", Some("expected `{`")),
        (
            "module m\n\nproperty \"p\"",
            Some("expected a `for` clause or `{`"),
        ),
        ("module m\n\nproperty \"p\"\n    for", None),
        ("module m\n\nproperty \"p\"\n    for x", None),
        ("module m\n\nproperty \"p\"\n    for x in", None),
        (
            "module m\n\nproperty \"p\"\n    for x in g",
            Some("expected a `for` clause or `{`"),
        ),
    ] {
        open_at_eof(Cpl, src, prefix);
    }
    // D64 has a message of its own, still at the end of the file.
    let src = "module m\n\ntype A = Int";
    let d = reject(Cpl, src);
    assert_prefix(&d, TYPE_EQUATION);
    assert_eq!(d.span.expect("span").start, src.len() as u32);
}

#[test]
fn constructs_open_at_the_end_of_a_body_report_end_of_file() {
    for (stmt, prefix) in [
        ("let", None),
        ("let x", None),
        ("let x =", None),
        ("let x: ", None),
        ("let x: List<", None),
        ("var", None),
        ("x =", None),
        ("return 1 +", None),
        ("assert", None),
        ("while", None),
        ("while c", Some("expected `{` or a clause")),
        ("while c\n        invariant", None),
        (
            "while c\n        invariant x",
            Some("expected `{` or a clause"),
        ),
        ("if", None),
        ("if c", Some("expected `{`")),
        ("if c {", None),
        ("if c { 1 } else", Some("expected `{`")),
        ("if c { 1 } else if", None),
        ("match", None),
        ("match x", Some("expected `{`")),
        ("match x {", None),
        ("match x {\n        _", None),
        ("match x {\n        _ =>", None),
        ("match x {\n        Some(", None),
        ("match x {\n        -", None),
        ("match x {\n        a.", None),
        ("(", None),
        ("(1", None),
        ("[", None),
        ("[1,", None),
        ("f(", None),
        ("f(1,", None),
        ("f(using", None),
        ("A {", None),
        ("A { a", None),
        ("A { a:", None),
        ("A { ..", None),
        ("A { ..p,", None),
        ("x.", None),
        ("x?.", None),
        ("not", None),
        ("-", None),
        ("forall", None),
        ("forall x", None),
        ("forall x in", None),
        ("forall x in xs", None),
        ("forall x in xs:", None),
        ("1 +", None),
        ("1 <", None),
        ("a and", None),
        ("check<", None),
    ] {
        open_at_eof(
            Cpl,
            &format!("module m\n\nfn f() -> Int {{\n    {stmt}"),
            prefix,
        );
    }
}

#[test]
fn constructs_open_at_the_end_of_a_work_file_report_end_of_file() {
    let no_change = Some("expected `change` (a .work file declares at least one change)");
    for (src, prefix) in [
        ("", no_change),
        ("\n\n// comment only\n", no_change),
        ("use a.b.{c}\n", no_change),
        ("use a.b.{c}\nuse d.e\n", no_change),
        ("use", None),
        ("change", None),
        ("change c", Some("expected `{`")),
        ("change c {", None),
        ("change c {\n    base", None),
        ("change c {\n    base git", None),
        ("change c {\n    base git(", None),
        ("change c {\n    base git(\"main\"", None),
        ("change c {\n    workers", None),
        ("change c {\n    task", None),
        ("change c {\n    task t", Some("expected `{`")),
        ("change c {\n    task t after", None),
        ("change c {\n    task t after a,", None),
        ("change c {\n    task t {", None),
        ("change c {\n    task t {\n        write", None),
        ("change c {\n    task t {\n        write {", None),
        ("change c {\n    task t {\n        write { a.", None),
        ("change c {\n    task t {\n        write { a,", None),
        ("change c {\n    task t {\n        goal", None),
        ("change c {\n    integrate", Some("expected `{`")),
        ("change c {\n    integrate {", None),
        (
            "change c {\n    integrate {\n        require",
            Some(NO_REQUIREMENT),
        ),
        ("change c {\n    integrate {\n        require prove(", None),
        ("change c {\n    integrate {\n        require prove(a", None),
        ("change c {\n    integrate {\n        require review(", None),
        ("change c {\n    integrate {\n        require test", None),
    ] {
        open_at_eof(Work, src, prefix);
    }
}

#[test]
fn constructs_open_at_the_end_of_a_patch_file_report_end_of_file() {
    for (src, prefix) in [
        ("", None),
        ("// comment only\n", None),
        ("patch", None),
        ("patch a.", None),
        ("patch a.body", Some("expected `expect` on its own line")),
        ("patch a.body\n    expect", None),
        ("patch a.body\n    expect base", None),
        ("patch a.body\n    expect base(", None),
        ("patch a.body\n    expect base(a.body", None),
        (
            "patch a.body\n    expect base(a.body)",
            Some("expected `{` on its own line"),
        ),
        ("patch a.body\n    expect base(a.body)\n{", None),
        (
            "patch a.body\n    expect base(a.body)\n{\n    let x =",
            None,
        ),
        (
            "patch a.body\n    expect base(a.body)\n{\n}\n\npatch b.body",
            Some("expected `expect` on its own line"),
        ),
    ] {
        open_at_eof(Patch, src, prefix);
    }
}

#[test]
fn expressions_open_at_the_end_of_parse_expr_input_report_end_of_file() {
    for (src, prefix) in [
        ("", None),
        ("\n\n", None),
        ("(", None),
        ("[1,", None),
        ("f(", None),
        ("-", None),
        ("not", None),
        ("x.", None),
        ("if c", Some("expected `{`")),
        ("if c {", None),
        ("if c { 1 } else", Some("expected `{`")),
        ("match x", Some("expected `{`")),
        ("match x {", None),
        ("A {", None),
        ("forall x in xs:", None),
        ("1 +", None),
        ("a.b(", None),
        ("Page { ..p, text:", None),
    ] {
        open_at_eof(Expression, src, prefix);
    }
}

#[test]
fn lexical_constructs_open_at_the_end_of_a_file() {
    for (form, marked, code) in [
        (
            Cpl,
            "module m\n\nfn f() -> Int {\n    let x = ‸\"abc",
            codes::UNTERMINATED_TEXT_LITERAL,
        ),
        (
            Cpl,
            "module m\n\nfn f() -> Int {\n    let x = ‸\"abc\\",
            codes::UNTERMINATED_TEXT_LITERAL,
        ),
        (
            Cpl,
            "module m\n\nfn f() -> Int {\n    let x = ‸\"\\u{4",
            codes::UNTERMINATED_TEXT_LITERAL,
        ),
        (Cpl, "module m\n\n‸/// doc", codes::DOC_COMMENT_POSITION),
        (Cpl, "‸/// doc", codes::DOC_COMMENT_POSITION),
        (
            Cpl,
            "module m\n\n@prove\n‸/// doc\n",
            codes::DOC_COMMENT_POSITION,
        ),
        (
            Cpl,
            "module m\n\nfn f() -> Int {\n    1\n}\n‸/// a\n/// b\n",
            codes::DOC_COMMENT_POSITION,
        ),
        (Work, "‸/// doc", codes::DOC_COMMENT_POSITION),
        (
            Work,
            "change c {\n}\n‸/// doc\n",
            codes::DOC_COMMENT_POSITION,
        ),
        (Patch, "‸/// doc", codes::DOC_COMMENT_POSITION),
        (Expression, "‸\"", codes::UNTERMINATED_TEXT_LITERAL),
        (Expression, "1 + ‸?Hole", codes::HOLE_NAME_CASE),
    ] {
        let (_, d) = reject_at(form, marked, code);
        if code == codes::DOC_COMMENT_POSITION {
            assert_prefix(&d, MISPLACED_DOC);
        }
    }
}

// ===========================================================================
// Messages and spans the grammar fixes
// ===========================================================================

#[test]
fn rule3_a_brace_only_header_without_its_brace() {
    let cases: Vec<(Form, String, &str)> = vec![
        (Cpl, body("    if c‸\n    x"), "end of line"),
        (Cpl, body("    if c ‸x {\n    }"), "identifier `x`"),
        (
            Cpl,
            body("    if c {\n        1\n    } else‸\n    x"),
            "end of line",
        ),
        (Cpl, body("    match x‸\n    y"), "end of line"),
        (
            Cpl,
            "module m\n\ntest \"t\"‸\nfn f() -> Int {\n    1\n}\n".into(),
            "end of line",
        ),
        (
            Cpl,
            "module m\n\nstruct S‸\nfn f() -> Int {\n    1\n}\n".into(),
            "end of line",
        ),
        (Cpl, "module m\n\nstruct S ‸= 1\n".into(), "`=`"),
        (
            Cpl,
            "module m\n\nenum E<T>‸\nfn f() -> Int {\n    1\n}\n".into(),
            "end of line",
        ),
        (
            Cpl,
            "module m\n\ncapability C‸\nfn f() -> Int {\n    1\n}\n".into(),
            "end of line",
        ),
        (Work, "change c‸\nchange d {\n}\n".into(), "end of line"),
        (
            Work,
            "change c {\n    task t‸\n    base git(\"x\")\n}\n".into(),
            "end of line",
        ),
        (
            Work,
            "change c {\n    task t after a, b‸\n    base git(\"x\")\n}\n".into(),
            "end of line",
        ),
        (
            Work,
            "change c {\n    integrate‸\n    base git(\"x\")\n}\n".into(),
            "end of line",
        ),
    ];
    for (form, marked, found) in cases {
        let (_, d) = reject_at(form, &marked, codes::SYNTAX_ERROR);
        assert_generic(&d, "expected `{`", found);
    }
}

#[test]
fn rule3_while_property_and_patch_headers_without_their_bodies() {
    let cases: Vec<(Form, String, &str)> = vec![
        (Cpl, body("    while c‸\n    x"), "expected `{` or a clause"),
        (
            Cpl,
            body("    while c\n        invariant x‸\n    y"),
            "expected `{` or a clause",
        ),
        (
            Cpl,
            "module m\n\nproperty \"p\"‸\nfn f() -> Int {\n    1\n}\n".into(),
            "expected a `for` clause or `{`",
        ),
        (
            Cpl,
            "module m\n\nproperty \"p\"\n    for x in g‸\nfn f() -> Int {\n    1\n}\n".into(),
            "expected a `for` clause or `{`",
        ),
        (
            Patch,
            "patch a.body‸\n{\n}\n".into(),
            "expected `expect` on its own line",
        ),
        (
            Patch,
            "patch a.body‸\n    expects base(a.body)\n{\n}\n".into(),
            "expected `expect` on its own line",
        ),
        (
            Patch,
            "patch a.body\n    expect base(a.body)‸\npatch b.body\n".into(),
            "expected `{` on its own line",
        ),
        (
            Patch,
            "patch a.body\n    expect base(a.body)‸\n    expect base(a.body)\n{\n}\n".into(),
            "expected `{` on its own line",
        ),
    ];
    for (form, marked, prefix) in cases {
        let (_, d) = reject_at(form, &marked, codes::SYNTAX_ERROR);
        assert_generic(&d, prefix, "end of line");
    }
}

#[test]
fn rule3_a_doc_comment_after_a_header_is_the_offending_token() {
    // Grammar 1.2: "one between a header and its `{`", "one before the
    // `expect` line of a patch form"; reported at the doc comment.
    for (form, marked) in [
        (Cpl, body("    if c\n‸/// d\n    {\n    }")),
        (Cpl, body("    if c {\n    } else\n‸/// d\n    {\n    }")),
        (Cpl, body("    match x\n‸/// d\n    {\n    }")),
        (Cpl, body("    while c\n‸/// d\n    {\n    }")),
        (
            Cpl,
            body("    while c\n        invariant x\n‸/// d\n    {\n    }"),
        ),
        (Cpl, "module m\n\ntest \"t\"\n‸/// d\n{\n}\n".into()),
        (Cpl, "module m\n\nstruct S\n‸/// d\n{\n}\n".into()),
        (
            Cpl,
            "module m\n\nproperty \"p\"\n    for x in g\n‸/// d\n{\n}\n".into(),
        ),
        (Work, "change c\n‸/// d\n{\n}\n".into()),
        (
            Work,
            "change c {\n    task t\n‸/// d\n    {\n    }\n}\n".into(),
        ),
        (
            Patch,
            "patch a.body\n‸/// d\n    expect base(a.body)\n{\n}\n".into(),
        ),
        (
            Patch,
            "patch a.body\n    expect base(a.body)\n‸/// d\n{\n}\n".into(),
        ),
    ] {
        let (_, d) = reject_at(form, &marked, codes::DOC_COMMENT_POSITION);
        assert_prefix(&d, MISPLACED_DOC);
        assert_no_generic_fields(&d);
    }
}

#[test]
fn the_return_type_arrow_is_mandatory() {
    for (marked, found) in [
        ("module m\n\nfn f() ‸{\n    1\n}\n", "`{`"),
        ("module m\n\nfn f()‸\n", "end of line"),
        ("module m\n\nfn f() ‸: Int {\n    1\n}\n", "`:`"),
        ("module m\n\nfn f() ‸Int {\n    1\n}\n", "identifier `Int`"),
        (
            "module m\n\ncapability C {\n    fn get()‸\n}\n",
            "end of line",
        ),
    ] {
        let (_, d) = reject_at(Cpl, marked, codes::SYNTAX_ERROR);
        assert_generic(&d, "expected `->`", found);
    }
}

#[test]
fn a_line_break_after_an_operator_or_equals_sign_is_expected_expression_found_end_of_line() {
    for (form, marked) in [
        (Cpl, body("    let total = a +‸\n        b")),
        (Cpl, body("    let x =‸\n        compute()")),
        (Cpl, body("    x =‸\n        1")),
        (Cpl, body("    return a -‸\n        1")),
        (Cpl, body("    assert‸\n        x")),
        (Cpl, body("    if a and‸\n        b {\n    }")),
        (Cpl, body("    let x = not‸\n        y")),
        (
            Cpl,
            "module m\n\ntype A = Int where‸\n    self > 0\n".into(),
        ),
        (
            Cpl,
            "module m\n\nfn f(x: Int) -> Int\n    requires‸\n    x > 0\n{\n    x\n}\n".into(),
        ),
        (
            Patch,
            "patch a.body\n    expect base(a.body)\n{\n    let x = 1 *‸\n        2\n}\n".into(),
        ),
    ] {
        let (_, d) = reject_at(form, &marked, codes::SYNTAX_ERROR);
        assert_generic(&d, "expected expression, found end of line", "end of line");
        assert_eq!(field(&d, "expected"), Some("expression"));
    }
}

#[test]
fn d55_separators_name_the_element_and_carry_no_generic_fields() {
    for (form, marked, message) in [
        (
            Cpl,
            body("    1‸, 2"),
            "statements are separated by newlines",
        ),
        (
            Cpl,
            body("    let x = f(a, b)‸,\n    x"),
            "statements are separated by newlines",
        ),
        (
            Cpl,
            "module m\n\nstruct S {\n    a: Int‸,\n    b: Int\n}\n".into(),
            "fields are separated by newlines",
        ),
        (
            Cpl,
            "module m\n\nenum E {\n    A(Int)‸, B\n}\n".into(),
            "variants are separated by newlines",
        ),
        (
            Cpl,
            "module m\n\ncapability C {\n    fn get() -> Int‸,\n}\n".into(),
            "operations are separated by newlines",
        ),
        (
            Cpl,
            body("    match x {\n        _ => 1‸,\n    }"),
            "match arms are separated by newlines",
        ),
        (
            Cpl,
            body("    match x {\n        _ => {\n        }‸,\n    }"),
            "match arms are separated by newlines",
        ),
        (
            Work,
            "change c {\n    base git(\"a\")‸,\n}\n".into(),
            "change items are separated by newlines",
        ),
        (
            Work,
            "change c {\n    task t {\n        goal \"g\"‸,\n    }\n}\n".into(),
            "task items are separated by newlines",
        ),
        (
            Work,
            "change c {\n    integrate {\n        require typecheck‸,\n    }\n}\n".into(),
            "requirements are separated by newlines",
        ),
        (
            Patch,
            "patch a.body\n    expect base(a.body)\n{\n    let x = 1‸,\n    x\n}\n".into(),
            "statements are separated by newlines",
        ),
    ] {
        let (src, d) = reject_at(form, &marked, codes::SYNTAX_ERROR);
        assert_prefix(&d, message);
        assert_no_generic_fields(&d);
        assert_eq!(span_text(&src, &d), ",");
    }
}

#[test]
fn d64_a_type_equation_needs_a_where_predicate() {
    for marked in [
        "module m\n\ntype A = Int‸\n",
        "module m\n\npub type A = List<Int>‸\nfn f() -> Int {\n    1\n}\n",
        "module m\n\ntype A = Int ‸if self > 0\n",
        "module m\n\ntype A = reader.pages.Page // an alias‸\n",
    ] {
        let (_, d) = reject_at(Cpl, marked, codes::SYNTAX_ERROR);
        assert_prefix(&d, TYPE_EQUATION);
        assert_no_generic_fields(&d);
    }
}

#[test]
fn rule2_the_message_of_a_struct_literal_after_a_variant_path() {
    for src in [
        body("    let s = StoreError.Missing { id: x }\n    s"),
        body("    let s = a.B.c.D { id: x }\n    s"),
        body("    reader.pages.StoreError.Missing {}"),
    ] {
        let d = reject(Cpl, &src);
        assert_eq!(d.code, codes::SYNTAX_ERROR, "{d:#?}");
        assert_prefix(&d, VARIANT_STRUCT);
        assert_no_generic_fields(&d);
    }
}

#[test]
fn rule2_a_struct_literal_after_a_variant_path_is_reported_at_its_brace() {
    // Grammar 3.12 rule 2: "the `{` is a syntax error (`a struct literal
    // names a type; variants have positional payloads`)"; D59: the span is
    // that of the offending token.
    for marked in [
        body("    let s = StoreError.Missing ‸{ id: x }\n    s"),
        body("    let s = a.B.c.D ‸{ id: x }\n    s"),
    ] {
        let (src, d) = reject_at(Cpl, &marked, codes::SYNTAX_ERROR);
        assert_prefix(&d, VARIANT_STRUCT);
        assert_eq!(span_text(&src, &d), "{");
    }
}

#[test]
fn rule2_the_struct_literal_brace_decides_the_earliest_error() {
    // The `{` is the syntax error, so a lexical error between the variant
    // path and the `{` is earlier and is the one reported (D70).
    reject_at(
        Cpl,
        &body("    let s = StoreError.Missing ‸# { id: x }\n    s"),
        codes::INVALID_CHARACTER,
    );
}

#[test]
fn d13_use_after_a_declaration() {
    for marked in [
        "module m\n\nfn f() -> Int {\n    1\n}\n‸use a.b\n",
        "module m\n\nuse a.b\npub type A(Int)\n\n‸use c.d\n",
        "module m\n\nfn f() -> Int\n‸use a.b\n",
        "module m\n\ntest \"t\" {\n}\n‸use a.{b}\n",
    ] {
        let (_, d) = reject_at(Cpl, marked, codes::USE_AFTER_DECLARATION);
        assert_prefix(&d, "use declarations must precede all other declarations");
        assert_no_generic_fields(&d);
    }
    let (_, d) = reject_at(
        Work,
        "use a.b\nchange c {\n}\n‸use d.e\n",
        codes::USE_AFTER_DECLARATION,
    );
    assert_prefix(&d, "use declarations must precede all change declarations");
    // After an annotation the grammar requires a declaration: `use` there is
    // the generic error, and a doc comment before `use` is misplaced.
    let (_, d) = reject_at(Cpl, "module m\n\n@prove\n‸use a.b\n", codes::SYNTAX_ERROR);
    assert_eq!(field(&d, "found"), Some("`use`"));
    reject_at(
        Cpl,
        "module m\n\n‸/// d\nuse a.b\n",
        codes::DOC_COMMENT_POSITION,
    );
}

#[test]
fn d12_misplaced_else_in_every_significant_newline_context() {
    let cases: Vec<(Form, String)> =
        vec![
        (
            Cpl,
            body("    if c {\n        1\n    }\n    ‸else {\n        2\n    }"),
        ),
        (
            Cpl,
            body("    if c {\n    } else if d {\n    }\n    ‸else {\n    }"),
        ),
        (Cpl, body("    while c {\n    }\n    ‸else {\n    }")),
        (Cpl, "module m\n\nstruct S {\n    a: Int\n    ‸else\n}\n".into()),
        (Cpl, "module m\n\nenum E {\n    ‸else\n}\n".into()),
        (Cpl, "module m\n\ncapability C {\n    ‸else\n}\n".into()),
        (
            Cpl,
            body("    match x {\n        _ => 1\n        ‸else => 2\n    }"),
        ),
        (
            Cpl,
            "module m\n\nfn f() -> Int {\n    1\n}\n‸else {\n}\n".into(),
        ),
        (Cpl, "module m\n‸else\n".into()),
        (Cpl, "module m\n\ntest \"t\" {\n}\n‸else {\n}\n".into()),
        (Work, "‸else\n".into()),
        (Work, "change c {\n}\n‸else {\n}\n".into()),
        (Work, "change c {\n    ‸else\n}\n".into()),
        (Work, "change c {\n    task t {\n        ‸else\n    }\n}\n".into()),
        (
            Work,
            "change c {\n    integrate {\n        ‸else\n    }\n}\n".into(),
        ),
        (Patch, "‸else\n".into()),
        (
            Patch,
            "patch a.body\n    expect base(a.body)\n{\n}\n‸else\n".into(),
        ),
        (
            Patch,
            "patch a.body\n    expect base(a.body)\n{\n    if c {\n    }\n    ‸else {\n    }\n}\n"
                .into(),
        ),
    ];
    for (form, marked) in cases {
        let (_, d) = reject_at(form, &marked, codes::MISPLACED_ELSE);
        assert_prefix(&d, ELSE_MESSAGE);
        assert_no_generic_fields(&d);
    }
}

#[test]
fn d12_else_where_one_token_is_required_or_inside_brackets() {
    // Grammar 2.4 rule 4: where one particular token is required (the module
    // header, the declaration after an annotation) it is the generic error.
    let (_, d) = reject_at(Cpl, "‸else\n", codes::SYNTAX_ERROR);
    assert_eq!(field(&d, "found"), Some("`else`"));
    let (_, d) = reject_at(Cpl, "module m\n\n@prove\n‸else\n", codes::SYNTAX_ERROR);
    assert_eq!(field(&d, "found"), Some("`else`"));
    // Inside brackets the NEWLINE is discarded and `else` continues the `if`.
    accept(
        Cpl,
        &body("    let x = f(if c { 1 }\n    else { 2 })\n    x"),
    );
    accept(
        Cpl,
        &body("    let x = [if c { 1 }\n        else if d { 2 }\n        else { 3 }]\n    x"),
    );
    // On the closing brace's line it is fine everywhere.
    accept(
        Patch,
        "patch a.body\n    expect base(a.body)\n{\n    if c {\n    } else {\n    }\n}\n",
    );
}

#[test]
fn duplicate_clauses_are_reported_at_the_second_keyword() {
    let cases: Vec<(String, &str)> = vec![
        (
            "module m\n\nfn f(using db: Db) -> Int\n    effects { db.get }\n    requires true\n    ‸effects { db.put }\n{\n    1\n}\n".into(),
            "effects",
        ),
        (
            "module m\n\nfn f(x: Int) -> Int\n    decreases x\n    ensures true\n    ‸decreases x\n{\n    1\n}\n".into(),
            "decreases",
        ),
        (
            "module m\n\nfn f(x: Int) -> Int\n    decreases x\n    ‸decreases x\n".into(),
            "decreases",
        ),
        (
            "module m\n\nfn f(using db: Db) -> Int\n    effects {}\n    ‸effects {}\n".into(),
            "effects",
        ),
        (
            body("    while c\n        decreases x\n        invariant y\n        ‸decreases z\n    {\n    }"),
            "decreases",
        ),
    ];
    for (marked, name) in cases {
        let (src, d) = reject_at(Cpl, &marked, codes::DUPLICATE_CLAUSE);
        assert_prefix(&d, &format!("duplicate {name} clause"));
        assert_eq!(field(&d, "name"), Some(name), "9.4: `name`");
        assert_eq!(span_text(&src, &d), name);
    }
    // Any number of `requires`, `ensures`, and `invariant` clauses (3.9, 3.11).
    module_ok(
        "module m\n\nfn f(x: Int) -> Int\n    requires a\n    ensures c\n    requires b\n    ensures d\n{\n    while c\n        invariant a\n        invariant b\n    {\n    }\n    1\n}\n",
    );
}

#[test]
fn duplicate_work_items_are_reported_at_the_second_occurrence() {
    let cases: Vec<(&str, &str, Option<&str>)> = vec![
        (
            "change c {\n    base git(\"a\")\n    ‸base git(\"b\")\n}\n",
            "base",
            Some("duplicate base"),
        ),
        (
            "change c {\n    target git(\"a\")\n    workers 2\n    ‸target git(\"b\")\n}\n",
            "target",
            None,
        ),
        ("change c {\n    workers 1\n    ‸workers 2\n}\n", "workers", None),
        (
            "change c {\n    integrate {\n    }\n    ‸integrate {\n    }\n}\n",
            "integrate",
            None,
        ),
        (
            "change c {\n    task t {\n    }\n    task u {\n    }\n    task ‸t {\n    }\n}\n",
            "t",
            None,
        ),
        (
            "change c {\n    task t {\n        write {}\n        ‸write { a }\n    }\n}\n",
            "write",
            Some("duplicate write set"),
        ),
        (
            "change c {\n    task t {\n        add { a.* }\n        goal \"g\"\n        ‸add { b.* }\n    }\n}\n",
            "add",
            None,
        ),
        (
            "change c {\n    task t {\n        depend {}\n        ‸depend {}\n    }\n}\n",
            "depend",
            None,
        ),
        (
            "change c {\n    task t {\n        preserve {}\n        ‸preserve {}\n    }\n}\n",
            "preserve",
            None,
        ),
        (
            "change c {\n    task t {\n        goal \"a\"\n        ‸goal \"b\"\n    }\n}\n",
            "goal",
            None,
        ),
    ];
    for (marked, name, message) in cases {
        let (_, d) = reject_at(Work, marked, codes::DUPLICATE_WORK_ITEM);
        assert_eq!(field(&d, "name"), Some(name), "9.4: `name`");
        if let Some(message) = message {
            assert_prefix(&d, message);
        }
    }
    // Unique within a change, per item kind within a task (4.1, 4.2).
    accept(
        Work,
        "change a {\n    task t {\n        write {}\n    }\n    task u {\n        write {}\n    }\n}\nchange b {\n    task t {\n    }\n}\n",
    );
}

#[test]
fn positional_after_using_in_parameters_and_arguments() {
    let (_, d) = reject_at(
        Cpl,
        "module m\n\nfn f(x: Int, using db: Db, ‸y: Int) -> Int {\n    1\n}\n",
        codes::POSITIONAL_AFTER_USING,
    );
    assert_prefix(&d, "positional parameter after using parameter");
    assert_no_generic_fields(&d);
    let (_, d) = reject_at(
        Cpl,
        &body("    f(1, using db, ‸x)"),
        codes::POSITIONAL_AFTER_USING,
    );
    assert_prefix(&d, "positional argument after using argument");
    assert_no_generic_fields(&d);
    reject_at(
        Cpl,
        &body("    f(\n        using db,\n        ‸x,\n    )"),
        codes::POSITIONAL_AFTER_USING,
    );
    reject_at(
        Cpl,
        &body("    a.b(using db, ‸[1])"),
        codes::POSITIONAL_AFTER_USING,
    );
    accept(Cpl, &body("    f(1, 2, using db, using ctx.reader,)"));
    module_ok("module m\n\nfn f(using a: A, using b: B,) -> Int {\n    1\n}\n");
}

#[test]
fn d36_unknown_requirements_carry_found() {
    for (requirement, mention) in [
        ("‸compile", "compile"),
        ("‸Typecheck", "Typecheck"),
        ("‸42", "42"),
        ("‸\"typecheck\"", "typecheck"),
        ("‸(typecheck)", "("),
        ("‸fn", "fn"),
        ("‸?h", "h"),
        ("‸test_all", "test_all"),
        ("‸no_hole", "no_hole"),
    ] {
        let marked =
            format!("change c {{\n    integrate {{\n        require {requirement}\n    }}\n}}\n");
        let (_, d) = reject_at(Work, &marked, codes::UNKNOWN_REQUIREMENT);
        assert_prefix(&d, "unknown requirement ");
        assert!(d.message.contains(SIX_FORMS), "D36: {:?}", d.message);
        let found = field(&d, "found").expect("9.4: `found`");
        assert!(found.contains(mention), "{found:?}");
    }
}

#[test]
fn d36_require_followed_by_nothing_is_the_generic_syntax_error() {
    for (marked, found) in [
        (
            "change c {\n    integrate {\n        require‸\n    }\n}\n",
            "end of line",
        ),
        (
            "change c {\n    integrate {\n        require ‸}\n}\n",
            "`}`",
        ),
        ("change c {\n    integrate { require ‸}\n}\n", "`}`"),
        (
            "change c {\n    integrate {\n        require // nothing‸\n    }\n}\n",
            "end of line",
        ),
    ] {
        let (_, d) = reject_at(Work, marked, codes::SYNTAX_ERROR);
        assert_generic(&d, NO_REQUIREMENT, found);
    }
    // A reserved word there is `reserved_word` (D36, D4).
    let (_, d) = reject_at(
        Work,
        "change c {\n    integrate {\n        require ‸async\n    }\n}\n",
        codes::RESERVED_WORD,
    );
    assert_eq!(field(&d, "name"), Some("async"));
}

#[test]
fn add_sets_name_namespaces_only() {
    let task = |set: &str| format!("change c {{\n    task t {{\n        add {set}\n    }}\n}}\n");
    for (set, path) in [
        ("{ ‸tests.pagination }", "tests.pagination"),
        ("{ ‸reader.pages.body }", "reader.pages"),
        ("{ ‸paginate }", "paginate"),
        ("{ ‸a.B.effects }", "a.B"),
        ("{ ‸a.b.definition }", "a.b"),
        ("{ tests.*, ‸a.b, c.* }", "a.b"),
        ("{\n            ‸a.b,\n        }", "a.b"),
    ] {
        let (src, d) = reject_at(Work, &task(set), codes::SYNTAX_ERROR);
        assert_prefix(&d, &format!("add names a namespace; write {path}.*"));
        assert_no_generic_fields(&d);
        assert!(span_text(&src, &d).starts_with(path.split('.').next().unwrap()));
    }
    for set in [
        "{}",
        "{ tests.pagination.* }",
        "{ a.*, b.c.*, }",
        "{\n            Upper.*,\n        }",
    ] {
        accept(Work, &task(set));
    }
    // The namespace check is the earliest error even with a broken selector
    // after it in the same set (D70).
    reject_at(Work, &task("{ ‸a.b, c. }"), codes::SYNTAX_ERROR);
}

#[test]
fn d45_forall_is_never_an_operand() {
    for marked in [
        body("    a and ‸forall x in c: p"),
        body("    1 + ‸forall x in c: p"),
        body("    not ‸forall x in c: p"),
        body("    -‸forall x in c: p"),
        body("    f(x) == ‸forall x in c: p"),
        body("    forall x in ‸forall y in c: p: q"),
        body("    let b = a or ‸forall x in c: p\n    b"),
    ] {
        let (_, d) = reject_at(Cpl, &marked, codes::SYNTAX_ERROR);
        assert_eq!(field(&d, "found"), Some("`forall`"));
        assert!(d.data.contains_key("expected"));
    }
    for stmt in [
        "(forall x in c: p) and a",
        "f(forall x in c: p)",
        "[forall x in c: p]",
        "forall x in c: forall y in d: x == y",
        "forall x in c: p and q or r",
        "match x {\n        _ => forall y in c: p\n    }",
        "return forall x in c: p",
        "assert forall x in c: p",
    ] {
        accept(Cpl, &body(&format!("    {stmt}")));
    }
}

#[test]
fn a_hole_admits_no_postfix_operator() {
    for (marked, found) in [
        (body("    ?h‸.len"), "`.`"),
        (body("    ?h‸(1)"), "`(`"),
        (body("    ?h‸?"), "`?`"),
        (body("    let x = 1 + ?h‸.len\n    x"), "`.`"),
    ] {
        let (_, d) = reject_at(Cpl, &marked, codes::SYNTAX_ERROR);
        assert_eq!(field(&d, "found"), Some(found));
    }
    for stmt in ["(?h).len", "?h + 1", "f(?h)", "not ?h", "-?h", "[?a, ?b]"] {
        accept(Cpl, &body(&format!("    {stmt}")));
    }
}

#[test]
fn empty_payload_parentheses_are_a_syntax_error() {
    let (_, d) = reject_at(
        Cpl,
        "module m\n\nenum E {\n    A(‸)\n}\n",
        codes::SYNTAX_ERROR,
    );
    assert_eq!(field(&d, "found"), Some("`)`"));
    let (_, d) = reject_at(
        Cpl,
        &body("    match x {\n        A(‸) => 1\n    }"),
        codes::SYNTAX_ERROR,
    );
    assert_eq!(field(&d, "found"), Some("`)`"));
    // Payload lists accept a trailing comma (2.5).
    module_ok("module m\n\nenum E {\n    A(Int,)\n    B(Int, Text,)\n}\n");
    accept(Cpl, &body("    match x {\n        A(y,) => 1\n    }"));
}

#[test]
fn generic_lists_take_no_trailing_comma_and_no_significant_line_break() {
    for marked in [
        "module m\n\nfn f() -> List<Int,‸> {\n    1\n}\n",
        "module m\n\nfn f<T,‸>() -> Int {\n    1\n}\n",
        "module m\n\nstruct S<A,‸> {\n}\n",
        "module m\n\nfn f(x: Result<Int,‸>) -> Int {\n    1\n}\n",
    ] {
        let (_, d) = reject_at(Cpl, marked, codes::SYNTAX_ERROR);
        assert_eq!(field(&d, "found"), Some("`>`"));
    }
    let (_, d) = reject_at(
        Cpl,
        &body("    let x: List<‸\n        Int> = y\n    x"),
        codes::SYNTAX_ERROR,
    );
    assert_eq!(field(&d, "found"), Some("end of line"));
    let (_, d) = reject_at(
        Cpl,
        "module m\n\nfn f() -> List<‸\n    Int> {\n    1\n}\n",
        codes::SYNTAX_ERROR,
    );
    assert_eq!(field(&d, "found"), Some("end of line"));
    // Inside parentheses the line break is discarded (2.2).
    module_ok("module m\n\nfn f(x: List<\n    Int>) -> Int {\n    1\n}\n");
}

#[test]
fn type_declarations_with_generic_parameters_end_after_them() {
    for (marked, found) in [
        ("module m\n\ntype A<T> ‸= Int where self > 0\n", "`=`"),
        ("module m\n\ntype A<T>‸(T)\n", "`(`"),
    ] {
        let (_, d) = reject_at(Cpl, marked, codes::SYNTAX_ERROR);
        assert_generic(&d, "expected end of line after the declaration", found);
    }
    module_ok("module m\n\npub type List<T>\npub type Gen<T, U>\n");
}

#[test]
fn doc_comments_inside_bracketed_lists_are_misplaced() {
    for (form, marked) in [
        (
            Cpl,
            "module m\n\nfn f(\n    ‸/// d\n    x: Int,\n) -> Int {\n    1\n}\n".to_string(),
        ),
        (Cpl, body("    f(\n        ‸/// d\n        1,\n    )")),
        (Cpl, body("    [\n        ‸/// d\n        1,\n    ]")),
        (Cpl, body("    A {\n        ‸/// d\n        a: 1,\n    }")),
        (
            Cpl,
            "module m\n\nfn f(using db: Db) -> Int\n    effects {\n        ‸/// d\n        db.get,\n    }\n{\n    1\n}\n".to_string(),
        ),
        (
            Cpl,
            "module m\n\nuse a.b.{\n    ‸/// d\n    c,\n}\n".to_string(),
        ),
        (
            Cpl,
            "module m\n\n@id(\n    ‸/// d\n    \"x\")\nfn f() -> Int {\n    1\n}\n".to_string(),
        ),
        (
            Cpl,
            body("    match x {\n        Some(\n            ‸/// d\n            y) => 1\n    }"),
        ),
        (Cpl, body("    match x {\n        ‸/// d\n        _ => 1\n    }")),
        (
            Work,
            "change c {\n    task t {\n        write {\n            ‸/// d\n            a.b,\n        }\n    }\n}\n".to_string(),
        ),
        (
            Work,
            "change c {\n    integrate {\n        ‸/// d\n        require typecheck\n    }\n}\n"
                .to_string(),
        ),
        (
            Work,
            "change c {\n    task t {\n        ‸/// d\n        goal \"g\"\n    }\n}\n".to_string(),
        ),
        (Expression, "f(\n‸/// d\n1)".to_string()),
    ] {
        let (_, d) = reject_at(form, &marked, codes::DOC_COMMENT_POSITION);
        assert_prefix(&d, MISPLACED_DOC);
    }
}

#[test]
fn a_doc_comment_after_code_must_begin_a_line() {
    for (form, marked) in [
        (Cpl, body("    1 ‸/// d")),
        (Cpl, "module m ‸/// d\n".to_string()),
        (
            Cpl,
            "module m\n\nstruct S { ‸/// d\n    a: Int\n}\n".to_string(),
        ),
        (Work, "change c { ‸/// d\n}\n".to_string()),
        (
            Patch,
            "patch a.body ‸/// d\n    expect base(a.body)\n{\n}\n".to_string(),
        ),
        (Expression, "1 ‸/// d".to_string()),
    ] {
        let (_, d) = reject_at(form, &marked, codes::DOC_COMMENT_POSITION);
        assert_prefix(&d, DOC_AFTER_CODE);
    }
    // `////` is an ordinary comment anywhere (D60).
    module_ok("module m //// banner\n\n//// banner\nfn f() -> Int {\n    1 //// x\n}\n");
}

// ===========================================================================
// .work and .patch syntax errors
// ===========================================================================

#[test]
fn work_files_hold_no_cpl_or_patch_syntax() {
    for (marked, found) in [
        ("‸module m\n", "`module`"),
        ("‸fn f() -> Int {\n}\n", "`fn`"),
        (
            "‸patch a.body\n    expect base(a.body)\n{\n}\n",
            "identifier `patch`",
        ),
        ("change c {\n}\n‸patch a.body\n", "identifier `patch`"),
        ("change c {\n}\n‸task t {\n}\n", "identifier `task`"),
        ("‸base git(\"main\")\n", "identifier `base`"),
        ("‸{\n}\n", "`{`"),
    ] {
        let (_, d) = reject_at(Work, marked, codes::SYNTAX_ERROR);
        assert_eq!(field(&d, "found"), Some(found), "{marked:?}");
    }
    // ...and neither `.cpl` nor `.patch` files hold a change (D40).
    let (_, d) = reject_at(Cpl, "module m\n\n‸change c {\n}\n", codes::SYNTAX_ERROR);
    assert_eq!(field(&d, "found"), Some("identifier `change`"));
    reject_at(Patch, "‸change c {\n}\n", codes::SYNTAX_ERROR);
    reject_at(Cpl, "module m\n\n‸patch a.body\n", codes::SYNTAX_ERROR);
}

#[test]
fn work_change_and_task_item_shapes() {
    let change = |items: &str| format!("change c {{\n{items}\n}}\n");
    for (items, found) in [
        ("    base ‸\"main\"", None),
        ("    base git(‸main)", Some("identifier `main`")),
        ("    base git(‸)", Some("`)`")),
        ("    base git(\"a\"‸, \"b\")", Some("`,`")),
        ("    base git ‸\"main\"", None),
        ("    base ‸Git(\"main\")", Some("identifier `Git`")),
        ("    workers ‸-1", Some("`-`")),
        ("    workers ‸x", Some("identifier `x`")),
        ("    ‸owner \"x\"", Some("identifier `owner`")),
        ("    ‸Base git(\"x\")", Some("identifier `Base`")),
        (
            "    base git(\"a\") ‸target git(\"b\")",
            Some("identifier `target`"),
        ),
        (
            "    task t {\n        ‸read { a }\n    }",
            Some("identifier `read`"),
        ),
        (
            "    task t {\n        goal ‸x\n    }",
            Some("identifier `x`"),
        ),
        ("    task t {\n        goal \"a\" ‸\"b\"\n    }", None),
        (
            "    task t {\n        write ‸a\n    }",
            Some("identifier `a`"),
        ),
        (
            "    task t {\n        write { a } ‸add { b.* }\n    }",
            Some("identifier `add`"),
        ),
        ("    task ‸T {\n    }", Some("identifier `T`")),
        ("    task t after ‸{\n    }", Some("`{`")),
    ] {
        let (_, d) = reject_at(Work, &change(items), codes::SYNTAX_ERROR);
        assert!(d.data.contains_key("expected"), "{d:#?}");
        if let Some(found) = found {
            assert_eq!(field(&d, "found"), Some(found), "{items:?}");
        }
    }
    reject_at(Work, "change c {} ‸change d {}\n", codes::SYNTAX_ERROR);
    accept(Work, "change c { base git(\"main\") }\n");
    accept(Work, "change c {\n    task t { write { a.body } }\n}\n");
    accept(
        Work,
        "change c\n{\n    task t\n    {\n    }\n    integrate\n    {\n    }\n}\n",
    );
}

#[test]
fn work_selector_shapes() {
    let write =
        |set: &str| format!("change c {{\n    task t {{\n        write {set}\n    }}\n}}\n");
    for (set, found) in [
        ("{ a. ‸}", Some("`}`")),
        ("{ a.*‸.b }", Some("`.`")),
        ("{ ‸.a }", Some("`.`")),
        ("{ ‸* }", Some("`*`")),
        ("{ a.‸\"name\" }", None),
        ("{ a ‸b }", Some("identifier `b`")),
        ("{ a.effects‸.b }", Some("`.`")),
        ("{ a‸..b }", Some("`..`")),
        ("{ a,‸, b }", Some("`,`")),
        ("{ a.‸1 }", None),
        ("{ ‸?h }", None),
    ] {
        let (_, d) = reject_at(Work, &write(set), codes::SYNTAX_ERROR);
        assert!(d.data.contains_key("expected"), "{d:#?}");
        if let Some(found) = found {
            assert_eq!(field(&d, "found"), Some(found), "{set:?}");
        }
    }
    for set in [
        "{}",
        "{ a }",
        "{ A }",
        "{ a.B.c }",
        "{ a.* }",
        "{ a.effects }",
        "{ a.body, b.contract, c.definition, }",
        "{\n            a.signature,\n            b.*,\n        }",
    ] {
        accept(Work, &write(set));
    }
}

#[test]
fn work_after_lists() {
    for (marked, found) in [
        ("change c {\n    task t after a, ‸{\n    }\n}\n", "`{`"),
        (
            "change c {\n    task t after ‸A {\n    }\n}\n",
            "identifier `A`",
        ),
        (
            "change c {\n    task t after a ‸b {\n    }\n}\n",
            "identifier `b`",
        ),
        ("change c {\n    task t after ‸{\n    }\n}\n", "`{`"),
        (
            "change c {\n    task t after a,‸\n        b {\n    }\n}\n",
            "end of line",
        ),
    ] {
        let (_, d) = reject_at(Work, marked, codes::SYNTAX_ERROR);
        assert_eq!(field(&d, "found"), Some(found), "{marked:?}");
    }
    accept(
        Work,
        "change c {\n    task a {\n    }\n    task b after a {\n    }\n    task d after a, b\n    {\n    }\n}\n",
    );
}

#[test]
fn work_requirement_argument_shapes() {
    let integrate = |r: &str| format!("change c {{\n    integrate {{\n        {r}\n    }}\n}}\n");
    for (requirement, found) in [
        ("require prove(‸)", Some("`)`")),
        ("require review(a‸.b)", Some("`.`")),
        ("require review(‸A)", Some("identifier `A`")),
        ("require test(‸)", Some("`)`")),
        ("require test(tests.pagination.‸\"name\")", None),
        ("require prove ‸x", Some("identifier `x`")),
        ("require test‸", Some("end of line")),
        ("require typecheck‸(x)", Some("`(`")),
        ("require no_holes ‸no_holes", Some("identifier `no_holes`")),
        ("‸requires typecheck", Some("`requires`")),
    ] {
        let (_, d) = reject_at(Work, &integrate(requirement), codes::SYNTAX_ERROR);
        assert!(d.data.contains_key("expected"), "{d:#?}");
        if let Some(found) = found {
            assert_eq!(field(&d, "found"), Some(found), "{requirement:?}");
        }
    }
    accept(
        Work,
        &integrate(
            "require typecheck\n        require no_holes\n        require no_unresolved_assumptions\n        require prove(paginate)\n        require prove(a.b.contract)\n        require test(tests.*)\n        require review(audit)",
        ),
    );
}

#[test]
fn patch_header_and_clause_layout() {
    for (marked, found) in [
        (
            "patch a.body ‸expect base(a.body)\n{\n}\n",
            Some("identifier `expect`"),
        ),
        ("patch a.body\n    expect base(a.body) ‸{\n}\n", Some("`{`")),
        (
            "patch a.body\n    expect ‸head(a.body)\n{\n}\n",
            Some("identifier `head`"),
        ),
        (
            "patch a.body\n    expect base ‸a.body\n{\n}\n",
            Some("identifier `a`"),
        ),
        (
            "patch a.body\n    expect base(a.body‸, b.body)\n{\n}\n",
            Some("`,`"),
        ),
        ("patch‸\n", Some("end of line")),
        ("patch ‸*\n", Some("`*`")),
        (
            "patch a.body\n    expect base(a.body)\n{\n} ‸patch b.body\n",
            Some("identifier `patch`"),
        ),
        ("patch a.body\n    expect‸\n", Some("end of line")),
    ] {
        let (_, d) = reject_at(Patch, marked, codes::SYNTAX_ERROR);
        assert!(d.data.contains_key("expected"), "{d:#?}");
        if let Some(found) = found {
            assert_eq!(field(&d, "found"), Some(found), "{marked:?}");
        }
    }
    // Blank lines and comments between the header, its clause, and the brace
    // collapse into the one NEWLINE (2.4); doc comments precede a form.
    accept(
        Patch,
        "/// one\n/// two\npatch a.body\n\n    // note\n    expect base(a.body)\n\n    // note\n{\n}\n\n\npatch b.body\n    expect base(b.body)\n{\n    1\n}\n",
    );
}

#[test]
fn patch_files_hold_only_patch_forms() {
    for (marked, found) in [
        ("‸module m\n", "`module`"),
        ("‸use a.b.{c}\n", "`use`"),
        ("‸fn f() -> Int {\n}\n", "`fn`"),
        (
            "patch a.body\n    expect base(a.body)\n{\n}\n‸fn f() -> Int {\n}\n",
            "`fn`",
        ),
        ("‸{\n}\n", "`{`"),
        ("‸1\n", "integer literal `1`"),
    ] {
        let (_, d) = reject_at(Patch, marked, codes::SYNTAX_ERROR);
        if found.starts_with('`') {
            assert_eq!(field(&d, "found"), Some(found), "{marked:?}");
        }
        assert!(d.data.contains_key("expected"));
    }
}

#[test]
fn patch_blocks_are_cpl_blocks() {
    // The block keeps the whole statement grammar, its errors included.
    for (marked, code) in [
        (
            "patch a.body\n    expect base(a.body)\n{\n    let x = 1 ‸let y = 2\n}\n",
            codes::SYNTAX_ERROR,
        ),
        (
            "patch a.body\n    expect base(a.body)\n{\n    ‸fn g() -> Int {\n    }\n}\n",
            codes::SYNTAX_ERROR,
        ),
        (
            "patch a.body\n    expect base(a.body)\n{\n    f(using db, ‸x)\n}\n",
            codes::POSITIONAL_AFTER_USING,
        ),
        (
            "patch a.body\n    expect base(a.body)\n{\n    ‸/// d\n    1\n}\n",
            codes::DOC_COMMENT_POSITION,
        ),
        (
            "patch a.body\n    expect base(a.body)\n{\n    while c\n        decreases x\n        ‸decreases y\n    {\n    }\n}\n",
            codes::DUPLICATE_CLAUSE,
        ),
    ] {
        reject_at(Patch, marked, code);
    }
}

// ===========================================================================
// parse_expr (toolchain 1.5)
// ===========================================================================

#[test]
fn parse_expr_rejects_statements_and_declarations() {
    for (marked, found) in [
        ("‸let x = 1", "`let`"),
        ("‸var x = 1", "`var`"),
        ("‸return 1", "`return`"),
        ("‸assert x", "`assert`"),
        ("‸while c {}", "`while`"),
        ("‸fn f() -> Int {}", "`fn`"),
        ("‸module m", "`module`"),
        ("‸{}", "`{`"),
        ("x ‸= 1", "`=`"),
        ("change ‸c {}", "identifier `c`"),
        ("‸use a.b", "`use`"),
        ("‸@id(\"x\")", "`@`"),
    ] {
        let (_, d) = reject_at(Expression, marked, codes::SYNTAX_ERROR);
        assert_eq!(field(&d, "found"), Some(found), "{marked:?}");
    }
}

#[test]
fn parse_expr_trailing_input_is_expected_end_of_input() {
    // toolchain 1.5: any other token after the expression is `expected end of
    // input after the expression, found <token>`, also an `else` on the line
    // after an `if` (no statement or arm can begin there).
    for (marked, found) in [
        ("if c {\n    1\n}\n‸else {\n    2\n}", "`else`"),
        ("1\n‸2", "integer literal `2`"),
        ("Page\n‸{ id: 1 }", "`{`"),
        ("a.B.c ‸{ x: 1 }", "`{`"),
        ("f(x) ‸g", "identifier `g`"),
    ] {
        let (_, d) = reject_at(Expression, marked, codes::SYNTAX_ERROR);
        if !found.starts_with("integer") {
            assert_generic(&d, "expected end of input after the expression", found);
        } else {
            assert_prefix(&d, "expected end of input after the expression");
        }
    }
}

#[test]
fn parse_expr_lexical_errors_bom_and_layout() {
    reject_at(Expression, "1‸;", codes::INVALID_CHARACTER);
    reject_at(Expression, "‸?Foo", codes::HOLE_NAME_CASE);
    reject_at(Expression, "‸?while", codes::HOLE_NAME_KEYWORD);
    reject_at(Expression, "‸1.25", codes::FLOAT_LITERAL_UNSUPPORTED);
    // A leading byte-order mark is skipped (1.1), CRLF is one terminator.
    expr_ok("\u{feff}1");
    expr_ok("\r\n1\r\n");
    expr_ok("(1\r\n+ 2)");
    // Struct literals are allowed at the top level (D41) and span lines inside
    // their braces.
    let ExprKind::Struct(_) = expr_ok("Page {\n    id: 1,\n}").kind else {
        panic!("a struct literal")
    };
    let ExprKind::Struct(_) = expr_ok("a.b.Page { ..p }").kind else {
        panic!("a qualified struct literal")
    };
}

#[test]
fn parse_expr_keeps_the_header_rules_of_if_and_match() {
    accept(Expression, "if c\n{\n    1\n} else\n{\n    2\n}");
    accept(Expression, "match x\n{\n    _ => 1\n}");
    // Struct literals are disabled in the condition (D41).
    // `{` after `Page` opens the body, whose first statement `id` cannot be
    // followed by `:`.
    let (_, d) = reject_at(
        Expression,
        "if p == Page { id‸: 1 } {\n}",
        codes::SYNTAX_ERROR,
    );
    assert!(d.data.contains_key("expected"));
    accept(Expression, "if p == (Page { id: 1 }) {\n}");
}

// ===========================================================================
// No panic, no unbounded recursion
// ===========================================================================

const FRAGMENTS: &[&str] = &[
    "module m",
    "module",
    "use a.b.{c}",
    "use",
    "fn",
    "f",
    "(",
    ")",
    "[",
    "]",
    "{",
    "}",
    "<",
    ">",
    ",",
    ".",
    "..",
    ":",
    "->",
    "=>",
    "=",
    "==",
    "!=",
    "+",
    "-",
    "*",
    "/",
    "%",
    "++",
    "?",
    "?h",
    "?H",
    "?if",
    "@",
    "@id(\"x\")",
    "@property",
    "pub",
    "type",
    "struct",
    "enum",
    "capability",
    "test",
    "property",
    "let",
    "var",
    "return",
    "assert",
    "while",
    "if",
    "else",
    "match",
    "forall",
    "in",
    "for",
    "not",
    "and",
    "or",
    "true",
    "false",
    "requires",
    "ensures",
    "effects",
    "decreases",
    "invariant",
    "where",
    "using",
    "x",
    "Y",
    "_",
    "_x",
    "0",
    "007",
    "1_000",
    "1__0",
    "1.5",
    "2x",
    "\"t\"",
    "\"\\q\"",
    "\"\\u{110000}\"",
    "\"open",
    "b\"x\"",
    "\n",
    "\n",
    "\n",
    "\r\n",
    "\r",
    " ",
    "\t",
    "// c",
    "/// d",
    "//// b",
    ";",
    "!",
    "&",
    "|",
    "#",
    "é",
    "\u{feff}",
    "\u{0}",
    "change c",
    "task t",
    "after",
    "base",
    "target",
    "git(\"main\")",
    "workers",
    "integrate",
    "require",
    "typecheck",
    "prove",
    "review",
    "write",
    "add",
    "depend",
    "preserve",
    "goal",
    "patch",
    "expect",
    "*",
    "a.body",
    "tests.*",
    "as",
    "loop",
    "async",
    "Page { ..p, a: 1 }",
    "check<Positive>(n)",
    "List<List<T>>",
];

/// A deterministic xorshift generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[test]
fn fuzzed_token_soup_keeps_the_parse_result_contract() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for _ in 0..3000 {
        let n = 1 + rng.below(40);
        let mut soup = String::new();
        for _ in 0..n {
            soup.push_str(FRAGMENTS[rng.below(FRAGMENTS.len())]);
            match rng.below(4) {
                0 => {}
                1 | 2 => soup.push(' '),
                _ => soup.push('\n'),
            }
        }
        for form in FORMS {
            contract(form, &soup);
        }
        contract(Cpl, &format!("module m\n\n{soup}"));
        contract(Cpl, &body(&soup));
        contract(Work, &format!("change c {{\n{soup}\n}}\n"));
        contract(
            Patch,
            &format!("patch a.body\n    expect base(a.body)\n{{\n{soup}\n}}\n"),
        );
    }
}

const DENSE_CPL: &str = r#"/// Module docs.
module reader.pages

use std.text.{trim, parse_int}
use a.b.C

/// A page id.
@id("ty_page_id")
pub type PageId(Text)
pub type Positive = Int where self > 0
pub type List<T>

pub struct Page<T> {
    /// The id.
    id: PageId
    text: Text
}

pub enum StoreError {
    Missing(PageId, Int,)
    Unavailable
}

pub capability PageRead {
    fn get(id: PageId) -> Result<Page, StoreError>
        requires id != PageId("")
        ensures true
}

@prove
@property
pub fn paginate<T>(items: List<T>, size: Positive, using db: PageRead) -> List<List<T>>
    effects { db.get, }
    requires size > 0
    ensures forall page in result: page.len > 0 and page.len <= size
    decreases items.len
{
    var rest = items
    let _ = check<Positive>(-5)?
    while rest.len > 0
        invariant rest.len >= 0
        decreases rest.len
    {
        rest = rest.drop(size)
    }
    let label = if rest.is_empty() {
        "empty"
    } else if size == 1 {
        "one"
    } else {
        "many\t\u{41}"
    }
    match db.get(PageId("x"), using db) {
        Ok(p) => Page { ..p, text: trim(p.text) }
        Err(StoreError.Missing(_)) => { return [] }
        Err(reader.pages.StoreError.Unavailable) => ?todo
        _ => [[1, 2], []] ++ [f(a < B, C > (d))]
    }
    assert not (1 == 2) or -x.len * 3 % 2 / 1 - 1 + 1_000 == 7
    [items]
}

test "pagination" {
    let x = 1_000
}

property "laws"
    for size in gen.positive_int
    for items in gen.list(gen.int)
{
    assert true
}
"#;

const DENSE_WORK: &str = r#"use reader.pages.{paginate, Positive}

/// The change.
change pagination_update {
    base git("main")
    target git("main")
    workers 3
    /// A task.
    task implementation {
        write { paginate.body }
        add { tests.pagination.* }
        depend { Positive.definition, reader.pages.Page }
        preserve { paginate.signature, paginate.effects }
        goal "implement \"it\""
    }
    task audit after implementation, other {
        write {}
    }
    integrate {
        require typecheck
        require no_holes
        require no_unresolved_assumptions
        require prove(paginate)
        require test(tests.pagination.*)
        require review(audit)
    }
}
"#;

const DENSE_PATCH: &str = r#"/// Replace the body.
patch paginate.body
    expect base(paginate.body)
{
    let patch = 1
    if patch > 0 {
        flatten([[patch]])
    } else {
        []
    }
}

patch reader.pages.label.body
    // a comment
    expect base(reader.pages.label.body)
{
    match result {
        Ok(page) => page.text
        Err(_) => "?"
    }
}
"#;

fn char_starts(src: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    src.char_indices().map(|(i, c)| (i, c.len_utf8()))
}

#[test]
fn every_prefix_and_deletion_of_dense_sources_keeps_the_contract() {
    for (form, src) in [(Cpl, DENSE_CPL), (Work, DENSE_WORK), (Patch, DENSE_PATCH)] {
        accept(form, src);
        for end in (0..=src.len()).filter(|&i| src.is_char_boundary(i)) {
            contract(form, &src[..end]);
            contract(Expression, &src[end..]);
        }
        for (at, len) in char_starts(src) {
            contract(form, &format!("{}{}", &src[..at], &src[at + len..]));
        }
    }
}

#[test]
fn single_character_substitutions_in_dense_sources_keep_the_contract() {
    for (form, src) in [(Cpl, DENSE_CPL), (Work, DENSE_WORK), (Patch, DENSE_PATCH)] {
        for (at, len) in char_starts(src) {
            for with in ["{", "}", "(", "\"", "\n", "#", "<", "é"] {
                contract(form, &format!("{}{with}{}", &src[..at], &src[at + len..]));
            }
        }
    }
}

#[test]
fn deep_and_long_inputs_are_rejected_within_a_one_mebibyte_stack() {
    // D73's rationale: the limits exist so that no input, however deep or
    // long, can overflow the parser's stack, and "at 64 levels the reference
    // parser stays under 1 MiB of stack in a debug build". (Nested `while`
    // loops have a test of their own below.)
    in_child_process_with_one_mebibyte_stack(
        "deep_and_long_inputs_are_rejected_within_a_one_mebibyte_stack",
        || {
            const N: usize = 100_000;
            let nesting: Vec<(Form, String)> = vec![
                (Expression, "(".repeat(N)),
                (Expression, "[".repeat(N)),
                (Expression, format!("{}x", "not ".repeat(N))),
                (Expression, format!("{}x", "-".repeat(N))),
                (Expression, "A { a: ".repeat(N)),
                (Expression, "f(".repeat(N)),
                (Expression, "if c { ".repeat(N)),
                (Expression, "match x { _ => ".repeat(N)),
                (Expression, format!("{}true", "forall x in xs: ".repeat(N))),
                (Cpl, format!("module m\n\nfn f() -> {}", "List<".repeat(N))),
                (Cpl, body(&format!("match x {{\n{}", "Some(".repeat(N)))),
                (
                    Cpl,
                    body(&format!("{}{{ 2 }}", "if c { 1 } else ".repeat(N))),
                ),
                (Cpl, body(&"match x {\n_ => {\n".repeat(N))),
                (
                    Patch,
                    format!(
                        "patch a.body\n    expect base(a.body)\n{{\n{}",
                        "(".repeat(N)
                    ),
                ),
            ];
            for (form, src) in &nesting {
                let d = reject(*form, src);
                assert!(
                    d.message.starts_with(NESTING),
                    "{form:?} {:?}: {d:#?}",
                    short(src)
                );
            }
            let long: Vec<(Form, String)> = vec![
                (Expression, format!("x{}", ".b".repeat(N))),
                (Expression, format!("1{}", " + 1".repeat(N))),
                (Expression, format!("x{}", "?".repeat(N))),
                (Expression, format!("f{}", "()".repeat(N))),
                (
                    Cpl,
                    body(&format!("a{}", " and b or c < d * e + f".repeat(N / 10))),
                ),
                (Cpl, body(&format!("A {{}}{}", ".x".repeat(N)))),
            ];
            for (form, src) in &long {
                let d = reject(*form, src);
                assert!(
                    d.message.starts_with(TOO_MANY_OPERATORS),
                    "{form:?} {:?}: {d:#?}",
                    short(src)
                );
            }
            // Inputs whose deep shape is malformed early: only the contract holds.
            for (form, src) in [
                (Expression, format!("f<{}", "List<".repeat(N))),
                (Expression, format!("{}(", "a<".repeat(N))),
                (Expression, "}".repeat(N)),
                (Expression, ")".repeat(N)),
                (Expression, "<".repeat(N)),
                (Expression, "?".repeat(N)),
                (Expression, "\"".repeat(N)),
                (Expression, "-".repeat(N)),
                (Cpl, "///\n".repeat(N)),
                (Cpl, "@".repeat(N)),
                (Cpl, body(&"{".repeat(N))),
                (Work, "change c {\n".repeat(N)),
                (Patch, "patch a.body\n".repeat(N)),
            ] {
                contract(form, &src);
            }
        },
    );
}

#[test]
fn inputs_at_the_nesting_limit_parse_within_a_one_mebibyte_stack() {
    // Programs exactly at level 64 must parse (D73), within the stack the
    // rationale budgets.
    in_child_process_with_one_mebibyte_stack(
        "inputs_at_the_nesting_limit_parse_within_a_one_mebibyte_stack",
        || {
            let nest = |open: &str, inner: &str, close: &str, n: usize| {
                format!("{}{inner}{}", open.repeat(n), close.repeat(n))
            };
            let cases: Vec<(Form, String)> = vec![
                (Cpl, body(&nest("if c {\n", "1\n", "} else {\n2\n}\n", 31))),
                (Cpl, body(&nest("let x = if c {\n", "1\n", "} else {\n2\n}\n", 31))),
                (Cpl, body(&nest("match x {\n_ => {\n", "1\n", "}\n}\n", 21))),
                (Cpl, body(&nest("match x {\n_ => ", "1", "\n}", 31))),
                (Cpl, body(&nest("(", "1", ")", 63))),
                (Cpl, body(&nest("[", "1", "]", 63))),
                (Cpl, body(&nest("A { a: ", "1", " }", 63))),
                (Cpl, body(&nest("f(", "1", ")", 63))),
                (Cpl, body(&nest("x.f(", "1", ")", 63))),
                (Cpl, body(&format!("{}x", "not ".repeat(63)))),
                (Cpl, body(&format!("{}x", "-".repeat(63)))),
                (
                    Cpl,
                    body(&format!("match x {{\n{} => 1\n}}", nest("Some(", "y", ")", 61))),
                ),
                (
                    Cpl,
                    body(&format!(
                        "if c {{ 1 }}{} else {{ 2 }}",
                        " else if c { 1 }".repeat(61)
                    )),
                ),
                (
                    Cpl,
                    format!(
                        "module m\n\nfn f(xs: List<Int>) -> Bool\n    requires {}true\n{{\n    true\n}}\n",
                        "forall x in xs: ".repeat(64)
                    ),
                ),
                (
                    Cpl,
                    format!(
                        "module m\n\nfn f() -> {} {{\n    1\n}}\n",
                        nest("List<", "Int", ">", 64)
                    ),
                ),
                (
                    Cpl,
                    body(&format!("f<{}>(1)", nest("List<", "Int", ">", 62))),
                ),
                (Expression, nest("(", "1", ")", 64)),
                (Expression, nest("if c { ", "1", " }", 32)),
            ];
            for (form, src) in &cases {
                accept(*form, src);
            }
        },
    );
}

#[test]
fn nested_while_loops_at_the_nesting_limit_parse_within_a_one_mebibyte_stack() {
    // D73's rationale: "at 64 levels the reference parser stays under 1 MiB of
    // stack in a debug build" (repeated by the crate's MAX_DEPTH doc). A
    // `while` statement is not a level, its block is: 63 nested loops in a
    // function body are at level 64 and must parse.
    in_child_process_with_one_mebibyte_stack(
        "nested_while_loops_at_the_nesting_limit_parse_within_a_one_mebibyte_stack",
        || {
            accept(Cpl, &in_whiles(63, "1"));
            accept(
                Cpl,
                &body(&format!(
                    "{}1\n{}",
                    "while c\ninvariant x\ndecreases y\n{\n".repeat(63),
                    "}\n".repeat(63)
                )),
            );
            accept(
                Patch,
                &format!(
                    "patch a.body\n    expect base(a.body)\n{{\n{}1\n{}}}\n",
                    "while c {\n".repeat(63),
                    "}\n".repeat(63)
                ),
            );
            let d = reject(Cpl, &body(&"while c {\n".repeat(100_000)));
            assert_prefix(&d, NESTING);
        },
    );
}

#[test]
fn huge_flat_inputs_parse() {
    let mut src = String::from("module m\n");
    for i in 0..20_000 {
        src.push_str(&format!("\nfn f{i}(x: Int) -> Int {{\n    x + {i}\n}}\n"));
    }
    assert_eq!(module_ok(&src).declarations.len(), 20_000);
    let list = format!("[{}0]", "1, ".repeat(100_000));
    let ExprKind::List(l) = expr_ok(&list).kind else {
        panic!("a list")
    };
    assert_eq!(l.elements.len(), 100_001);
    let text = format!("\"{}\"", "ab".repeat(500_000));
    let ExprKind::Literal(lit) = expr_ok(&text).kind else {
        panic!("a literal")
    };
    assert!(matches!(lit.kind, LiteralKind::Text(ref t) if t.len() == 1_000_000));
    accept(Cpl, &body(&"    let _ = 1\n".repeat(100_000)));
    accept(
        Work,
        &format!(
            "change c {{\n{}}}\n",
            (0..5_000)
                .map(|i| format!("    task t{i} {{\n        write {{ a{i}.body }}\n    }}\n"))
                .collect::<String>()
        ),
    );
}

/// Parse `plain` and `commented` (the same text with a trailing comment)
/// and compare the times.
#[track_caller]
fn assert_comment_costs_little(form: Form, plain: &str, commented: &str) {
    let time = |src: &str| {
        let start = Instant::now();
        accept(form, src);
        start.elapsed()
    };
    let base = time(plain);
    let with_comment = time(commented);
    assert!(
        with_comment <= base * 50 + Duration::from_millis(500),
        "{form:?}: {} bytes on one line parse in {base:?}, and in {with_comment:?} with a trailing `// c`",
        plain.len()
    );
}

#[test]
fn a_trailing_comment_after_a_long_one_line_group_keeps_the_parse_linear() {
    // The grammar fixes no complexity bound, so this follows its intent: D70
    // makes lexing and parsing total over any input, D73 exists so that no
    // input "however deep or long" breaks the toolchain, and D36 anticipates
    // stale or malicious `.work` files submitted to the coordinator. A comment
    // after a long comma group on one line must not turn a parse of a few
    // milliseconds into one of seconds (quadratic in the line's length).
    let n = 8_000;
    let list = format!("[{}1]", "1, ".repeat(n));
    assert_comment_costs_little(Expression, &list, &format!("{list} // c"));
    let set = |comment: &str| {
        format!(
            "change c {{\n    task t {{\n        write {{ {}a }}{comment}\n    }}\n}}\n",
            "a, ".repeat(n)
        )
    };
    assert_comment_costs_little(Work, &set(""), &set(" // c"));
}

#[test]
fn a_flood_of_lexical_errors_yields_one_diagnostic_at_the_first() {
    let src = format!("module m\n\n{}", "#".repeat(100_000));
    let d = reject(Cpl, &src);
    assert_eq!(d.code, codes::INVALID_CHARACTER);
    assert_eq!(d.span.expect("span").start, 10);
    let src = format!("module m\n\n{}", "\"open\n".repeat(50_000));
    let d = reject(Cpl, &src);
    assert_eq!(d.code, codes::UNTERMINATED_TEXT_LITERAL);
    assert_eq!(d.span.expect("span").start, 10);
    let src = "\r".repeat(100_000);
    let d = reject(Work, &src);
    assert_eq!(d.code, codes::INVALID_LINE_TERMINATOR);
    assert_eq!(d.span.expect("span").start, 0);
}
