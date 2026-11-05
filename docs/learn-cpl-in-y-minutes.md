# Learn CPL in Y Minutes

**CPL (Churn Programming Language)** is a statically typed language for
people and AI agents working on the same codebase. Immutable values,
explicit contracts, capability interfaces, and stable symbol identities
make a change's assumptions and effects visible in the source.

This is an annotated tour in the spirit of
[Learn X in Y Minutes](https://learnxinyminutes.com/python/). The main code
block is one complete module: its examples are executable tests. CPL 0.1
has a reference interpreter, a native C11 backend, C and Python interop,
Rust, Go, Java, and Kotlin hosts that call CPL as their own code,
structured parallel expressions and concurrent tasks, application state
machines, an automatic verifier, typestate protocols, versioned data, and
tools that edit, verify, and merge programs by stable identity.

From the root of this repository, install the command-line tool with Rust and Cargo:

```sh
cargo install --path crates/cpl-cli --locked
mkdir -p hello/src
```

Git must be available to the toolchain. Native execution additionally
needs a supported C compiler on Linux; see the
[platform matrix](../spec/cabi.md#17-target-abi-and-platform-matrix-for-edition-01).

Save this as `hello/cpl.toml`:

```toml
[project]
name = "hello"
edition = "0.1"
```

Save the following module as `hello/src/learn.cpl`. Declarations live at
module scope; expressions run inside functions, tests, and properties.
`assert` checks the result of an example when you run `cpl test`.

```cpl
// ================================================================
// 1. Modules, imports, and comments
// ================================================================

// Every source file declares its module. learn conventionally lives
// in src/learn.cpl; a.b would live in src/a/b.cpl.
module learn

// Import individual declarations or a module under its final path name.
use std.text.{trim, split, join, parse_int}
use std.bytes

// Comments start with //.
// Documentation comments start with ///, as on greet below.
// Braces delimit blocks; newlines separate statements. No semicolons.
// Names use snake_case for values and functions, PascalCase for types.

// ================================================================
// 2. Values, bindings, and operators
// ================================================================

test "numbers and bindings" {
    // Int is arbitrary precision. Arithmetic does not wrap at 64 bits.
    let huge = 9_223_372_036_854_775_807
    assert huge + 1 == 9_223_372_036_854_775_808
    assert 2 + 3 * 4 == 14
    assert (2 + 3) * 4 == 20

    // Integer division truncates toward zero.
    // Remainder has the dividend's sign.
    assert 7 / 3 == 2
    assert -7 / 3 == -2
    assert -7 % 3 == -1
    assert (-7).abs() == 7
    assert 3.min(8) == 3

    // let binds a name once; var allows rebinding a local name.
    let answer: Int = 42
    var count = 0
    count = count + 1
    assert count == 1
    assert answer == 42

    // Bool is a distinct type. Conditions must be Bool.
    assert true and not false
    assert 1 < 2 and 2 <= 3
    assert 1 != 2

    // and / or short-circuit: the division is never evaluated here.
    assert true or 1 / 0 == 0

    // Unit is both a type name and its sole value.
    let nothing: Unit = Unit
    assert nothing == Unit
}

// ================================================================
// 3. Text, bytes, and immutable lists
// ================================================================

test "text and bytes" {
    // Text literals use double quotes. ++ concatenates text.
    let name = "CPL"
    assert "Hello, " ++ name ++ "!" == "Hello, CPL!"
    assert "line one\nline two".len == 17
    assert "λ🙂".len == 2 // Unicode scalar values, not UTF-8 bytes.
    assert 42.to_text() == "42"
    assert trim("  hello  ") == "hello"
    assert split("a,b,c", ",") == ["a", "b", "c"]
    assert join(["a", "b", "c"], "/") == "a/b/c"

    // Bytes are built explicitly and decoded as UTF-8 with a Result.
    let encoded = bytes.from_text("λ")
    assert encoded.len == 2
    assert bytes.to_text(encoded) == Ok("λ")
    assert bytes.to_list(bytes.from_list([0, 127, 255])) == [0, 127, 255]
}

test "lists are values" {
    let xs: List<Int> = [10, 20, 30]
    assert xs.len == 3 // len is a property.
    assert not xs.is_empty() // is_empty is a method.
    assert xs.get(0) == Some(10) // Zero-based, safe access.
    assert xs.get(99) == None
    assert xs.first() == Some(10)
    assert xs.last() == Some(30)
    assert xs.contains(20)
    assert xs.take(2) == [10, 20]
    assert xs.drop(1) == [20, 30]
    assert xs.take(100) == xs // Nonnegative counts clamp to length.
    assert xs.drop_last() == [10, 20]
    assert xs.reverse() == [30, 20, 10]
    assert xs ++ [40] == [10, 20, 30, 40]
    assert flatten([[1, 2], [], [3]]) == [1, 2, 3]

    // push returns a value. Earlier snapshots keep their contents.
    var growing = xs
    growing = growing.push(40)
    assert growing == [10, 20, 30, 40]
    assert xs == [10, 20, 30]

    // An empty collection sometimes needs an element type.
    let empty: List<Text> = []
    assert empty.is_empty()

    // Equality compares contents, including nested collections.
    assert [[1], [2]] == [[1], [2]]
    assert [[1], [2]] != [[2], [1]]
}

// ================================================================
// 4. Functions, generics, and control flow
// ================================================================

/// Return a greeting. pub makes this declaration visible to imports.
@id("greet_01")
@intent("Greet a person by name")
@preserve("The greeting contains the name")
pub pure fn greet(name: Text) -> Text {
    // The last expression is the block's value.
    "Hello, " ++ name ++ "!"
}

// Function parameter and return types are explicit.
// A pure function can calculate with local vars; it cannot perform
// external effects or call a mutate function.
@id("absolute_01")
pure fn absolute(n: Int) -> Int {
    if n < 0 { -n } else { n } // if is an expression.
}

// Type arguments are usually inferred from the call.
pure fn duplicate<T>(value: T) -> List<T> {
    [value, value]
}

pure fn first_or_zero(items: List<Int>) -> Int {
    if items.is_empty() {
        return 0 // Explicit early return.
    }
    items.first().unwrap_or(0)
}

// Sequential repetition uses while. Loop contracts are optional.
@id("sum_below_01")
pure fn sum_below(n: Int) -> Int
    requires n >= 0
{
    var i = 0
    var total = 0
    while i < n
        invariant i >= 0 and i <= n
        decreases n - i
    {
        total = total + i
        i = i + 1
    }
    total
}

test "functions and loops" {
    assert greet("world") == "Hello, world!"
    assert absolute(-5) == 5
    assert duplicate("hi") == ["hi", "hi"]
    assert duplicate<Int>(3) == [3, 3]
    assert first_or_zero([]) == 0
    assert sum_below(10) == 45
}

// ================================================================
// 5. Records, enums, nominal types, and pattern matching
// ================================================================

struct Point {
    x: Int
    y: Int
}

// A nominal wrapper: a UserId is distinct from an ordinary Text.
type UserId(Text)

enum Connection {
    Offline
    Online(UserId)
}

pure fn describe(connection: Connection) -> Text {
    // match must cover every possibility. Patterns can nest.
    match connection {
        Offline => "offline"
        Online(UserId(name)) => "online: " ++ name
    }
}

test "records and variants" {
    let p = Point { x: 2, y: 3 }
    let moved = Point { ..p, x: 10 } // Copy with a field replaced.
    assert moved == Point { x: 10, y: 3 }
    assert p.x == 2
    assert describe(Offline) == "offline"
    assert describe(Online(UserId("ada"))) == "online: ada"
}

// ================================================================
// 6. Optional values and recoverable errors
// ================================================================

// Option<T> has Some(value) and None. There is no null.
pure fn optional_label(value: Option<Int>) -> Text {
    match value {
        Some(n) => n.to_text()
        None => "missing"
    }
}

// Result<T, E> has Ok(value) and Err(error).
// A postfix ? unwraps Ok or returns Err from the enclosing function.
// The propagated error type must match the function's error type.
pure fn parse_and_add_one(text: Text) -> Result<Int, ConstraintError> {
    let n = parse_int(text)?
    Ok(n + 1)
}

test "options and results" {
    let missing: Option<Int> = None
    assert missing.unwrap_or(99) == 99
    assert optional_label(Some(7)) == "7"
    assert optional_label(missing) == "missing"
    assert parse_and_add_one("41") == Ok(42)
    assert parse_and_add_one("forty-one").is_err()
}

// ================================================================
// 7. Contracts, refinements, and construction invariants
// ================================================================

// @prove asks the automatic verifier to prove every obligation of a
// function for all inputs that satisfy its requires clauses: here the
// division by b and the ensures clause. cpl check reports the verdict.
@prove
pure fn divide_exactly(a: Int, b: Int) -> Int
    requires b != 0
    requires a % b == 0
    ensures result * b == a
{
    a / b
}

// requires clauses run in order before the body. ensures can refer to
// the returned value as result. The default execution mode checks them.
// Violating a contract is an execution fault, not an Err value.

// The verifier decides integer and Bool arithmetic, branches, matches,
// records, enums, refinements, and calls it can follow.
@prove
pure fn clamp(x: Int, lo: Int, hi: Int) -> Int
    requires lo <= hi
    ensures result >= lo and result <= hi
{
    x.max(lo).min(hi)
}

// A loop is cut at its invariants. When they are too weak for a claim,
// the verifier looks for more before giving up: here total >= 0.
@prove
pure fn sum_to(n: Int) -> Int
    requires n >= 0
    ensures result >= 0
{
    var i = 0
    var total = 0
    while i < n
        invariant i >= 0 and i <= n
        decreases n - i
    {
        total = total + i
        i = i + 1
    }
    total
}

// A false claim fails with the inputs that break it. Uncomment this to see
// cpl check report the failure, with x = 11 as the counterexample:
//
// @prove
// pure fn next(x: Int) -> Int
//     ensures result > x
// {
//     if x > 10 { x } else { x + 1 }
// }
//
// A claim outside what the verifier decides, such as a property of text
// or of a list of unknown length, is unknown, with the reason. cpl prove
// lists every obligation; cpl check --release accepts only proved.

type Positive = Int where self > 0

pure fn parse_positive(text: Text) -> Result<Positive, ConstraintError> {
    let n = parse_int(text)?
    check<Positive>(n) // Validate a dynamic value explicitly.
}

struct Bounds {
    lo: Int
    hi: Int
}
    where self.lo <= self.hi

// Every Bounds construction must satisfy its invariant, including
// spread updates. Dynamic construction is checked at runtime.
pure fn shift(bounds: Bounds, delta: Int) -> Bounds {
    Bounds { lo: bounds.lo + delta, hi: bounds.hi + delta }
}

test "contracts and constraints" {
    assert divide_exactly(12, 3) == 4
    let two: Positive = 2 // This literal satisfies the refinement.
    assert two == 2
    assert parse_positive("7").is_ok()
    assert parse_positive("0").is_err()
    assert parse_positive("oops").is_err()
    assert shift(Bounds { lo: 1, hi: 4 }, 10) == Bounds { lo: 11, hi: 14 }

    // forall checks a predicate for every element of a finite list.
    assert (forall x in [1, 2, 3]: x > 0)
    let empty: List<Int> = []
    assert (forall x in empty: x > 0) // True for an empty list.
}

// ================================================================
// 8. Structured parallel computation
// ================================================================

pure fn squares_with_offset(items: List<Int>, offset: Int) -> List<Int> {
    parallel for item in items: item * item + offset
}

test "parallel results retain input order" {
    assert squares_with_offset([3, 1, 2], 7) == [16, 8, 11]
    assert squares_with_offset([], 7) == []
}

// The input is evaluated once; all tasks join before returning.
// Captured values such as offset are immutable snapshots.
// Task bodies cannot call capabilities or mutators, assign variables,
// or return/propagate out of the task. Use a pure helper for a local loop.
// Set CPL_PARALLELISM before launching the process to choose the worker
// limit. A value of 1 runs sequentially; small tasks may be faster that way.

// ================================================================
// 9. Capability interfaces and explicit effects
// ================================================================

// interface and capability are two spellings of the same declaration.
// The host supplies a handle implementing these operations.
interface Log {
    fn write(message: Text) -> Unit
}

mutate fn announce(name: Text, using log: Log) -> Unit
    effects { log.write }
{
    log.write(greet(name))
}

mutate fn welcome(name: Text, using log: Log) -> Unit
    effects { log.write }
{
    announce(name, using log) // Forward authority explicitly.
}

// An effects clause bounds the operations a function can perform.
// It does not grant a handle. These functions need a host binding to run.
// A plain fn is also supported: an empty effect bound makes it pure.

// concurrent runs independent tasks. Each line is a let or an expression;
// tasks cannot read each other's bindings, which appear after the block.
// Two tasks may not use one handle unless the capability declares both
// operations @read_only, so the result never depends on the schedule.
mutate fn welcome_both(a: Text, b: Text, using log: Log, using audit: Log) -> Unit
    effects { log.write, audit.write }
{
    concurrent {
        log.write(greet(a))
        audit.write(greet(b))
    }
}

// @read_only names operations that do not change what a handle answers,
// so tasks may call them on one handle at once. A put in one task and a
// get in another would be a concurrent_conflict.
@read_only("get")
interface Store {
    fn get(key: Text) -> Int
    fn put(key: Text, value: Int) -> Unit
}

mutate fn total_of(a: Text, b: Text, using store: Store) -> Int
    effects { store.get }
{
    concurrent {
        let x = store.get(a)
        let y = store.get(b)
    }
    x + y // The tasks' bindings are in scope after the block.
}

// ================================================================
// 10. Application roots, state machines, and commands
// ================================================================

program Counter {
    value: Int
}

enum CounterEvent {
    Increment
    Reset
}

enum CounterCommand {
    Report(Int)
}

struct Step<S, C> {
    state: S
    commands: List<C>
}

initial fn start() -> Counter {
    Counter { value: 0 }
}

// A program has one root machine. Machines and initializers are pure.
// Only that root machine and its initializers may construct the root.
machine update(state: Counter, event: CounterEvent) -> Step<Counter, CounterCommand> {
    match event {
        Increment => Step {
            state: Counter { ..state, value: state.value + 1 },
            commands: [Report(state.value + 1)],
        }
        Reset => Step { state: Counter { value: 0 }, commands: [] }
    }
}

test "a transition returns a new state and command values" {
    let before = start()
    let next = update(before, Increment)
    assert before.value == 0
    assert next.state.value == 1
    assert next.commands == [Report(1)]
    assert update(next.state, Reset).state.value == 0
}

// Calling update computes a candidate transition. A host MachineContext
// validates and commits it atomically, preserving the old state on faults.
// The host executes returned commands after commit; replay runs no effects.
// Child machines can operate on a subtree of the root's state.

// cpl explore runs every sequence of events from start() up to a depth,
// checks invariant predicates at each reachable state, and searches for
// goal states. Commands are data; exploration never executes them.
pure fn never_negative(state: Counter) -> Bool {
    state.value >= 0
}

pure fn reached_three(state: Counter) -> Bool {
    state.value == 3
}

// ================================================================
// 11. Protocols: states as types
// ================================================================

// A protocol gives each state of an interaction its own nominal type and
// each step its own function from one state to the next, so close_door on
// a ClosedDoor is a type error. The checker requires one initial state
// and a way to finish from every state.
@protocol("door", "closed", "initial_terminal")
type ClosedDoor(Text)

@protocol("door", "open", "state")
type OpenDoor(Text)

@transition("door", "closed", "open")
@message("Visitor", "Door")
pure fn open_door(door: ClosedDoor) -> OpenDoor {
    match door {
        ClosedDoor(name) => OpenDoor(name)
    }
}

@transition("door", "open", "closed")
@message("Visitor", "Door")
pure fn close_door(door: OpenDoor) -> ClosedDoor {
    match door {
        OpenDoor(name) => ClosedDoor(name)
    }
}

// States are affine: each value is used at most once. Reading door again
// after open_door consumed it is protocol_state_reused, so a stale state
// cannot be acted on. @role marks the functions of one participant: they
// may perform only the messages that role sends, and together all of them.
@role("door", "Visitor")
pure fn visit(door: ClosedDoor) -> ClosedDoor {
    let opened = open_door(door)
    close_door(opened)
}

test "a visit leaves the door closed" {
    assert visit(ClosedDoor("front")) == ClosedDoor("front")
}

// ================================================================
// 12. Versioned data and migrations
// ================================================================

// A stored value names its schema family and version. A new version is a
// new type, and a migration function connects two versions. Changes must
// keep every stored version available and reachable by migrations: a
// version edited in place is reported by verification.
@schema("note", "1")
struct NoteV1 {
    text: Text
}

@schema("note", "2")
struct NoteV2 {
    text: Text
    pinned: Bool
}

@migration("note", "1", "2")
pure fn upgrade_note(old: NoteV1) -> NoteV2 {
    NoteV2 { text: old.text, pinned: false }
}

test "migrating a note keeps its text" {
    assert upgrade_note(NoteV1 { text: "hi" }) == NoteV2 { text: "hi", pinned: false }
}

// ================================================================
// 13. Tests and generated properties
// ================================================================

// test blocks above use concrete inputs. A property draws generated
// inputs, checks assertions, and shrinks a failure to a smaller example.
// gen is in scope automatically. --seed makes a run reproducible.
property "reversing twice preserves the list"
    for items in gen.list(gen.int)
{
    assert items.reverse().reverse() == items
}

property "squares are nonnegative"
    for n in gen.range(-100, 100) // Both endpoints are included.
{
    assert n * n >= 0
}

// ================================================================
// 14. Stable identities, intent, and unfinished code
// ================================================================

// The @id on greet gives tools an identity that survives a rename or
// move. Keep an existing identity; cpl id assign can generate missing IDs.
// cpl inspect and cpl query expose signatures, contracts, effects,
// callers, tests, proof obligations, and more.

// @intent and @preserve on greet are documentation that tools keep with
// the declaration. A change that replaces greet's body must acknowledge
// the @preserve statement or revise it, and the change records which.

// A prefix ?name is a typed hole, distinct from the postfix Result ?.
// Uncomment this declaration to explore development-time hole reports:
//
// @id("unfinished_01")
// pure fn unfinished(items: List<Int>) -> Int {
//     ?total
// }
//
// Holes carry the expected type and visible bindings. They can check in
// development mode, fault if executed, and are rejected by release checks.
// cpl query holes lists each one as a structured request, and
// cpl edit fill-hole --symbol unfinished_01 --hole total --expression EXPR
// answers it with a checked edit.
```

Run the tour from the directory that contains `hello`:

```sh
cpl --project hello check
cpl --project hello run learn.greet '"world"'
cpl --project hello run learn.sum_below 10
cpl --project hello test --seed 1 --cases 100

# Every obligation of a @prove function, with the verifier's verdict.
cpl --project hello prove learn.divide_exactly
cpl --project hello prove learn.sum_to

# With a C compiler: compare the interpreter and native results.
cpl --project hello test --engine both --seed 1 --cases 100
CPL_PARALLELISM=4 cpl --project hello run learn.squares_with_offset '[3, 1, 2]' 7 --engine both

# Ask questions of the checked program.
cpl --project hello query 'callers of learn.greet' --json
cpl --project hello query 'functions with learn.Log.write effects' --json
cpl --project hello query 'protocol door' --json

# Every sequence of up to 20 events: the goal's shortest witness is
# three Increments, and the invariant holds at every state reached.
cpl --project hello explore learn.update --invariant learn.never_negative \
  --goal learn.reached_three --json

# Store a value with its schema version, then migrate it to version 2.
cpl --project hello schema learn.NoteV1 --value '{"$type":"learn.NoteV1","text":"hi"}' --json \
  | python3 -c 'import json, sys; print(json.dumps(json.load(sys.stdin)["stored"]))' > note-v1.json
cpl --project hello migrate --input note-v1.json --to 2 --json

# Format source, inspect a declaration, and pin checked inputs.
cpl --project hello fmt
cpl --project hello inspect learn.greet --include signature,contract,effects,docs --json
cpl --project hello check --lock
cpl --project hello check --release
```

## Change the program with checked edits

People and agents can change a program through semantic edits addressed
by stable identity. `cpl edit` prepares a change: it checks the whole
candidate program, runs the tests the change can affect, and saves a
reviewable change document. `cpl transaction` validates the document
again and installs it atomically; no partial edit reaches the files.

Replace the body of `absolute` (`@id("absolute_01")`) with the builtin,
and require that the new body behave exactly like the old one:

```sh
printf '{\n    n.abs()\n}\n' > absolute.body
cpl --project hello edit replace-body --symbol absolute_01 --input absolute.body --patch absolute.json
python3 -c 'import json; c = json.load(open("absolute.json")); c["contract"]["preserve_behavior"] = True; json.dump(c, open("absolute.json", "w"), indent=2)'
cpl --project hello transaction absolute.json --json      # the behavior obligation is proved
cpl --project hello transaction absolute.json --apply
```

`preserve_behavior` is decided by the verifier: both versions run
symbolically on every input the old one accepts, and a difference is
reported with inputs that show it, confirmed by running both versions.
A change contract can also require preserved facets, behavior on chosen
inputs, generated differential testing, and measured improvements.
Replace the loop of `sum_below` with a formula and ask for a better time
complexity:

```sh
printf '{\n    n * (n - 1) / 2\n}\n' > sum_below.body
cpl --project hello edit replace-body --symbol sum_below_01 --input sum_below.body --patch faster.json
python3 -c 'import json; c = json.load(open("faster.json")); c["contract"]["improve"] = ["time_complexity"]; c["contract"]["differential"] = [{"symbol": "sum_below_01", "cases": 200}]; json.dump(c, open("faster.json", "w"), indent=2)'
cpl --project hello transaction faster.json --json
```

The improvement is measured: both versions run on growing inputs while
the interpreter counts steps, and it is accepted when the new version is
never slower and its advantage grows with the input. The 200 generated
cases agree, which is test evidence, not a proof; inputs too large for
the loop to finish are counted as inconclusive.

A change to `greet`'s body must acknowledge its `@preserve` statement
(`--acknowledge 'The greeting contains the name'`), and its candidate must
still pass the affected tests. `cpl edit rename`, `replace-declaration`,
`remove`, and `fill-hole` work the same way, and `--append` stages several
edits into one transaction. An authority policy can limit the facets and
namespaces each actor may write and read; `cpl query 'writable by ACTOR'`
lists them.

In a Git repository, `cpl verify --changed --since REV` reruns only the
tests and proofs a change can affect and records checkable evidence.
`cpl merge` and `cpl conflicts` merge two revisions declaration by
declaration, following renames, and test the composition, which catches
semantic conflicts a text merge misses. `cpl transaction --publish` and
`cpl merge --publish` write commits that carry their evidence. `cpl
concurrent` plans effectful task scopes, runs them against fixture hosts,
and replays the recorded effects. The
[evolution walkthrough](../examples/evolution/README.md) runs all of these.

## Where to go next

The CLI can run a named function directly; a `main` function is unnecessary
for these examples. Add `--json` for structured results and diagnostics.
Native packages and Python bindings are configured in `cpl.toml` and built
with `cpl build --target c` or `cpl build --target python`.

Rust, Go, Java, and Kotlin call CPL without a foreign-function boundary.
List the host in `[interop] targets`, and `cpl build --target rust`, `go`,
or `java` writes the module as that language's source: a Cargo crate, a Go
module, or Java packages, which Kotlin uses through its Java interop. A CPL
function becomes a host function, a struct a struct or record, an enum a
Rust enum or a sealed interface, and a capability a trait or interface the
host implements; contracts are checked, and a fault is the host's error.
The [hosts example](../examples/hosts/README.md) calls one module from all
four.

```sh
cpl build --target rust      # .cpl/build/rust: depend on it by path
cpl build --target go        # .cpl/build/go: require it with a replace directive
cpl build --target java      # .cpl/build/java/src: compile with javac 17 or later
```

CPL is early software. Its current core types are `Int`, `Bool`, `Text`,
`Bytes`, `Unit`, `List`, `Option`, `Result`, and user-defined value types.
Node.js bindings and automatic work execution/integration remain future
work. `cpl prove` and `@prove` are backed by an automatic
[verifier](../spec/verification.md): it decides contracts, loop invariants,
measures, divisions, and call preconditions over integer and boolean
arithmetic, data types, and calls it can follow, infers loop invariants
when the declared ones are too weak, and reports `proved`, `failed` with
the inputs that break an obligation, or `unknown` with the reason outside
that fragment. An `unknown` result is not a proof; runtime contract checks
and passing tests are separate evidence.

The [program-evolution amendment](../spec/evolution.md) specifies the edits,
change contracts, authority policies, merges, evidence, queries,
exploration, protocols, and migrations used above, and
[`concurrent` blocks](../spec/concurrency.md) the effect footprints that
let independent tasks run at once. The
[evolution walkthrough](../examples/evolution/README.md) is executable on
both language engines; effectful task scheduling currently uses the
interpreter.

Continue with the [standard library](../spec/stdlib.md),
[language specifications](../spec/README.md),
[state-machine example and replay walkthrough](../examples/state_machine/README.md),
[parallel examples and benchmarks](../examples/parallel/README.md),
[C interop](../spec/cabi.md), [Python interop](../spec/python.md), and
[Rust, Go, Java, and Kotlin hosts](../spec/hosts.md).
