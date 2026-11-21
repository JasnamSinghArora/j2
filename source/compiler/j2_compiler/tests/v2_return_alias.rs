use j2_compiler::compile_to_rust;

fn emit(source: &str) -> String {
    compile_to_rust(source).expect("valid J2 source")
}

#[test]
fn returned_collection_parameters_remain_dynamic() {
    for body in [
        "xs",
        "{ discarded = map(xs, func(x) = { doubled = x * 2\n doubled })\n xs }",
        "{ if len(xs) > 0 { give xs }\n map(xs, _ + 1) }",
        "{ for i in 0..1 { if i > 0 { give xs } }\n map(xs, _ + 1) }",
    ] {
        let code = emit(&format!("func caller(xs: seq<int>) -> seq<int> = {body}"));
        assert!(!code.contains("fn caller__native"), "{body}");
    }
    let code = emit("func caller(values: map<text,int>) -> map<text,int> = values");
    assert!(!code.contains("fn caller__native"));
}

#[test]
fn collection_parameter_aliases_propagate_through_callers() {
    let code = emit("func identity(xs: seq<int>) -> seq<int> = xs\nfunc caller(xs: seq<int>) -> seq<int> = { discarded = map(xs, _ + 1)\n identity(xs) }");
    assert!(!code.contains("fn identity__native"));
    assert!(!code.contains("fn caller__native"));
}

#[test]
fn fresh_collection_results_remain_native() {
    for body in ["map(xs, _ + 1)", "filter(xs, _ > 0)", "copy(xs)", "[1, 2]"] {
        let code = emit(&format!("func caller(xs: seq<int>) -> seq<int> = {body}"));
        assert!(code.contains("fn caller__native"), "{body}");
    }
}
