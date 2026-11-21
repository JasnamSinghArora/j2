use j2_compiler::compile_to_rust;

fn emit(source: &str) -> String { compile_to_rust(source).expect("valid source") }

#[test]
fn readonly_array_captures_use_borrowed_views_and_checked_signed_indices() {
    let code = emit("func gather(xs: seq<int>, ids: seq<int>) -> seq<int> = map(ids, func(i) = xs[len(xs) - i - 1])");
    assert!(code.contains("fn gather__native"));
    assert!(code.contains("parallel::map_native"));
    assert!(code.contains("let xs = &xs[..];"));
    assert!(code.contains("checked_sub"));
    assert!(code.contains("__seq.get(__index as usize)"));
    assert!(code.contains("J2Err::index"));
}

#[test]
fn block_callbacks_support_local_mutation_branches_and_early_returns() {
    let code = emit("func f(xs: seq<int>) -> seq<int> = map(xs, func(x) = { y := x\n y += 1\n if y > 5 { give y }\n x++\n x * 2 })\nfunc g(xs: seq<int>) -> seq<int> = filter(xs, func(x) = { y = x + 1\n if y > 5 { give true }\n false })");
    assert!(code.contains("fn f__native"));
    assert!(code.contains("fn g__native"));
    assert!(code.contains("parallel::map_native"));
    assert!(code.contains("parallel::filter_native"));
    assert!(code.matches("checked_add").count() >= 3);
}

#[test]
fn block_callback_sugar_and_top_level_result_inference_work() {
    let code = emit("values = collect(0..40000)\nresult = map(values, func(x) = { doubled = x * 2\n if doubled > 10 { give doubled }\n x })");
    assert!(code.contains("fn __topbind_1"));
    assert!(code.contains("parallel::map_native"));
}

#[test]
fn callback_local_names_cannot_shadow_the_generated_placeholder_argument() {
    let code = emit("func f(xs: seq<int>) -> seq<int> = map(xs, func(_) = { __j2_item_0 = 100\n _ + __j2_item_0 })");
    assert!(code.contains("fn f__native"));
    assert!(code.contains("|&__j2_item_0_: &i64|"));
}

#[test]
fn callbacks_keep_dynamic_scope_mutability_and_value_types() {
    for body in [
        "{ y = x\n y += 1\n y }",
        "{ capture = capture + 1\n x + capture }",
        "{ if x > 1 { y = 2 }\n y = 3\n y }",
        "{ if x > 1 { give 2.5 }\n x }",
        "{ print(x)\n x }",
    ] {
        let code = emit(&format!("func f(xs: seq<int>, capture: int) -> seq<int> = map(xs, func(x) = {body})"));
        assert!(!code.contains("fn f__native"), "{body}");
    }
}
