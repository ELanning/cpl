//! The acceptance evaluations of `spec/stdlib.md` 12 as CPL source checked
//! by the real checker: every row of the value table is an `assert` of a
//! test that must be `tested`, and every row of the fault table a function
//! whose run raises exactly the named fault. Through source the rows also
//! exercise what the checker makes of them: member calls as calls of their
//! stubs with inferred type arguments (toolchain T19), `@property` members
//! read without parentheses, literals proved against refinements, and the
//! `std` root and `gen` alias.

mod common;

use cpl_diag::json::TestStatus;
use cpl_eval::{TestConfig, TestSelector};
use serde_json::json;

/// The value rows. `[]`, `None`, and `Err(...)` that need a type are bound
/// with an annotated `let` first, as stdlib 12 says.
const VALUES: &str = r#"module accept.values

test "division truncates toward zero" {
    assert 7 / 2 == 3
    assert -7 / 2 == -3
    assert 7 / -2 == -3
    assert -7 / -2 == 3
}

test "the remainder takes the dividend's sign" {
    assert 7 % 2 == 1
    assert -7 % 2 == -1
    assert 7 % -2 == 1
    assert -7 % -2 == -1
}

test "ints are arbitrary precision" {
    assert 12345678901234567890 * 10 == 123456789012345678900
}

test "int members" {
    assert (-5).abs() == 5
    assert 3.min(5) == 3
    assert (-3).max(0) == 0
    assert 4.min(4) == 4
    assert (-5).to_text() == "-5"
    assert 0.to_text() == "0"
    assert 1_000.to_text() == "1000"
}

test "take and drop clamp" {
    assert [1, 2, 3].take(2) == [1, 2]
    assert [1, 2, 3].take(5) == [1, 2, 3]
    assert [1, 2, 3].take(0) == []
    assert [1, 2, 3].drop(2) == [3]
    assert [1, 2, 3].drop(5) == []
    assert [1, 2, 3].drop(0) == [1, 2, 3]
}

test "drop_last, push, and concatenation" {
    let empty: List<Int> = []
    assert [1, 2, 3].drop_last() == [1, 2]
    assert empty.drop_last() == []
    assert [1].push(2) == [1, 2]
    assert [1] ++ [2, 3] == [1, 2, 3]
    assert empty ++ empty == []
}

test "flatten" {
    let none: List<List<Int>> = []
    assert flatten([[1, 2], [], [3]]) == [1, 2, 3]
    assert flatten(none) == []
}

test "list lengths and lookups" {
    let empty: List<Int> = []
    assert [1, 2, 3].len == 3
    assert empty.len == 0
    assert empty.is_empty()
    assert not [1].is_empty()
    assert [1, 2, 3].get(1) == Some(2)
    assert [1, 2, 3].get(3) == None
    assert [1, 2, 3].get(-1) == None
    assert [1, 2, 3].first() == Some(1)
    assert [1, 2, 3].last() == Some(3)
    assert empty.last() == None
    assert [1, 2, 3].reverse() == [3, 2, 1]
    assert [1, 2, 3].contains(2)
    assert not [1, 2, 3].contains(4)
}

test "structural equality and forall" {
    let empty: List<Int> = []
    assert [[1], [2]] == [[1], [2]]
    assert not ([1, 2] == [2, 1])
    assert forall x in [1, 2, 3]: x > 0
    assert forall x in empty: false
}

test "text counts scalar values" {
    assert "h\u{e9}llo".len == 5
    assert "\u{1f600}".len == 1
    assert "e\u{301}".len == 2
    assert "".len == 0
    assert "".is_empty()
    assert "a" ++ "b" == "ab"
    assert not ("\u{e9}" == "e\u{301}")
}

test "trimming" {
    assert std.text.trim("  5 ") == "5"
    assert std.text.trim("\u{a0}x\u{a0}") == "x"
    assert std.text.trim("\u{200b}x") == "\u{200b}x"
    assert std.text.trim_start("  a ") == "a "
    assert std.text.trim_end("  a ") == "  a"
}

test "prefixes, suffixes, and needles" {
    assert std.text.starts_with("abc", "ab")
    assert std.text.starts_with("abc", "")
    assert not std.text.starts_with("a", "ab")
    assert std.text.ends_with("abc", "bc")
    assert std.text.contains("abc", "b")
    assert std.text.contains("abc", "")
}

test "split and join" {
    let none: List<Text> = []
    assert std.text.split("a,b,,c", ",") == ["a", "b", "", "c"]
    assert std.text.split("", ",") == [""]
    assert std.text.split("a,", ",") == ["a", ""]
    assert std.text.split("aaa", "aa") == ["", "a"]
    assert std.text.join(["a", "b"], ", ") == "a, b"
    assert std.text.join(none, ",") == ""
    assert std.text.join(["x"], ",") == "x"
}

test "case mapping" {
    assert std.text.to_upper("abc\u{e9}") == "ABC\u{e9}"
    assert std.text.to_lower("ABC") == "abc"
}

test "parse_int" {
    assert std.text.parse_int("42") == Ok(42)
    assert std.text.parse_int("-0") == Ok(0)
    assert std.text.parse_int("+7") == Ok(7)
    assert std.text.parse_int("007") == Ok(7)
    assert std.text.parse_int("99999999999999999999") == Ok(99999999999999999999)
    assert std.text.parse_int(" 42").is_err()
    assert std.text.parse_int("").is_err()
    assert std.text.parse_int("4_2").is_err()
    assert std.text.parse_int("\u{ff11}").is_err()
    assert std.text.parse_int("-").is_err()
}

test "from_int and chars" {
    assert std.text.from_int(-12) == "-12"
    assert std.text.chars("h\u{e9}y") == ["h", "\u{e9}", "y"]
}

test "bytes" {
    assert std.bytes.from_list([104, 105]).len == 2
    assert std.bytes.to_list(std.bytes.from_list([1, 2])) == [1, 2]
    assert std.bytes.from_text("\u{e9}").len == 2
    assert std.bytes.to_text(std.bytes.from_text("\u{e9}")) == Ok("\u{e9}")
    assert std.bytes.to_text(std.bytes.from_list([255])).is_err()
}

test "option members" {
    let o: Option<Int> = None
    assert Some(1).unwrap_or(0) == 1
    assert o.unwrap_or(0) == 0
    assert Some(1).is_some()
    assert o.is_none()
    assert Some(2).ok_or("e") == Ok(2)
    assert o.ok_or("e") == Err("e")
}

test "result members" {
    let failed: Result<Int, Text> = Err("x")
    let one: Result<Int, Text> = Ok(1)
    assert one.unwrap_or(0) == 1
    assert failed.unwrap_or(0) == 0
    assert one.is_ok()
    assert failed.is_err()
    assert one.ok() == Some(1)
    assert failed.err() == Some("x")
    assert one.err() == None
}

test "unit and wrapper equality" {
    assert Unit == Unit
    assert Tag("a") == Tag("a")
}

pub type Tag(Text)
"#;

/// The `check<R>` rows: the type names of S44 in `reader.pages` and `m`.
const PAGES: &str = r#"module reader.pages

pub type Positive = Int where self > 0

test "check results" {
    assert check<Positive>(3) == Ok(3)
    assert check<Positive>(0).is_err()
}

test "type names of a failed check" {
    let mine = match check<Positive>(0) {
        Err(e) => e.type_name
        Ok(_) => ""
    }
    let library = match check<gen.Positive>(0) {
        Err(e) => e.type_name
        Ok(_) => ""
    }
    let parsed = match std.text.parse_int("x") {
        Err(e) => e.type_name
        Ok(_) => ""
    }
    assert mine == "reader.pages.Positive"
    assert library == "std.gen.Positive"
    assert parsed == "Int"
}
"#;

const SMALL: &str = r#"module m

use reader.pages.{Positive}

pub type Small = Positive where self < 10

test "a chain names the first predicate that fails" {
    let root = match check<Small>(0) {
        Err(e) => e.type_name
        Ok(_) => ""
    }
    let own = match check<Small>(20) {
        Err(e) => e.type_name
        Ok(_) => ""
    }
    assert root == "reader.pages.Positive"
    assert own == "m.Small"
}
"#;

#[test]
fn every_value_row_is_a_passing_assert() {
    let p = common::check_files(&[
        ("src/accept/values.cpl", VALUES),
        ("src/reader/pages.cpl", PAGES),
        ("src/m.cpl", SMALL),
    ]);
    let report = cpl_eval::run_tests(&p, &TestSelector::all(), &TestConfig::with_seed(0));
    let failed: Vec<String> = report
        .results
        .iter()
        .filter(|r| r.status != TestStatus::Tested)
        .map(|r| {
            format!(
                "{}: {}",
                r.qualified_name,
                serde_json::to_string(&r.fault).unwrap()
            )
        })
        .collect();
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert_eq!((report.selected, report.tested), (23, 23));
}

/// The fault rows, each the body of a function of `accept.faults`.
const FAULTS: &str = r#"module accept.faults

fn take_negative() -> List<Int> {
    [1].take(-1)
}

fn drop_negative() -> List<Int> {
    [1].drop(-1)
}

fn divide() -> Int {
    1 / 0
}

fn remainder() -> Int {
    1 % 0
}

fn split_empty() -> List<Text> {
    std.text.split("a", "")
}

fn byte_too_big() -> Bytes {
    std.bytes.from_list([256])
}

fn byte_negative() -> Bytes {
    std.bytes.from_list([-1])
}
"#;

#[test]
fn every_fault_row_raises_its_fault() {
    let p = common::check_files(&[("src/accept/faults.cpl", FAULTS)]);
    let bytes_clause = "forall v in values: v >= 0 and v <= 255";
    for (f, symbol, clause, input) in [
        (
            "take_negative",
            "std.prelude.List.take",
            "n >= 0",
            json!({"self": [1], "n": -1}),
        ),
        (
            "drop_negative",
            "std.prelude.List.drop",
            "n >= 0",
            json!({"self": [1], "n": -1}),
        ),
        (
            "split_empty",
            "std.text.split",
            "not sep.is_empty()",
            json!({"text": "a", "sep": ""}),
        ),
        (
            "byte_too_big",
            "std.bytes.from_list",
            bytes_clause,
            json!({"values": [256]}),
        ),
        (
            "byte_negative",
            "std.bytes.from_list",
            bytes_clause,
            json!({"values": [-1]}),
        ),
    ] {
        let name = format!("accept.faults.{f}");
        let d = common::fault(&p, &common::run(&p, &name, &[]));
        assert_eq!(d["code"], "contract_violation", "{f}");
        assert_eq!(d["clause_kind"], "requires", "{f}");
        assert_eq!(d["attributed_to"], "caller", "{f}");
        assert_eq!(d["symbol"], symbol, "{f}");
        assert_eq!(d["facet"], "contract", "{f}");
        assert_eq!(d["clause"], clause, "{f}");
        assert_eq!(d["input"], input, "{f}");
        assert_eq!(d["caller"], name.as_str(), "{f}");
        assert_eq!(
            d["span"]["file"].as_str().map(|f| f.starts_with("std/")),
            Some(true)
        );
    }
    for f in ["divide", "remainder"] {
        let name = format!("accept.faults.{f}");
        let d = common::fault(&p, &common::run(&p, &name, &[]));
        assert_eq!(d["code"], "division_by_zero", "{f}");
        assert_eq!(d["symbol"], name.as_str());
        assert_eq!(d["input"], json!({"a": 1, "b": 0}));
        assert!(
            d.get("clause").is_none(),
            "no clause: b != 0 is an obligation only"
        );
    }
}
