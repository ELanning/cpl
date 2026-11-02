//! `cpl run` with Python bindings on every engine (`spec/python.md` Section 7, 10; Milestone 3 checklist items 3-5):
//! the fixture `pyshop` (its `catalog` binding is a Python adapter in a worker) with `--engine eval`, `native`, and
//! `both`. The eval and native envelopes are byte-identical after the S75 normalization, `both` reports no
//! divergence, and each outcome is the one the specifications give it: every kind of value through the worker, an
//! adapter exception (with `cpl_foreign_kind` and `cpl_foreign_details`), a worker that exits (`Transport`, or
//! `binding_host_failed`), an ill-typed result (`InvalidResult`, or `binding_error_unrepresentable`), a refinement
//! violation (`InvalidResult`), an `Int` of 6000 digits, a timeout, a failing grant, and the run's `bindings`.

mod common;

use common::*;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn require_python() -> bool {
    let ok = std::process::Command::new(
        std::env::var("CPL_PYTHON").unwrap_or_else(|_| "python3".to_string()),
    )
    .args(["-c", "import sys; assert sys.version_info >= (3, 10)"])
    .status()
    .is_ok_and(|s| s.success());
    if !ok {
        if std::env::var("CPL_REQUIRE_PYTHON").is_ok_and(|v| v == "1") {
            panic!("CPL_REQUIRE_PYTHON=1, but there is no CPython 3.10 or later");
        }
        println!("skipped: no CPython 3.10 or later (set CPL_REQUIRE_PYTHON=1 to fail)");
    }
    ok && {
        if cpl_rt::testing::find_cc().is_some() {
            true
        } else if std::env::var("CPL_REQUIRE_CC").is_ok_and(|v| v == "1") {
            panic!("CPL_REQUIRE_CC=1, but there is no C compiler");
        } else {
            println!("skipped: no C compiler (set CPL_REQUIRE_CC=1 to fail)");
            false
        }
    }
}

/// A fresh copy of `crates/cpl-cabi/tests/projects/pyshop` with `extra` appended to its manifest.
fn pyshop(name: &str, extra: &str) -> PathBuf {
    let dir = temp_dir(name).join("pyshop");
    copy_dir(
        &workspace().join("crates/cpl-cabi/tests/projects/pyshop"),
        &dir,
    );
    if !extra.is_empty() {
        let path = dir.join("cpl.toml");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, format!("{text}{extra}")).unwrap();
    }
    dir
}

fn normalized(mut v: Value) -> String {
    cpl_cabi::compare::normalize(&mut v);
    let mut work = vec![&mut v];
    while let Some(x) = work.pop() {
        match x {
            Value::Object(m) => {
                if m.contains_key("produced_at") {
                    m.insert("produced_at".to_string(), Value::from("<produced_at>"));
                }
                work.extend(m.values_mut());
            }
            Value::Array(a) => work.extend(a.iter_mut()),
            _ => {}
        }
    }
    v.to_string()
}

/// `cpl run --json --engine E <args>` for eval, native, and both: equal eval and native envelopes, equal exit codes,
/// no divergence under both. The native envelope and exit code.
fn three_engines(dir: &Path, args: &[&str]) -> (i32, Value) {
    let with = |engine: &str| {
        let mut all = vec!["run", "--json", "--engine", engine];
        all.extend_from_slice(args);
        let r = cpl(dir, &all);
        (r.code, r.json())
    };
    let (ec, eval) = with("eval");
    let (nc, native) = with("native");
    let (bc, both) = with("both");
    let (a, b) = (normalized(eval.clone()), normalized(native.clone()));
    if a != b {
        let at = a
            .bytes()
            .zip(b.bytes())
            .position(|(x, y)| x != y)
            .unwrap_or(a.len().min(b.len()));
        panic!("{args:?}: the engines diverge at byte {at}\n  eval:   {a}\n  native: {b}");
    }
    assert_eq!(
        (ec, nc),
        (bc, bc),
        "{args:?}: exit codes eval {ec}, native {nc}, both {bc}"
    );
    assert!(
        with_code(&both, "engine_divergence").is_empty(),
        "{args:?}: --engine both diverges: {both}"
    );
    (nc, native)
}

fn sku(s: &str) -> String {
    format!("{{\"$type\": \"pyshop.store.Sku\", \"$payload\": [\"{s}\"]}}")
}

#[test]
fn every_outcome_of_a_python_adapter_agrees_on_every_engine() {
    if !require_python() {
        return;
    }
    let dir = pyshop("python-run", "");
    let b1 = sku("b1");
    let t1 = sku("t1");
    let k1 = sku("k1");
    let zz = sku("zz");
    // (args, exit code, a substring of result_text or of the first diagnostic's message)
    let cases: Vec<(Vec<&str>, i32, &str)> = vec![
        (vec!["pyshop.store.item_name", "--arg", &b1], 0, "Ok(\"Dune\")"),
        (
            vec!["pyshop.store.lookup", "--arg", &k1],
            0,
            "Ok(Item { sku: Sku(\"k1\"), name: \"Kit\", price: 30, category: Bundle([Sku(\"b1\"), Sku(\"t1\")], 2), tags: [\"bundle\"], thumbnail: Bytes(\"6b6974\"), discontinued: false, note: Some(\"\") })",
        ),
        (vec!["pyshop.store.lookup", "--arg", &t1], 0, "name: \"Hammer \u{e9}\u{1f528}\""),
        (vec!["pyshop.store.lookup", "--arg", &zz], 0, "kind: Exception, message: \"no item zz\""),
        (vec!["pyshop.store.stock_count"], 0, "3"),
        (vec!["pyshop.store.fail_with", "\"plain\""], 0, "value: \"ValueError\""),
        (vec!["pyshop.store.fail_with", "\"empty\""], 0, "message: \"RuntimeError\""),
        (vec!["pyshop.store.fail_with", "\"attr\""], 0, "kind: Unavailable"),
        (vec!["pyshop.store.fail_with", "\"Timeout\""], 0, "value: \"ForeignFailure\" }, ForeignDetail { key: \"hint\""),
        (vec!["pyshop.store.crash", "3"], 0, "kind: Transport, message: \"the adapter host for catalog exited with status 3\""),
        (vec!["pyshop.store.crash_plain", "4"], 1, "binding_host_failed"),
        (vec!["pyshop.store.wrong"], 0, "is not a value of Item at /$payload/0"),
        (vec!["pyshop.store.wrong_plain"], 1, "binding_error_unrepresentable"),
        (vec!["pyshop.store.negative"], 0, "does not satisfy Price at /$payload/0: self >= 0"),
        (vec!["pyshop.store.big", "6000"], 0, "Ok(777777"),
    ];
    for (args, code, expected) in cases {
        let (c, native) = three_engines(&dir, &args);
        assert_eq!(c, code, "{args:?}: {native}");
        let text = native["result_text"]
            .as_str()
            .map(str::to_string)
            .or_else(|| {
                native["diagnostics"][0]["message"]
                    .as_str()
                    .map(str::to_string)
            })
            .unwrap_or_default();
        assert!(text.contains(expected), "{args:?}: {text:.400}");
    }
    // the attributes of an exception: the kind, then foreign_type and the valid pairs in order (stdlib S48)
    let (_, native) = three_engines(&dir, &["pyshop.store.fail_with", "\"attr\""]);
    let text = native["result_text"].as_str().unwrap();
    assert!(
        text.contains("ForeignDetail { key: \"foreign_type\", value: \"KeyError\" }, ForeignDetail { key: \"region\", value: \"eu\" }, ForeignDetail { key: \"retry_safe\", value: \"true\" }"),
        "{text}"
    );
    // the run's bindings: the parameter, the binding, its handle, and the adapter's input digest (spec/python.md 9)
    let (_, native) = three_engines(&dir, &["pyshop.store.stock_count"]);
    let b = &native["bindings"][0];
    assert_eq!(
        (
            b["parameter"].as_str(),
            b["binding"].as_str(),
            b["handle"].as_u64()
        ),
        (Some("catalog"), Some("catalog"), Some(0))
    );
    assert_eq!(b["inputs"].as_str().map(str::len), Some(64));
}

#[test]
fn a_timeout_kills_the_worker_and_later_calls_get_transport() {
    if !require_python() {
        return;
    }
    let dir = pyshop("python-timeout", "");
    // a worker that never answers: the adapter module blocks in create? No: an operation that sleeps past the timeout
    let path = dir.join("cpl.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        text.replace(
            "adapter = \"shop_adapters.catalog\"\n",
            "adapter = \"shop_adapters.catalog\"\ntimeout_ms = 1500\n",
        ),
    )
    .unwrap();
    let adapter = dir.join("adapters/shop_adapters/catalog.py");
    let source = std::fs::read_to_string(&adapter).unwrap();
    std::fs::write(&adapter, source.replace("    def count(self):\n        return len(self.items)", "    def count(self):\n        import time\n        time.sleep(30)\n        return len(self.items)")).unwrap();
    let started = std::time::Instant::now();
    let (code, native) = three_engines(&dir, &["pyshop.store.stock_count"]);
    // count() returns Int: a Timeout failure cannot be represented, so it is binding_error_unrepresentable (S82)
    assert_eq!(code, 1, "{native}");
    let message = native["diagnostics"][0]["message"].as_str().unwrap();
    assert!(
        message.contains("binding_error_unrepresentable"),
        "{message}"
    );
    assert!(
        message.contains("did not answer within 1500 ms"),
        "{message}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(60),
        "three runs of a 1.5 s timeout"
    );
}

#[test]
fn a_grant_that_fails_is_interop_bind_failed_on_every_engine() {
    if !require_python() {
        return;
    }
    let dir = pyshop("python-bind", "");
    // the adapter module does not exist
    let path = dir.join("cpl.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        text.replace(
            "adapter = \"shop_adapters.catalog\"",
            "adapter = \"shop_adapters.nowhere\"",
        ),
    )
    .unwrap();
    for engine in ["eval", "native"] {
        let r = cpl(
            &dir,
            &[
                "run",
                "--json",
                "--engine",
                engine,
                "pyshop.store.stock_count",
            ],
        );
        assert_eq!(r.code, 1, "{engine}: {}", r.stdout);
        let json = r.json();
        let d = &with_code(&json, "interop_bind_failed")[0];
        assert_eq!(d["binding"], "catalog", "{engine}");
        let note = d["notes"][0]["message"].as_str().unwrap_or_default();
        assert!(note.contains("ModuleNotFoundError"), "{engine}: {note}");
    }
}
