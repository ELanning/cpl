//! The verifier's verdicts against execution on random programs (the oracle of
//! `examples/prove_fuzz.rs`, which runs many more cases): every fault is attributable to an
//! obligation that is not proved, every failed obligation's model faults when run, and
//! equivalence verdicts on mutants agree with running both versions.

#[allow(dead_code)]
#[path = "../examples/prove_fuzz.rs"]
mod fuzz;

#[test]
fn verdicts_agree_with_execution_on_random_programs() {
    // `CPL_PROVE_FUZZ_CASES` raises the number of programs.
    let cases: u64 = std::env::var("CPL_PROVE_FUZZ_CASES")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(60);
    let mut stats = fuzz::Stats::default();
    for seed in 1..=cases {
        fuzz::one(seed, &mut stats, false);
    }
    assert_eq!(stats.violations, 0, "the violations are printed above");
    assert!(
        stats.cases as u64 >= cases * 9 / 10,
        "only {} of {cases} programs checked",
        stats.cases
    );
}
