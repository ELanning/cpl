//! The input sweeps of the differential suites (`cpl_eval::testing::sweep`) on recursive types:
//! every draw is finite, at every size.

mod common;
use common::{check_source, def};
use cpl_eval::testing::sweep::Sweep;
use cpl_eval::{on_eval_stack, value_to_json};

const SOURCE: &str = "module m
struct Tree {
    children: List<Tree>
}
enum Expr {
    Lit(Int)
    Pair(Option<Expr>, Option<Expr>)
    Many(List<Expr>)
}
struct Chain {
    next: Option<Chain>
}
struct Pair {
    left: Option<Option<Int>>
    right: List<List<Int>>
}
fn f(t: Tree, e: Expr, c: Chain, p: Pair) -> Int {
    0
}
";

#[test]
fn recursive_types_are_drawn_finitely() {
    on_eval_stack(|| {
        let p = check_source(SOURCE);
        let f = p
            .def(def(&p, "m.f"))
            .kind
            .as_fn()
            .expect("a function")
            .clone();
        let mut sweep = Sweep::new([7; 32]);
        let mut largest = 0;
        for size in 0..=100u32 {
            for param in &f.params {
                let v = sweep.draw(&p, &param.ty, size).expect("drawable");
                let json = value_to_json(&p, &v, &param.ty).to_string();
                largest = largest.max(json.len());
            }
        }
        // A tree of trees at size 100 has at most 100 * 25 * 6 * 1 nodes; in practice far fewer.
        assert!(largest < 2_000_000, "{largest}");
    });
}
