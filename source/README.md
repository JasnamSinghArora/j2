# J2 compiler source

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

The frontend, runtime and driver build on their own with `cargo build`.
The passes, the std file and the patch apply onto the backend compiler
tree; the patch header lists where each file goes.
