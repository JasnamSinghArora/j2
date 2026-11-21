use j2_compiler::compile_to_rust;

fn emit(source: &str) -> String {
    compile_to_rust(source).expect("valid J2 source")
}

fn shim<'a>(code: &'a str, name: &str) -> &'a str {
    code.split(&format!("fn {name}__shim(")).nth(1).expect("native shim")
        .split("fn real_main()").next().unwrap()
}

#[test]
fn native_panics_write_back_every_mutated_argument_before_resuming() {
    let code = emit("func update(xs: seq<int>, ys: seq<int>) -> int = { xs[0] = ys[0]\n ys[1] = xs[0]\n 1 }");
    let body = shim(&code, "update");
    let caught = body.find("catch_unwind(std::panic::AssertUnwindSafe(|| update__native").unwrap();
    let first_write = body.find("__writeback.and(rebox_seq_i64(&__args[0], a0))").unwrap();
    let second_write = body.find("__writeback.and(rebox_seq_i64(&__args[1], a1))").unwrap();
    let resumed = body.find("Err(payload) => std::panic::resume_unwind(payload)").unwrap();
    assert!(caught < first_write && first_write < second_write && second_write < resumed);
    assert!(resumed < body.find("__writeback?;").unwrap());
}

#[test]
fn mutable_sequence_aliases_are_guarded_before_unboxing() {
    let code = emit("func update(xs: seq<int>, ys: seq<int>, zs: seq<int>) -> int = { xs[0] = ys[0] + zs[0]\n xs[0] }");
    let body = shim(&code, "update");
    let alias = body.find("if __same_seq_storage(&__args[0], &__args[1]) || __same_seq_storage(&__args[0], &__args[2])").unwrap();
    assert!(alias < body.find("__unbox_exact_seq_i64(&__args[0])").unwrap());
    assert!(!body.contains("__same_seq_storage(&__args[1], &__args[2])"));
    let registration = code.split("fn real_main()").nth(1).unwrap();
    assert!(registration.contains("if __a.len() == 3 && (__same_seq_storage"));
    assert!(registration.contains("let _ = unbox_seq_i64(&__a[0])?;"));
    assert!(registration.contains("__call(__dyn.clone(), __a)"));
    assert!(registration.contains("else { update__shim(__a) }"));
}

#[test]
fn shared_sequence_arguments_keep_native_calls() {
    let code = emit("func sum(xs: seq<int>, ys: seq<int>) -> int = xs[0] + ys[0]\nfunc same(xs: seq<int>) -> int = sum(xs, xs)");
    assert!(code.contains("fn same__native"));
    assert!(!shim(&code, "sum").contains("aliased native function arguments"));
}

#[test]
fn repeated_mutable_call_arguments_and_slices_fall_back() {
    for arguments in ["xs, xs", "xs[0..1], xs[1..2]", "xs, xs[1..2]"] {
        let code = emit(&format!("func update(dst: seq<int>, src: seq<int>) -> int = {{ dst[0] = src[0] + 1\n dst[0] }}\nfunc same(xs: seq<int>) -> int = {{ xs[0] = 1\n update({arguments}) }}"));
        assert!(code.contains("fn update__native"));
        assert!(!code.contains("fn same__native"), "{arguments}");
    }
}

#[test]
fn float_sequences_use_exact_native_unboxing_and_validated_dynamic_fallback() {
    let code = emit("func values(xs: seq<float>, count: int) -> seq<float> = map(xs, _ + count)");
    assert!(shim(&code, "values").contains("__unbox_exact_seq_f64(&__args[0])?"));
    let registration = code.split("fn real_main()").nth(1).unwrap();
    assert!(registration.contains("if __a.len() == 2 && (matches!(&__a[0], J2Value::Seq(__s)"));
    let seq_check = registration.find("let _ = unbox_seq_f64(&__a[0])?;").unwrap();
    let scalar_check = registration.find("let _ = unbox_i64(&__a[1])?;").unwrap();
    let fallback = registration.find("__call(__dyn.clone(), __a)").unwrap();
    assert!(seq_check < scalar_check && scalar_check < fallback);
    assert!(registration.contains("else { values__shim(__a) }"));
}

#[test]
fn inferred_dispatch_retains_pre_call_fallback_and_packed_float_dispatch() {
    let code = emit("func values(xs) = map(xs, _ + 1)");
    assert!(code.contains("J2Value::SeqF64(_) | J2Value::Float(_) => true"));
    assert!(shim(&code, "values").contains("__unbox_exact_seq_f64(&__args[0])?"));
    assert!(shim(&code, "values").contains("__unbox_exact_seq_i64(&__args[0])?"));
    assert!(code.contains("match values__shim(__a) { Ok(__v) => Ok(__v), Err(_) => __call(__dyn.clone(), __a) }"));
}

#[test]
fn integer_sequences_fall_back_for_float_elements_without_saturating_them() {
    let code = emit("func values(xs: seq<int>) -> seq<int> = map(xs, _ + 1)\na = values([2.0, 100000000000000000000.0])");
    assert!(shim(&code, "values").contains("__unbox_exact_seq_i64(&__args[0])?"));
    let registration = code.split("fn real_main()").nth(1).unwrap();
    assert!(registration.contains("!matches!(__v, J2Value::Int(_))"));
    assert!(registration.contains("J2Value::SeqF64(_) => true"));
    assert!(registration.contains("let _ = unbox_seq_i64(&__a[0])?;"));
    assert!(registration.contains("__call(__dyn.clone(), __a)"));
}

#[test]
fn mismatched_native_sequence_calls_use_dynamic_boundaries() {
    for expression in ["xs", "xs[0..1]"] {
        let code = emit(&format!("func values(xs: seq<int>) -> seq<int> = map(xs, _ + 1)\nfunc caller(xs: seq<float>) -> seq<int> = values({expression})"));
        assert!(code.contains("fn values__native"));
        assert!(!code.contains("fn caller__native"));
    }
    let code = emit("func values(xs: seq<int>) -> seq<int> = map(xs, _ + 1)\nxs = [2.0]\nys = values(xs)");
    assert!(!code.contains("fn __topbind_2"));
}
