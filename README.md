# CPL

**CPL (Churn Programming Language)** is a statically typed language and
toolchain for codebases that people and AI agents change together.
Immutable values, explicit contracts, capability interfaces, and stable
symbol identities make the assumptions and effects of every change visible
in the source, where tools can check them.

> **Status:** early software, edition 0.1. The language, standard library,
> and toolchain are specified in [`spec/`](spec/README.md) and implemented
> in this repository. Expect breaking changes.

New to the language? Read [Learn CPL in Y Minutes](docs/learn-cpl-in-y-minutes.md),
an annotated tour whose examples are executable tests.

## Features

- **A small typed language.** Arbitrary-precision `Int`, immutable values,
  structs, enums, nominal wrapper types, generics, refinement types
  (`type Positive = Int where self > 0`), and `Option` and `Result` instead
  of null and exceptions.
- **Contracts.** `requires`, `ensures`, loop `invariant` and `decreases`
  clauses, and struct invariants are checked at run time. Functions marked
  `@prove` go to an [automatic verifier](spec/verification.md) that reports
  `proved`, `failed` with the inputs that break an obligation, or `unknown`
  with a reason.
- **Explicit effects.** External operations go through capability
  interfaces passed as `using` parameters and bounded by `effects` clauses;
  `pure` functions cannot perform them.
- **Tests built in.** `test` blocks and generated `property` tests with
  shrinking and reproducible seeds.
- **Concurrency.** Order-preserving `parallel for` expressions,
  [`concurrent` task blocks](spec/concurrency.md) whose effects are checked
  not to interfere, and [application state machines](spec/state-machines.md)
  with atomic transitions and replay.
- **Two engines.** A reference interpreter and a C11 backend, tested
  against each other; [C](spec/cabi.md) and [Python](spec/python.md)
  packages and C libraries as capability implementations.
- **Rust, Go, Java, and Kotlin hosts.** `cpl build --target rust|go|java`
  compiles a program to the [host's own source](spec/hosts.md), so the host
  calls CPL the way Kotlin calls Java: CPL functions, structs, and enums are
  the host's, capabilities are its traits and interfaces, and no call
  crosses a foreign-function boundary. Every backend is tested against the
  interpreter.
- **Tools for change.** Stable `@id` identities, a semantic index
  (`cpl inspect`, `cpl query`), semantic diffs and merges keyed by identity,
  checked edits and transactions, differential testing of changed
  functions, change contracts such as `preserve_behavior`, state-machine
  exploration, and versioned data migrations
  ([program evolution](spec/evolution.md)).
- **Typed holes.** `?name` placeholders type-check during development,
  report their expected type and the bindings in scope, and are rejected by
  release checks.

## A taste of CPL

```cpl
module shop

use std.text.{parse_int}

/// A number of items, at least one.
pub type Quantity = Int where self > 0

/// The price of `quantity` items, in cents.
@prove
pub pure fn total(unit_cents: Int, quantity: Quantity) -> Int
    requires unit_cents >= 0
    ensures result >= unit_cents
{
    unit_cents * quantity
}

/// A quantity typed by a customer: an error unless it is a positive integer.
pub pure fn parse_quantity(text: Text) -> Result<Quantity, ConstraintError> {
    let n = parse_int(text)?
    check<Quantity>(n)
}

/// Where receipts go. The host supplies the implementation.
pub capability Printer {
    fn print(line: Text) -> Unit
}

pub mutate fn checkout(unit_cents: Int, quantity: Quantity, using printer: Printer) -> Int
    effects { printer.print }
    requires unit_cents >= 0
{
    let cents = total(unit_cents, quantity)
    printer.print("total: " ++ cents.to_text())
    cents
}

test "a dozen at 250 cents" {
    assert total(250, 12) == 3000
    assert parse_quantity("12") == Ok(12)
    assert parse_quantity("zero").is_err()
    assert parse_quantity("0").is_err()
}

property "two cost twice as much as one"
    for unit in gen.range(0, 1000)
{
    assert total(unit, 2) == total(unit, 1) * 2
}
```

`cpl check` proves `total`'s postcondition, `cpl test` runs the test and
the property, and `cpl run shop.total 250 0` stops with a
`refinement_violation`, because `0` is not a `Quantity`.

## Installation

You need a recent stable [Rust](https://www.rust-lang.org/tools/install)
toolchain with Cargo, and Git. From a clone of this repository:

```sh
cargo install --path crates/cpl-cli --locked
```

This installs the `cpl` command into Cargo's `bin` directory
(usually `~/.cargo/bin`).

Native execution (`--engine native`) and `cpl build --target c` also need a C
compiler: gcc 9 or later or clang 12 or later, on Linux
([platform matrix](spec/cabi.md#17-target-abi-and-platform-matrix-for-edition-01)).
`cpl build --target python` needs CPython 3.10 or later with its headers.
`cpl build --target rust|go|java` writes source and needs nothing; the host
compiles it with its own toolchain (Rust, Go 1.21 or later, Java 17 or
later; Kotlin through Java).

## Quick start

A project is a directory with a `cpl.toml` and sources under `src/`; module
`a.b` lives in `src/a/b.cpl`.

```sh
mkdir -p shop/src
cd shop
printf '[project]\nname = "shop"\nedition = "0.1"\n' > cpl.toml
# save the example above as src/shop.cpl, then:
cpl check                       # type-check, and verify @prove functions
cpl test                        # run tests and properties
cpl run shop.total 250 12       # call a function on constant arguments: 3000
cpl prove shop.total            # the verifier's verdict for each obligation
cpl inspect shop.total          # signature, contract, effects, callers, ...
cpl fmt                         # format the project's sources
```

Every command accepts `--json` for one structured result on stdout, and
`--project <dir>` to point at a project from elsewhere. `cpl <command> --help`
lists a command's options.

## Commands

| Command | What it does |
| - | - |
| `check` | type-check the project; `--release` also rejects holes and unproved `@prove` functions |
| `run` | run one function on constant arguments, on the interpreter, natively, or on both |
| `test` | run tests and properties |
| `fmt` | format `.cpl`, `.work`, and `.patch` files |
| `build` | build a C package (`--target c`) or a Python package (`--target python`), or write a Rust, Go, or Java package (`--target rust\|go\|java`) |
| `prove` | report the verification status of one function |
| `inspect`, `query` | query the semantic index: symbols, types, dependencies, effects, obligations |
| `diff` | semantic diff between two revisions |
| `id assign` | insert missing `@id` annotations |
| `verify` | verify a snapshot, or the declarations affected since a revision |
| `edit`, `transaction` | prepare and apply checked semantic edits and transactional changes |
| `conflicts`, `merge` | merge two revisions by stable identity and test the composition |
| `explore`, `replay` | explore a state machine over a finite event alphabet; replay recorded events |
| `schema`, `migrate` | inspect versioned schemas; migrate persisted values |
| `concurrent` | plan effectful tasks, run them with fixture hosts, or replay recorded effects |
| `work check` | validate a `.work` coordination file against the project |

## Documentation

- [Learn CPL in Y Minutes](docs/learn-cpl-in-y-minutes.md): an executable
  tour of the language and the toolchain.
- [Specifications](spec/README.md): the grammar, semantics, toolchain,
  standard library, C, Python, Rust, Go, and Java interop, and the
  amendments for state machines, parallelism, program evolution,
  concurrency, and verification.
- [Design document](design-doc.md): the original statement of intent.
- Examples:
  - [`examples/reader`](examples/reader): contracts, capabilities, and a
    `.work` file with a semantic patch.
  - [`examples/state_machine`](examples/state_machine/README.md): a
    scoped application machine with replay.
  - [`examples/parallel`](examples/parallel/README.md): structured parallel
    computation and benchmarks.
  - [`examples/evolution`](examples/evolution/README.md): checked program
    evolution, end to end.
  - [`examples/holes`](examples/holes): typed holes.
  - [`examples/hosts`](examples/hosts/README.md): one module called from
    Rust, Go, Java, and Kotlin.

## Repository layout

| Path | Contents |
| - | - |
| `crates/cpl-cli` | the `cpl` command-line tool |
| `crates/cpl-syntax` | lexer, parser, AST, and formatter |
| `crates/cpl-sema` | the checker: names, types, effects, contracts |
| `crates/cpl-hir` | the typed, resolved intermediate representation |
| `crates/cpl-eval` | the reference interpreter and the test runner |
| `crates/cpl-prove` | the automatic verifier |
| `crates/cpl-index` | stable identities, the semantic index, receipts, and semantic diff |
| `crates/cpl-evolve` | checked program evolution |
| `crates/cpl-work` | work coordination and semantic patches |
| `crates/cpl-project` | project loading: `cpl.toml` and `cpl.lock` |
| `crates/cpl-diag` | source positions and diagnostics |
| `crates/cpl-emit-c`, `crates/cpl-rt`, `crates/cpl-rtspec`, `crates/cpl-cabi` | the C11 backend, its runtime, the tables both engines share, and the C boundary |
| `crates/cpl-emit-host` | the Rust, Go, and Java backends and their runtimes |
| `std/` | the standard library's declarations |
| `spec/` | the specifications |
| `examples/` | example projects |
| `docs/` | guides |

## Development

```sh
cargo build --release                  # target/release/cpl
cargo test --workspace                 # every suite
cargo clippy --workspace --all-targets
cargo fmt --all
```

The native differential suites (`cargo test -p cpl-cabi --test differential`)
compare the C backend with the interpreter and take several minutes. Tests
that need a C compiler are skipped without one; set `CPL_REQUIRE_CC=1` to
make that a failure. Golden files regenerate with `UPDATE_GOLDEN=1`. The host
backends' tests (`cargo test -p cpl-emit-host --test hosts`) run each host
whose toolchain is installed: Go and Java from `PATH` (or `JAVAC` and `JAVA`),
Kotlin from `PATH` or `KOTLINC`.

Fuzzers for deeper runs:

```sh
cargo run --release -p cpl-prove --example prove_fuzz    # verifier verdicts against execution
cargo run --release -p cpl-cabi --example native_fuzz    # C backend against the interpreter
cargo run --release -p cpl-cli --example robust_fuzz     # mutated sources: no panics, stable formatting
cargo run --release -p cpl-evolve --example merge_fuzz   # semantic merges against the expected program
cargo run --release -p cpl-emit-host --example host_diff -- go 200   # a host backend against the interpreter
```

## License

CPL is released under the [MIT License](LICENSE).
