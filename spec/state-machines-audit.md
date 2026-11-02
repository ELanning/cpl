# State-machine language audit — 2026-11-01

Scope: the language amendment in `state-machines.md`, its parser and
formatter, semantic checks, proof evaluator, interpreted and emitted C
execution, foreign-value boundaries, semantic index, and replay CLI.
The full workspace regression suite also exercises the existing C ABI,
Python adapters, worker transport, ownership, concurrency, and allocation
failure behavior.

Environment: Linux x86_64 on WSL2, Rust 1.99.1, system GCC 11.4.0, and
Python 3.10.12. These results do not constitute a cross-platform test run.

## Threat model and review

Reviewed inputs include malformed source, recursive call graphs,
expensive compile-time predicates, invalid JSON/events, forged replay
receipts, invalid nested host values, and native execution faults.
The compiler's checked HIR, generated type tables, `RunEngine`
implementation, and host embedding code are trusted. This is not a
sandbox for malicious native adapters or arbitrary hostile Rust/C code.

The review followed each new syntax form through name resolution,
typing, effect/contract checks, HIR, indexing, lowering, boundary
validation, and fault reporting. It also checked the distinction between
an immutable candidate value and the context's committed state.

## Hardening implemented

| Finding | Resolution and regression coverage |
| - | - |
| Indirect calls and recursion could undermine an explicit purity designation without a transitive check. | Reverse graph reachability detects all paths to mutators; diagnostics show a witness chain. Generated cycles of 1–80 functions exercise this. |
| Rendering a full witness for every function in a long invalid chain could produce quadratic diagnostic output. | The reachability pass retains the terminal mutator; rendering bounds the prefix to 64 entries and explicitly marks truncation while preserving that endpoint. Generated long cycles check the bound and endpoint. |
| Struct invariants would be bypassable if attached only to source constructors. | The checker, interpreter, native lowering, and nested foreign boundaries share the invariant. Both engines reject malicious host values with optional contract checks disabled. |
| A constant-proof attempt could consume excessive time or memory. | Expression, aggregate allocation, integer, text, and list budgets apply to constructor and legacy refinement proof attempts. Exponential string growth falls back to a runtime check for constructors and reports an unproved legacy refinement. |
| Program-root updates could escape through constructors or spread syntax in helpers. | Both forms require a matching root machine or initializer. Initializers reject root snapshots nested in lists, options, generic records, and nominal wrappers. |
| Replay identity based on declarations alone would miss changed imports. | Identity hashes whole formatted modules, including imports, plus toolchain version and optional-contract policy. An import-only change invalidates identity even if current outputs happen to match. |
| A failed transition or invalid command could expose partially updated state. | Event validation, execution, result validation, and receipt comparison precede commit. Fault, invariant, malformed-event, resource-limit, and forged-receipt cases assert unchanged state and sequence. |
| Replay could repeat external effects. | Machines accept no capabilities and cannot reach mutators. Commands remain data; replay binds no handlers. Command execution belongs to the host after commit. |
| An unbounded reducer could hang the replay CLI. | Replay has a positive, configurable time budget, defaulting to 5 seconds per call or boundary predicate, plus the existing call-depth limit. A nonterminating reducer is tested on both engines. |
| Constructor constraints could be invisible in semantic review. | Invariants have contract facets and dependency edges. Constructor proof obligations expose proved versus runtime-checked status. Function roles participate in signature/effects facets. |

## Fuzzing and regression coverage

- **5,000 deterministic source mutations**, seed
  `0x6ae7df1d89e483e8`: parser recovery, formatter reparsing/idempotence,
  contextual prefixes, Unicode, NULs, truncated syntax, and malformed
  contracts. **231** syntactically valid mutations reached semantic
  checking without a panic.
- **80 generated recursive call graphs**, containing 1 through 80
  functions, verify that mutation cannot be hidden behind a cycle.
- **10,000 model-based events**, seed `0x6ae7df1d89e483e8`, compare the
  context against an independent bounded-counter model, replay every
  successful transition, and check rollback after rejected updates.
- **2,000 native/evaluator event comparisons**, seed
  `0x4d414348494e4553`, plus native replay of accepted events, run under
  AddressSanitizer and UndefinedBehaviorSanitizer. They compare states,
  commands, receipts, and normalized faults, including division by zero
  and invariant failures. Shutdown reports zero live native objects,
  handles, capabilities, and buffers.
- Targeted tests cover generic constructor invariants, spread updates,
  nested foreign values, constant proofs and known faults, explicit
  effects, state/event exhaustiveness, duplicate root machines,
  initializer authority, contextual holes, stale request IDs, receipt
  tampering in every field, duplicate receipts, import invalidation,
  CLI duplicate-key rejection, timeouts, and stack-limit diagnostics.
- Existing differential runtime fuzzing runs at its full default scale:
  **10,000 inputs per operation**, covering large integers, arithmetic,
  comparisons, numeric conversion, random UTF-8/Unicode, text operations,
  value rendering, and malformed JSON. The existing coverage gate and
  sanitized differential suite run alongside it.

The cases are reproducible in `crates/cpl-{syntax,sema,eval,index,cabi,cli}/tests`.
The new test files are `machines.rs`, `machine_fuzz.rs`, and
`constructor_invariants.rs`; the preexisting runtime campaign is
`crates/cpl-cabi/tests/differential/fuzz.rs`.

## Dependency audit

`cargo-audit 0.22.2` checked the locked dependency graph against the
[RustSec advisory database](https://rustsec.org/). Result: **75 dependencies,
zero known vulnerabilities, zero informational warnings**. Database:
1,395 advisories; commit
`2295466c4e9df26647fd67d8fa1c705ac2fc7f81`, updated
2026-10-30T12:12:39+01:00. No advisories were ignored and no target filter
was applied. The audit tool was installed under `/tmp`, without changing
the project's dependencies.

## Completed validation

- The final consolidated workspace run passes **3,291 tests, zero
  failures, zero ignored**, with C compiler, sanitizers, ThreadSanitizer,
  and Python coverage required. Only the three already-completed long
  differential gates below were filtered from this rerun: **3,294 tests
  pass across the two runs**.
- The full differential coverage gate, runtime fuzz campaign, and
  AddressSanitizer/UndefinedBehaviorSanitizer differential suite pass
  (combined test-binary time: 874.02 seconds). The native machine suite
  and allocation-failure sweep also pass.
- Native host concurrency passes under ThreadSanitizer. Worker transport,
  protocol round trips, C/C++ headers, generated packages, Python
  integration, and the C runtime's address/undefined/leak sanitizer
  suites pass. The new example has an AST golden; no existing golden
  needed a semantic change.
- Strict workspace Clippy (`-D warnings`), Rust formatting, and
  documentation (`RUSTDOCFLAGS=-Dwarnings`) pass.
- The documented scoped-machine example runs identically on both
  engines. Its locked release check passes with warnings denied:
  zero errors, warnings, or holes.

## Reproduction

```sh
CPL_REQUIRE_CC=1 CPL_REQUIRE_SANITIZERS=1 CPL_REQUIRE_TSAN=1 CPL_REQUIRE_PYTHON=1 \
  cargo test --workspace --locked --no-fail-fast
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps --locked
cargo audit --file Cargo.lock
```

`CPL_DIFF_SEED` changes the existing runtime-fuzz seed;
`CPL_DIFF_SCALE` multiplies its default workload. Do not set the scale to
zero. The new native machine test always requests address/undefined
sanitizers. A missing required compiler or sanitizer must fail a release
validation run rather than count as passing coverage.

## Production limits

The context is an in-memory state owner. Persistence, authenticated event
logs, migration of old receipts, transactional outboxes, command
idempotency/delivery, and process supervision are application-host work.
Receipts are deterministic verification records, not cryptographic
authentication. They include full snapshots, so their storage and access
controls must suit the application's data.

Pure code is not necessarily terminating. Evaluation budgets and native
worker deadlines help contain failures; deployments accepting untrusted
programs still need process-level CPU/memory limits. The proof evaluator
is deliberately incomplete and does not replace a general verifier.
Native adapters and custom engines must uphold their API contracts.

The campaign is finite regression and security evidence, not proof that
all defects or vulnerabilities have been eliminated. Newly changed
dependencies and application-specific effect handlers need their own
ongoing review.
