# J2 compiler source

This is the v2 development source (`0.2.0-dev`) on the `v2` branch.
The published v1 release is `0.1.0`; its source remains on `main` and its
[downloads](https://github.com/JasnamSinghArora/j2/releases/tag/v0.1.0) remain
available. There is no v2 installer yet.

Read the [v2 development notes](V2_NOTES.md) for features, limitations, and
completed validation, or the [version guide](https://j2-lang.org/docs/versions.html)
for a comparison with v1.

## Build the driver

Run these commands from this `source/` directory with Rust and Cargo installed:

```sh
cargo build --release --manifest-path src/bin/j2/Cargo.toml
./src/bin/j2/target/release/j2 --version
./src/bin/j2/target/release/j2 j2-examples/hello.j2
./src/bin/j2/target/release/j2 build j2-examples/hello.j2 -o hello
./hello
```

The driver reports `0.2.0-dev`. These commands build and use a local executable;
they do not replace an installed v1 command. Cargo downloads dependencies on
the first build unless they are already cached.

The new collection and indexed-loop parallel helpers work with stock Rust.
The existing MIR parallelization passes require the J2 fork of the backend
compiler. To use an existing fork build, set `J2_BACKEND` to its `rustc` binary.
The files in `compiler/j2_passes/`, `library/j2_std/`, and `patches/` belong in
that backend tree; the patch header lists where they go. Building the driver
alone does not build the fork.

For a serial native build, set `J2_PARALLEL=0`:

```sh
J2_PARALLEL=0 ./src/bin/j2/target/release/j2 build j2-examples/hello.j2 -o hello-serial
```

## Test

```sh
cargo test --manifest-path compiler/j2_compiler/Cargo.toml
cargo test --manifest-path src/bin/j2/Cargo.toml -p j2_runtime
cargo test --manifest-path src/bin/j2/Cargo.toml
./j2-tests/diff_harness.sh v2_syntax v2_parallel v2_errors v2_shadowing
./j2-tests/diff_harness.sh v2_capture_parallel v2_loop_parallel
```

The differential tests use the release driver built above. More checks and
the results from the development workspace are recorded in [V2_NOTES.md](V2_NOTES.md).

## Layout

| Path | Contents |
|---|---|
| `compiler/j2_compiler/` | Frontend: lexer, parser, type checker, lowering to native source |
| `compiler/j2_passes/` | Auto-parallelization MIR passes and control-flow analysis |
| `library/j2_runtime/` | Runtime library for the interpreter and native builds |
| `library/j2_std/` | Parallel runtime added to the standard library |
| `src/bin/j2/` | The `j2` driver and interpreter |
| `patches/` | Hooks that register the passes, flag and attributes in the backend compiler |
| `j2-tests/`, `j2-examples/` | J2 test programs and examples |
| `test_*.rs`, `bench_*.rs`, `traffic_system_rs/` | Matcher tests and benchmarks |
| `deploy/` | Bundle, package, sign and release scripts |
| `editors/vscode/` | VS Code extension |

J2 uses the [MIT license](../LICENSE-MIT). Third-party components retain their
own licenses; see [THIRD-PARTY-NOTICES.md](../THIRD-PARTY-NOTICES.md).
