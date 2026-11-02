# Parallelism and optimization audit — 2026-11-01

Scope: [the language amendment](parallelism.md), both execution engines,
optimized native lowering, immutable list ownership, machine mailboxes,
and the existing whole-workspace regression and security gates.

## Measurements

Environment: AMD Ryzen 5 7600X (6 cores, 12 logical processors), Linux
x86_64 on WSL2, Rust 1.99.1, GCC 11.4.0, Python 3.10.12. Rust was built
in release mode; the native driver used the optimized profile without
sanitizers. Each row is the median of five warmed runs. Every sample's
result is checked against the reference engine, outside the timed region.
Native timings include the driver protocol and result transfer.

The baseline was captured after adding parallel syntax/scheduling and
before the optimization passes. It used the same four benchmark kernels
and four workers. Both versions used optimized C compiler flags. The
project later gained regression functions, so checker and cold compile
numbers are recorded separately and are not used to claim a compiler
speedup.

| Workload | Engine | Before, ms | After, ms | Before / after |
| - | - | -: | -: | -: |
| `work` | eval | 36.722 | 36.628 | 1.00× |
| `work` | native | 2.221 | 0.982 | 2.26× |
| `build` | eval | 527.431 | 1.334 | 395.51× |
| `build` | native | 42.654 | 5.747 | 7.42× |
| `text_scan` | eval | 11.918 | 2.262 | 5.27× |
| `text_scan` | native | 0.220 | 0.188 | 1.17× |
| `batches` | eval | 82.289 | 86.018 | 0.96× |
| `batches` | native | 4.763 | 1.927 | 2.47× |

`work` performs 100,000 arithmetic-loop iterations. `build` appends
10,000 integers and returns the complete list. `text_scan` measures
10,000 Unicode-length reads of a 20,000-scalar string. `batches` maps eight
100,000-iteration jobs. These are workload measurements, not universal
speedup claims. Interpreter arithmetic is effectively unchanged; its
parallel baseline comparison is noisy and shows no optimization gain.

Using the optimized implementation, four workers versus one gives:

| Parallel workload | One worker, ms | Four workers, ms | Throughput factor |
| - | -: | -: | -: |
| `batches`, eval | 290.360 | 86.018 | 3.38× |
| `batches`, native | 6.986 | 1.927 | 3.63× |

Raw samples: [before](benchmarks/2026-11-01-before.json),
[after, one worker](benchmarks/2026-11-01-after-1.json),
[after, four workers](benchmarks/2026-11-01-after-4.json).
Cold driver compilation/startup is about 2.2 seconds in the recorded
four-worker run; the following cached one-worker start is about 17 ms.
Neither is included in per-call execution time.

## Review and hardening

| Risk reviewed | Implementation and validation |
| - | - |
| Hidden effects or mutation inside tasks | Transitive effect analysis rejects mutators, capability use, assignments, and escaping control flow. The collection evaluates once under the caller's normal rules. |
| Concurrent access to interpreter `Rc` values | Only flat owned value graphs cross worker boundaries. Each worker reconstructs thread-local values; capability hosts remain on their owner. |
| Native sharing and races | Inputs/captures are immutable, reference counts and worker permits are atomic, result slots are disjoint, and every worker joins before cleanup. ThreadSanitizer exercises overlapping work and nested maps. |
| Unbounded nested worker creation | A global permit cap limits extra workers; nested scopes run inline, including when the parent could not obtain a permit. |
| Deadline/depth reset in a task | Workers inherit the absolute deadline, active call count, contract policy, and recursive measure context. Native workers also observe the host cancellation token. |
| Nondeterministic failure selection | The first failing input index wins. Higher-index work cooperatively cancels at checkpoints; lower-index work finishes. A division failure followed by infinite tasks must return before the inherited five-second budget. |
| Fault cleanup and allocation failure | Fault ownership transfers after join; partial results, worker contexts, and permits are released. Native tests sweep 20 heap-budget settings and check zero leaked objects/bytes. |
| Overflow or fault-erasing optimizations | Small-integer arithmetic falls back to big integers. Bounded constant folding never folds division by zero or removes evaluation of a potentially faulting operand such as `(100 / n) * 0`. |
| Mutation of shared list snapshots | Capacity reuse requires unique ownership. Shared lists copy; aliasing arguments are evaluated before reuse. Snapshot and nested-list cases compare both engines under sanitizers. |
| Diagnostic text outside the shared catalog | The full regression caught the new private parallelism text. Type errors and task cancellation now use the existing shared templates; catalog, generated-table, and native task regressions pass. |
| Loss of validation during machine optimization | Direct iterative boundary traversal retains structural checks and mandatory invariants; existing transition/rollback and foreign-value suites remain applicable. |
| Concurrent state updates or unbounded event queue | A bounded mailbox owns one machine; each accepted event either atomically commits one receipt or fails without advancing state. Eight producers submit and replay 800 events; accepted events drain on normal/panic scope exit even when replies are dropped. |

The checker and generated HIR/type tables are trusted, as are custom
native adapters and embedding engines. This is not a sandbox for hostile
Rust/C hosts. Compile-time folding has explicit node, integer, and text
budgets and never runs user functions.

## Validation

The final consolidated workspace run passes **3,300 tests, zero failures,
zero ignored**. Its three long differential/fuzz gates were filtered after
separate completion: **3,303 tests pass across these runs**. C, Python,
AddressSanitizer, UndefinedBehaviorSanitizer, and ThreadSanitizer were
required. The final code also passes strict Clippy and rustdoc checks.

- Runtime fuzzing: **240,000 inputs compared**, with zero divergences.
  The JSON-reader subset accepted 3,849 inputs and rejected 6,151 in both
  implementations.
- Debug differential execution: **108,556 comparisons, zero divergences**,
  with every HIR kind covered, including 8,946 visits to `Parallel`.
- Optimized execution under ASan/UBSan: **108,566 comparisons, zero
  divergences**. Both campaigns exercise 97 harvested programs; 15
  snippets that cannot check independently are excluded by the existing
  harness. Random-sweep resource skips stay below 0.5%, within its 2% gate.
- Native host concurrency and the new parallel-runtime suite pass under
  ThreadSanitizer. The focused native map suite passes with one worker
  and with the default parallel setting, and reports zero live native
  handles, capabilities, objects, and buffers at shutdown.
- Strict Clippy, formatting, documentation, and the locked example's
  release check pass. The example runs and tests identically on both
  engines. No existing AST or C-output golden required a semantic change.

Added coverage includes 5,000 deterministic source mutations (seed
`0x6ae7df1d89e483e8`; 142 parse-valid cases reached semantic checking),
2,000 native/reference arbitrary-precision arithmetic
comparisons (seed `0x504152414c4c454c`), repeated parallel result/fault
comparisons, serial fallback, nested groups, inherited deadlines,
capability/effect rejection, immutable aliases, and mailbox ordering.
Existing full-scale runtime fuzzing uses 10,000 inputs per operation;
state-machine model fuzzing and replay verification also remain enabled.
Random-input parallel kernels bound their work independently of input
magnitude. Deliberately nonterminating helpers remain in the dedicated
50 ms deadline and five-second cancellation tests. The initial benchmark
copy was removed from the random sweep after it produced excessive timed
cases; the affected campaigns were restarted with the corrected corpus.
The sanitized differential campaign now exercises optimized lowering;
the separate debug campaign retains all HIR nodes for the coverage gate.

The locked dependency audit used `cargo-audit 0.22.2`: **75 dependencies,
zero known vulnerabilities, zero informational warnings**, with no ignored
advisories or target filter. RustSec database: 1,395 advisories, commit
`2295466c4e9df26647fd67d8fa1c705ac2fc7f81`, updated
2026-10-30T12:12:39+01:00. `Cargo.lock` is unchanged by this work.

## Reproduction and limits

```sh
CPL_PARALLELISM=4 cargo run --release --locked -p cpl-cabi --example performance
CPL_PARALLELISM=1 cargo run --release --locked -p cpl-cabi --example performance
CPL_REQUIRE_CC=1 CPL_REQUIRE_SANITIZERS=1 CPL_REQUIRE_TSAN=1 CPL_REQUIRE_PYTHON=1 \
  cargo test --workspace --locked --no-fail-fast
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps --locked
cargo audit --file Cargo.lock
```

Run benchmarks without competing CPU-heavy jobs. Small tasks can be
slower when parallelized, and interpreter capture transfer copies values.
Task cancellation is cooperative; long primitive operations are not
interrupted mid-operation. Pure code can diverge, so configure deadlines
and process-level resource limits for untrusted programs.

Mailboxes currently host the reference evaluator. Native hosts own their
contexts on host threads. Asynchronous I/O, effect delivery, durable
queues/outboxes, and language `async`/`await` remain outside this change.
The public C API layout is unchanged; Rust embedders using the previous
`Value::List(Rc<[Value]>)` payload should use `Value::list`/`as_list` instead.
These finite tests are evidence for this implementation on Linux x86_64,
not proof of universal speed, cross-platform behavior, or absence of bugs.

## Follow-up: evaluator integer allocation

Integer operators now borrow literal/local operands when both reads cannot
execute code, including erased widening. Other arithmetic evaluates both
operands left to right, then reuses the owned left temporary's limb buffer
for addition, subtraction, and multiplication. Negation consumes its
temporary. Locals and shared snapshots remain immutable; division faults
retain their operands, location, and evaluation order.

On the same machine, with four workers and five warmed release samples:

| Workload | Eval before, ms | Eval after, ms | Before / after |
| - | -: | -: | -: |
| `work` | 37.042 | 24.357 | 1.52× |
| `build` | 1.389 | 0.954 | 1.46× |
| `text_scan` | 2.364 | 1.837 | 1.29× |
| `batches` | 76.509 | 60.676 | 1.26× |

These measurements compare the initial repository commit with the integer
allocation change, using the same benchmark sources and configuration.
Native code is unchanged. Raw samples:
[before](benchmarks/2026-11-01-integer-before-4.json),
[after](benchmarks/2026-11-01-integer-after-4.json).

Validation: the complete `cpl-eval` suite, focused native/reference
comparisons under ASan/UBSan (including 2,000 seeded arbitrary-precision
arithmetic cases), strict Clippy for all evaluator targets, formatting,
and whitespace checks pass. New regressions cover immutable operands,
256-bit integers, right-operand mutation, and division fault locations.

## Follow-up: native loop checkpoints

Optimized loops now inline the common checkpoint path. Every condition
still polls host cancellation and the parallel failure index atomically;
active deadlines and pending cancellation use the existing runtime helper
for fault handling. Debug and coverage builds keep the original helper.

With the evaluator optimization already applied, five warmed samples with
four workers gave these native medians:

| Workload | Before, ms | After, ms | Before / after |
| - | -: | -: | -: |
| `work` | 0.875 | 0.765 | 1.14× |
| `batches` | 1.916 | 1.696 | 1.13× |

Raw samples, including workloads without a measured improvement:
[before](benchmarks/2026-11-01-checkpoint-before-4.json),
[after](benchmarks/2026-11-01-checkpoint-after-4.json).

Validation: all 34 runtime/emitter Rust test cases pass, including the
generated C corpus and the C unit programs in debug, optimized, and
ASan/UBSan configurations. The parallel C suite passes under TSan.
Native/reference comparisons pass with one and four workers, including
cancellation of infinite tasks without a deadline. Direct checkpoint
regressions cover host cancellation, parallel failure ordering, and time
exhaustion. Strict Clippy, formatting, and whitespace checks pass.

## Follow-up: lazy structural equality

The evaluator now compares the first value directly and queues slices of
remaining children only when needed. Scalar comparisons and shared
buffers allocate no traversal stack. Flat lists no longer enqueue every
element before checking the first one; traversal memory follows nesting
depth rather than list width. Comparison and destruction remain iterative.

The benchmark now includes a missing-element search over 10,000 integers
repeated 100 times, first-element mismatches between two 10,000-element
lists repeated 1,000 times, and full equal-list comparisons repeated 100
times. Both sides use the same expanded benchmark and the preceding two
optimizations. Five warmed samples with four workers give:

| Workload | Eval before, ms | Eval after, ms | Before / after |
| - | -: | -: | -: |
| `contains_scan` | 9.723 | 2.690 | 3.61× |
| `equality_scan` | 3.226 | 0.400 | 8.07× |
| `equality_equal` | 3.355 | 3.453 | 0.97× |

Full equal-list traversal shows no speedup. These changes affect reference
evaluation; native equality code is unchanged. Raw samples:
[before](benchmarks/2026-11-01-equality-before-4.json),
[after](benchmarks/2026-11-01-equality-after-4.json).

Validation: **3,303 workspace tests pass across the main run and targeted
reruns**, including C, Python, ASan/UBSan, TSan, deep-value protocol round
trips, and the 200,000-level equality regressions. Three long differential
and fuzz campaigns were filtered from this follow-up run. Cached syntax
and runtime-specification test binaries still named the repository's old
path; rebuilding those packages resolved the missing-file failures. The
syntax golden now includes the two added benchmark functions. The example
passes its locked release check, and strict workspace Clippy, Rust/CPL
formatting, and whitespace checks pass.
