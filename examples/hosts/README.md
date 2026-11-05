# One CPL module, four host languages

`src/shop/pricing.cpl` prices an order: refined amounts (`Cents`,
`Percent`), a struct with an invariant, an enum of discounts, contracts, a
recursive function with a `decreases` measure, `check<R>`, integers past
64 bits, and two capabilities that the host implements (`TaxTable` and
`AuditLog`).

`cpl build --target rust|go|java` compiles the module to the source of the
host language (`spec/hosts.md`). The host calls CPL the way Kotlin calls
Java: a CPL function is a host function, a CPL struct or enum is a host
type, and a capability is a host trait or interface. No call crosses a
foreign-function boundary. The four programs below print the same lines
(`expected.txt`).

From this directory:

```sh
# Rust: a path dependency on the generated crate
cargo run -p cpl-cli -- --project . build --target rust
(cd rust && cargo run)

# Go: a replace directive to the generated module
cargo run -p cpl-cli -- --project . build --target go
(cd go && go run .)

# Java: the generated sources and the program, compiled together
cargo run -p cpl-cli -- --project . build --target java
javac -d classes $(find .cpl/build/java/src -name '*.java') java/Main.java
java -cp classes Main

# Kotlin: the same Java classes, through Kotlin's Java interop
kotlinc kotlin/Main.kt -cp classes -d main.jar
java -cp classes:main.jar:$KOTLIN_HOME/lib/kotlin-stdlib.jar MainKt
```

What the host sees:

| | Rust | Go | Java | Kotlin |
| - | - | - | - | - |
| call | `shop::subtotal(order)?` | `pricing.Subtotal(order)` | `Pricing.subtotal(order)` | `Pricing.subtotal(order)` |
| a struct | `Line { sku, unit_price, quantity }` | `pricing.Line{Sku: ..}` | `new Line(sku, price, qty)` | `Line(sku, price, qty)` |
| an enum | `Discount::PercentOff(n)` | `pricing.Discount_PercentOff{F0: n}` | `new Discount.PercentOff(n)` | `Discount.PercentOff(n)` |
| a capability | `impl TaxTable for Rates` | a type with `RateFor` | a lambda | a lambda |
| a fault | `Err(Fault)` | `error` (`*cpl.Fault`) | `Fault`, unchecked | `Fault`, unchecked |

A call outside a function's contract is refused at the boundary
(`line_total` with a negative price is a `refinement_violation`), and a
contract that fails inside CPL is a fault the host receives as an error
(`dollars(-5)` violates `requires cents >= 0`). In Kotlin, CPL's `Int`
takes Kotlin's operators (`total * CplInt.of(2) + CplInt.of(1)`), and a
`when` over a CPL enum is exhaustive.
