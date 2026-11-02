# Structured parallel computation

`compute.batches` computes independent inputs concurrently and returns
results in input order. `compute.captured` captures an immutable offset;
`compute.nested` demonstrates nested scopes. The other functions exercise
ownership, arbitrary-precision arithmetic, and failure behavior.

From the repository root:

```sh
cargo run -p cpl-cli -- --project examples/parallel check
CPL_PARALLELISM=4 cargo run -p cpl-cli -- --project examples/parallel \
  run compute.batches '[100000, 200000, 300000, 400000]' --engine both --json
cargo run -p cpl-cli -- --project examples/parallel test --engine both
```

Use `CPL_PARALLELISM=1` to compare sequential execution of the same source.
The setting caps additional worker creation, and nested scopes reuse the
existing worker. Every task joins before the expression returns. Return
commands from reducers and let the host execute their effects.

Run the reproducible benchmark with a release toolchain build:

```sh
CPL_PARALLELISM=4 cargo run --release --locked -p cpl-cabi --example performance
CPL_PARALLELISM=1 cargo run --release --locked -p cpl-cabi --example performance
```

It checks both engines' results, warms the native driver, reports five
samples and their median for each workload, separates compiler/startup
cost, and checks that the native runtime releases every object and handle.
Run it without other CPU-heavy jobs. The native timings include its driver
protocol and result transfer, so large returned lists have a transfer cost.
See the [language amendment](../../spec/parallelism.md) for purity,
cancellation, and bounded application mailboxes.
