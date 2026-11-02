//! Suite D7 (toolchain 12.6): the runtime fuzzed against the evaluator. `Int` `+ - * / %`, the comparisons, `to_text`,
//! and `parse_int` on random and edge integers (every limb boundary, 1 to 2,000 digits, every sign mix); UTF-8 decoding
//! on random bytes; the text builtins on random Unicode; and `quote_text`, value text, value JSON, and the JSON reader
//! on random Unicode, through the driver's `render` and `from_json` requests: 10,000 inputs per operation (times
//! `CPL_DIFF_SCALE`), keyed by `CPL_DIFF_SEED`.

use crate::{assert_clean_shutdown, driver_config, Gate};
use cpl_cabi::NativeSession;
use cpl_diag::SymbolPath;
use cpl_eval::{prepare_run, run_function, EvalConfig, RunArg, Value};
use num_bigint::BigInt;
use rand::{Rng, RngExt, SeedableRng};
use rand_chacha::ChaCha20Rng;
use serde_json::{json, Value as Json};

/// The edge integers: 0, ±1, around every limb boundary, and around powers of ten.
fn edges() -> Vec<BigInt> {
    let mut out = vec![BigInt::from(0), BigInt::from(1), BigInt::from(-1)];
    for k in [
        7u32, 8, 15, 16, 31, 32, 62, 63, 64, 65, 127, 128, 129, 191, 192, 255, 256, 1024,
    ] {
        let p = BigInt::from(1) << k;
        for d in [-1i64, 0, 1] {
            let x = &p + BigInt::from(d);
            out.push(-x.clone());
            out.push(x);
        }
    }
    for k in [1u32, 2, 9, 10, 18, 19, 20, 38, 39, 100] {
        let p = BigInt::from(10).pow(k);
        for d in [-1i64, 0, 1] {
            let x = &p + BigInt::from(d);
            out.push(-x.clone());
            out.push(x);
        }
    }
    out
}

/// A random integer: an edge three times in ten, else 1 to 2,000 digits with a random sign.
fn int(rng: &mut ChaCha20Rng, edges: &[BigInt]) -> BigInt {
    if rng.random_range(0..10) < 3 {
        return edges[rng.random_range(0..edges.len())].clone();
    }
    let digits = if rng.random_range(0..4) == 0 {
        rng.random_range(1..=2000)
    } else {
        rng.random_range(1..=40)
    };
    let mut s = String::with_capacity(digits + 1);
    if rng.random_range(0..2) == 0 {
        s.push('-');
    }
    s.push(char::from(b'1' + rng.random_range(0..9)));
    for _ in 1..digits {
        s.push(char::from(b'0' + rng.random_range(0..10)));
    }
    s.parse().unwrap()
}

/// A random text over ASCII, Latin-1, the BMP, astral scalars, combining marks, every White_Space scalar, U+200B, and
/// U+FEFF, of 0 to 40 scalars.
fn text(rng: &mut ChaCha20Rng) -> String {
    const SPECIAL: [char; 31] = [
        '\u{9}', '\u{a}', '\u{b}', '\u{c}', '\u{d}', ' ', '\u{85}', '\u{a0}', '\u{1680}',
        '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}', '\u{2006}',
        '\u{2007}', '\u{2008}', '\u{2009}', '\u{200a}', '\u{2028}', '\u{2029}', '\u{202f}',
        '\u{205f}', '\u{3000}', '\u{200b}', '\u{feff}', '\u{301}', '\u{0}', '"', '\\',
    ];
    let n = rng.random_range(0..=40);
    let mut s = String::new();
    for _ in 0..n {
        let c = match rng.random_range(0..6) {
            0 => char::from(rng.random_range(0x20u8..0x7f)),
            1 => char::from_u32(rng.random_range(0xa0..0x250)).unwrap_or('x'),
            2 => char::from_u32(rng.random_range(0x370..0xd7ff)).unwrap_or('y'),
            3 => char::from_u32(rng.random_range(0x1f300..0x1fa00)).unwrap_or('z'),
            4 => SPECIAL[rng.random_range(0..SPECIAL.len())],
            _ => char::from(rng.random_range(b'a'..=b'z')),
        };
        s.push(c);
    }
    s
}

fn number(n: &BigInt) -> Json {
    serde_json::from_str(&n.to_string()).unwrap()
}

pub(crate) fn d7(gate: &mut Gate) {
    let seed = std::env::var("CPL_DIFF_SEED").unwrap_or_else(|_| "0".to_string());
    let scale: usize = std::env::var("CPL_DIFF_SCALE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let n = 10_000 * scale;
    let mut rng = ChaCha20Rng::from_seed(*blake3::hash(format!("D7 {seed}").as_bytes()).as_bytes());
    let source = std::fs::read_to_string(crate::diff_corpus_dir().join("stdlib.cpl")).unwrap();
    let project = crate::project_of("stdlib", "stdlib.rows", &source);
    let checked = cpl_sema::check(&project);
    assert!(!checked.has_errors());
    let p = &checked.program;
    let sm = project.source_map.clone();
    let plan = cpl_sema::interop::InteropPlan::empty(project.manifest.c_prefix());
    let cfg = EvalConfig::default();
    let mut session =
        NativeSession::start(p, &project, &plan, &cfg, &driver_config(gate.sanitize)).unwrap();
    session.set_request_timeout(Some(std::time::Duration::from_secs(60)));
    let edges = edges();
    // (function, argument generator)
    let call =
        |gate: &mut Gate, session: &mut NativeSession<'_>, function: &str, args: Vec<Json>| {
            let path = SymbolPath::parse(&format!("stdlib.rows.{function}")).unwrap();
            let run_args: Vec<RunArg> = args.iter().cloned().map(RunArg::Json).collect();
            let eval =
                run_function(p, &path, &run_args, &cfg).expect("the evaluator runs the case");
            let prepared = prepare_run(p, &path, &run_args, &|_| false).expect("prepare_run");
            let native = session
                .call(prepared.function, &prepared.arguments, &[], true)
                .expect("the driver answers");
            gate.compare(
                session,
                p,
                &sm,
                &eval.return_ty,
                &eval.value,
                &native,
                format!("D7 {function} {args:?}"),
            );
        };
    for op in ["add", "sub", "mul", "div", "rem", "compare"] {
        for _ in 0..n {
            let (a, b) = (int(&mut rng, &edges), int(&mut rng, &edges));
            call(gate, &mut session, op, vec![number(&a), number(&b)]);
        }
    }
    for _ in 0..n {
        let a = int(&mut rng, &edges);
        call(gate, &mut session, "int_text", vec![number(&a)]);
        // parse_int on the canonical text, and on a perturbed one
        let mut t = a.to_string();
        if rng.random_range(0..3) == 0 {
            let at = rng.random_range(0..=t.len());
            let junk = ["+", "-", " ", "_", "0", "\u{ff11}", "x", ""][rng.random_range(0..8)];
            if t.is_char_boundary(at) {
                t.insert_str(at, junk);
            }
        }
        call(gate, &mut session, "parse_int", vec![json!(t)]);
    }
    // UTF-8 decoding on random bytes (the offset of the first invalid sequence is in the error)
    for _ in 0..n {
        let len = rng.random_range(0..=24);
        let mut bytes = vec![0u8; len];
        rng.fill_bytes(&mut bytes);
        if rng.random_range(0..2) == 0 {
            // mostly valid UTF-8 with one damaged byte
            let s = text(&mut rng);
            bytes = s.into_bytes();
            if !bytes.is_empty() && rng.random_range(0..2) == 0 {
                let at = rng.random_range(0..bytes.len());
                bytes[at] = rng.random_range(0x80..=0xff);
            }
        }
        let list: Vec<Json> = bytes.iter().map(|b| json!(b)).collect();
        call(gate, &mut session, "decode", vec![Json::Array(list)]);
    }
    // the text builtins on random Unicode
    for op in [
        "trim",
        "trim_start",
        "trim_end",
        "to_upper",
        "to_lower",
        "chars",
        "text_len",
    ] {
        for _ in 0..n {
            call(gate, &mut session, op, vec![json!(text(&mut rng))]);
        }
    }
    for op in [
        "split",
        "starts_with",
        "ends_with",
        "text_contains",
        "text_concat",
        "text_equal",
    ] {
        for _ in 0..n {
            let a = text(&mut rng);
            let b = if rng.random_range(0..3) == 0 && !a.is_empty() {
                // a piece of a, so that a match occurs
                let chars: Vec<char> = a.chars().collect();
                let i = rng.random_range(0..chars.len());
                let j = rng.random_range(i..chars.len());
                chars[i..=j].iter().collect()
            } else {
                let mut b = text(&mut rng);
                b.truncate(b.char_indices().nth(3).map_or(b.len(), |(i, _)| i));
                b
            };
            // split refuses an empty separator (its contract): both engines fault alike
            call(gate, &mut session, op, vec![json!(a), json!(b)]);
        }
    }
    // quote_text, value text, value JSON, and the JSON reader on random Unicode
    for _ in 0..n {
        let s = text(&mut rng);
        let v = Value::text(&s);
        let json = serde_json::to_string(&cpl_eval::json::value_to_json_untyped(p, &v)).unwrap();
        let shown = cpl_eval::json::value_text_untyped(p, &v);
        gate.compared += 1;
        if shown != cpl_diag::quote_text(&s) {
            gate.divergences
                .push(format!("D7 value text of {s:?} is not quote_text"));
        }
        match session.render(&v) {
            Ok((back, r)) => {
                if !back.structurally_equal(&v)
                    || r.json.as_deref() != Some(json.as_str())
                    || r.text.as_deref() != Some(shown.as_str())
                {
                    gate.divergences.push(format!(
                        "D7 render of {s:?}: json {:?}, text {:?}",
                        r.json, r.text
                    ));
                }
            }
            Err(e) => gate
                .divergences
                .push(format!("D7 render of {s:?}: {:?}", e.into_diagnostics())),
        }
        match session.from_json("Text", &json) {
            Ok(Ok((back, _))) if back.structurally_equal(&v) => {}
            other => gate.divergences.push(format!(
                "D7 from_json of {json}: {:?}",
                other.map(|r| r.map(|_| ()))
            )),
        }
    }
    reader(gate, &mut session, p, &mut rng, n);
    gate.absorb(&mut session);
    assert_clean_shutdown(session, "D7");
}

/// The evaluator's side of the reader: the strict reader of `cpl run --arg` (`crates/cpl-cli/src/json_arg.rs`:
/// `serde_json` with every digit kept, an object's duplicate key refused; the inputs below stay far inside its
/// nesting limit, which the C reader does not have) and then `cpl_eval::json::value_from_json` at the type.
fn strict_read(p: &cpl_hir::Program, text: &str, ty: &cpl_hir::Ty) -> Result<Value, String> {
    let parsed: Json = serde_json::from_str(text).map_err(|e| format!("syntax: {e}"))?;
    // serde_json collapses an object's duplicate keys into one entry, so they are looked for on the text
    if has_duplicate_key(text) {
        return Err("duplicate key".to_string());
    }
    cpl_eval::json::value_from_json(p, &parsed, ty)
        .map_err(|e| format!("decode at {:?}: {}", e.pointer, e.message))
}

/// Whether some object of a valid JSON text repeats a key, by a walk with serde's visitor (as json_arg.rs does).
fn has_duplicate_key(text: &str) -> bool {
    use serde::de::{DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
    #[derive(Clone, Copy)]
    struct Walk;
    impl<'de> DeserializeSeed<'de> for Walk {
        type Value = ();
        fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
            d.deserialize_any(self)
        }
    }
    impl<'de> Visitor<'de> for Walk {
        type Value = ();
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a JSON value")
        }
        fn visit_bool<E>(self, _: bool) -> Result<(), E> {
            Ok(())
        }
        fn visit_i64<E>(self, _: i64) -> Result<(), E> {
            Ok(())
        }
        fn visit_u64<E>(self, _: u64) -> Result<(), E> {
            Ok(())
        }
        fn visit_f64<E>(self, _: f64) -> Result<(), E> {
            Ok(())
        }
        fn visit_str<E>(self, _: &str) -> Result<(), E> {
            Ok(())
        }
        fn visit_unit<E>(self) -> Result<(), E> {
            Ok(())
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
            while seq.next_element_seed(self)?.is_some() {}
            Ok(())
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
            let mut keys = std::collections::BTreeSet::new();
            while let Some(k) = map.next_key::<String>()? {
                if !keys.insert(k) {
                    return Err(serde::de::Error::custom("duplicate key"));
                }
                map.next_value_seed(self)?;
            }
            Ok(())
        }
    }
    let mut de = serde_json::Deserializer::from_str(text);
    Walk.deserialize(&mut de).is_err()
}

/// One edit of a JSON text: a character deleted, inserted, or replaced from the characters that matter to JSON and
/// to value JSON, or a fragment that a strict reader must refuse or must accept spliced in.
fn mutate(rng: &mut ChaCha20Rng, text: &str) -> String {
    const CHARS: [&str; 34] = [
        "{",
        "}",
        "[",
        "]",
        ",",
        ":",
        "\"",
        "\\",
        "0",
        "1",
        "9",
        "-",
        "+",
        ".",
        "e",
        "E",
        "t",
        "f",
        "n",
        "u",
        " ",
        "\t",
        "\n",
        "\r",
        "\u{a0}",
        "\u{feff}",
        "\u{0}",
        "\u{1}",
        "\u{7f}",
        "\u{e9}",
        "\u{1f600}",
        "\u{2028}",
        "x",
        "/",
    ];
    const FRAGMENTS: [&str; 22] = [
        "\\ud800",
        "\\udc00",
        "\\ud83d\\ude00",
        "\\u00e9",
        "\\x",
        "\\/",
        "\\u12",
        "-0",
        "01",
        "1.0",
        "1e3",
        "1E+2",
        "123456789012345678901234567890",
        "true",
        "null",
        "[]",
        "{}",
        "\"$type\"",
        "\"$variant\"",
        "\"$payload\"",
        ",,",
        "\u{feff}",
    ];
    let chars: Vec<char> = text.chars().collect();
    let at = rng.random_range(0..=chars.len());
    let mut out: String;
    match rng.random_range(0..4) {
        0 if !chars.is_empty() => {
            let at = at.min(chars.len() - 1);
            out = chars[..at].iter().collect();
            out.extend(&chars[at + 1..]);
        }
        1 => {
            out = chars[..at].iter().collect();
            out.push_str(CHARS[rng.random_range(0..CHARS.len())]);
            out.extend(&chars[at..]);
        }
        2 if !chars.is_empty() => {
            let at = at.min(chars.len() - 1);
            out = chars[..at].iter().collect();
            out.push_str(CHARS[rng.random_range(0..CHARS.len())]);
            out.extend(&chars[at + 1..]);
        }
        _ => {
            out = chars[..at].iter().collect();
            out.push_str(FRAGMENTS[rng.random_range(0..FRAGMENTS.len())]);
            out.extend(&chars[at..]);
        }
    }
    out
}

/// The compact text of a JSON value with one member of one of its objects written twice (with its own value, or with
/// another member's), or `None` when the value holds no object.
fn with_duplicate(rng: &mut ChaCha20Rng, v: &Json) -> Option<String> {
    fn objects(v: &Json, out: &mut usize) {
        match v {
            Json::Object(m) => {
                *out += 1;
                m.values().for_each(|x| objects(x, out));
            }
            Json::Array(items) => items.iter().for_each(|x| objects(x, out)),
            _ => {}
        }
    }
    fn write(v: &Json, target: usize, seen: &mut usize, pick: usize, out: &mut String) {
        match v {
            Json::Object(m) => {
                let me = *seen;
                *seen += 1;
                out.push('{');
                let entries: Vec<(&String, &Json)> = m.iter().collect();
                for (i, (k, x)) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&Json::String(k.to_string()).to_string());
                    out.push(':');
                    write(x, target, seen, pick, out);
                }
                if me == target && !entries.is_empty() {
                    // the repeated key, with the value of the member `pick` (its own or another's)
                    let (k, _) = entries[pick % entries.len()];
                    let (_, x) = entries[(pick / 2) % entries.len()];
                    out.push(',');
                    out.push_str(&Json::String(k.to_string()).to_string());
                    out.push(':');
                    out.push_str(&x.to_string());
                }
                out.push('}');
            }
            Json::Array(items) => {
                out.push('[');
                for (i, x) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write(x, target, seen, pick, out);
                }
                out.push(']');
            }
            other => out.push_str(&other.to_string()),
        }
    }
    let mut count = 0;
    objects(v, &mut count);
    if count == 0 {
        return None;
    }
    let target = rng.random_range(0..count);
    let pick = rng.random_range(0..64);
    let mut out = String::new();
    write(v, target, &mut 0, pick, &mut out);
    Some(out)
}

/// The JSON reader on random and damaged texts (toolchain 12.6 D7): the C reader of the driver's `from_json` and the
/// evaluator's strict reader accept the same texts, decode them to equal values, and refuse the same texts.
fn reader(
    gate: &mut Gate,
    session: &mut NativeSession<'_>,
    p: &cpl_hir::Program,
    rng: &mut ChaCha20Rng,
    n: usize,
) {
    use cpl_hir::Ty;
    let option = |t: Ty| Ty::Adt(p.std_defs.option, vec![t]);
    let constraint = p
        .lookup(&SymbolPath::parse("std.prelude.ConstraintError").unwrap())
        .expect("ConstraintError");
    // a struct inside an enum: the parse_int row's return type
    let parsed = Ty::Adt(
        p.std_defs.result,
        vec![Ty::Int, Ty::Adt(constraint, vec![])],
    );
    let types = [
        Ty::Int,
        Ty::Text,
        Ty::Bool,
        Ty::list(Ty::Int),
        Ty::list(Ty::Text),
        option(Ty::Int),
        parsed,
    ];
    let mut sweep = cpl_eval::testing::sweep::Sweep::new(*blake3::hash(b"D7 reader").as_bytes());
    let (mut accepted, mut refused, mut unknown) = (0, 0, 0);
    for i in 0..n {
        let ty = &types[i % types.len()];
        let type_text = cpl_sema::interop::type_text(p, ty);
        let v = match sweep.draw(p, ty, (i % 64) as u32) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let valid = cpl_eval::value_to_json(p, &v, ty).to_string();
        // damaged once or twice, a member of an object repeated, or valid with random whitespace
        let text = match rng.random_range(0..5) {
            0 | 1 => mutate(rng, &valid),
            4 => with_duplicate(rng, &cpl_eval::value_to_json(p, &v, ty))
                .unwrap_or_else(|| mutate(rng, &valid)),
            2 => {
                let once = mutate(rng, &valid);
                mutate(rng, &once)
            }
            _ => format!(
                "{}{valid}{}",
                [" ", "\n", "\t\r ", ""][rng.random_range(0..4)],
                [" ", "\n", ""][rng.random_range(0..3)]
            ),
        };
        let eval = strict_read(p, &text, ty);
        let native = match session.from_json(&type_text, &text) {
            Ok(r) => r,
            Err(e) => {
                gate.divergences.push(format!(
                    "D7 from_json {text:?} at {type_text}: {:?}",
                    e.into_diagnostics()
                ));
                continue;
            }
        };
        gate.compared += 1;
        match (&eval, &native) {
            (Ok(a), Ok((b, _))) if a.structurally_equal(b) => accepted += 1,
            (Err(_), Err(d)) if d.code.as_str() == "interop_json_invalid" => refused += 1,
            (_, Err(d)) if d.code.as_str() == "interop_type_unknown" => unknown += 1,
            _ => gate.divergences.push(format!(
                "D7 reader at {type_text} on {text:?}: evaluator {:?}, C {:?}",
                eval.as_ref()
                    .map(|v| cpl_eval::json::value_text_untyped(p, v)),
                native
                    .as_ref()
                    .map(|(v, _)| cpl_eval::json::value_text_untyped(p, v))
                    .map_err(|d| d.message.clone())
            )),
        }
    }
    assert_eq!(
        unknown, 0,
        "every type of the reader inputs is in the driver's table"
    );
    println!("D7 reader: {accepted} accepted alike, {refused} refused alike");
}
