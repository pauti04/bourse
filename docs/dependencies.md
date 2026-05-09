# Dependencies

Every dependency added to a member crate must be justified here. Default to
zero-dep where possible; the std library plus a tiny set of well-vetted
crates is the goal.

## Workspace dependencies

### `thiserror = "2"`

- **Used by:** `matchx-core`.
- **Purpose:** Ergonomic typed-error derivation. Replaces hand-rolled
  `impl std::error::Error for Error` boilerplate.
- **Runtime cost:** Zero. `thiserror` is purely a `proc-macro` crate that
  expands to plain `impl` blocks; nothing of it ships in the binary.
- **Alternatives considered:** Hand-rolled `impl Error` — rejected for
  boilerplate burden in a project with many error variants.

### `proptest = "1"` (dev-dependency)

- **Used by:** `matchx-core` tests.
- **Purpose:** Property-based testing with shrinking. The charter mandates
  property tests for the order-book and replay invariants; `proptest`
  shrinks counterexamples to minimal failing cases, which is essential for
  diagnosing concurrency or arithmetic bugs.
- **Runtime cost:** Test-only — never linked into a release binary.
- **Alternatives considered:** `quickcheck` — rejected; `proptest` has
  strictly better shrinking and a more active maintainer base.

## Per-crate dependencies

None yet beyond the workspace-shared ones above. Add entries here as new
deps land, with the same fields: **used by**, **purpose**, **runtime cost**,
**alternatives considered**.
