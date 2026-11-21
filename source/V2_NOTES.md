# J2 v2 development notes

v1 is the published `0.1.0` release. v2 is `0.2.0-dev`, developed on the `v2`
branch. The v1 downloads have not been replaced, and no v2 installer is available
yet. Build the development driver to try the changes below.

v2 expands collection and indexed-loop parallelism and adds two syntax shortcuts.
Existing syntax and the MIR parallelizer remain available.

| Area | v1 (`0.1.0`) | v2 (`0.2.0-dev`) |
| --- | --- | --- |
| Parallel work | Supported loop patterns, numeric kernels, and independent native calls | Adds more maps, filters, pipes, and independent indexed writes |
| Callbacks | Explicit functions and existing condition syntax | Adds `_` shorthand, with more callback bodies eligible for parallel execution |
| Array captures | Existing execution paths | Read-only numeric array captures can use the new parallel helpers |
| Syntax | Existing literals, calls, and parameters | Also accepts trailing commas |
| Availability | Published macOS Apple Silicon downloads | Development source; build locally |

## More automatic parallelism

Native `map`, `filter`, mapping pipes (`>>`), and condition filters (`?`) can use
ordered, scoped worker threads for numeric sequences. The compiler accepts
supported native scalar functions, numeric lambdas, scalar captures, and read-only
numeric array captures. Callbacks can also contain local bindings, checked local
updates, branches, and early `give` returns. Eligible recursive scalar functions
can be used as map callbacks.

```j2
values = collect(1..40000)
factor = 3
scaled = map(values, _ * factor)
evens = filter(values, _ % 2 == 0)
squares = values >> (_ * _)

func twice(x: int) -> int = x * 2
result = map(values, twice)
```

Collection dispatch uses stable Rust and works with both the forked backend and
its stock-backend fallback. It is enabled by default for native builds;
`J2_PARALLEL=0` disables it. The existing MIR transformations still use the fork. Inputs below 32,768 elements stay serial. Larger
inputs use at most eight workers, capped by available CPU parallelism. Output
order is preserved; workers finish before a result or error is returned. Nested
operations stay on their existing worker to avoid spawning more thread groups.
Heavy callbacks benefit most; cheap callbacks can cost more to dispatch than to
run serially. `J2_PARALLEL=0` remains available for such workloads.

Writes to captured collections, class captures, I/O, and unknown function values
are excluded from this transform. Unsupported callbacks retain the dynamic execution path.
Lambda captures keep their existing snapshot semantics. Top-level transform
outlines and function boundaries fall back to dynamic execution when numeric
sequence conversion would change element types or precision.

Read-only captures support gather operations and conditional lookups:

```j2
indices = collect(0..39999)
result = map(indices, func(i) = {
    value = values[i]
    if value > 100 { give value * 2 }
    value + 1
})
```

Independent indexed loops can also use workers. Supported bodies write one
destination at the current index, read other arrays, use scalar local bindings,
and contain filters or branches:

```j2
output := make_seq(40000, 0)
for i in 0..len(output)-1 if values[i] > 10 {
    adjusted = values[i] + 3
    output[i] = adjusted * adjusted + output[i]
}
```

The compiler rejects loop-carried dependencies such as `output[i - 1]`, writes
to multiple arrays, and observable callback effects. Aliased input/output arrays
use the original serial implementation. Descending or out-of-bounds ranges use
a checked serial path. Existing simple float scale/affine loops retain their MIR
optimization path. The general indexed helper stages results before committing
them in source order; if an iteration fails, only the successful prefix is written
back, and the original J2 error is propagated. This staging uses additional memory
proportional to the range length.

```sh
j2 build program.j2 -o program                 # native, automatic parallelism
J2_PARALLEL=0 j2 build program.j2 -o serial     # native, serial
J2_NO_NATIVE=1 J2_PARALLEL=0 j2 program.j2      # dynamic compiled reference
```

Use `collect(range)` when the same values will be read repeatedly: compiled
ranges are consumable flows, while a collected sequence can be reused.

## Less callback syntax

In the second argument of `map` or `filter`, `_` names the current element:

```j2
map(values, _ + 1)             # func(x) = x + 1
filter(values, _ > 10)         # func(x) = x > 10
values >> (_ * 2) >> (_ + 1)
```

These forms become ordinary one-argument lambdas, including lexical captures.
Existing `func(...) = ...` callbacks and `values ? (_ > 10)` syntax continue to
work. This shorthand does not introduce implicit arguments into arbitrary calls.

Trailing commas are accepted in sequence/map/pair literals, positional/named
calls, and function/lambda/method parameter lists:

```j2
items = [1, 2, 3,]
settings = {limit: 10, enabled: true,}
func add(a, b,) = a + b
answer = add(2, 3,)
map(items, func(x,) = x + answer,)
```

## Faster repeated native builds

Serial native builds now reuse a persistent Cargo dependency directory, as
parallel builds already did. Editing a program recompiles the program while
Cargo reuses unchanged runtime/dependency artifacts. An explicit
`CARGO_TARGET_DIR` is honored for serial builds; relative paths resolve from the
directory where `j2` was invoked. Cargo validates compiler and source fingerprints
before reusing artifacts.

Concurrent CLI runs use separate temporary executables, and cache entries are
published atomically. Cache identity also distinguishes driver builds, so two
compiler versions cannot overwrite or reuse each other's generated code.

## Correctness fixes

- Large integer sums preserve checked sequential overflow behavior, including
  overflow followed by cancellation. Float sums retain source addition order.
- Inline native callback arithmetic preserves checked integer addition/
  subtraction/multiplication and Euclidean integer remainder. Value-dependent
  operations that cannot preserve the dynamic result type retain the dynamic path.
- Native rounding uses the runtime's integer result and ties-to-even rule;
  integer `abs` preserves integer precision. Checked callback failures retain
  their J2 error types, and generated callback locals avoid capture-name collisions.
- Indexed-loop failures preserve completed writes, including through native
  function boundaries. Sequence aliases and element-type mismatches fall back
  before native calls; functions returning an input collection retain its alias.
- Callback block return inference checks early returns in every branch. Builtin
  shadowing and conditional binding scopes retain their dynamic semantics.
- Integer endpoint negation/division report overflow instead of panicking or
  wrapping; modulo handles `i64::MIN % -1`, and integer powers retain all exponent bits.
- Interpreter ranges handle descending ranges and terminal `i64` endpoints
  without incrementing past the endpoint. Consumed flows now use an explicitly
  empty sentinel, so reading them again cannot produce a spurious `[0, -1]`.
- Binary cache identity includes transitive import contents and the entry path.
  Edited or deleted imports cannot silently reuse an old binary.
- `J2_PAR_DUMP=1 j2 build program.j2 -o program` bypasses the binary cache and
  displays successful fork-compiler diagnostics for inspecting MIR matches.
- The differential harness checks process failures in all four execution modes,
  keeps failure diagnostics, and rejects missing tests. Signal-terminated
  programs report failure. `J2_NO_NATIVE=1` also disables top-level native outlines.

## Verification commands

```sh
cargo test --manifest-path compiler/j2_compiler/Cargo.toml --offline
cargo test --manifest-path src/bin/j2/Cargo.toml -p j2_runtime --offline
cargo test --manifest-path src/bin/j2/Cargo.toml --offline
cargo build --release --manifest-path src/bin/j2/Cargo.toml --offline
./j2-tests/diff_harness.sh v2_syntax v2_parallel v2_errors v2_shadowing
./j2-tests/diff_harness.sh v2_capture_parallel v2_loop_parallel
./j2-tests/cache_regression.sh
./j2-tests/harness_selftest.sh
./j2-tests/ci.sh
```

The differential harness compares compiled dynamic, native serial, native
parallel, and interpreter output. Its interpreter run may use the normal compile
fallback for language constructs the interpreter does not support. These commands
are the verification procedure, not a claim that every platform or the complete
upstream Rust test suite has been validated.

## Verified in this workspace

On macOS arm64, the completed checks passed: 86 focused compiler/runtime/driver
tests; 42 deterministic programs compared across four execution modes; all eight
original reference checksums; and a 124-file parse/lower sweep. Cache invalidation,
concurrent-run, atomic-wrapper-publication, and harness-failure regressions also
passed. The new collection and indexed-loop fixtures passed on stock Rust, and
the loop fixture passed on the forked backend without serial fallback. Aliasing,
numeric representation, error-prefix, callback-scope, and generated-name cases
are covered by dedicated regressions.

The development driver was rebuilt at `src/bin/j2/target/release/j2`. The installed
system command and release bundles have not been replaced.
