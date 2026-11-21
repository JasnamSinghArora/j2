use j2_compiler::compile_to_rust;

fn emit(source: &str) -> String {
    compile_to_rust(source).expect("valid J2 source")
}

#[test]
fn indexed_updates_accept_scalar_calls_locals_and_readonly_lookups() {
    let code = emit("func score(x: int, bias: int) -> int = x * x + bias\nfunc update(dst: seq<int>, src: seq<int>, lookup: seq<int>, bias: int) -> int = {\n for i in 0..len(dst)-1 { value = score(src[i], bias)\n dst[i] = dst[i] + value + lookup[src[i]] }\n 0 }");
    assert!(code.contains("fn update__native"));
    assert!(code.contains("parallel::update_native"));
    assert!(code.contains("checked_add"));
    assert!(code.contains("score__native"));
}

#[test]
fn filters_and_conditional_writes_use_ordered_updates() {
    let code = emit("func update(dst: seq<int>, src: seq<int>, limit: int) -> int = {\n for i in 0..len(dst)-1 if src[i] > limit {\n if dst[i] > 0 { dst[i] = src[i] + dst[i] } else { dst[i] = src[i] - 1 }\n }\n 0 }");
    assert!(code.contains("fn update__native"));
    assert!(code.contains("parallel::update_native"));
    assert!(code.contains("__j2_loop_old_"));
}

#[test]
fn signed_bounds_keep_guarded_serial_descending_and_invalid_index_paths() {
    let code = emit("func update(dst: seq<int>, start: int, end: int) -> int = {\n for i in start..end if i >= 0 { dst[i] = i + 1 }\n 0 }");
    assert!(code.contains("parallel::update_native"));
    assert!(code.contains(").rev()"));
    assert!(code.contains("negative indices are not supported"));
    assert!(code.contains("out of range for seq of length"));
}

#[test]
fn loop_carried_state_effects_and_nonterminal_writes_stay_serial() {
    for body in [
        "dst[i] = dst[i - 1] + 1",
        "dst[i + 1] = src[i]",
        "dst[i] = src[i]\n dst[i] = dst[i] + 1",
        "dst[i] = src[i]\n x = src[i + 1]",
        "dst[i] = src[i]\n src[i] = 0",
        "print(i)\n dst[i] = src[i]",
        "total += src[i]\n dst[i] = total",
        "stop if i > 2\n dst[i] = src[i]",
        "if i > 0 { dst[i] = src[i] }\n total += 1",
    ] {
        let code = emit(&format!("func update(dst: seq<int>, src: seq<int>) -> int = {{ total := 0\n for i in 1..len(dst)-1 {{ {body} }}\n total }}"));
        assert!(!code.contains("parallel::update_native"), "{body}");
    }
}

#[test]
fn collection_calls_and_shadowed_functions_are_not_assumed_pure() {
    for source in [
        "func read(xs: seq<int>, i: int) -> int = xs[i]\nfunc update(dst: seq<int>, src: seq<int>) -> int = { for i in 0..len(dst)-1 { dst[i] = read(src, i) }\n 0 }",
        "func update(dst: seq<int>, abs: int) -> int = { for i in 0..len(dst)-1 { dst[i] = abs(dst[i]) }\n 0 }",
        "func update(dst: seq<int>) -> int = { for i in 0..len(dst)-1 until i > 5 { dst[i] = i + 2 }\n 0 }",
    ] {
        assert!(!emit(source).contains("parallel::update_native"), "{source}");
    }
}

#[test]
fn filtered_top_level_loop_captures_predicate_only_variables() {
    let code = emit("dst := make_seq(40000, 0)\nlimit = 50\nfor i in 0..len(dst)-1 if i > limit { dst[i] = i + 1 }");
    assert!(code.contains("fn __toploop_2"));
    assert!(code.contains("parallel::update_native"));
}

#[test]
fn worker_names_do_not_shadow_source_locals() {
    let code = emit("func update(dst: seq<int>, __j2_loop_old_2: int) -> int = {\n for i in 0..len(dst)-1 { __j2_loop_value_4 = i + 3\n dst[i] = dst[i] + __j2_loop_old_2 + __j2_loop_value_4 }\n 0 }");
    assert!(code.contains("parallel::update_native"));
    assert!(code.contains("__j2_loop_old_2_"));
    assert!(code.contains("__j2_loop_value_4_"));
}

#[test]
fn existing_simple_float_scale_and_affine_kernels_keep_mir_paths() {
    for rhs in ["dst[i] * factor", "dst[i] * factor + bias", "dst[i] + bias"] {
        let code = emit(&format!("func update(dst: seq<float>, factor: float, bias: float) -> int = {{\n for i in 0..len(dst)-1 {{ dst[i] = {rhs} }}\n 0 }}"));
        assert!(code.contains("fn update__native"));
        assert!(!code.contains("parallel::update_native"), "{rhs}");
    }
}

#[test]
fn top_level_float_inputs_require_exact_elements_before_unboxing() {
    let code = emit("src = [9007199254740993, 0.5]\ndst := [0.0, 0.0]\nfor i in 0..1 if true { dst[i] = src[i] }");
    assert!(code.contains("parallel::update_native"));
    assert!(code.contains("__unbox_exact_seq_f64(&__get(env, \"src\")?)?"));
    assert!(code.contains("__unbox_exact_seq_f64(&__get(env, \"dst\")?)?"));
}

#[test]
fn newly_admitted_mixed_assignment_types_stay_dynamic() {
    let code = emit("func update(dst: seq<float>) -> int = { for i in 0..len(dst)-1 if true { dst[i] = i }\n 0 }");
    assert!(!code.contains("parallel::update_native"));
}

#[test]
fn stale_integer_inference_checks_runtime_elements_at_loop_and_map_boundaries() {
    let code = emit("src := [1, 2, 3]\nsrc[1] = 2.5\ndst := [0, 0, 0]\nfor i in 0..2 if true { dst[i] = src[i] + 1 }\nmapped = map(src, _ + 1)");
    assert!(code.contains("parallel::update_native"));
    assert!(code.matches("__unbox_exact_seq_i64(&__get(env, \"src\")?)?").count() >= 2);
}
