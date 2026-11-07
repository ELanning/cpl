# A scoped application machine

`src/app/main.cpl` keeps settings and a download state under one `App`
root. `download` is a child reducer. `reduce` installs its candidate
state in a new root and returns command values. `perform` is the only
function that performs I/O; a host supplies its `Http` interface.

`fetch` is where the state machine of one download lives. Its `context
app = start()` statement starts the call stack's only state machine;
`fetch` dispatches `Start`, performs each command after its transition
commits, and dispatches the resulting event, until no commands remain.
`outcome` receives the same context as `using app: Context<App>` and reads
its state. No function that `fetch` calls can start a second state
machine, and only `reduce` calls `download`. A host calls `fetch` with
its `Http` implementation (`cpl build --target rust|go|java|c|python`).

From the repository root:

```sh
cargo run -p cpl-cli -- --project examples/state_machine check
cargo run -p cpl-cli -- --project examples/state_machine run app.main.start --json
cargo run -p cpl-cli -- --project examples/state_machine replay app.main.reduce \
  --state '{"$type":"app.main.App","settings":{"$type":"app.main.Settings","retries":3},"download":{"$type":"app.main.Download","$variant":"Ready"}}' \
  --events '[{"$type":"app.main.Event","$variant":"Start","$payload":[7,"/example"]},{"$type":"app.main.Event","$variant":"Received","$payload":[7,"hello"]}]' \
  --engine both --json
```

The first transition emits `HttpGet(7, "/example")`. The second records
the reply. Replay performs no HTTP request. An event with a different
request ID leaves the pending download unchanged. The host must allocate
IDs that distinguish outstanding and stale requests.

Pass the output's `records` array back with `--records` and the same
initial `--state` to verify the history. Changing a receipt's result,
commands, sequence, or program digest causes verification to stop before
that transition commits.

The example is intentionally an in-memory protocol. Its host owns
persistence, authenticated input, command delivery, and retry policy.
See [the language amendment](../../spec/state-machines.md) for the full
contract and [the audit report](../../spec/state-machines-audit.md) for
validation results.
