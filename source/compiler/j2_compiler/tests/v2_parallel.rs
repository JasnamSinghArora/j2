use j2_compiler::compile_to_rust;

fn emit(source: &str) -> String {
    compile_to_rust(source).expect("valid J2 source")
}

#[test]
fn maps_filters_and_pipes_select_ordered_parallel_helpers() {
    let code = emit("func square(x: int) -> int = x * x\nfunc transform(xs: seq<int>, scale: int) -> seq<int> = map(xs, func(x) = square(x) + scale)\nfunc select(xs: seq<int>) -> seq<int> = filter(xs, _ % 2 == 0)\nfunc pipe(xs: seq<int>) -> seq<int> = xs >> square\n");
    assert!(code.contains("fn transform__native"));
    assert!(code.contains("fn select__native"));
    assert!(code.contains("fn pipe__native"));
    assert!(code.matches("parallel::map_native").count() >= 2);
    assert!(code.contains("parallel::filter_native"));
}

#[test]
fn top_level_transforms_capture_scalars_and_infer_generic_results() {
    let code = emit("func square(x) = x * x\nvalues = collect(1..40000)\nscale = 3\na = map(values, square)\nb = values >> (_ + scale)\nc = filter(b, _ > scale)\n");
    assert!(code.contains("fn __topbind_3"));
    assert!(code.contains("fn __topbind_4"));
    assert!(code.contains("fn __topbind_5"));
    assert!(code.contains("square__native_i64"));
}

#[test]
fn callbacks_with_effects_or_unknown_calls_stay_dynamic() {
    for source in [
        "func log(x) = { print(x)\n x }\nfunc transform(xs: seq<int>) -> seq<int> = map(xs, log)",
        "func transform(xs: seq<int>, shared: seq<int>) -> seq<int> = map(xs, func(x) = { shared[0] = x\n x })",
        "func transform(xs: seq<int>, f: func(int) -> int) -> seq<int> = map(xs, f)",
        "func transform(xs: seq<int>) -> seq<int> = map(xs, func(x) = time.now())",
    ] {
        let code = emit(source);
        assert!(!code.contains("parallel::map_native"), "{source}");
    }
}

#[test]
fn callback_parameter_shadows_length_locals_and_generated_names_are_hygienic() {
    let code = emit("func transform(xs: seq<int>, __j2_source_1: int) -> seq<int> = { n = len(xs)\n map(xs, func(n) = n + __j2_source_1) }");
    assert!(code.contains("fn transform__native"));
    assert!(code.contains("let __j2_source_1_ ="));
    assert!(!code.contains("(n as i64)"));
}

#[test]
fn native_math_matches_integer_types_and_rounding_rules() {
    let code = emit("func absolute(x: int) -> int = abs(x)\nfunc rounded(x: float) -> int = round(x)\nfunc remainder(x: int) -> int = x % 2\n");
    assert!(code.contains("wrapping_abs()"));
    assert!(code.contains("round_ties_even()"));
    assert!(code.contains("f64_to_i64"));
    assert!(code.contains("rem_euclid"));
}

#[test]
fn raw_range_bindings_keep_consumable_flow_semantics() {
    let code = emit("func f(n: int) -> seq<int> = { r = 1..n\n a = map(r, _ + 1)\n map(r, _ + 1) }");
    assert!(!code.contains("fn f__native"));
    let code = emit("func f(n: int) -> seq<int> = map(1..n, _ + 1)");
    assert!(code.contains("fn f__native"));
}

#[test]
fn inline_arithmetic_is_checked_and_value_dependent_results_fall_back() {
    let code = emit("func f(xs: seq<int>) -> seq<int> = map(xs, _ * 2 + 1)");
    assert!(code.contains("checked_mul"));
    assert!(code.contains("checked_add"));
    assert!(code.contains("panic_any"));
    for expression in ["_ / 2", "_ ** 2", "_ << 2"] {
        let code = emit(&format!("func f(xs: seq<int>) -> seq<int> = map(xs, {expression})"));
        assert!(!code.contains("fn f__native"), "{expression}");
    }
}

#[test]
fn shadowed_collection_builtins_are_not_parallelized() {
    let code = emit("func map(xs, f) = { print(42)\n xs }\nfunc caller(xs: seq<int>) -> seq<int> = map(xs, _ + 1)\nvalues = [1, 2]\nresult = map(values, _ + 1)");
    assert!(!code.contains("fn caller__native"));
    assert!(!code.contains("parallel::map_native"));
    let code = emit("func caller(xs: seq<int>, map: int) -> seq<int> = map(xs, _ + 1)");
    assert!(!code.contains("fn caller__native"));
}

#[test]
fn collection_alias_bindings_retain_reference_semantics() {
    let code = emit("original := collect(1..3)\nalias := original\nfor i in 1..2 { original[i] = alias[i - 1] }");
    assert!(!code.contains("fn __topbind_1"));
    assert!(code.contains("if __same_seq_storage("));
    for source in [
        "func f(xs: seq<int>) -> seq<int> = { alias = xs\n alias }",
        "func f(xs: map<text,int>) -> int = { alias = xs\n alias.key }",
    ] {
        assert!(!emit(source).contains("fn f__native"));
    }
}
