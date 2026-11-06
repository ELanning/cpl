//! A Rust program that calls the CPL module `shop.pricing` as Rust code (`cpl build --target rust`).

use pricing::cpl::{Fault, Int, List, Text, ValueText};
use pricing::shop::pricing::{self as shop, AuditLog, Discount, Entry, Line, TaxTable};
use std::cell::RefCell;

/// The host's tax table: 5% on food, 20% on everything else.
struct Rates;

impl TaxTable for Rates {
    fn rate_for(&self, sku: Text) -> Int {
        Int::small(if sku.starts_with("FOOD-") { 5 } else { 20 })
    }
}

/// A tax table with a bug: 150 is not a `Percent`.
struct Broken;

impl TaxTable for Broken {
    fn rate_for(&self, _sku: Text) -> Int {
        Int::small(150)
    }
}

/// The host's audit log: it keeps what it is told.
#[derive(Default)]
struct Log(RefCell<Vec<String>>);

impl AuditLog for Log {
    fn record(&self, message: Text) {
        self.0.borrow_mut().push(message.to_string());
    }
}

fn main() -> Result<(), Fault> {
    let apples = Line { sku: "FOOD-APPLE".into(), unit_price: Int::small(125), quantity: Int::small(6) };
    let mug = Line { sku: "MUG".into(), unit_price: Int::small(899), quantity: Int::small(2) };
    let order: List<Entry> = vec![
        Entry { line: apples, discount: Discount::BuyNGetOneFree(Int::small(2)) },
        Entry { line: mug, discount: Discount::PercentOff(Int::small(10)) },
    ]
    .into();

    println!("subtotal: {}", shop::subtotal(order.clone())?);
    let log = Log::default();
    let total = shop::checkout(order.clone(), &Rates, &log)?;
    println!("total: {} ({})", total, shop::dollars(total.clone())?);
    println!("log: {}", log.0.borrow().join("; "));
    println!("parse_quantity(\" 12 \"): {}", shop::parse_quantity(" 12 ".into())?.value_text());
    println!("parse_quantity(\"0\"): {}", shop::parse_quantity("0".into())?.value_text());
    println!("factorial(25): {}", shop::factorial(Int::small(25))?);

    // An argument outside its type is refused at the boundary; a failed contract is a fault.
    // Both reach the host as an error.
    let gift = Line { sku: "GIFT".into(), unit_price: Int::small(-1), quantity: Int::small(1) };
    match shop::line_total(gift, Discount::NoDiscount) {
        Err(fault) => println!("line_total(unit_price -1): {}", fault.code),
        Ok(v) => println!("line_total(unit_price -1): {v}"),
    }
    match shop::dollars(Int::small(-5)) {
        Err(fault) => println!("dollars(-5): {} {}", fault.code, fault.clause_kind.unwrap_or("")),
        Ok(v) => println!("dollars(-5): {v}"),
    }
    // What a capability returns is checked against its type too.
    match shop::total_with_tax(order, &Broken) {
        Err(fault) => println!("a rate of 150: {} {}", fault.code, fault.symbol),
        Ok(v) => println!("a rate of 150: {v}"),
    }
    Ok(())
}
