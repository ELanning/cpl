//! Unit tests of the Milestone 2 additions to cpl-diag (`spec/toolchain.md`
//! T76, 1.4.2, Appendix C): the categories `build` and `interop`, the
//! `cpl build` documents, and the `--engine` wrappers.

use crate::json::*;
use crate::*;
use serde_json::json;
use std::collections::BTreeMap;

fn digest(c: char) -> Digest {
    Digest(std::iter::repeat_n(c, 64).collect())
}

// ---- categories (T76) ---------------------------------------------------------

#[test]
fn category_all_has_the_eighteen_categories_in_declaration_order() {
    assert_eq!(Category::ALL.len(), 18);
    let names: Vec<&str> = Category::ALL.iter().map(|c| c.as_str()).collect();
    assert_eq!(
        names,
        [
            "parse",
            "resolve",
            "type",
            "effect",
            "contract",
            "hole",
            "test",
            "release_policy",
            "runtime",
            "internal",
            "project",
            "index",
            "git",
            "work",
            "patch",
            "usage",
            "build",
            "interop",
        ]
    );
    let mut sorted = Category::ALL;
    sorted.sort();
    assert_eq!(
        sorted,
        Category::ALL,
        "the derived order is declaration order"
    );
}

#[test]
fn build_and_interop_spell_snake_case_everywhere() {
    for (category, text) in [(Category::Build, "build"), (Category::Interop, "interop")] {
        assert_eq!(category.as_str(), text);
        assert_eq!(category.to_string(), text);
        assert_eq!(Category::parse(text), Some(category));
        assert_eq!(serde_json::to_value(category).unwrap(), json!(text));
        assert_eq!(
            serde_json::from_value::<Category>(json!(text)).unwrap(),
            category
        );
    }
    assert_eq!(Category::parse("Build"), None);
    assert!(serde_json::from_value::<Category>(json!("builds")).is_err());
}

#[test]
fn an_interop_diagnostic_from_a_driver_reads_back() {
    // A driver's `{"error": E}` answer carries an S49 object of category
    // `interop` (spec/cabi.md 4.6); the toolchain reads it as DiagnosticJson.
    let text = r#"{"code":"interop_null_pointer","category":"interop","severity":"error","message":"m","argument":"out"}"#;
    let d: DiagnosticJson = serde_json::from_str(text).unwrap();
    assert_eq!(d.category, Category::Interop);
    assert_eq!(d.data["argument"], json!("out"));
    assert_eq!(serde_json::to_string(&d).unwrap(), text);
    let build = Diagnostic::error(
        DiagCode("build_cc_unavailable"),
        Category::Build,
        "no C compiler",
    )
    .with_data("searched", json!(["cc"]));
    let sm = SourceMap::new();
    let v = serde_json::to_value(build.to_json(&sm)).unwrap();
    assert_eq!(
        v,
        json!({ "code": "build_cc_unavailable", "category": "build", "severity": "error",
                "message": "no C compiler", "searched": ["cc"] })
    );
    assert_eq!(
        build.to_string(),
        "error[build_cc_unavailable]: no C compiler"
    );
}

// ---- cpl build (Section 7.2) ----------------------------------------------------

fn reader_build() -> BuildOutput {
    let export = |symbol: &str, function: &str, effects: &[&str]| BuildExport {
        symbol: symbol.into(),
        function: function.into(),
        id: StableId::parse(function).unwrap(),
        signature: digest('5'),
        int: "handle".into(),
        effects: effects.iter().map(|s| s.to_string()).collect(),
    };
    let artifact = |kind: &str, path: &str| BuildArtifact {
        kind: kind.into(),
        path: path.into(),
        digest: digest('6'),
    };
    BuildOutput {
        target: "c".into(),
        release: false,
        profile: "optimized".into(),
        sanitize: vec![],
        out_dir: ".cpl/build/c".into(),
        compiler: CompilerInfo {
            path: "/usr/bin/x86_64-linux-gnu-gcc-11".into(),
            family: "gcc".into(),
            version: "11.4.0".into(),
            target: "x86_64-linux-gnu".into(),
            digest: digest('1'),
        },
        runtime: RuntimeInfo {
            version: "0.1.0".into(),
            api: "0.1".into(),
            source: "embedded".into(),
            digest: digest('2'),
        },
        program_digest: digest('3'),
        exports_digest: digest('4'),
        exports: vec![
            export(
                "reader_image_width",
                "reader.images.image_width",
                &["codec.dimensions"],
            ),
            export("reader_load_page", "reader.pages.load_page", &["db.get"]),
            export("reader_load_text", "reader.pages.load_text", &["db.get"]),
            export("reader_paginate_ints", "reader.pages.paginate_ints", &[]),
        ],
        bindings: vec![],
        artifacts: vec![
            artifact("header", ".cpl/build/c/include/cpl.h"),
            artifact("header", ".cpl/build/c/include/cpl_adapter.h"),
            artifact("header", ".cpl/build/c/include/reader.h"),
            artifact("static_library", ".cpl/build/c/lib/libreader.a"),
            artifact("shared_library", ".cpl/build/c/lib/libreader.so.0"),
        ],
        inputs: vec![],
        holes: 0,
        lock: BuildLock {
            checked: false,
            written: false,
        },
        cached: false,
        duration_ms: 812,
        python: None,
    }
}

#[test]
fn build_output_writes_every_key_in_the_order_of_the_example() {
    let out = reader_build();
    let v = serde_json::to_value(&out).unwrap();
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    // serde_json's map is sorted (T4); the key order of the text is the
    // field order, checked on the string below.
    assert_eq!(keys.len(), 17);
    let text = serde_json::to_string(&out).unwrap();
    let order = [
        "\"target\"",
        "\"release\"",
        "\"profile\"",
        "\"sanitize\"",
        "\"out_dir\"",
        "\"compiler\"",
        "\"runtime\"",
        "\"program_digest\"",
        "\"exports_digest\"",
        "\"exports\"",
        "\"bindings\"",
        "\"artifacts\"",
        "\"inputs\"",
        "\"holes\"",
        "\"lock\"",
        "\"cached\"",
        "\"duration_ms\"",
    ];
    let mut at = 0;
    for key in order {
        let found = text[at..]
            .find(&format!("{key}:"))
            .unwrap_or_else(|| panic!("{key} after byte {at} in {text}"));
        at += found + key.len();
    }
    // Every key is always present (Appendix C), the empty arrays included.
    assert_eq!(v["sanitize"], json!([]));
    assert_eq!(v["bindings"], json!([]));
    assert_eq!(v["inputs"], json!([]));
    assert_eq!(v["lock"], json!({ "checked": false, "written": false }));
    assert_eq!(
        v["compiler"],
        json!({ "path": "/usr/bin/x86_64-linux-gnu-gcc-11", "family": "gcc", "version": "11.4.0",
                "target": "x86_64-linux-gnu", "digest": digest('1').0 })
    );
    assert_eq!(
        v["runtime"],
        json!({ "version": "0.1.0", "api": "0.1", "source": "embedded", "digest": digest('2').0 })
    );
    assert_eq!(
        v["exports"][0],
        json!({ "symbol": "reader_image_width", "function": "reader.images.image_width",
                "id": "reader.images.image_width", "signature": digest('5').0, "int": "handle",
                "effects": ["codec.dimensions"] })
    );
    assert_eq!(
        v["artifacts"][3],
        json!({ "kind": "static_library", "path": ".cpl/build/c/lib/libreader.a", "digest": digest('6').0 })
    );
    let export_text = serde_json::to_string(&out.exports[3]).unwrap();
    assert_eq!(
        export_text,
        format!(
            r#"{{"symbol":"reader_paginate_ints","function":"reader.pages.paginate_ints","id":"reader.pages.paginate_ints","signature":"{}","int":"handle","effects":[]}}"#,
            digest('5').0
        )
    );
    let back: BuildOutput = serde_json::from_str(&text).unwrap();
    assert_eq!(back, out);
}

#[test]
fn build_bindings_and_inputs_have_the_keys_of_section_7_2() {
    let binding = BuildBinding {
        binding: "image_codec".into(),
        capability: "reader.images.ImageCodec".into(),
        form: "wrapper".into(),
        isolation: "in_process".into(),
        grant: "cboundary_bind_image_codec".into(),
    };
    assert_eq!(
        serde_json::to_string(&binding).unwrap(),
        r#"{"binding":"image_codec","capability":"reader.images.ImageCodec","form":"wrapper","isolation":"in_process","grant":"cboundary_bind_image_codec"}"#
    );
    let input = BuildInput {
        kind: "vendored_source".into(),
        binding: "image_codec".into(),
        path: "vendor/imgtest/imgtest.c".into(),
        digest: digest('a'),
    };
    assert_eq!(
        serde_json::to_string(&input).unwrap(),
        format!(
            r#"{{"kind":"vendored_source","binding":"image_codec","path":"vendor/imgtest/imgtest.c","digest":"{}"}}"#,
            digest('a').0
        )
    );
    let mut out = reader_build();
    out.bindings = vec![binding];
    out.inputs = vec![input];
    out.release = true;
    out.sanitize = vec!["address".into(), "undefined".into()];
    out.lock = BuildLock {
        checked: true,
        written: true,
    };
    let text = serde_json::to_string_pretty(&out).unwrap();
    assert_eq!(serde_json::from_str::<BuildOutput>(&text).unwrap(), out);
}

#[test]
fn build_output_flattens_into_the_envelope_after_its_five_keys() {
    let env = Envelope {
        command: "build".into(),
        toolchain: ToolchainInfo::current(),
        revision: None,
        status: Status::Ok,
        diagnostics: vec![],
        result: reader_build(),
    };
    let text = serde_json::to_string(&env).unwrap();
    let diagnostics = text.find("\"diagnostics\"").unwrap();
    let target = text.find("\"target\"").unwrap();
    assert!(diagnostics < target, "{text}");
    let back: Envelope<BuildOutput> = serde_json::from_str(&text).unwrap();
    assert_eq!(back, env);
    // A failure carries no BuildOutput keys (NoOutput).
    let failed = Envelope {
        result: NoOutput {},
        status: Status::Unsuccessful,
        ..env.map_result()
    };
    let v = serde_json::to_value(&failed).unwrap();
    assert!(v.get("target").is_none());
}

trait MapResult {
    fn map_result(self) -> Envelope<NoOutput>;
}

impl MapResult for Envelope<BuildOutput> {
    fn map_result(self) -> Envelope<NoOutput> {
        Envelope {
            command: self.command,
            toolchain: self.toolchain,
            revision: self.revision,
            status: self.status,
            diagnostics: self.diagnostics,
            result: NoOutput {},
        }
    }
}

// ---- --engine (Section 7.2) --------------------------------------------------------

#[test]
fn engine_kinds_spell_eval_native_both() {
    assert_eq!(EngineKind::default(), EngineKind::Eval);
    for (engine, text) in [
        (EngineKind::Eval, "eval"),
        (EngineKind::Native, "native"),
        (EngineKind::Both, "both"),
    ] {
        assert_eq!(engine.as_str(), text);
        assert_eq!(engine.to_string(), text);
        assert_eq!(EngineKind::parse(text), Some(engine));
        assert_eq!(serde_json::to_value(engine).unwrap(), json!(text));
        assert_eq!(
            serde_json::from_value::<EngineKind>(json!(text)).unwrap(),
            engine
        );
    }
    assert_eq!(EngineKind::parse("c"), None);
}

fn version(path: &str) -> SymbolVersion {
    SymbolVersion {
        id: StableId::parse(path).unwrap(),
        path: path.into(),
        provisional: true,
        facets: BTreeMap::from([(Facet::Identity, FacetHash("i".into()))]),
        definition: FacetHash("d".into()),
    }
}

fn run_output() -> RunOutput {
    RunOutput {
        function: version("reader.pages.paginate_ints"),
        arguments: vec![json!([1, 2, 3]), json!(2)],
        result: Some(json!({ "$type": "Result", "$variant": "Ok", "$payload": [[[1, 2], [3]]] })),
        result_text: Some("Ok([[1, 2], [3]])".into()),
        fault: None,
    }
}

#[test]
fn an_eval_run_without_bindings_is_the_milestone_1_object() {
    let m1 = serde_json::to_string(&run_output()).unwrap();
    let wrapped = EngineRunOutput {
        run: run_output(),
        engine: None,
        bindings: vec![],
    };
    assert_eq!(serde_json::to_string(&wrapped).unwrap(), m1);
    assert_eq!(
        serde_json::from_str::<EngineRunOutput>(&m1).unwrap(),
        wrapped
    );
    // A fault keeps the nulls of RunOutput.
    let mut faulted = wrapped.clone();
    faulted.run.result = None;
    faulted.run.result_text = None;
    let v = serde_json::to_value(&faulted).unwrap();
    assert_eq!(v["result"], serde_json::Value::Null);
    assert_eq!(v["result_text"], serde_json::Value::Null);
    assert_eq!(v["fault"], serde_json::Value::Null);
    assert!(v.get("engine").is_none());
    assert!(v.get("bindings").is_none());
    assert_eq!(
        serde_json::from_value::<EngineRunOutput>(v).unwrap(),
        faulted
    );
}

#[test]
fn a_native_run_with_bindings_appends_engine_and_bindings() {
    let out = EngineRunOutput {
        run: run_output(),
        engine: Some(EngineKind::Native),
        bindings: vec![RunBinding {
            parameter: "codec".into(),
            binding: "image_codec".into(),
            handle: 0,
            inputs: digest('7'),
        }],
    };
    let text = serde_json::to_string(&out).unwrap();
    let fault = text.find("\"fault\"").unwrap();
    let engine = text.find("\"engine\":\"native\"").unwrap();
    let bindings = text.find("\"bindings\"").unwrap();
    assert!(fault < engine && engine < bindings, "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        v["bindings"],
        json!([{ "parameter": "codec", "binding": "image_codec", "handle": 0, "inputs": digest('7').0 }])
    );
    assert_eq!(serde_json::from_str::<EngineRunOutput>(&text).unwrap(), out);
}

fn test_output() -> TestOutput {
    let record = TestCaseRecord {
        symbol: StableId::parse("tests.pagination.\"pagination preserves input\"").unwrap(),
        qualified_name: "tests.pagination.\"pagination preserves input\"".into(),
        kind: TestKind::Property,
        status: TestStatus::Tested,
        duration_ms: 3,
        seed: Some(1446357982114401307),
        property_seed: Some("c0f6".into()),
        cases: Some(100),
        failing_case: None,
        shrunk: None,
        fault: None,
        counterexamples: vec![],
        receipt: None,
    };
    let report = TestReport {
        seed: 1446357982114401307,
        cases: 100,
        selected: 1,
        tested: 1,
        failed: 0,
        results: vec![record],
    };
    let receipt = TestReceipt {
        receipt: ReceiptKind::Test,
        schema: version::JSON_SCHEMA,
        toolchain: ToolchainInfo::current(),
        revision: Revision {
            commit: None,
            state: RevisionState::Worktree,
            digest: Digest("9a0b".into()),
        },
        inputs: ReceiptInputsJson {
            manifest: Digest("m".into()),
            lock: None,
            std: Digest("s".into()),
            sources: BTreeMap::new(),
        },
        core: ReceiptCore {
            source_revision: "unversioned".into(),
            dirty: None,
            toolchain: ReceiptToolchain::current(Digest("s".into())),
            modules: vec![],
            check_configuration: json!({ "cases": 100, "engine": "native",
                "native": { "runtime_digest": digest('2').0, "compiler_digest": digest('1').0 } }),
            assumptions: vec![],
            fixtures: Some(vec![]),
            environment: Some(BTreeMap::new()),
        },
        configuration: TestRunConfig {
            seed: 1446357982114401307,
            cases: 100,
            max_shrinks: 1000,
            timeout_ms: 0,
            selector: TestSelectorSpec {
                namespace: None,
                name: None,
                id: None,
            },
            check_contracts: true,
            fail_fast: false,
        },
        tested: vec![],
        tests: vec![],
        summary: TestSummary {
            selected: 1,
            tested: 1,
            failed: 0,
        },
        fixtures: vec![],
        assumptions: vec![],
        environment: BTreeMap::new(),
        status: TestStatus::Tested,
        produced_at: "2026-10-29T12:00:00Z".into(),
        duration_ms: 12,
    };
    TestOutput { report, receipt }
}

#[test]
fn engine_test_output_omits_engine_under_eval_and_flattens_test_output() {
    let m1 = serde_json::to_string(&test_output()).unwrap();
    let eval = EngineTestOutput {
        test: test_output(),
        engine: None,
    };
    assert_eq!(serde_json::to_string(&eval).unwrap(), m1);
    assert_eq!(serde_json::from_str::<EngineTestOutput>(&m1).unwrap(), eval);
    let both = EngineTestOutput {
        test: test_output(),
        engine: Some(EngineKind::Both),
    };
    let text = serde_json::to_string(&both).unwrap();
    assert!(text.ends_with(",\"engine\":\"both\"}"), "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["seed"], json!(1446357982114401307u64));
    assert_eq!(
        v["receipt"]["core"]["check_configuration"]["engine"],
        "native"
    );
    let back: EngineTestOutput = serde_json::from_str(&text).unwrap();
    assert_eq!(back, both);
}

#[test]
fn json_schema_stays_one() {
    // T73: Milestone 2 changes no Milestone 1 wire type.
    assert_eq!(version::JSON_SCHEMA, 1);
}
