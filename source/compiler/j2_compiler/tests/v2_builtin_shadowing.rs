use j2_compiler::compile_to_rust;

fn emit(source: &str) -> String {
    compile_to_rust(source).expect("valid J2 source")
}

#[test]
fn custom_length_functions_keep_their_behavior_in_callbacks_and_bounds() {
    for replacement in [
        "func len(xs) = { print(42)\n 1 }",
        "func len(xs: seq<int>) -> int = 1",
        "len = 7",
    ] {
        let code = emit(&format!("{replacement}\nfunc mapped(xs: seq<int>) -> seq<int> = map(xs, func(x) = len(xs) + x)\nfunc bound(xs: seq<int>) -> int = {{ n = len(xs)\n xs[n - 1] }}\nvalues = [1, 2]\nresult = map(values, func(x) = len(values) + x)"));
        assert!(!code.contains("fn mapped__native"), "{replacement}");
        assert!(!code.contains("fn bound__native"), "{replacement}");
        assert!(!code.contains("parallel::map_native"), "{replacement}");
    }
}

#[test]
fn local_length_bindings_cannot_enter_length_fast_paths() {
    for body in [
        "{ n = len(xs)\n xs[n - 1] }",
        "xs[len(xs) - 1]",
        "{ for i in 0..len(xs)-1 { xs[i] = 1 }\n 0 }",
    ] {
        let code = emit(&format!("func caller(xs: seq<int>, len: int) -> int = {body}"));
        assert!(!code.contains("fn caller__native"), "{body}");
    }
}

#[test]
fn dynamic_math_replacements_are_not_reinterpreted_as_builtins() {
    for name in ["abs", "sqrt", "round", "floor", "ceil", "min", "max", "clamp"] {
        let code = emit(&format!("func {name}(x) = {{ print(x)\n x }}\nfunc mapped(xs: seq<int>) -> seq<int> = map(xs, func(x) = {name}(x))\nvalues = [1, 2]\nresult = map(values, func(x) = {name}(x))"));
        assert!(!code.contains("fn mapped__native"), "{name}");
        assert!(!code.contains("parallel::map_native"), "{name}");
    }
}

#[test]
fn ordinary_length_calls_still_allow_parallel_captured_maps() {
    let code = emit("func mapped(xs: seq<int>) -> seq<int> = map(xs, func(x) = len(xs) + x)");
    assert!(code.contains("fn mapped__native"));
    assert!(code.contains("parallel::map_native"));
}
