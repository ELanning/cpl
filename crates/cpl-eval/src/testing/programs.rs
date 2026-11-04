//! Random programs for differential testing (doc-hidden): [`generate`] writes a module of helpers
//! and a target function over `Int`, `Bool`, `Option<Int>`, an enum, a struct with an invariant,
//! and a refinement, with contracts, loops with invariants and measures, recursion, matches,
//! `check<R>`, struct updates, and `concurrent` blocks; [`inputs`] draws arguments for its
//! functions in value JSON. A mutation index rewrites one node of the target (some rewrites
//! preserve its behavior, some do not), drawing nothing from the program's own random stream, so
//! the mutant differs from the program in that node alone.
//!
//! The verifier's soundness fuzzer (`cargo run --release -p cpl-prove --example prove_fuzz`) and
//! the native engine's (`cargo run --release -p cpl-cabi --example native_fuzz`) use them.

use cpl_hir::{DefId, Program};
use serde_json::{json, Value as Json};
use std::fmt::Write as _;

/// splitmix64.
pub struct Rng(u64);
impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }
    pub fn next_u64(&mut self) -> u64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }
    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + self.below((hi - lo + 1) as u64) as i64
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum T {
    Int,
    Bool,
    Point,
}

/// The generator. Each mutation names a function (a region: the helpers `h0`, `h1`, ..., then the
/// target), the pre-order index of a node in it, and the rewrite's own random stream, so that
/// the original and the mutant draw the same numbers.
struct G {
    rng: Rng,
    scope: Vec<(String, T, bool)>,
    helpers: Vec<(String, Vec<T>, T, bool)>,
    fresh: usize,
    option: bool,
    shape: bool,
    point: bool,
    /// The function being generated (`None` outside the mutable ones).
    region: Option<usize>,
    /// The nodes generated so far in each function.
    counts: Vec<usize>,
    /// The target is recursive on `x` (`requires x >= 0`, `decreases x`) with these parameters.
    recursive: Option<Vec<String>>,
    point_helpers: Vec<String>,
    /// Names of type `Nat`: read as `(n + 0)`, an `Int` (the first branch of an `if` sets its
    /// type, D-096).
    nats: Vec<String>,
    in_clause: bool,
    mutations: Vec<(usize, usize, Rng)>,
    mutated: Vec<String>,
}

impl G {
    fn name(&mut self, prefix: &str) -> String {
        self.fresh += 1;
        format!("{prefix}{}", self.fresh)
    }

    fn lit(&mut self) -> String {
        match self.rng.below(10) {
            0 => format!("{}", self.rng.range(-100, 100)),
            1 | 2 => format!("{}", self.rng.range(-3, -1)),
            _ => format!("{}", self.rng.range(0, 9)),
        }
    }

    fn vars(&self, t: T) -> Vec<String> {
        self.scope
            .iter()
            .filter(|(_, ty, _)| *ty == t)
            .map(|(n, _, _)| n.clone())
            .collect()
    }

    /// Apply the mutation when this is the chosen node. `parts` are the node's rendered
    /// children; `text` is the node as generated.
    fn node(&mut self, t: T, text: String, parts: &[&str], kind: &str) -> String {
        let Some(region) = self.region else {
            return text;
        };
        if self.counts.len() <= region {
            self.counts.resize(region + 1, 0);
        }
        self.counts[region] += 1;
        let at = self.counts[region];
        let Some((_, _, rng)) = self
            .mutations
            .iter_mut()
            .find(|(r, node, _)| *r == region && *node == at)
        else {
            return text;
        };
        let choice = rng.below(100);
        let out = match (t, kind, parts) {
            // preserving rewrites
            (_, "+", [a, b]) if choice < 30 => format!("({b} + {a})"),
            (_, "+", [a, b]) if choice < 45 => format!("({a} - (-{b}))"),
            (_, "-", [a, b]) if choice < 30 => format!("({a} + (-{b}))"),
            (_, "-", [a, b]) if choice < 45 => format!("(-({b} - {a}))"),
            (_, "*", [a, b]) if choice < 40 => format!("({b} * {a})"),
            (_, "if", [c, a, b]) if choice < 40 => format!("(if not {c} {{ {b} }} else {{ {a} }})"),
            (_, "min", [a, b]) if choice < 40 => {
                format!("(if {a} <= {b} {{ {a} }} else {{ {b} }})")
            }
            (_, "max", [a, b]) if choice < 40 => {
                format!("(if {a} >= {b} {{ {a} }} else {{ {b} }})")
            }
            (_, "abs", [a]) if choice < 40 => format!("({a}).max(-{a})"),
            (_, "<", [a, b]) if choice < 30 => format!("({b} > {a})"),
            (_, "<", [a, b]) if choice < 50 => format!("not ({a} >= {b})"),
            (_, "<=", [a, b]) if choice < 30 => format!("({a} < {b} + 1)"),
            (_, "and", [a, b]) if choice < 40 => format!("({b} and {a})"),
            (_, "or", [a, b]) if choice < 40 => format!("not (not {a} and not {b})"),
            (_, "==", [a, b]) if choice < 40 => format!("({b} == {a})"),
            (T::Int, "leaf", [a]) if choice < 30 => format!("({a} + 0)"),
            (T::Int, "leaf", [a]) if choice < 40 => format!("(2 * {a} - {a})"),
            (T::Bool, "leaf", [a]) if choice < 30 => format!("(not not {a})"),
            // changing rewrites
            (_, "+", [a, b]) => format!("({a} - {b})"),
            (_, "-", [a, b]) => format!("({b} - {a})"),
            (_, "*", [a, b]) => format!("({a} * {b} + 1)"),
            (_, "if", [c, a, b]) => format!("(if {c} {{ {b} }} else {{ {a} }})"),
            (_, "min", [a, b]) => format!("({a}).max({b})"),
            (_, "max", [a, b]) => format!("({a}).min({b})"),
            (_, "abs", [a]) => format!("(-{a})"),
            (_, "<", [a, b]) => format!("({a} <= {b})"),
            (_, "<=", [a, b]) => format!("({a} < {b})"),
            (_, "and", [a, b]) => format!("({a} or {b})"),
            (_, "or", [a, b]) => format!("({a} and {b})"),
            (_, "==", [a, b]) => format!("({a} != {b})"),
            (T::Int, _, _) if choice < 70 => format!("({text} + 1)"),
            (T::Int, _, _) => format!("({text} - {text})"),
            (T::Bool, _, _) => format!("(not {text})"),
            (T::Point, _, _) => text.clone(),
        };
        self.mutated.push(format!("{kind}: {text} => {out}"));
        out
    }

    fn int_leaf(&mut self) -> String {
        let mut options = self.vars(T::Int);
        if self.point {
            options.push("p.lo".into());
            options.push("p.hi".into());
        }
        for q in self.vars(T::Point) {
            options.push(format!("{q}.lo"));
            options.push(format!("{q}.hi"));
        }
        let text = if options.is_empty() || self.rng.chance(30) {
            self.lit()
        } else {
            options[self.rng.below(options.len() as u64) as usize].clone()
        };
        let text = if self.nats.contains(&text) {
            format!("({text} + 0)")
        } else {
            text
        };
        self.node(T::Int, text.clone(), &[&text], "leaf")
    }

    fn int(&mut self, d: u32) -> String {
        if d == 0 || self.rng.chance(20) {
            return self.int_leaf();
        }
        match self.rng.below(26) {
            0..=2 => {
                let (a, b) = (self.int(d - 1), self.int(d - 1));
                self.node(T::Int, format!("({a} + {b})"), &[&a, &b], "+")
            }
            3 | 4 => {
                let (a, b) = (self.int(d - 1), self.int(d - 1));
                self.node(T::Int, format!("({a} - {b})"), &[&a, &b], "-")
            }
            5 => {
                let a = self.int(d - 1);
                self.node(T::Int, format!("(-{a})"), &[&a], "neg")
            }
            6 | 7 => {
                let a = self.int(d - 1);
                let c = format!("{}", self.rng.range(-3, 5));
                self.node(T::Int, format!("({a} * {c})"), &[&a, &c], "*")
            }
            8 => {
                let (a, b) = (self.int(d - 1), self.int(d - 1));
                self.node(T::Int, format!("({a} * {b})"), &[&a, &b], "*")
            }
            9 => {
                let a = self.int(d - 1);
                let c = if self.rng.chance(85) {
                    format!("{}", [1, 2, 3, 4, 5, -2, -3, 7][self.rng.below(8) as usize])
                } else {
                    self.int(d - 1)
                };
                let op = if self.rng.chance(50) { "/" } else { "%" };
                self.node(T::Int, format!("({a} {op} {c})"), &[&a, &c], op)
            }
            10 => {
                let a = self.int(d - 1);
                self.node(T::Int, format!("({a}).abs()"), &[&a], "abs")
            }
            11 => {
                let (a, b) = (self.int(d - 1), self.int(d - 1));
                self.node(T::Int, format!("({a}).min({b})"), &[&a, &b], "min")
            }
            12 => {
                let (a, b) = (self.int(d - 1), self.int(d - 1));
                self.node(T::Int, format!("({a}).max({b})"), &[&a, &b], "max")
            }
            13..=15 => {
                let c = self.boolean(d - 1);
                let (a, b) = (self.int(d - 1), self.int(d - 1));
                self.node(
                    T::Int,
                    format!("(if {c} {{ {a} }} else {{ {b} }})"),
                    &[&c, &a, &b],
                    "if",
                )
            }
            16 | 17 => {
                let hs: Vec<_> = self
                    .helpers
                    .iter()
                    .filter(|h| h.2 == T::Int)
                    .cloned()
                    .collect();
                if hs.is_empty() {
                    return self.int_leaf();
                }
                let h = &hs[self.rng.below(hs.len() as u64) as usize];
                let args: Vec<String> =
                    h.1.iter()
                        .map(|t| match t {
                            T::Bool => self.boolean(d - 1),
                            _ => self.int(d - 1),
                        })
                        .collect();
                let text = format!("{}({})", h.0, args.join(", "));
                self.node(T::Int, text, &[], "call")
            }
            18 if self.option => {
                let v = self.name("m");
                let none = self.int(d - 1);
                self.scope.push((v.clone(), T::Int, false));
                let some = self.int(d - 1);
                self.scope.pop();
                let text = format!(
                    "(match o {{\n        Some({v}) => {some}\n        None => {none}\n    }})"
                );
                self.node(T::Int, text, &[], "match")
            }
            19 if self.shape => {
                let (v, w) = (self.name("m"), self.name("m"));
                let dot = self.int(d - 1);
                self.scope.push((v.clone(), T::Int, false));
                let line = self.int(d - 1);
                self.scope.push((w.clone(), T::Int, false));
                let boxed = self.int(d - 1);
                self.scope.pop();
                self.scope.pop();
                let text = if self.rng.chance(30) {
                    format!("(match s {{\n        Line({v}) => {line}\n        _ => {dot}\n    }})")
                } else {
                    format!("(match s {{\n        Dot => {dot}\n        Line({v}) => {line}\n        Box({v}, {w}) => {boxed}\n    }})")
                };
                self.node(T::Int, text, &[], "match")
            }
            22 => {
                let e = self.int(d - 1);
                let v = self.name("m");
                let err = self.int(d - 1);
                self.nats.push(v.clone());
                self.scope.push((v.clone(), T::Int, false));
                let ok = self.int(d - 1);
                self.scope.pop();
                let text = format!(
                    "(match check<Nat>({e}) {{\n        Ok({v}) => {ok}\n        Err(_) => {err}\n    }})"
                );
                self.node(T::Int, text, &[], "match")
            }
            23 => {
                let mut points = self.vars(T::Point);
                if self.point {
                    points.push("p".into());
                }
                if points.is_empty() || self.point_helpers.is_empty() {
                    return self.int_leaf();
                }
                let w = self.point_helpers
                    [self.rng.below(self.point_helpers.len() as u64) as usize]
                    .clone();
                let q = points[self.rng.below(points.len() as u64) as usize].clone();
                let text = format!("{w}({q})");
                self.node(T::Int, text, &[], "call")
            }
            24 | 25 if self.recursive.is_some() && !self.in_clause => {
                let params = self.recursive.clone().unwrap_or_default();
                let mut args = vec!["x - 1".to_string()];
                for param in params.iter().skip(1) {
                    args.push(match param.as_str() {
                        "y" | "z" if self.rng.chance(60) => self.int(d - 1),
                        "flag" if self.rng.chance(50) => self.boolean(d - 1),
                        other => other.to_string(),
                    });
                }
                let other = self.int(d - 1);
                let text = format!(
                    "(if x > 0 {{ target({}) }} else {{ {other} }})",
                    args.join(", ")
                );
                self.node(T::Int, text, &[], "call")
            }
            _ => self.int_leaf(),
        }
    }

    fn boolean(&mut self, d: u32) -> String {
        if d == 0 || self.rng.chance(15) {
            let bs = self.vars(T::Bool);
            let text = if !bs.is_empty() && self.rng.chance(50) {
                bs[self.rng.below(bs.len() as u64) as usize].clone()
            } else if self.rng.chance(15) {
                if self.rng.chance(50) { "true" } else { "false" }.to_string()
            } else {
                let (a, b) = (self.int_leaf(), self.int_leaf());
                format!("({a} < {b})")
            };
            return self.node(T::Bool, text.clone(), &[&text], "leaf");
        }
        match self.rng.below(16) {
            0..=5 => {
                let (a, b) = (self.int(d - 1), self.int(d - 1));
                let op = ["<", "<=", ">", ">=", "==", "!="][self.rng.below(6) as usize];
                let kind = match op {
                    "<" | "<=" | "==" => op,
                    _ => "cmp",
                };
                self.node(T::Bool, format!("({a} {op} {b})"), &[&a, &b], kind)
            }
            6 | 7 => {
                let (a, b) = (self.boolean(d - 1), self.boolean(d - 1));
                self.node(T::Bool, format!("({a} and {b})"), &[&a, &b], "and")
            }
            8 | 9 => {
                let (a, b) = (self.boolean(d - 1), self.boolean(d - 1));
                self.node(T::Bool, format!("({a} or {b})"), &[&a, &b], "or")
            }
            10 => {
                let a = self.boolean(d - 1);
                self.node(T::Bool, format!("(not {a})"), &[&a], "not")
            }
            11 if self.option => {
                let text = if self.rng.chance(50) {
                    "(o == None)".to_string()
                } else {
                    let a = self.int(d - 1);
                    format!("(o == Some({a}))")
                };
                self.node(T::Bool, text, &[], "opt")
            }
            12 => {
                let hs: Vec<_> = self
                    .helpers
                    .iter()
                    .filter(|h| h.2 == T::Bool)
                    .cloned()
                    .collect();
                if hs.is_empty() {
                    return self.boolean(d - 1);
                }
                let h = &hs[self.rng.below(hs.len() as u64) as usize];
                let args: Vec<String> = h.1.iter().map(|_| self.int(d - 1)).collect();
                let text = format!("{}({})", h.0, args.join(", "));
                self.node(T::Bool, text, &[], "call")
            }
            14 if !self.vars(T::Point).is_empty() => {
                let mut points = self.vars(T::Point);
                if self.point {
                    points.push("p".into());
                }
                let a = points[self.rng.below(points.len() as u64) as usize].clone();
                let b = points[self.rng.below(points.len() as u64) as usize].clone();
                self.node(T::Bool, format!("({a} == {b})"), &[], "opt")
            }
            13 => {
                let (a, b, c) = (
                    self.boolean(d - 1),
                    self.boolean(d - 1),
                    self.boolean(d - 1),
                );
                self.node(
                    T::Bool,
                    format!("(if {a} {{ {b} }} else {{ {c} }})"),
                    &[&a, &b, &c],
                    "if",
                )
            }
            _ => {
                let (a, b) = (self.int(d - 1), self.int(d - 1));
                self.node(T::Bool, format!("({a} < {b})"), &[&a, &b], "<")
            }
        }
    }

    /// Statements at `indent`; `depth` bounds nesting. Returns the lines.
    fn stmts(&mut self, out: &mut String, indent: usize, n: usize, depth: u32) {
        let pad = " ".repeat(indent);
        for _ in 0..n {
            match self.rng.below(20) {
                0..=3 => {
                    let v = self.name("l");
                    let e = self.int(3);
                    let _ = writeln!(out, "{pad}let {v} = {e}");
                    self.scope.push((v, T::Int, false));
                }
                4 => {
                    let v = self.name("b");
                    let e = self.boolean(2);
                    let _ = writeln!(out, "{pad}let {v} = {e}");
                    self.scope.push((v, T::Bool, false));
                }
                5..=7 => {
                    let v = self.name("v");
                    let e = self.int(2);
                    let _ = writeln!(out, "{pad}var {v} = {e}");
                    self.scope.push((v, T::Int, true));
                }
                8..=10 => {
                    let vars: Vec<_> = self
                        .scope
                        .iter()
                        .filter(|s| s.2)
                        .map(|s| s.0.clone())
                        .collect();
                    if let Some(v) = vars.get(self.rng.below(vars.len().max(1) as u64) as usize) {
                        let e = self.int(3);
                        let _ = writeln!(out, "{pad}{v} = {e}");
                    }
                }
                11 => {
                    let c = self.boolean(2);
                    let _ = writeln!(out, "{pad}assert {c}");
                }
                12 | 13 if depth > 0 => {
                    let c = self.boolean(2);
                    let mark = self.scope.len();
                    let _ = writeln!(out, "{pad}if {c} {{");
                    let k = self.rng.below(3) as usize;
                    self.stmts(out, indent + 4, k, depth - 1);
                    if self.rng.chance(15) {
                        let e = self.int(2);
                        let _ = writeln!(out, "{pad}    return {e}");
                    }
                    self.scope.truncate(mark);
                    if self.rng.chance(60) {
                        let _ = writeln!(out, "{pad}}} else {{");
                        let k = self.rng.below(3) as usize;
                        self.stmts(out, indent + 4, k, depth - 1);
                        self.scope.truncate(mark);
                    }
                    let _ = writeln!(out, "{pad}}}");
                }
                14 if self.point => {
                    let q = self.name("q");
                    let (lo, hi) = (self.int(2), self.int(2));
                    let (lo, hi) = if self.rng.chance(50) {
                        (lo.clone(), format!("({lo}).max({hi})"))
                    } else {
                        (lo, hi)
                    };
                    let _ = writeln!(out, "{pad}let {q} = P {{ lo: {lo}, hi: {hi} }}");
                    self.scope.push((q, T::Point, false));
                }
                15 | 16 if depth > 0 => self.while_loop(out, indent, depth),
                17 => {
                    let mut points = self.vars(T::Point);
                    if self.point {
                        points.push("p".into());
                    }
                    if points.is_empty() {
                        continue;
                    }
                    let from = points[self.rng.below(points.len() as u64) as usize].clone();
                    let q = self.name("q");
                    let e = self.int(2);
                    let field = if self.rng.chance(50) { "lo" } else { "hi" };
                    let _ = writeln!(out, "{pad}let {q} = P {{ ..{from}, {field}: {e} }}");
                    self.scope.push((q, T::Point, false));
                }
                18 => {
                    let (u, v) = (self.name("c"), self.name("c"));
                    let (e1, e2) = (self.int(2), self.int(2));
                    let _ = writeln!(out, "{pad}concurrent {{\n{pad}    let {u} = {e1}\n{pad}    let {v} = {e2}\n{pad}}}");
                    self.scope.push((u, T::Int, false));
                    self.scope.push((v, T::Int, false));
                }
                _ => {}
            }
        }
    }

    fn while_loop(&mut self, out: &mut String, indent: usize, depth: u32) {
        let pad = " ".repeat(indent);
        let i = self.name("i");
        let init = self.rng.range(-2, 3);
        let bound = {
            let ints: Vec<_> = self
                .scope
                .iter()
                .filter(|s| s.1 == T::Int && !s.2)
                .map(|s| s.0.clone())
                .collect();
            if ints.is_empty() || self.rng.chance(20) {
                format!("{}", self.rng.range(0, 8))
            } else {
                ints[self.rng.below(ints.len() as u64) as usize].clone()
            }
        };
        let step = match self.rng.below(10) {
            0 => 2,
            1 => 3,
            _ => 1,
        };
        let _ = writeln!(out, "{pad}var {i} = {init}");
        // an accumulator with a matching invariant
        let acc = self.name("a");
        let k = self.rng.range(-2, 4);
        let a0 = self.rng.range(-3, 3);
        let _ = writeln!(out, "{pad}var {acc} = {a0}");
        let _ = writeln!(out, "{pad}while {i} < {bound}");
        let mut invariants = Vec::new();
        if self.rng.chance(80) {
            invariants.push(format!("{i} >= {init}"));
        }
        if self.rng.chance(60) {
            invariants.push(if step == 1 && self.rng.chance(85) {
                format!("{i} <= ({bound}).max({init})")
            } else {
                format!("{i} <= {bound}")
            });
        }
        if self.rng.chance(60) {
            invariants.push(if step == 1 && self.rng.chance(80) {
                format!("{acc} == {a0} + {k} * ({i} - {init})")
            } else {
                format!("{acc} >= {a0}")
            });
        }
        if self.rng.chance(20) {
            self.scope.push((i.clone(), T::Int, false));
            self.scope.push((acc.clone(), T::Int, false));
            invariants.push(self.boolean(1));
            self.scope.truncate(self.scope.len() - 2);
        }
        for inv in &invariants {
            let _ = writeln!(out, "{pad}    invariant {inv}");
        }
        if self.rng.chance(92) {
            let _ = writeln!(out, "{pad}    decreases {bound} - {i}");
        }
        let _ = writeln!(out, "{pad}{{");
        let mark = self.scope.len();
        // the body may read the counter and accumulator but not assign them
        self.scope.push((i.clone(), T::Int, false));
        self.scope.push((acc.clone(), T::Int, false));
        let k2 = self.rng.below(2) as usize;
        self.stmts(out, indent + 4, k2, depth - 1);
        self.scope.truncate(mark);
        let _ = writeln!(out, "{pad}    {acc} = {acc} + {k}");
        let _ = writeln!(out, "{pad}    {i} = {i} + {step}");
        let _ = writeln!(out, "{pad}}}");
        self.scope.push((i, T::Int, false));
        self.scope.push((acc, T::Int, true));
    }

    fn program(&mut self) -> String {
        let mut out = String::from("module m\n\n");
        out.push_str("struct P {\n    lo: Int\n    hi: Int\n} where self.lo <= self.hi\n\n");
        out.push_str("enum Shape {\n    Dot\n    Line(Int)\n    Box(Int, Int)\n}\n\n");
        out.push_str("type Nat = Int where self >= 0\n\n");
        for k in 0..self.rng.below(3) {
            let name = format!("w{k}");
            self.scope = vec![];
            let lo_hi = ["p.lo", "p.hi"];
            let a = lo_hi[self.rng.below(2) as usize];
            let b = lo_hi[self.rng.below(2) as usize];
            let c = self.rng.range(-2, 3);
            let body = match self.rng.below(4) {
                0 => format!("{a} - {b}"),
                1 => format!("p.hi - p.lo + {c}"),
                2 => format!("({a}).max({b}) * {c}"),
                _ => format!("{a} + {c}"),
            };
            let _ = writeln!(out, "fn {name}(p: P) -> Int");
            if self.rng.chance(60) {
                let e = [
                    "result >= 0",
                    "result >= p.lo - p.hi",
                    "result <= p.hi",
                    "result != 3",
                ][self.rng.below(4) as usize];
                let _ = writeln!(out, "    ensures {e}");
            }
            let _ = writeln!(out, "{{\n    {body}\n}}\n");
            self.point_helpers.push(name);
        }
        let helpers = self.rng.below(4);
        for h in 0..helpers {
            self.region = Some(h as usize);
            let name = format!("h{h}");
            let arity = 1 + self.rng.below(2) as usize;
            let params: Vec<String> = ["a", "b"][..arity].iter().map(|s| s.to_string()).collect();
            let ret = if self.rng.chance(80) { T::Int } else { T::Bool };
            self.scope = params.iter().map(|n| (n.clone(), T::Int, false)).collect();
            let sig = params
                .iter()
                .map(|n| format!("{n}: Int"))
                .collect::<Vec<_>>()
                .join(", ");
            let rty = if ret == T::Int { "Int" } else { "Bool" };
            let recursive = ret == T::Int && arity == 1 && self.rng.chance(20);
            let _ = writeln!(out, "fn {name}({sig}) -> {rty}");
            if recursive {
                let _ = writeln!(out, "    requires a >= 0");
            } else if self.rng.chance(35) {
                let c = self.boolean(1);
                let _ = writeln!(out, "    requires {c}");
            }
            if ret == T::Int && self.rng.chance(60) {
                self.scope.push(("result".into(), T::Int, false));
                let c = self.boolean(1);
                self.scope.pop();
                let _ = writeln!(out, "    ensures {c}");
            }
            if recursive {
                let _ = writeln!(out, "    decreases a");
                let base = self.int(1);
                let step = self.int(1);
                let _ = writeln!(
                    out,
                    "{{\n    if a == 0 {{ {base} }} else {{ {step} + {name}(a - 1) }}\n}}\n"
                );
            } else {
                let body = if ret == T::Int {
                    self.int(2)
                } else {
                    self.boolean(2)
                };
                let _ = writeln!(out, "{{\n    {body}\n}}\n");
            }
            self.helpers
                .push((name, vec![T::Int; arity], ret, recursive));
        }
        self.region = None;
        // the target
        self.option = self.rng.chance(40);
        self.shape = self.rng.chance(30);
        self.point = self.rng.chance(30);
        let mut params = vec![("x", "Int", T::Int), ("y", "Int", T::Int)];
        if self.rng.chance(50) {
            params.push(("z", "Int", T::Int));
        }
        if self.rng.chance(40) {
            params.push(("flag", "Bool", T::Bool));
        }
        if self.option {
            params.push(("o", "Option<Int>", T::Int));
        }
        if self.shape {
            params.push(("s", "Shape", T::Int));
        }
        if self.point {
            params.push(("p", "P", T::Int));
        }
        if self.rng.chance(25) {
            params.push(("n", "Nat", T::Int));
        }
        let recursive = self.rng.chance(20);
        if recursive {
            self.recursive = Some(params.iter().map(|(n, _, _)| n.to_string()).collect());
        }
        self.scope = params
            .iter()
            .filter(|(_, ty, _)| *ty == "Int" || *ty == "Bool" || *ty == "Nat")
            .map(|(n, _, t)| (n.to_string(), *t, false))
            .collect();
        self.region = Some(self.helpers.len());
        let sig = params
            .iter()
            .map(|(n, ty, _)| format!("{n}: {ty}"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "fn target({sig}) -> Int");
        if recursive {
            let _ = writeln!(out, "    requires x >= 0");
            let _ = writeln!(out, "    decreases x");
        }
        self.in_clause = true;
        for _ in 0..self.rng.below(3) {
            let c = self.boolean(1);
            let _ = writeln!(out, "    requires {c}");
        }
        let ensures = 1 + self.rng.below(2);
        let mut ens = Vec::new();
        self.scope.push(("result".into(), T::Int, false));
        for _ in 0..ensures {
            ens.push(self.boolean(2));
        }
        self.scope.pop();
        for c in ens {
            let _ = writeln!(out, "    ensures {c}");
        }
        self.in_clause = false;
        let _ = writeln!(out, "{{");
        let n = self.rng.below(6) as usize;
        self.stmts(&mut out, 4, n, 2);
        let tail = self.int(3);
        let _ = writeln!(out, "    {tail}\n}}");
        self.region = None;
        out
    }
}

/// A rewrite of one node: the function (an index into [`Generated::nodes`]), the node (1-based,
/// pre-order), and the seed of the rewrite.
pub type Mutation = (usize, usize, u64);

/// A generated program.
pub struct Generated {
    pub source: String,
    /// The nodes a mutation can rewrite in each function: the helpers `h0`, `h1`, ..., then
    /// `target` (always last).
    pub nodes: Vec<usize>,
    /// The rewrites applied, as `kind: before => after`.
    pub mutations: Vec<String>,
}

/// The program of `seed`, with `mutations` applied.
pub fn generate(seed: u64, mutations: &[Mutation]) -> Generated {
    let mut g = G {
        rng: Rng::new(seed),
        scope: Vec::new(),
        helpers: Vec::new(),
        fresh: 0,
        option: false,
        shape: false,
        point: false,
        region: None,
        counts: Vec::new(),
        recursive: None,
        point_helpers: Vec::new(),
        nats: vec!["n".into()],
        in_clause: false,
        mutations: mutations
            .iter()
            .map(|&(region, node, seed)| (region, node, Rng::new(seed)))
            .collect(),
        mutated: Vec::new(),
    };
    let source = g.program();
    let functions = g.helpers.len() + 1;
    let mut nodes = g.counts;
    nodes.resize(functions, 0);
    Generated {
        source,
        nodes,
        mutations: g.mutated,
    }
}

/// Random inputs for `f`'s parameters.
pub fn inputs(p: &Program, f: DefId, rng: &mut Rng, wide: bool) -> Vec<Json> {
    let fd = p.def(f).kind.as_fn().expect("a function");
    let int = |rng: &mut Rng| -> Json {
        let v = if wide && rng.chance(20) {
            rng.range(-1000, 1000)
        } else if rng.chance(70) {
            rng.range(-6, 10)
        } else {
            rng.range(-30, 30)
        };
        json!(v)
    };
    // A recursive function's measure bounds its depth, and its values can grow
    // doubly exponentially with it.
    let recursive = fd.decreases.is_some();
    fd.params
        .iter()
        .map(|param| {
            let ty = p.display_ty_in(&param.ty, &fd.generics);
            match ty.as_str() {
                "Int" if recursive && param.name == "x" => json!(rng.range(-1, 10)),
                "Int" => int(rng),
                "Bool" => json!(rng.chance(50)),
                "Nat" | "m.Nat" => {
                    if rng.chance(85) {
                        json!(rng.range(0, 12))
                    } else {
                        int(rng)
                    }
                }
                "Option<Int>" => {
                    if rng.chance(30) {
                        json!({"$type": "Option", "$variant": "None"})
                    } else {
                        json!({"$type": "Option", "$variant": "Some", "$payload": [int(rng)]})
                    }
                }
                "Shape" | "m.Shape" => match rng.below(3) {
                    0 => json!({"$type": "m.Shape", "$variant": "Dot"}),
                    1 => json!({"$type": "m.Shape", "$variant": "Line", "$payload": [int(rng)]}),
                    _ => json!({"$type": "m.Shape", "$variant": "Box", "$payload": [int(rng), int(rng)]}),
                },
                "P" | "m.P" => {
                    let lo = int(rng);
                    let hi = if rng.chance(80) {
                        json!(lo.as_i64().unwrap() + rng.range(0, 8))
                    } else {
                        int(rng)
                    };
                    json!({"$type": "m.P", "lo": lo, "hi": hi})
                }
                other => panic!("no generator for {other}"),
            }
        })
        .collect()
}
