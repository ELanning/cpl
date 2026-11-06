//! The CPL benchmark kernels (`bench/cpl/src/bench.cpl`) written as idiomatic Rust: `i64`, `Vec`,
//! `String`, and `num-bigint` for the factorial. Prints `name n result ms` for each kernel.
use num_bigint::BigUint;
use std::hint::black_box;
use std::time::Instant;

fn arith(n: i64) -> i64 {
    let mut acc = 0i64;
    for i in 0..n {
        acc = (acc + i * i) % 1000003;
    }
    acc
}

fn fib(n: i64) -> i64 {
    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
}

fn collatz(n: i64) -> i64 {
    let mut total = 0;
    for k in 1..=n {
        let mut x = k;
        while x != 1 {
            x = if x % 2 == 0 { x / 2 } else { 3 * x + 1 };
            total += 1;
        }
    }
    total
}

fn list_index(n: i64) -> i64 {
    let xs: Vec<i64> = (0..n).map(|i| i * 3 % 17).collect();
    xs.iter().sum()
}

fn list_walk(n: i64) -> i64 {
    let xs: Vec<i64> = (0..n).map(|i| i % 11).collect();
    let mut rest = &xs[..];
    let mut sum = 0;
    while let Some((first, tail)) = rest.split_first() {
        sum += first;
        rest = tail;
    }
    sum
}

fn merge(a: &[i64], b: &[i64]) -> Vec<i64> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        if j >= b.len() || (i < a.len() && a[i] <= b[j]) {
            out.push(a[i]);
            i += 1;
        } else {
            out.push(b[j]);
            j += 1;
        }
    }
    out
}

fn sort(xs: &[i64]) -> Vec<i64> {
    if xs.len() <= 1 {
        return xs.to_vec();
    }
    let half = xs.len() / 2;
    merge(&sort(&xs[..half]), &sort(&xs[half..]))
}

fn sorting(n: i64) -> i64 {
    let mut seed: i64 = 12345;
    let xs: Vec<i64> = (0..n)
        .map(|_| {
            seed = (seed * 1103515245 + 12345) % 2147483648;
            seed % 100000
        })
        .collect();
    sort(&xs).iter().fold(0, |s, &x| (s * 31 + x) % 1000000007)
}

#[derive(Clone, Copy)]
struct Point {
    x: i64,
    y: i64,
}

fn step(p: Point, k: i64) -> Point {
    Point { x: p.y % 1000 + k, y: p.x % 1000 + 2 * k }
}

fn structs(n: i64) -> i64 {
    let mut p = Point { x: 1, y: 2 };
    for i in 0..n {
        p = step(p, i % 7);
    }
    p.x + p.y
}

fn text(n: i64) -> i64 {
    let mut t = String::new();
    for i in 0..n {
        t.push('w');
        t.push_str(&(i % 100).to_string());
        t.push(' ');
    }
    let words: i64 = t.split(' ').map(|w| w.chars().count() as i64).sum();
    words + t.chars().count() as i64
}

fn bigint(n: i64) -> i64 {
    let mut f = BigUint::from(1u32);
    for i in 2..=n as u64 {
        f *= i;
    }
    f.to_string().len() as i64
}

struct Tree {
    left: Option<Box<Tree>>,
    right: Option<Box<Tree>>,
}

fn make(d: i64) -> Tree {
    if d == 0 {
        Tree { left: None, right: None }
    } else {
        Tree { left: Some(Box::new(make(d - 1))), right: Some(Box::new(make(d - 1))) }
    }
}

fn count(t: Option<&Tree>) -> i64 {
    match t {
        None => 0,
        Some(n) => 1 + count(n.left.as_deref()) + count(n.right.as_deref()),
    }
}

fn trees(n: i64) -> i64 {
    let mut total = 0;
    for _ in 0..n {
        let t = make(16);
        total += count(Some(&t));
    }
    total
}

#[derive(Clone, Copy)]
struct Pt {
    x: i64,
    y: i64,
}

fn records(n: i64) -> i64 {
    let ps: Vec<Pt> = (0..n).map(|i| Pt { x: i % 1000, y: i % 7 }).collect();
    ps.iter().map(|p| p.x * p.y).sum()
}

#[derive(Clone)]
struct Item {
    #[allow(dead_code)]
    sku: String,
    price: i64,
    qty: i64,
}

enum Disc {
    NoDisc,
    Pct(i64),
    Bogo(i64),
}

#[inline(never)]
fn item_total(it: &Item, d: &Disc) -> i64 {
    let gross = it.price * it.qty;
    match *d {
        Disc::NoDisc => gross,
        Disc::Pct(p) => gross - gross * p / 100,
        Disc::Bogo(n) => if n <= 0 { gross } else { gross - it.qty / (n + 1) * it.price },
    }
}

fn business(n: i64) -> i64 {
    let items: Vec<Item> = (0..1000i64).map(|i| Item { sku: format!("sku{i}"), price: 100 + i % 50, qty: 1 + i % 5 }).collect();
    let mut total = 0i64;
    for _ in 0..n {
        for (j, it) in items.iter().enumerate() {
            let j = j as i64;
            let d = if j % 3 == 0 { Disc::NoDisc } else if j % 3 == 1 { Disc::Pct(j % 30) } else { Disc::Bogo(j % 4) };
            total = (total + item_total(it, &d)) % 1000000007;
        }
    }
    total
}

fn matrix(n: i64) -> i64 {
    let n = n as usize;
    let a: Vec<Vec<i64>> = (0..n).map(|i| (0..n).map(|j| ((i * j + 1) % 10) as i64).collect()).collect();
    let mut sum = 0i64;
    for i in 0..n {
        for j in 0..n {
            let mut acc = 0i64;
            for k in 0..n {
                acc += a[i][k] * a[k][j];
            }
            sum = (sum + acc) % 1000000007;
        }
    }
    sum
}

fn main() {
    let only: Vec<String> = std::env::args().skip(1).collect();
    let kernels: [(&str, fn(i64) -> i64, i64); 13] = [
        ("arith", arith, 50_000_000),
        ("fib", fib, 32),
        ("collatz", collatz, 300_000),
        ("list_index", list_index, 5_000_000),
        ("list_walk", list_walk, 5_000_000),
        ("sorting", sorting, 500_000),
        ("structs", structs, 20_000_000),
        ("text", text, 1_000_000),
        ("bigint", bigint, 5_000),
        ("trees", trees, 20),
        ("records", records, 2_000_000),
        ("matrix", matrix, 150),
        ("business", business, 2000),
    ];
    for (name, f, n) in kernels {
        if !only.is_empty() && !only.iter().any(|o| o == name) {
            continue;
        }
        let mut best = f64::MAX;
        let mut result = 0;
        for _ in 0..3 {
            let t = Instant::now();
            result = black_box(f(black_box(n)));
            best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!("{name} {n} {result} {best:.2}");
    }
}
