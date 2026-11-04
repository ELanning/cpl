# Checked program evolution

From this directory, with `cpl` installed:

```sh
cpl check
cpl test --engine both --seed 1 --cases 100
cpl query 'symbol fn_price' --json
cpl query 'callers of demo.price' --json
cpl query 'functions with demo.Store.read effects' --json
cpl query 'protocols' --json
cpl query 'protocol connection' --json
cpl query 'tests of fn_price' --json
cpl query 'unproven contracts' --json
cpl query 'writable by optimizer' --policy authority.json --json
```

`writable by` applies the policy's grants, forbids, and regions to every
declaration: here the optimizer may write only the body of `price`.

Prepare a semantic edit, with authority limited to the implementation of
`price`. Preparation checks the complete candidate and runs affected tests
and properties. It leaves source files alone and saves a transaction with
the expected body hash. Because `price` carries `@preserve`, replacing its
body requires acknowledging that statement (or revising it in the same
change); the report records the acknowledgement as the actor's attestation:

```sh
cpl edit replace-body --symbol fn_price --input price.body \
  --acknowledge 'Nonnegative quantities retain their price' \
  --actor optimizer --policy authority.json --patch /tmp/price-change.json --json
cpl transaction /tmp/price-change.json --actor optimizer --policy authority.json --apply --json
```

Add `--append` to stage further edits into the same `--patch` file; every
staged edit is validated together and later applied as one transaction.
`cpl edit fill-hole --symbol ID --hole NAME --expression EXPR` answers a
typed hole that `cpl query holes` reports, and `cpl edit remove --symbol
ID` deletes a declaration that nothing references.

The body edit can still apply after a rename of `price`, since it addresses
`fn_price` and checks only the body hash. A concurrent body edit is rejected.
`cpl edit rename --symbol fn_price --name calculate_price --apply` performs
a checked rename of the declaration, resolved references, and imports.

Add a `contract` object to the transaction to require preserved facets,
sampled behavior, or generated differential testing of the old and new
function. Inputs that violate `price`'s `requires` are discarded; a
disagreement is shrunk to a small counterexample. Agreement remains test
evidence:

```json
{
  "preserve": [{"symbol": "fn_price", "facets": ["signature", "contract", "effects", "docs"]}],
  "behavior_samples": [{"symbol": "fn_price", "arguments": [[0], [1], [100]]}],
  "differential": [{"symbol": "fn_price", "cases": 500}],
  "acknowledge": [{"symbol": "fn_price", "statement": "Nonnegative quantities retain their price"}]
}
```

`"preserve_behavior": true` requires universal equivalence, which the
verifier decides symbolically (`spec/verification.md`): the new body
`10 * quantity` is proved to agree with `quantity * 10` on every quantity
that satisfies `requires quantity >= 0`. A body that differs is rejected
with an input that shows the difference, confirmed by running both
versions; one the prover cannot decide stays `unknown` and is not accepted,
so test results are never promoted to proofs. `"improve":
["time_complexity"]` is measured instead: both versions run on inputs of
growing size, and it is accepted only when the new version is never slower
and its advantage grows with the input.

Inside a Git checkout, publish a validated immutable candidate instead of
updating source files:

```sh
cpl transaction /tmp/price-change.json --actor optimizer --policy authority.json \
  --publish refs/cpl/changes/price --json
cpl verify --changed --since HEAD --receipt /tmp/evidence.json --json
cpl verify --changed --since HEAD --evidence /tmp/evidence.json --json
```

Use the publication command before applying the patch, or prepare a fresh
patch afterward. Published commits carry the request and verification
evidence in `.cpl/evidence/`. The current branch and index stay in place.
`verify --changed` also reports persisted-schema incompatibilities with
the baseline, such as an in-place edit of `UserV1`.

Two agents can work from the same commit and publish independently: one
renames `price`, the other replaces its body. A text merge conflicts when
such edits touch adjacent lines, and silently breaks any caller one side
adds under the name the other side renamed. A semantic merge combines the
facets by stable identity, renames references on both sides, checks the
composition, and runs every affected test:

```sh
cpl edit rename --symbol fn_price --name calculate_price --patch /tmp/rename.json --json
cpl transaction /tmp/rename.json --publish refs/cpl/changes/rename --json
cpl transaction /tmp/price-change.json --actor optimizer --policy authority.json \
  --publish refs/cpl/changes/body --json
cpl conflicts refs/cpl/changes/rename refs/cpl/changes/body --json
cpl merge refs/cpl/changes/rename refs/cpl/changes/body \
  --publish refs/cpl/changes/merged --json
```

The merge base defaults to the revisions' Git merge base. `worktree` names
the current source; `cpl merge worktree OTHER --apply` installs an accepted
merge over the checkout. A test that passes on each side but fails after
composition is a `regression`, a semantic conflict no text merge reports;
the same facet changed differently on both sides is a structural
`conflict`.

Explore every transition reachable from this initial state over a finite
event alphabet. The counter saturates, so this search finishes exhaustively
after visiting four states:

```sh
cpl explore demo.update_counter \
  --state '{"$type":"demo.Counter","value":0}' \
  --events '[{"$type":"demo.CounterEvent","$variant":"Increment"},{"$type":"demo.CounterEvent","$variant":"Reset"}]' \
  --depth 100 --json
cpl explore demo.update_counter --goal demo.counter_full --json
```

Without `--state`, the program's `initial fn` supplies the initial state;
without `--events`, the alphabet is every value of the finite event type.
The goal query returns a shortest witness: three `Increment` events.
`--invariant FN` checks a pure predicate at every reachable state.
Changing the increment to `state.value + 1` produces a shortest failing trace
of four `Increment` events. Returned commands are never executed by exploration.

Pack data with its schema version and fingerprint, then migrate it. The
CLI envelopes contain a `stored` field; migration consumes that field:

```sh
cpl schema demo.UserV1 --value '{"$type":"demo.UserV1","name":"Ada"}' --json \
  | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["stored"]))' > /tmp/user-v1.json
cpl migrate demo.upgrade_user --input /tmp/user-v1.json --json
cpl migrate --input /tmp/user-v1.json --to 2 --json
```

`--to` follows the shortest chain of declared migrations, validating each
intermediate value.

Plan two effectful tasks, execute against explicit fixture hosts, and replay
without invoking either host:

```sh
cpl concurrent scope.json --json
cpl concurrent scope.json --responses responses.json --receipt /tmp/effects.json --json
cpl concurrent scope.json --replay /tmp/effects.json --workers 1 --json
```

Production hosts implement `cpl_evolve::concurrent::EffectHost`. Resource
aliases must share a canonical identity. Unclassified operations are
exclusive; a conflicting pair requires an explicit `after` dependency.
The fixture CLI is useful for tests; it does not contact databases or networks.

The complete formats, guarantees, and limits are in
[the evolution amendment](../../spec/evolution.md).
