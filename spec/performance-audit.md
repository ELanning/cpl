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

## Follow-up: scope lifetimes and allocation reuse

This pass removes repeated allocations and unnecessary collection copies
in both engines:

- The evaluator releases locals when their scope ends and reuses empty
  activation buffers. The pool holds at most 16 environments, each with
  capacity for at most 1,024 locals and scope entries; values are dropped
  before an environment enters the pool.
- Optimized native blocks release their locals and intermediate values
  when the block ends. A borrowed result is retained before cleanup, and
  nested blocks emit each temporary's cleanup once. Both engines avoid
  constructing an unused `Unit` for a loop body without a tail.
- `xs = xs ++ other` grows a uniquely owned list geometrically. Shared
  snapshots still copy. The evaluator retains the left operand until the
  right operand finishes, including when it reassigns `xs`; native
  lowering uses this path only when the right operand cannot reassign it.
  Native push and concatenation share a capacity reservation helper, with
  borrowed arguments retained before replacing the old buffer.
- Integer assignments with a local left operand and literal/local right
  operand reuse the destination's limb buffer for addition, subtraction,
  and multiplication. Expressions that can execute code use the ordinary
  evaluation path. Division and remainder are unchanged.
- Builtin implementations are resolved lazily and cached by declaration.
  Calls with up to three arguments use stack storage, preserving argument
  order and fault attribution.
- The evaluator's Unicode-length cache holds eight strings rather than
  one, so alternating between strings does not rescan them every time.
  Each cached string remains limited to 1 MiB, bounding retained text
  storage at 8 MiB per interpreter.

The benchmark now loads an in-memory project containing the existing
parallel example and
[`performance.cpl`](../crates/cpl-cabi/examples/performance.cpl). Added
kernels cover 100,000 counter increments, 50,000 small function calls,
5,000 list appends after a scoped snapshot, 5,000 singleton list
concatenations, alternating length reads of two 20,000-scalar strings,
and 20,000 quotient/remainder iterations. List-building kernels return a
length so their native timings measure computation without transferring
the whole list. The existing `build` kernel still returns its full list.

The new native/reference regression runs in debug and optimized profiles
under ASan/UBSan. It covers capacity boundaries, self-concatenation,
immutable aliases, right-operand mutations, values escaping blocks,
early returns and propagation, builtin argument order and faults, cache
eviction, and 256-bit arithmetic. Direct C regressions also check borrowed
heap children, self-append, reuse with no remaining heap budget, and
unchanged inputs/reference counts after allocation failure.

Baseline: `117c9d8` with the expanded benchmark driver and kernels.
The final comparison uses the same machine described above, release Rust,
optimized native code without sanitizers, five warmed samples per row,
and no competing test or build jobs. Selected four-worker medians:

| Workload | Engine | Before, ms | After, ms | Before / after |
| - | - | -: | -: | -: |
| `counter` | eval | 7.878 | 3.120 | 2.52× |
| `counter` | native | 0.524 | 0.247 | 2.12× |
| `calls` | eval | 7.940 | 6.010 | 1.32× |
| `calls` | native | 0.566 | 0.374 | 1.51× |
| `scoped_build` | eval | 143.358 | 0.843 | 170.02× |
| `scoped_build` | native | 11.801 | 0.107 | 110.48× |
| `concat_build` | eval | 131.950 | 0.493 | 267.82× |
| `concat_build` | native | 9.191 | 0.161 | 57.01× |
| `alternating_text` | eval | 21.883 | 1.402 | 15.61× |
| `alternating_text` | native | 0.335 | 0.301 | 1.11× |
| `work` | eval | 25.553 | 18.906 | 1.35× |
| `work` | native | 0.882 | 0.522 | 1.69× |
| `build` | eval | 0.982 | 0.444 | 2.21× |
| `build` | native | 5.932 | 5.821 | 1.02× |
| `batches` | eval | 54.866 | 45.274 | 1.21× |
| `batches` | native | 1.885 | 1.275 | 1.48× |

Raw samples for every workload: [before, four workers](benchmarks/2026-11-01-ownership-before-4.json),
[after, four workers](benchmarks/2026-11-01-ownership-after-4.json),
[before, one worker](benchmarks/2026-11-01-ownership-before-1.json), and
[after, one worker](benchmarks/2026-11-01-ownership-after-1.json).

The largest gains remove quadratic copying in these particular list-building
patterns. Full equal-list comparison and native list-result transfer show
no meaningful gain. Small native calls and first-mismatch comparisons are
noisy across runs, including some slower samples; these are workload
measurements, not a universal speedup claim. Checking and driver startup
are reported separately and are not included in these speedup claims.

Validation: **3,304 workspace tests pass across the main run and a targeted
rerun**, plus **108,544 optimized native/reference comparisons under
ASan/UBSan, zero divergences**. The campaign runs 97 harvested programs;
15 snippets that cannot check independently remain excluded. Its 473
resource-limited sweep skips are below the existing 2% gate (0.44%).
C, Python, ASan/UBSan, and TSan were required in the workspace run.
Strict workspace Clippy, rustdoc, Rust/CPL formatting, and whitespace checks
pass. The separate debug differential coverage campaign and D7 runtime
fuzz campaign were not rerun in this pass.

The workspace run caught one source-embedding mismatch because explanatory
C comments were added after its test binary compiled. The embedded/runtime
diff contained only those comments; rebuilding and rerunning the complete
header and mirror suites passed.

Commands used for the final regression run:

```sh
CPL_REQUIRE_CC=1 CPL_REQUIRE_SANITIZERS=1 CPL_REQUIRE_TSAN=1 CPL_REQUIRE_PYTHON=1 \
  cargo test --workspace --locked --no-fail-fast -- \
  --skip differential_suites_with_the_coverage_gate \
  --skip d8_sanitized_suites --skip d7_runtime_fuzz
CPL_REQUIRE_CC=1 cargo test --locked -p cpl-rt --test headers --test mirrors
CPL_REQUIRE_CC=1 CPL_REQUIRE_SANITIZERS=1 \
  cargo test --locked -p cpl-cabi --test differential d8_sanitized_suites
```


## Follow-up: text/byte growth, searches, and value transfer (2026-11-02)

A second pass extends buffer reuse to text and bytes, reduces wire-protocol
allocation, and measures text processing and ordinary division. The baseline
is the completed ownership pass above with the additional benchmark kernels,
before the changes in this section. The machine and compilers are unchanged.

Changes:

- Both engines grow uniquely owned text and byte buffers geometrically for
  `x = x ++ rhs`. Snapshots and self-concatenation remain immutable; the
  evaluator retains the original left operand while the right operand runs.
  Native lowering uses the owned helper only when the right operand cannot
  reassign locals. The native runtime discovers spare capacity through its
  existing allocation header, with no public C ABI or object-layout change.
- The evaluator's eight-entry text-length cache now holds weak identities,
  so it retains no text contents and does not create copy-on-write snapshots.
  Known scalar counts are updated by the appended text's count. This avoids
  rescanning a growing text, including texts above the previous 1 MiB cache
  limit. Weak identities prevent address reuse; host `Rc::make_mut` detaches
  them, so host edits cannot produce stale hits.
- Native substring search uses Linux `memmem` through the platform layer.
  Splitting a text with no separator occurrence shares its original buffer
  in both engines and searches only once. Unicode and embedded NULs remain
  byte-exact, with scalar-aligned matches for valid UTF-8.
- Single-part joins share that part. Multi-part evaluator joins reserve the
  required bytes once, and flattening validates and sizes its result without
  intermediate vectors. Evaluator text concatenation and case conversion
  also avoid copying a completed `String` into a second backing allocation.
- `Node` serialization/deserialization streams tagged arrays directly,
  retaining the protocol's shape, canonical decimal/hex rules, and strict
  indices. Byte encoding uses one output allocation instead of formatting
  each byte separately. Native value-returning calls parse nodes directly;
  faults and other responses preserve the general parser and its variant
  precedence. Native renderings move into their destination without cloning.
- Shared Rust division uses unsigned machine-word magnitudes when both
  operands fit, restoring the signs specified by S37 and retaining the
  arbitrary-precision fallback. Native optimized division/remainder inline
  the small-integer path, including overflow and zero-divisor fallbacks.

Rust embedding compatibility: `Value::Text` now contains `Rc<String>` and
`Value::Bytes` contains `Rc<Vec<u8>>`, replacing their slice payloads.
The stable constructor/accessor usage (`Value::text`/`as_text`,
`Value::bytes`/`as_bytes`) is unchanged. Code directly constructing those
payloads needs adjustment. The public C ABI, CPL values, and driver wire
format are unchanged. Growth can reserve spare capacity; heap-budget failures
leave native inputs and their reference counts unchanged.

The expanded benchmark includes 20,000 appends of a 72-byte Unicode text
chunk or a 64-byte byte chunk; repetitive-prefix search/split; joins;
flattening; and encoding/decoding 10,000 integer nodes plus 8 KiB of bytes.
Language calls warm up and compare engine results before measurement. Every
row reports five samples and their median; each wire sample contains 20
operations. Checking/startup is separate, and native times include protocol
and result transfer. No competing test/build jobs ran during measurement.

Selected medians with four workers:

| Workload | Engine | Before, ms | After, ms | Before / after |
| - | - | -: | -: | -: |
| `text_build` | eval | 1351.382 | 1.260 | 1072.60× |
| `text_build` | native | 659.401 | 0.408 | 1616.27× |
| `bytes_build` | eval | 949.801 | 1.129 | 841.59× |
| `bytes_build` | native | 471.691 | 0.397 | 1189.38× |
| `text_build_lengths` | eval | 630.417 | 2.585 | 243.88× |
| `text_contains_prefix` | native | 23.980 | 4.294 | 5.58× |
| `text_split_prefix` | native | 48.032 | 4.303 | 11.16× |
| `text_join_single` | eval | 3.860 | 0.159 | 24.33× |
| `text_join_single` | native | 1.169 | 0.192 | 6.10× |
| `flatten_lists` | eval | 19.499 | 12.718 | 1.53× |
| `divide_loop` | eval | 3.919 | 3.011 | 1.30× |
| `build` | native | 5.484 | 2.325 | 2.36× |
| `wire_encode_nodes` | wire | 32.438 | 10.337 | 3.14× |
| `wire_decode_nodes` | wire | 39.625 | 20.537 | 1.93× |

Raw results: [before](benchmarks/2026-11-02-buffers-before-4.json) and
[after](benchmarks/2026-11-02-buffers-after-4.json). The
`text_build_lengths` baseline is a separate
[intermediate run](benchmarks/2026-11-02-length-before-4.json), after buffer
reuse but before the weak/incremental length-cache change. That kernel reads
the text length after every append and returns their sum.

The largest gains remove quadratic buffer copying and rescanning in these
patterns. Other workloads show smaller gains or slower samples. The generalized
append path initially added cleanup overhead to list snapshots.
Keeping the existing `Rc` path for list snapshots reduced `concat_build`
from 0.473 ms to 0.445 ms in a consecutive comparison, close to the 0.436 ms
baseline. The [general-path run](benchmarks/2026-11-02-append-general-4.json)
and final after report retain all samples, including the remaining small
workload variations.

Validation completed:

- **3,305 workspace tests passed**, with C, Python, ASan/UBSan, and TSan
  required. The separate final reruns passed all 697 evaluator tests and
  11 protocol/optimization tests, including both native profiles under
  ASan/UBSan after the list-path correction.
- **240,000 D7 randomized comparisons passed**, including integers through
  2,000 decimal digits, Unicode, invalid UTF-8, rendering, and damaged JSON.
- **108,556 D8 comparisons passed under ASan/UBSan**, with no divergences or
  sanitizer failures. The D5 sweep skipped 461 resource-limited cases (0.43%).
- **108,557 debug/coverage comparisons passed**, with zero divergences and
  the full coverage gate satisfied. Its sweep skipped 460 resource-limited
  cases (0.42%). Both campaigns ran 97 harvested programs; 15 snippets that
  cannot check independently and seven undrawable functions remain the
  existing exclusions. Both skip rates are below the 2% gate.
- Strict workspace Clippy, rustdoc, Rust/CPL formatting, and whitespace
  checks passed. The release CLI was rebuilt and its version command ran.

The three differential campaigns compared **457,113 cases in total**. Direct
C tests check self-appends, preserved snapshots, cached Unicode scalar counts,
embedded NULs, reuse with no remaining heap budget, and failure without
changing the slot or reference counts. Protocol tests check exact wire shapes,
malformed tags/operands, and response precedence. Regression tests also check
host mutation after a weak-cache lookup and lengths beyond 1 MiB.

Commands (in addition to the workspace command above):

```sh
CPL_REQUIRE_CC=1 CPL_REQUIRE_SANITIZERS=1 CPL_DIFF_VERBOSE=1 \
  cargo test --locked -p cpl-cabi --test differential -- --nocapture --test-threads=1
cargo test --locked -p cpl-eval
CPL_REQUIRE_CC=1 CPL_REQUIRE_SANITIZERS=1 \
  cargo test --locked -p cpl-cabi --lib --test optimization
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps --locked
cargo build --release --locked -p cpl-cli
CPL_PARALLELISM=4 cargo run --release --locked -p cpl-cabi --example performance
```
