# Compiler tests with no J2 equivalent

These tests exercise compiler internals (MIR matchers, unsafe semantics,
fn-pointer unwinding) rather than J2 language semantics, so they have no J2
version.

| Test | Why |
|---|---|
| `test_invoke_e2e.rs` | Checks the `parallel_invoke_2` MIR matcher rewrites a two-call chain. J2 parallelism comes from runtime calls, not MIR rewriting. |
| `test_invoke_alloc_stress.rs` | Stresses `parallel_invoke` under heavy allocation. The helper takes `Send` closures and is only callable from compiler-side code. |
| `test_invoke_n_way.rs` | Scans the 2-way through 8-way invoke matchers. MIR-level only. |
| `test_invoke_panic_n.rs` | Unwind safety inside `parallel_invoke_N`. J2 does not expose `catch_unwind`; panics become `RuntimeError`. |
| `test_invoke_marg.rs`, `test_invoke_marg_big.rs` | The M-argument N-way invoke matcher (`_fn0..fn20`). No user-level analog. |
| `test_purity_unsafe.rs` | Uses `unsafe` blocks and raw pointer casts. J2 has neither; every J2 function is pure under the purity rules. |

J2 versions would pass unconditionally, so they are left out.
