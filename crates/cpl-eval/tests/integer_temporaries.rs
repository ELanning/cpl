mod common;

use cpl_eval::{EvalConfig, Interpreter, RunEngine, Value};
use num_bigint::BigInt;

#[test]
fn integer_temporaries_preserve_operands_and_left_to_right_mutations() {
    let p = common::check_source(
        r#"module integers

fn calculate(a: Int, b: Int) -> List<Int> {
    let sum = (a + b) + a
    let difference = (a - b) - a
    let product = (a * b) * a
    let negative = -(a + b)
    [a, b, sum, difference, product, negative]
}

fn mutate(seed: Int) -> List<Int> {
    var x = seed
    let before = x
    let sum = x + if true {
        x = x + 1
        x
    } else { 0 }
    let difference = (if true {
        x = x + 1
        x
    } else { 0 }) - x
    [before, x, sum, difference]
}
"#,
    );
    let mut eval = Interpreter::new(&p, EvalConfig::default());
    let big: BigInt = (BigInt::from(1) << 256) + 17;
    for a in [BigInt::from(0), BigInt::from(42), big.clone(), -big] {
        for b in [BigInt::from(0), BigInt::from(-23), &a + 1] {
            let args = vec![Value::int(a.clone()), Value::int(b.clone())];
            let expected = Value::list(
                [
                    a.clone(),
                    b.clone(),
                    &a + &b + &a,
                    -&b,
                    &a * &b * &a,
                    -(&a + &b),
                ]
                .into_iter()
                .map(Value::int)
                .collect(),
            );
            assert_eq!(
                eval.call_checked(common::def(&p, "integers.calculate"), args, vec![])
                    .unwrap(),
                expected
            );
        }
        assert_eq!(
            eval.call_checked(
                common::def(&p, "integers.mutate"),
                vec![Value::int(a.clone())],
                vec![],
            )
            .unwrap(),
            Value::list(
                [a.clone(), &a + 2, &a * 2 + 1, BigInt::from(0)]
                    .into_iter()
                    .map(Value::int)
                    .collect()
            )
        );
    }
}

#[test]
fn borrowed_and_owned_division_keep_fault_operands_and_locations() {
    let source = r#"module integers
fn borrowed(a: Int, b: Int) -> Int { a / b }
fn owned(a: Int, b: Int) -> Int { (a + 1) / (b - 1) }
fn first(a: Int) -> Int {
    (a / 0) + if true {
        assert false
        0
    } else { 0 }
}
"#;
    let p = common::check_source(source);
    for (name, args, expression, dividend) in [
        ("borrowed", vec![17, 0], "a / b", 17),
        ("owned", vec![17, 1], "(a + 1) / (b - 1)", 18),
        ("first", vec![17], "a / 0", 17),
    ] {
        let args = args
            .into_iter()
            .map(serde_json::Value::from)
            .collect::<Vec<_>>();
        let result = common::run_json(&p, &format!("integers.{name}"), &args);
        let fault = common::fault(&p, &result);
        assert_eq!(fault["code"], "division_by_zero");
        assert_eq!(fault["input"], serde_json::json!({"a": dividend, "b": 0}));
        assert_eq!(
            fault["span"],
            common::location_of(source, "src/integers.cpl", expression)
        );
    }
}
