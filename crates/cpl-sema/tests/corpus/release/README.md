# Release-mode cases

Every directory here is a complete project (`cpl.toml` + `src/`) that the
harness must check with `CheckConfig { release: true, .. }` (toolchain 1.6,
7.2 `cpl check --release`), i.e. `check_with(&project, &config)` rather than
`check(&project)`. Markers follow toolchain 12.2 exactly as in `errors/`.

| Directory | Expects |
| - | - |
| `hole_in_release/` | `hole_in_release` (error) on each `fn`-body hole, in place of `hole` (semantics S64, D-231); `hole` (warning) on each test/property-body hole; `project_lock_missing` (warning, span-less) |
| `verification_unsatisfied/` | `verification_unsatisfied` (error) and `verification_result` (info) for the one `@prove` function, both spanning the function's name (semantics S63, D-230); `project_lock_missing` |
| `clean/` | only `project_lock_missing`; the check succeeds |

Conventions these cases rely on (also listed in `../README.md`):

- No case carries a `cpl.lock`; a valid lock needs digests only the
  toolchain can compute, so `project_lock_missing` is expected everywhere
  and `project_lock_stale` / `project_lock_toolchain_mismatch` are not
  covered.
- `verification_result` and `verification_unsatisfied` are declaration-level
  diagnostics and span the declaration's name token (semantics S63), so
  their markers are on the `fn` line (one trailing, one in the `@+N` form).
- Under `--release` a `fn`-body hole is reported once, as `hole_in_release`
  *instead of* `hole` (semantics S64): no `expect-info: hole` accompanies
  it. A test-body hole stays `hole`, raised to a warning.
