// J -> backend source lowering

use crate::ast::*;
use crate::typeck::{self, Borrow, FnSig, NativeFns};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

pub struct Lowered {
    pub rust_src: String,
}

pub fn lower_program(prog: &Program) -> Lowered {
    // Resolve bare-identifier types that name native classes
    let prog = &normalize_class_types(prog);
    let mut out = String::new();
    // Prelude.
    out.push_str(PREAMBLE);

    // Decide which top-level functions lower natively
    let sel = select_native_fns(prog);
    let native = &sel.fns;

    // emit native class items first, before users
    for stmt in &prog.items {
        if let Stmt::Bind { name, value: Expr::ClassLit { extends, fields, methods, .. }, .. } = stmt {
            if let Some(csig) = sel.classes.get(name) {
                let _ = (extends, fields, methods);
                if let Ok(emit) = lower_native_class(csig, native, &sel.safe_gen, &sel.classes) {
                    out.push('\n');
                    out.push_str(&emit);
                    out.push('\n');
                }
            }
        }
    }

    // module-level native fns for MIR matchers
    for stmt in &prog.items {
        if let Stmt::Func { name, params, body, type_params, .. } = stmt {
            if let Some(sig) = native.get(name) {
                // always Ok; select_native_fns kept only lowerable fns
                if let Ok(emit) = lower_native_fn(name, params, body, sig, type_params, native, &sel.safe_gen, &sel.classes) {
                    out.push('\n');
                    out.push_str(&emit.items);
                    out.push('\n');
                    out.push_str(&emit.shim);
                    out.push('\n');
                }
            }
        }
    }

    // Outline native-able top-level for-range loops
    let gamma_top = infer_top_gamma(prog, native);
    let mut outlines: HashMap<usize, String> = HashMap::new();
    for (idx, stmt) in prog.items.iter().enumerate() {
        let outlined = match stmt {
            Stmt::ForLoop { .. } => try_outline_top_loop(idx, stmt, &gamma_top, native, &sel.safe_gen, &sel.classes),
            // top-level data-producing bindings
            Stmt::Bind { name, value, mutable } => {
                try_outline_top_bind(idx, name, value, *mutable, &gamma_top, native, &sel.safe_gen, &sel.classes)
            }
            _ => None,
        };
        if let Some((fn_item, boundary)) = outlined {
            out.push('\n');
            out.push_str(&fn_item);
            out.push('\n');
            outlines.insert(idx, boundary);
        }
    }

    out.push_str("\nfn main() {\n");
    // Convert any native panic
    out.push_str("    __install_panic_hook();\n");
    out.push_str("    let __r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(real_main));\n");
    out.push_str("    let __msg: Option<String> = match __r {\n");
    out.push_str("        Ok(Ok(())) => None,\n");
    out.push_str("        Ok(Err(e)) => Some(e.to_string()),\n");
    out.push_str("        Err(_) => Some(format!(\"RuntimeError: {}\", __take_panic_msg())),\n");
    out.push_str("    };\n");
    out.push_str("    if let Some(m) = __msg {\n");
    // annotate with executing top-level line, best-effort
    out.push_str("        match __current_line() { 0 => eprintln!(\"{}\", m), n => eprintln!(\"{} (at line {})\", m, n) }\n");
    out.push_str("        std::process::exit(1);\n");
    out.push_str("    }\n");
    out.push_str("}\n\n");
    out.push_str("fn real_main() -> J2Result<()> {\n");
    out.push_str("    let env_rc = new_env();\n");
    out.push_str("    let env: &EnvRef = &env_rc;\n");
    for (idx, stmt) in prog.items.iter().enumerate() {
        // record top-level line for runtime errors
        if let Some(&line) = prog.item_lines.get(idx) {
            if line > 0 {
                let _ = writeln!(out, "    __set_line({});", line);
            }
        }
        // An outlined top-level loop
        if let Some(boundary) = outlines.get(&idx) {
            out.push_str(boundary);
            continue;
        }
        match stmt {
            // A higher-order native fn
            Stmt::Func { name, .. } if sel.hof.contains(name) => {
                lower_stmt_top(stmt, &mut out, 1);
            }
            // inferred native fn; dynamic fallback on Err
            Stmt::Func { name, params, body, .. } if sel.inferred.contains(name) => {
                let dyn_fn = lower_func_body(params, body);
                let _ = writeln!(out, "    {{");
                let _ = writeln!(out, "        let __dyn = {};", dyn_fn);
                let _ = writeln!(
                    out,
                    "        __bind(env, \"{n}\", J2Value::Builtin(std::sync::Arc::new(move |__a: &[J2Value]| -> J2Result<J2Value> {{ match {n}__shim(__a) {{ Ok(__v) => Ok(__v), Err(_) => __call(__dyn.clone(), __a) }} }}), \"{n}\"), true)?;",
                    n = name,
                );
                let _ = writeln!(out, "        __globalize(env, \"{}\");", name);
                let _ = writeln!(out, "    }}");
            }
            // An *annotated* native function is a contract
            Stmt::Func { name, .. } if native.contains_key(name) => {
                let _ = writeln!(
                    out,
                    "    __bind(env, \"{n}\", J2Value::Builtin(std::sync::Arc::new(|__a: &[J2Value]| -> J2Result<J2Value> {{ {n}__shim(__a) }}), \"{n}\"), true)?;",
                    n = name,
                );
                let _ = writeln!(out, "    __globalize(env, \"{}\");", name);
            }
            _ => lower_stmt_top(stmt, &mut out, 1),
        }
    }
    out.push_str("    Ok(())\n}\n");
    Lowered { rust_src: out }
}

/// native selection result; inferred fns need fallback
struct NativeSelection {
    fns: NativeFns,
    inferred: HashSet<String>,
    safe_gen: HashSet<String>,
    classes: Classes,
    /// higher-order native fns; generic item, no shim
    hof: HashSet<String>,
}

/// any param has a function type
fn is_hof_sig(sig: &FnSig) -> bool {
    sig.params.iter().any(|t| matches!(t, Ty::Func(..)))
}

/// `Copy` types; others cloned on read
fn is_copy_ty(t: &Ty) -> bool {
    matches!(t, Ty::Int | Ty::Float | Ty::Bool | Ty::Nil)
}

/// Whether every `Ty::Class(n)` mentioned
fn class_refs_resolved(ty: &Ty, classes: &Classes) -> bool {
    match ty {
        Ty::Class(n) => classes.contains_key(n),
        Ty::Seq(e) => class_refs_resolved(e, classes),
        Ty::Map(k, v) => class_refs_resolved(k, classes) && class_refs_resolved(v, classes),
        Ty::Pair(a, b) => class_refs_resolved(a, classes) && class_refs_resolved(b, classes),
        Ty::Func(ps, r) => ps.iter().all(|t| class_refs_resolved(t, classes)) && class_refs_resolved(r, classes),
        _ => true,
    }
}

/// Copy scalar type for `Fn` bounds
fn scalar_rust(t: &Ty) -> NRes<&'static str> {
    match t {
        Ty::Int => Ok("i64"),
        Ty::Float => Ok("f64"),
        Ty::Bool => Ok("bool"),
        _ => Err(()),
    }
}

/// native class sig, `None` if not candidate
fn collect_class_sig(name: &str, fields: &[ClassField], methods: &[ClassMethod], extends: &[String]) -> Option<ClassSig> {
    if !extends.is_empty() {
        return None; // MVP: inheritance stays dynamic
    }
    let mut field_sigs = Vec::with_capacity(fields.len());
    for f in fields {
        let ty = f.ty.clone()?; // every field must be typed
        // Must have a representable backend type
        if rust_ty(&ty).ok().flatten().is_none() {
            return None;
        }
        field_sigs.push(ClassFieldSig { name: f.name.clone(), ty, default: f.default.clone() });
    }
    let mut method_sigs = HashMap::new();
    for m in methods {
        if m.params.iter().any(|p| p.pattern.is_some() || p.ty.is_none()) {
            return None;
        }
        let params: Vec<(String, Ty)> = m.params.iter().map(|p| (p.name.clone(), p.ty.clone().unwrap())).collect();
        let ret = m.ret_ty.clone().unwrap_or(Ty::Nil);
        let borrow_self = if m.mutating { Borrow::Mut } else { Borrow::Shared };
        method_sigs.insert(m.name.clone(), ClassMethodSig { params, ret, borrow_self, body: m.body.clone() });
    }
    Some(ClassSig { name: name.to_string(), fields: field_sigs, methods: method_sigs })
}

/// class-named `Ty::Var` to `Ty::Class`; tparams shadow
fn resolve_class_ty(ty: &Ty, class_names: &HashSet<String>, tparams: &[String]) -> Ty {
    match ty {
        Ty::Var(n) if class_names.contains(n) && !tparams.iter().any(|p| p == n) => {
            Ty::Class(n.clone())
        }
        Ty::Seq(e) => Ty::Seq(Box::new(resolve_class_ty(e, class_names, tparams))),
        Ty::Map(k, v) => Ty::Map(
            Box::new(resolve_class_ty(k, class_names, tparams)),
            Box::new(resolve_class_ty(v, class_names, tparams)),
        ),
        Ty::Pair(a, b) => Ty::Pair(
            Box::new(resolve_class_ty(a, class_names, tparams)),
            Box::new(resolve_class_ty(b, class_names, tparams)),
        ),
        Ty::Func(ps, r) => Ty::Func(
            ps.iter().map(|t| resolve_class_ty(t, class_names, tparams)).collect(),
            Box::new(resolve_class_ty(r, class_names, tparams)),
        ),
        other => other.clone(),
    }
}

/// normalize class types program-wide; dynamic classes untouched
fn normalize_class_types(prog: &Program) -> Program {
    let mut class_names: HashSet<String> = HashSet::new();
    for s in &prog.items {
        if let Stmt::Bind { name, value: Expr::ClassLit { extends, fields, methods, .. }, .. } = s {
            if collect_class_sig(name, fields, methods, extends).is_some() {
                class_names.insert(name.clone());
            }
        }
    }
    if class_names.is_empty() {
        return prog.clone();
    }
    let mut out = prog.clone();
    for s in &mut out.items {
        match s {
            Stmt::Func { params, ret_ty, type_params, .. } => {
                for p in params.iter_mut() {
                    if let Some(t) = p.ty.as_ref() {
                        let r = resolve_class_ty(t, &class_names, type_params);
                        p.ty = Some(r);
                    }
                }
                if let Some(rt) = ret_ty.as_ref() {
                    let r = resolve_class_ty(rt, &class_names, type_params);
                    *ret_ty = Some(r);
                }
            }
            Stmt::Bind { value: Expr::ClassLit { fields, methods, .. }, .. } => {
                for f in fields.iter_mut() {
                    if let Some(t) = f.ty.as_ref() {
                        let r = resolve_class_ty(t, &class_names, &[]);
                        f.ty = Some(r);
                    }
                }
                for m in methods.iter_mut() {
                    for p in m.params.iter_mut() {
                        if let Some(t) = p.ty.as_ref() {
                            let r = resolve_class_ty(t, &class_names, &[]);
                            p.ty = Some(r);
                        }
                    }
                    if let Some(rt) = m.ret_ty.as_ref() {
                        let r = resolve_class_ty(rt, &class_names, &[]);
                        m.ret_ty = Some(r);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Lookup a top-level function's
fn find_fn_def<'a>(prog: &'a Program, name: &str) -> Option<(&'a [Param], &'a FuncBody, &'a [String])> {
    prog.items.iter().find_map(|s| match s {
        Stmt::Func { name: n, params, body, type_params, .. } if n == name => {
            Some((params.as_slice(), body, type_params.as_slice()))
        }
        _ => None,
    })
}

/// explicit `<T>` or inferred `Ty::Var` in sig
fn effective_var(sig: &FnSig, type_params: &[String]) -> Option<String> {
    if !type_params.is_empty() {
        Some(type_params[0].clone())
    } else {
        first_var_name(sig)
    }
}

fn select_native_fns(prog: &Program) -> NativeSelection {
    // J2_NO_NATIVE forces the pure dynamic-interpreter path
    if std::env::var("J2_NO_NATIVE").map(|v| v == "1").unwrap_or(false) {
        return NativeSelection {
            fns: HashMap::new(),
            inferred: HashSet::new(),
            safe_gen: HashSet::new(),
            classes: HashMap::new(),
            hof: HashSet::new(),
        };
    }
    // Count func definitions per name
    let mut counts: HashMap<String, usize> = HashMap::new();
    for s in &prog.items {
        if let Stmt::Func { name, .. } = s {
            *counts.entry(name.clone()).or_insert(0) += 1;
        }
    }
    // annotated sigs seed; iterate for inferred fns
    let mut classes: Classes = HashMap::new();
    for s in &prog.items {
        if let Stmt::Bind { name, value: Expr::ClassLit { extends, fields, methods, .. }, .. } = s {
            if let Some(csig) = collect_class_sig(name, fields, methods, extends) {
                classes.insert(name.clone(), csig);
            }
        }
    }

    let mut fns: NativeFns = HashMap::new();
    let mut inferred: HashSet<String> = HashSet::new();
    for s in &prog.items {
        if let Stmt::Func { name, params, body, ret_ty, .. } = s {
            if counts.get(name) != Some(&1) {
                continue;
            }
            if typeck::is_native_candidate(params, ret_ty) {
                fns.insert(name.clone(), typeck::build_sig(params, ret_ty, body));
            }
        }
    }
    // Infer signatures for the rest
    loop {
        let mut changed = false;
        for s in &prog.items {
            if let Stmt::Func { name, params, body, .. } = s {
                if counts.get(name) != Some(&1) || fns.contains_key(name) {
                    continue;
                }
                if let Some(sig) = typeck::infer_fn_sig(params, body, &fns) {
                    fns.insert(name.clone(), sig);
                    inferred.insert(name.clone());
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    // fixpoint grows safe_gen, drops non-lowering candidates
    let mut safe_gen: HashSet<String> = HashSet::new();
    loop {
        let mut changed = false;
        // Grow safe_gen.
        let names: Vec<String> = fns.keys().cloned().collect();
        for name in &names {
            if safe_gen.contains(name) {
                continue;
            }
            let sig = fns[name].clone();
            let Some((params, body, tp)) = find_fn_def(prog, name) else { continue };
            let _ = params;
            let Some(var) = effective_var(&sig, tp) else { continue }; // non-generic: not tracked here
            let f64_ok = lower_native_fn_item(
                &format!("{name}__native_f64"), body, &subst_sig(&sig, &var, &Ty::Float), &fns, &safe_gen, &classes,
            ).is_ok();
            let i64_ok = lower_native_fn_item(
                &format!("{name}__native_i64"), body, &subst_sig(&sig, &var, &Ty::Int), &fns, &safe_gen, &classes,
            ).is_ok();
            if f64_ok && i64_ok {
                safe_gen.insert(name.clone());
                changed = true;
            }
        }
        // Drop non-lowering fn candidates.
        let mut to_drop: Vec<String> = Vec::new();
        for s in &prog.items {
            if let Stmt::Func { name, params, body, type_params, .. } = s {
                if let Some(sig) = fns.get(name) {
                    if lower_native_fn(name, params, body, sig, type_params, &fns, &safe_gen, &classes).is_err() {
                        to_drop.push(name.clone());
                    }
                }
            }
        }
        for n in &to_drop {
            fns.remove(n);
            inferred.remove(n);
            safe_gen.remove(n);
            changed = true;
        }
        // Drop classes whose struct/impl/boundary won't lower
        let class_names: Vec<String> = classes.keys().cloned().collect();
        for cn in &class_names {
            let csig = classes[cn].clone();
            if lower_native_class(&csig, &fns, &safe_gen, &classes).is_err() {
                classes.remove(cn);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let hof: HashSet<String> = fns
        .iter()
        .filter(|(_, sig)| is_hof_sig(sig))
        .map(|(n, _)| n.clone())
        .collect();
    NativeSelection { fns, inferred, safe_gen, classes, hof }
}

// outline hot top-level for-range loops natively

/// Infer the types of top-level bindings
fn infer_top_gamma(prog: &Program, fns: &NativeFns) -> HashMap<String, Ty> {
    let mut ctx = typeck::TyCtx::new(fns);
    for s in &prog.items {
        if let Stmt::Bind { name, value, .. } | Stmt::Global { name, value, .. } = s {
            let t = typeck::synth(&ctx, value);
            ctx.gamma.insert(name.clone(), t);
        }
    }
    ctx.gamma
}

fn gather_expr_refs(e: &Expr, refs: &mut HashSet<String>) {
    match e {
        Expr::Ident(n) => { refs.insert(n.clone()); }
        Expr::Binary { lhs, rhs, .. } => { gather_expr_refs(lhs, refs); gather_expr_refs(rhs, refs); }
        Expr::Unary { operand, .. } => gather_expr_refs(operand, refs),
        Expr::Index { coll, idx } => { gather_expr_refs(coll, refs); gather_expr_refs(idx, refs); }
        Expr::Call { callee, args } => { gather_expr_refs(callee, refs); for a in args { gather_expr_refs(a, refs); } }
        Expr::Range { start, end } => { gather_expr_refs(start, refs); if let Some(e) = end { gather_expr_refs(e, refs); } }
        Expr::Member { obj, .. } => gather_expr_refs(obj, refs),
        Expr::Pair(a, b) => { gather_expr_refs(a, refs); gather_expr_refs(b, refs); }
        Expr::SeqLit(items) => for it in items { gather_expr_refs(it, refs); },
        Expr::Pipe { lhs, rhs } | Expr::Filter { lhs, rhs } => { gather_expr_refs(lhs, refs); gather_expr_refs(rhs, refs); }
        _ => {}
    }
}

/// all identifiers recursively, for HOF capture check
fn gather_all_idents(e: &Expr, refs: &mut HashSet<String>) {
    match e {
        Expr::Ident(n) => { refs.insert(n.clone()); }
        Expr::Binary { lhs, rhs, .. } => { gather_all_idents(lhs, refs); gather_all_idents(rhs, refs); }
        Expr::Unary { operand, .. } => gather_all_idents(operand, refs),
        Expr::Index { coll, idx } => { gather_all_idents(coll, refs); gather_all_idents(idx, refs); }
        Expr::Call { callee, args } => { gather_all_idents(callee, refs); for a in args { gather_all_idents(a, refs); } }
        Expr::Range { start, end } => { gather_all_idents(start, refs); if let Some(e) = end { gather_all_idents(e, refs); } }
        Expr::Member { obj, .. } => gather_all_idents(obj, refs),
        Expr::Pair(a, b) => { gather_all_idents(a, refs); gather_all_idents(b, refs); }
        Expr::SeqLit(items) => { for it in items { gather_all_idents(it, refs); } }
        Expr::MapLit(entries) => { for (_, v) in entries { gather_all_idents(v, refs); } }
        Expr::Pipe { lhs, rhs } | Expr::Filter { lhs, rhs } => { gather_all_idents(lhs, refs); gather_all_idents(rhs, refs); }
        Expr::CondLambda(inner) => gather_all_idents(inner, refs),
        Expr::Block(b) => { for s in &b.stmts { gather_all_idents_stmt(s, refs); } }
        Expr::Lambda { params, body } => {
            let mut inner = HashSet::new();
            match body.as_ref() {
                FuncBody::Expr(e) => gather_all_idents(e, &mut inner),
                FuncBody::Block(b) => { for s in &b.stmts { gather_all_idents_stmt(s, &mut inner); } }
            }
            for p in params { inner.remove(&p.name); }
            refs.extend(inner);
        }
        // `Class::member` resolves class_name at runtime
        Expr::StaticAccess { class_name, .. } => { refs.insert(class_name.clone()); }
        // defaults and statics evaluate in enclosing scope
        Expr::ClassLit { fields, statics, .. } => {
            for f in fields {
                if let Some(d) = &f.default { gather_all_idents(d, refs); }
            }
            for st in statics { gather_all_idents(&st.value, refs); }
        }
        Expr::IntLit(_) | Expr::FloatLit(_) | Expr::TextLit(_) | Expr::Bool(_)
        | Expr::Null | Expr::Underscore | Expr::Const(_) | Expr::NativeBlock(_) => {}
    }
}

/// Statement-level companion to `gather_all_idents` (exhaustive over `Stmt`).
fn gather_all_idents_stmt(s: &Stmt, refs: &mut HashSet<String>) {
    match s {
        Stmt::Bind { value, .. } | Stmt::Global { value, .. } => gather_all_idents(value, refs),
        Stmt::CompoundAssign { name, value, .. } => { refs.insert(name.clone()); gather_all_idents(value, refs); }
        Stmt::IndexedAssign { target, idx, value } => { gather_all_idents(target, refs); gather_all_idents(idx, refs); gather_all_idents(value, refs); }
        Stmt::MemberAssign { target, value, .. } => { gather_all_idents(target, refs); gather_all_idents(value, refs); }
        Stmt::IncDec { name, .. } => { refs.insert(name.clone()); }
        Stmt::Func { body, .. } => match body {
            FuncBody::Expr(e) => gather_all_idents(e, refs),
            FuncBody::Block(b) => { for st in &b.stmts { gather_all_idents_stmt(st, refs); } }
        },
        Stmt::ForLoop { iter, filter, until, body, .. } => {
            gather_all_idents(iter, refs);
            if let Some(f) = filter { gather_all_idents(f, refs); }
            if let Some(u) = until { gather_all_idents(u, refs); }
            for st in &body.stmts { gather_all_idents_stmt(st, refs); }
        }
        Stmt::Repeat { cond, body } | Stmt::DoRepeat { body, cond } => {
            gather_all_idents(cond, refs);
            for st in &body.stmts { gather_all_idents_stmt(st, refs); }
        }
        Stmt::Loop { body } => { for st in &body.stmts { gather_all_idents_stmt(st, refs); } }
        Stmt::If { cond, then_block, else_block } => {
            gather_all_idents(cond, refs);
            for st in &then_block.stmts { gather_all_idents_stmt(st, refs); }
            if let Some(eb) = else_block { for st in &eb.stmts { gather_all_idents_stmt(st, refs); } }
        }
        Stmt::Stop { cond } | Stmt::Skip { cond } => { if let Some(c) = cond { gather_all_idents(c, refs); } }
        Stmt::Give { value } => { if let Some(v) = value { gather_all_idents(v, refs); } }
        Stmt::Try { body, typed_handlers, default_handler } => {
            for st in &body.stmts { gather_all_idents_stmt(st, refs); }
            for h in typed_handlers { for st in &h.handler.stmts { gather_all_idents_stmt(st, refs); } }
            if let Some(d) = default_handler { for st in &d.stmts { gather_all_idents_stmt(st, refs); } }
        }
        Stmt::Assert { cond, msg } => { gather_all_idents(cond, refs); if let Some(m) = msg { gather_all_idents(m, refs); } }
        Stmt::Expr(e) => gather_all_idents(e, refs),
    }
}

fn gather_stmt_refs(
    s: &Stmt,
    refs: &mut HashSet<String>,
    seq_writes: &mut HashSet<String>,
    scalar_writes: &mut HashSet<String>,
    locals: &mut HashSet<String>,
) {
    match s {
        Stmt::Bind { name, value, .. } => {
            gather_expr_refs(value, refs);
            // A fresh name is loop-local
            if !locals.contains(name) {
                scalar_writes.insert(name.clone()); // may be a top-level reassign
                locals.insert(name.clone());        // and is visible afterwards
            }
        }
        Stmt::CompoundAssign { name, value, .. } => {
            scalar_writes.insert(name.clone());
            refs.insert(name.clone());
            gather_expr_refs(value, refs);
        }
        Stmt::IncDec { name, .. } => { scalar_writes.insert(name.clone()); refs.insert(name.clone()); }
        Stmt::IndexedAssign { target, idx, value } => {
            if let Expr::Ident(t) = target { seq_writes.insert(t.clone()); refs.insert(t.clone()); }
            else { gather_expr_refs(target, refs); }
            gather_expr_refs(idx, refs);
            gather_expr_refs(value, refs);
        }
        Stmt::ForLoop { bindings, iter, body, .. } => {
            for b in bindings { locals.insert(b.clone()); }
            gather_expr_refs(iter, refs);
            for s in &body.stmts { gather_stmt_refs(s, refs, seq_writes, scalar_writes, locals); }
        }
        Stmt::If { cond, then_block, else_block } => {
            gather_expr_refs(cond, refs);
            for s in &then_block.stmts { gather_stmt_refs(s, refs, seq_writes, scalar_writes, locals); }
            if let Some(eb) = else_block { for s in &eb.stmts { gather_stmt_refs(s, refs, seq_writes, scalar_writes, locals); } }
        }
        Stmt::Repeat { cond, body } | Stmt::DoRepeat { body, cond } => {
            gather_expr_refs(cond, refs);
            for s in &body.stmts { gather_stmt_refs(s, refs, seq_writes, scalar_writes, locals); }
        }
        Stmt::Loop { body } => for s in &body.stmts { gather_stmt_refs(s, refs, seq_writes, scalar_writes, locals); },
        Stmt::Stop { cond } | Stmt::Skip { cond } => { if let Some(c) = cond { gather_expr_refs(c, refs); } }
        Stmt::Expr(e) => gather_expr_refs(e, refs),
        // other stmts just collect refs
        _ => {}
    }
}

/// outline top-level for-range loop into native fn
fn try_outline_top_loop(
    idx: usize,
    for_stmt: &Stmt,
    gamma: &HashMap<String, Ty>,
    fns: &NativeFns,
    safe_gen: &HashSet<String>,
    classes: &Classes,
) -> Option<(String, String)> {
    let Stmt::ForLoop { bindings, filter, until, body, .. } = for_stmt else { return None; };
    if filter.is_some() || until.is_some() || bindings.len() != 1 { return None; }
    // untyped literals default i32, huge bounds dynamic
    if let Stmt::ForLoop { iter: Expr::Range { start, end: Some(end) }, .. } = for_stmt {
        let too_big = |e: &Expr| matches!(e, Expr::IntLit(n) if *n >= i32::MAX as i64);
        if too_big(start) || too_big(end) { return None; }
    }

    let mut refs = HashSet::new();
    let mut seq_writes = HashSet::new();
    let mut scalar_writes = HashSet::new();
    let mut locals: HashSet<String> = bindings.iter().cloned().collect();
    if let Stmt::ForLoop { iter, .. } = for_stmt { gather_expr_refs(iter, &mut refs); }
    for s in &body.stmts {
        gather_stmt_refs(s, &mut refs, &mut seq_writes, &mut scalar_writes, &mut locals);
    }
    // Scalar writes to top-level bindings are accumulators
    let mut accumulators: Vec<String> = scalar_writes
        .iter()
        .filter(|w| gamma.contains_key(*w))
        .cloned()
        .collect();
    accumulators.sort();
    // cap accumulators; nested pair chain
    if accumulators.len() > 12 { return None; }

    // free vars and accumulators, passed by value
    let mut names: Vec<String> = refs
        .iter()
        .filter(|r| gamma.contains_key(*r) && (!locals.contains(*r) || scalar_writes.contains(*r)))
        .cloned()
        .collect();
    names.sort();
    if names.is_empty() { return None; }

    let mut params = Vec::new();
    let mut param_names = Vec::new();
    let mut borrows = Vec::new();
    for n in &names {
        let ty = gamma.get(n).cloned()?;
        // Only natively-representable types.
        let native_ok = matches!(&ty, Ty::Int | Ty::Float | Ty::Bool)
            || matches!(&ty, Ty::Seq(e) if matches!(e.as_ref(), Ty::Float | Ty::Int));
        if !native_ok { return None; }
        let borrow = match &ty {
            Ty::Seq(_) => if seq_writes.contains(n) { Borrow::Mut } else { Borrow::Shared },
            _ => Borrow::ByVal,
        };
        params.push(ty);
        param_names.push(n.clone());
        borrows.push(borrow);
    }
    // accumulator types determine `ret`, scalar or 2-tuple
    let acc_tys: Vec<Ty> = accumulators.iter().map(|a| gamma.get(a).cloned()).collect::<Option<_>>()?;
    let (ret, trailing): (Ty, Option<Expr>) = match accumulators.as_slice() {
        [] => (Ty::Nil, None),
        [a] => (acc_tys[0].clone(), Some(Expr::Ident(a.clone()))),
        _ => {
            // N accumulators -> right-nested pairs
            let mut ty = acc_tys.last().unwrap().clone();
            let mut ex = Expr::Ident(accumulators.last().unwrap().clone());
            for (a, t) in accumulators.iter().zip(acc_tys.iter()).rev().skip(1) {
                ty = Ty::Pair(Box::new(t.clone()), Box::new(ty));
                ex = Expr::Pair(Box::new(Expr::Ident(a.clone())), Box::new(ex));
            }
            (ty, Some(ex))
        }
    };
    let sig = FnSig { params, param_names, borrows, ret: ret.clone() };
    let fn_name = format!("__toploop_{idx}");
    // loop plus trailing accumulator read for return
    let mut stmts = vec![for_stmt.clone()];
    if let Some(t) = trailing {
        stmts.push(Stmt::Expr(t));
    }
    let stmt_lines = vec![0usize; stmts.len()]; // synthetic outlined block
    let body_block = FuncBody::Block(Block { stmts, stmt_lines });
    let fn_item = lower_native_fn_item(&fn_name, &body_block, &sig, fns, safe_gen, classes).ok()?;

    // unbox, call native, rebox; dynamic fallback
    let mut boundary = String::new();
    let _ = writeln!(boundary, "    {{");
    let _ = writeln!(boundary, "        let __outline: J2Result<()> = (|| {{");
    for (n, (ty, b)) in names.iter().zip(sig.params.iter().zip(sig.borrows.iter())) {
        let unbox = match ty {
            Ty::Int => format!("unbox_i64(&__get(env, \"{n}\")?)?"),
            Ty::Float => format!("unbox_f64(&__get(env, \"{n}\")?)?"),
            Ty::Bool => format!("unbox_bool(&__get(env, \"{n}\")?)?"),
            Ty::Seq(e) => match e.as_ref() {
                Ty::Float => format!("unbox_seq_f64(&__get(env, \"{n}\")?)?"),
                Ty::Int => format!("unbox_seq_i64(&__get(env, \"{n}\")?)?"),
                _ => return None,
            },
            _ => return None,
        };
        let mutbind = if matches!(ty, Ty::Seq(_)) && *b == Borrow::Mut { "mut " } else { "" };
        let _ = writeln!(boundary, "            let {mutbind}__ol_{n} = {unbox};");
    }
    let call_args: Vec<String> = names.iter().zip(sig.params.iter().zip(sig.borrows.iter())).map(|(n, (ty, b))| {
        match (ty, b) {
            (Ty::Seq(_), Borrow::Mut) => format!("&mut __ol_{n}"),
            (Ty::Seq(_), _) => format!("&__ol_{n}"),
            _ => format!("__ol_{n}"),
        }
    }).collect();
    if accumulators.is_empty() {
        let _ = writeln!(boundary, "            {fn_name}({});", call_args.join(", "));
    } else {
        let _ = writeln!(boundary, "            let __ol_ret = {fn_name}({});", call_args.join(", "));
    }
    // Write accumulators back first
    if !accumulators.is_empty() {
        let mut spine = String::from("__ol_ret");
        for (k, (a, t)) in accumulators.iter().zip(acc_tys.iter()).enumerate() {
            let leaf = if accumulators.len() == 1 || k + 1 == accumulators.len() {
                spine.clone()
            } else {
                format!("{spine}.0")
            };
            let boxed = box_scalar(t, &leaf).ok()?;
            let _ = writeln!(boundary, "            __rebind(env, \"{a}\", {boxed})?;");
            spine.push_str(".1");
        }
    }
    for (n, (ty, b)) in names.iter().zip(sig.params.iter().zip(sig.borrows.iter())) {
        if let (Ty::Seq(e), Borrow::Mut) = (ty, b) {
            let rebox = match e.as_ref() {
                Ty::Float => format!("rebox_seq_f64(&__get(env, \"{n}\")?, __ol_{n})?"),
                Ty::Int => format!("rebox_seq_i64(&__get(env, \"{n}\")?, __ol_{n})?"),
                _ => return None,
            };
            let _ = writeln!(boundary, "            {rebox};");
        }
    }
    let _ = writeln!(boundary, "            Ok(())");
    let _ = writeln!(boundary, "        }})();");
    let _ = writeln!(boundary, "        if __outline.is_err() {{");
    // Dynamic fallback: the original loop.
    if let Stmt::ForLoop { bindings, iter, filter, until, body } = for_stmt {
        lower_for(bindings, iter, filter.as_ref(), until.as_ref(), body, &mut boundary, 3);
    }
    let _ = writeln!(boundary, "        }}");
    let _ = writeln!(boundary, "    }}");
    Some((fn_item, boundary))
}

/// outline seq/map-reading top-level binding into native fn
fn try_outline_top_bind(
    idx: usize,
    name: &str,
    value: &Expr,
    mutable: bool,
    gamma: &HashMap<String, Ty>,
    fns: &NativeFns,
    safe_gen: &HashSet<String>,
    classes: &Classes,
) -> Option<(String, String)> {
    let mut refs = HashSet::new();
    gather_expr_refs(value, &mut refs);
    let mut names: Vec<String> = refs.iter().filter(|r| gamma.contains_key(*r)).cloned().collect();
    names.sort();
    let reads_data = names
        .iter()
        .any(|n| matches!(gamma.get(n), Some(Ty::Seq(_)) | Some(Ty::Map(..))));
    if !reads_data {
        return None;
    }
    // synthesized result type must be native-representable
    let mut tc = typeck::TyCtx::new(fns);
    tc.gamma = gamma.clone();
    let ret = typeck::synth(&tc, value);
    rust_ty(&ret).ok()?.as_ref()?;  // bail unless ret has a backend type
    let boxed_ret = box_value(&ret, "__res")?;

    let mut params = Vec::new();
    let mut param_names = Vec::new();
    let mut borrows = Vec::new();
    for n in &names {
        let ty = gamma.get(n).cloned()?;
        let native_ok = matches!(&ty, Ty::Int | Ty::Float | Ty::Bool)
            || matches!(&ty, Ty::Seq(e) if matches!(e.as_ref(), Ty::Float | Ty::Int))
            || matches!(&ty, Ty::Map(k, v) if **k == Ty::Text && matches!(v.as_ref(), Ty::Float | Ty::Int));
        if !native_ok {
            return None;
        }
        let borrow = match &ty {
            Ty::Seq(_) | Ty::Map(..) => Borrow::Shared,
            _ => Borrow::ByVal,
        };
        params.push(ty);
        param_names.push(n.clone());
        borrows.push(borrow);
    }
    let sig = FnSig { params, param_names, borrows, ret: ret.clone() };
    let fn_name = format!("__topbind_{idx}");
    let body = FuncBody::Expr(value.clone());
    let fn_item = lower_native_fn_item(&fn_name, &body, &sig, fns, safe_gen, classes).ok()?;

    let mut b = String::new();
    let _ = writeln!(b, "    {{");
    let _ = writeln!(b, "        let __outline: J2Result<()> = (|| {{");
    for (n, (ty, _)) in names.iter().zip(sig.params.iter().zip(sig.borrows.iter())) {
        let unbox = match ty {
            Ty::Int => format!("unbox_i64(&__get(env, \"{n}\")?)?"),
            Ty::Float => format!("unbox_f64(&__get(env, \"{n}\")?)?"),
            Ty::Bool => format!("unbox_bool(&__get(env, \"{n}\")?)?"),
            Ty::Seq(e) => match e.as_ref() {
                Ty::Float => format!("unbox_seq_f64(&__get(env, \"{n}\")?)?"),
                Ty::Int => format!("unbox_seq_i64(&__get(env, \"{n}\")?)?"),
                _ => return None,
            },
            Ty::Map(_, v) => match v.as_ref() {
                Ty::Float => format!("unbox_map_f64(&__get(env, \"{n}\")?)?"),
                Ty::Int => format!("unbox_map_i64(&__get(env, \"{n}\")?)?"),
                _ => return None,
            },
            _ => return None,
        };
        let _ = writeln!(b, "            let __ol_{n} = {unbox};");
    }
    let call_args: Vec<String> = names
        .iter()
        .zip(sig.params.iter())
        .map(|(n, ty)| match ty {
            Ty::Seq(_) | Ty::Map(..) => format!("&__ol_{n}"),
            _ => format!("__ol_{n}"),
        })
        .collect();
    let _ = writeln!(b, "            let __res = {fn_name}({});", call_args.join(", "));
    let _ = writeln!(b, "            __bind(env, \"{name}\", {boxed_ret}, {mutable})?;");
    let _ = writeln!(b, "            __globalize(env, \"{name}\");");
    let _ = writeln!(b, "            Ok(())");
    let _ = writeln!(b, "        }})();");
    let _ = writeln!(b, "        if __outline.is_err() {{");
    let _ = writeln!(b, "            __bind(env, \"{name}\", {{ let __v = {}; __v }}, {mutable})?;", lower_expr(value));
    let _ = writeln!(b, "            __globalize(env, \"{name}\");");
    let _ = writeln!(b, "        }}");
    let _ = writeln!(b, "    }}");
    Some((fn_item, b))
}

/// Box a native value of type `ty`
fn box_value(ty: &Ty, code: &str) -> Option<String> {
    Some(match ty {
        Ty::Float => format!("box_f64({code})"),
        Ty::Int => format!("box_i64({code})"),
        Ty::Bool => format!("box_bool({code})"),
        Ty::Text => format!("box_text({code})"),
        Ty::Seq(e) => match e.as_ref() {
            Ty::Float => format!("box_seq_f64({code})"),
            Ty::Int => format!("box_seq_i64({code})"),
            Ty::Text => format!("box_seq_text({code})"),
            _ => return None,
        },
        Ty::Map(_, v) => match v.as_ref() {
            Ty::Float => format!("box_map_f64({code})"),
            Ty::Int => format!("box_map_i64({code})"),
            _ => return None,
        },
        _ => return None,
    })
}

// ------------------------------ statements ------------------------------

fn lower_stmt_top(stmt: &Stmt, out: &mut String, depth: usize) {
    let ind = "    ".repeat(depth);
    match stmt {
        Stmt::Bind { name, value, mutable } if depth == 1 => {
            let _ = writeln!(
                out,
                "{ind}__bind(env, \"{}\", {{ let __v = {}; __v }}, {})?;",
                name, lower_expr(value), *mutable,
            );
            let _ = writeln!(out, "{ind}__globalize(env, \"{}\");", name);
            return;
        }
        Stmt::Bind { name, value, mutable } => {
            let _ = writeln!(
                out,
                "{ind}__bind(env, \"{}\", {{ let __v = {}; __v }}, {})?;",
                name,
                lower_expr(value),
                if *mutable { "true" } else { "false" },
            );
        }
        Stmt::CompoundAssign { name, op, value } => {
            let op_method = match op {
                BinOp::Add => "add", BinOp::Sub => "sub",
                BinOp::Mul => "mul", BinOp::Div => "div",
                BinOp::Rem => "rem", _ => unreachable!(),
            };
            let _ = writeln!(
                out,
                "{ind}{{ let __cur = __get(env, \"{}\")?; let __new = __cur.{}(&{})?; __rebind(env, \"{}\", __new)?; }}",
                name, op_method, lower_expr(value), name,
            );
        }
        Stmt::IndexedAssign { target, idx, value } => {
            let _ = writeln!(
                out,
                "{ind}{{ let __coll = {}; let __idx = {}; let __val = {}; __index_set(&__coll, &__idx, __val)?; }}",
                lower_expr(target), lower_expr(idx), lower_expr(value),
            );
        }
        Stmt::MemberAssign { target, field, value } => {
            let _ = writeln!(
                out,
                "{ind}{{ let __obj = {}; let __val = {}; __member_set(&__obj, \"{}\", __val)?; }}",
                lower_expr(target), lower_expr(value), field,
            );
        }
        Stmt::IncDec { name, inc } => {
            let _ = writeln!(
                out,
                "{ind}{{ let __cur = __get(env, \"{}\")?; let __one = J2Value::Int(1); let __new = if {} {{ __cur.add(&__one)? }} else {{ __cur.sub(&__one)? }}; __rebind(env, \"{}\", __new)?; }}",
                name, *inc, name,
            );
        }
        Stmt::Func { name, params, body, .. } => {
            let body_src = lower_func_body(params, body);
            let _ = writeln!(out, "{ind}__bind(env, \"{}\", {}, true)?;", name, body_src);
        }
        Stmt::Global { name, value, mutable } => {
            // Global is a hint
            let _ = writeln!(
                out,
                "{ind}__bind(env, \"{}\", {}, {})?;",
                name, lower_expr(value), *mutable,
            );
        }
        Stmt::ForLoop { bindings, iter, filter, until, body } => {
            lower_for(bindings, iter, filter.as_ref(), until.as_ref(), body, out, depth);
        }
        Stmt::Repeat { cond, body } => {
            let _ = writeln!(out, "{ind}loop {{");
            let _ = writeln!(out, "{ind}    if !{}.truthy() {{ break; }}", lower_expr(cond));
            let _ = writeln!(out, "{ind}    let __scope_snap = __scope_enter(env);");
            for s in &body.stmts { lower_stmt_top(s, out, depth + 1); }
            let _ = writeln!(out, "{ind}    __scope_leave(env, __scope_snap);");
            let _ = writeln!(out, "{ind}}}");
        }
        Stmt::DoRepeat { body, cond } => {
            let _ = writeln!(out, "{ind}loop {{");
            let _ = writeln!(out, "{ind}    let __scope_snap = __scope_enter(env);");
            for s in &body.stmts { lower_stmt_top(s, out, depth + 1); }
            let _ = writeln!(out, "{ind}    __scope_leave(env, __scope_snap);");
            let _ = writeln!(out, "{ind}    if !{}.truthy() {{ break; }}", lower_expr(cond));
            let _ = writeln!(out, "{ind}}}");
        }
        Stmt::Loop { body } => {
            let _ = writeln!(out, "{ind}loop {{");
            let _ = writeln!(out, "{ind}    let __scope_snap = __scope_enter(env);");
            for s in &body.stmts { lower_stmt_top(s, out, depth + 1); }
            let _ = writeln!(out, "{ind}    __scope_leave(env, __scope_snap);");
            let _ = writeln!(out, "{ind}}}");
        }
        Stmt::If { cond, then_block, else_block } => {
            let _ = writeln!(out, "{ind}if {}.truthy() {{", lower_expr(cond));
            for s in &then_block.stmts { lower_stmt_top(s, out, depth + 1); }
            if let Some(eb) = else_block {
                let _ = writeln!(out, "{ind}}} else {{");
                for s in &eb.stmts { lower_stmt_top(s, out, depth + 1); }
            }
            let _ = writeln!(out, "{ind}}}");
        }
        Stmt::Stop { cond } => {
            match cond {
                Some(c) => { let _ = writeln!(out, "{ind}if {}.truthy() {{ break; }}", lower_expr(c)); }
                None => { let _ = writeln!(out, "{ind}break;"); }
            }
        }
        Stmt::Skip { cond } => {
            match cond {
                Some(c) => { let _ = writeln!(out, "{ind}if {}.truthy() {{ continue; }}", lower_expr(c)); }
                None => { let _ = writeln!(out, "{ind}continue;"); }
            }
        }
        Stmt::Give { value } => {
            // `give` stashes value, returns GiveSignal to unwind
            let v = match value { Some(v) => lower_expr(v), None => "J2Value::Null".to_string() };
            let _ = writeln!(out, "{ind}return Err(__give({}));", v);
        }
        Stmt::Try { body, typed_handlers, default_handler } => {
            let _ = writeln!(out, "{ind}{{");
            let _ = writeln!(out, "{ind}    let __res: J2Result<()> = (|| {{");
            for s in &body.stmts { lower_stmt_top(s, out, depth + 2); }
            let _ = writeln!(out, "{ind}        Ok(())");
            let _ = writeln!(out, "{ind}    }})();");
            let _ = writeln!(out, "{ind}    if let Err(__err) = __res {{");
            // A `give` signal must escape try/catch entirely
            let _ = writeln!(out, "{ind}        if __err.is_give_signal() {{ return Err(__err); }}");
            let _ = writeln!(out, "{ind}        let __kind_name = __err.kind.name();");
            let _ = writeln!(out, "{ind}        let __handled = (|| -> J2Result<bool> {{");
            // Class-typed handlers in order
            for h in typed_handlers {
                let _ = writeln!(out, "{ind}            if __kind_matches(__kind_name, \"{}\") {{", h.class_name);
                for s in &h.handler.stmts { lower_stmt_top(s, out, depth + 4); }
                let _ = writeln!(out, "{ind}                return Ok(true);");
                let _ = writeln!(out, "{ind}            }}");
            }
            let _ = writeln!(out, "{ind}            Ok(false)");
            let _ = writeln!(out, "{ind}        }})()?;");
            let _ = writeln!(out, "{ind}        if !__handled {{");
            if let Some(h) = default_handler {
                for s in &h.stmts { lower_stmt_top(s, out, depth + 3); }
            }
            let _ = writeln!(out, "{ind}        }}");
            let _ = writeln!(out, "{ind}    }}");
            let _ = writeln!(out, "{ind}}}");
        }
        Stmt::Assert { cond, msg } => {
            match msg {
                Some(m) => {
                    let _ = writeln!(
                        out,
                        "{ind}{{ if !({}).truthy() {{ return Err(J2Err::runtime(format!(\"assertion failed: {{}}\", {}))); }} }}",
                        lower_expr(cond), lower_expr(m),
                    );
                }
                None => {
                    let _ = writeln!(
                        out,
                        "{ind}{{ if !({}).truthy() {{ return Err(J2Err::runtime(String::from(\"assertion failed\"))); }} }}",
                        lower_expr(cond),
                    );
                }
            }
        }
        Stmt::Expr(e) => {
            let _ = writeln!(out, "{ind}{{ let __ = {}; let _ = __; }}", lower_expr(e));
        }
    }
}

fn lower_for(
    bindings: &[String],
    iter: &Expr,
    filter: Option<&Expr>,
    until: Option<&Expr>,
    body: &Block,
    out: &mut String,
    depth: usize,
) {
    let ind = "    ".repeat(depth);
    // body emitted once, spliced into both arms
    let mut body_src = String::new();
    {
        let out = &mut body_src;
        // per-iteration scope; body bindings dropped each iteration
        let _ = writeln!(out, "{ind}        let __scope_snap = __scope_enter(env);");
        // bind loop vars; multiple expect pair
        if bindings.len() == 1 {
            let _ = writeln!(out, "{ind}        __bind(env, \"{}\", __elem.clone(), true)?;", bindings[0]);
        } else if bindings.len() == 2 {
            let _ = writeln!(out, "{ind}        if let J2Value::Pair(__p) = &__elem {{");
            let _ = writeln!(out, "{ind}            __bind(env, \"{}\", __p.0.clone(), true)?;", bindings[0]);
            let _ = writeln!(out, "{ind}            __bind(env, \"{}\", __p.1.clone(), true)?;", bindings[1]);
            let _ = writeln!(out, "{ind}        }}");
        }
        // `if filter` skips elements; fn filters auto-applied
        if let Some(f) = filter {
            let _ = writeln!(
                out,
                "{ind}        {{ let __f = {}; let __keep = match &__f {{ J2Value::Func(_) | J2Value::Builtin(_, _) => __call(__f.clone(), &[__elem.clone()])?.truthy(), _ => __f.truthy() }}; if !__keep {{ __scope_leave(env, __scope_snap); continue; }} }}",
                lower_expr(f),
            );
        }
        // Body.
        for s in &body.stmts { lower_stmt_top(s, out, depth + 2); }
        // `until` breaks after body once true
        if let Some(u) = until {
            let _ = writeln!(out, "{ind}        if ({}).truthy() {{ __scope_leave(env, __scope_snap); break; }}", lower_expr(u));
        }
        let _ = writeln!(out, "{ind}        __scope_leave(env, __scope_snap);");
    }

    let _ = writeln!(out, "{ind}{{");
    let _ = writeln!(out, "{ind}    let __iter = {};", lower_expr(iter));
    // flows iterate lazily; others materialize
    let _ = writeln!(out, "{ind}    if let J2Value::Flow(__f) = &__iter {{");
    if until.is_none() {
        let _ = writeln!(out, "{ind}        if __f.lock().unwrap().is_infinite() {{ return Err(J2Err::infinite_flow(\"cannot consume an infinite flow without until\")); }}");
    }
    let _ = writeln!(out, "{ind}        loop {{");
    let _ = writeln!(out, "{ind}            let __next = __f.lock().unwrap().next()?;");
    let _ = writeln!(out, "{ind}            let Some(__elem) = __next else {{ break }};");
    out.push_str(&body_src);
    let _ = writeln!(out, "{ind}        }}");
    let _ = writeln!(out, "{ind}    }} else {{");
    let _ = writeln!(out, "{ind}        let __items: Vec<J2Value> = __materialize(&__iter)?;");
    let _ = writeln!(out, "{ind}        for __elem in __items.into_iter() {{");
    out.push_str(&body_src);
    let _ = writeln!(out, "{ind}        }}");
    let _ = writeln!(out, "{ind}    }}");
    let _ = writeln!(out, "{ind}}}");
}

/// IIFE body converting `give` signal to value
fn emit_give_block(s: &mut String, b: &Block) {
    s.push_str("    let __res: J2Result<J2Value> = (|| -> J2Result<J2Value> {\n");
    let mut last_is_expr = false;
    for (i, st) in b.stmts.iter().enumerate() {
        // track statement lines for in-function error reports
        if let Some(&ln) = b.stmt_lines.get(i) {
            if ln > 0 { let _ = writeln!(s, "        __set_line({});", ln); }
        }
        if i == b.stmts.len() - 1 {
            if let Stmt::Expr(e) = st {
                let _ = write!(s, "        return Ok({});\n", lower_expr(e));
                last_is_expr = true;
                continue;
            }
        }
        lower_stmt_top(st, s, 2);
    }
    if !last_is_expr {
        s.push_str("        Ok(J2Value::Null)\n");
    }
    s.push_str("    })();\n");
    s.push_str("    match __res { Err(__e) if __e.is_give_signal() => Ok(__take_give()), __r => __r }\n");
}

fn lower_func_body(params: &[Param], body: &FuncBody) -> String {
    // Emit a J2Value::Func with one clause
    let mut s = String::new();
    s.push_str("J2Value::Func(std::sync::Arc::new(J2Func { name: String::from(\"<anon>\"), arity: ");
    let _ = write!(s, "{}", params.len());
    s.push_str(", clauses: vec![J2FuncClause { ");
    // patterns
    s.push_str("patterns: vec![");
    for (i, p) in params.iter().enumerate() {
        if i > 0 { s.push(','); }
        match &p.pattern {
            Some(lit) => { let _ = write!(s, " Some({}) ", lower_expr(lit)); }
            None => { s.push_str(" None "); }
        }
    }
    s.push_str("], ");
    // param_names
    s.push_str("param_names: vec![");
    for (i, p) in params.iter().enumerate() {
        if i > 0 { s.push(','); }
        let _ = write!(s, " String::from(\"{}\") ", p.name);
    }
    s.push_str("], ");
    // body
    s.push_str("body: std::sync::Arc::new(|__args: &[J2Value]| -> J2Result<J2Value> {\n");
    s.push_str("    let env_rc = new_env_with_parent();\n");
    s.push_str("    let env: &EnvRef = &env_rc;\n");
    // bind params as fresh shadowing locals
    for (i, p) in params.iter().enumerate() {
        if p.pattern.is_none() {
            let _ = write!(s, "    __bind_param(env, \"{}\", __args[{}].clone());\n", p.name, i);
        }
    }
    match body {
        FuncBody::Expr(e) => {
            let _ = write!(s, "    Ok({})\n", lower_expr(e));
        }
        FuncBody::Block(b) => emit_give_block(&mut s, b),
    }
    s.push_str("}) }] }))");
    s
}

/// lower lambda with clone-snapshot capture of locals
fn lower_lambda_expr(params: &[Param], body: &FuncBody) -> String {
    // free vars = body refs minus params
    let mut free: HashSet<String> = HashSet::new();
    match body {
        FuncBody::Expr(e) => gather_all_idents(e, &mut free),
        FuncBody::Block(b) => { for st in &b.stmts { gather_all_idents_stmt(st, &mut free); } }
    }
    for p in params { free.remove(&p.name); }
    let mut free: Vec<String> = free.into_iter().collect();
    free.sort(); // deterministic emission

    let mut s = String::new();
    s.push_str("{\n");
    s.push_str("    let mut __cap: Vec<(String, J2Value)> = Vec::new();\n");
    for name in &free {
        let _ = write!(
            s,
            "    if let Ok(__v) = __get(env, \"{}\") {{ __cap.push((String::from(\"{}\"), __v)); }}\n",
            name, name,
        );
    }
    s.push_str("    J2Value::Func(std::sync::Arc::new(J2Func { name: String::from(\"<anon>\"), arity: ");
    let _ = write!(s, "{}", params.len());
    s.push_str(", clauses: vec![J2FuncClause { ");
    s.push_str("patterns: vec![");
    for (i, p) in params.iter().enumerate() {
        if i > 0 { s.push(','); }
        match &p.pattern {
            Some(lit) => { let _ = write!(s, " Some({}) ", lower_expr(lit)); }
            None => { s.push_str(" None "); }
        }
    }
    s.push_str("], ");
    s.push_str("param_names: vec![");
    for (i, p) in params.iter().enumerate() {
        if i > 0 { s.push(','); }
        let _ = write!(s, " String::from(\"{}\") ", p.name);
    }
    s.push_str("], ");
    s.push_str("body: std::sync::Arc::new(move |__args: &[J2Value]| -> J2Result<J2Value> {\n");
    s.push_str("    let env_rc = new_env_with_parent();\n");
    s.push_str("    let env: &EnvRef = &env_rc;\n");
    // Overlay captured free vars
    s.push_str("    for (__k, __v) in __cap.iter() { env.borrow_mut().table.insert(__k.clone(), (__v.clone(), true)); }\n");
    for (i, p) in params.iter().enumerate() {
        if p.pattern.is_none() {
            let _ = write!(s, "    __bind_param(env, \"{}\", __args[{}].clone());\n", p.name, i);
        }
    }
    match body {
        FuncBody::Expr(e) => { let _ = write!(s, "    Ok({})\n", lower_expr(e)); }
        FuncBody::Block(b) => emit_give_block(&mut s, b),
    }
    s.push_str("}) }] }))\n}");
    s
}

// ------------------------------ expressions ------------------------------

fn lower_expr(e: &Expr) -> String {
    match e {
        Expr::IntLit(n) => format!("J2Value::Int({}i64)", n),
        Expr::FloatLit(x) => format!("J2Value::Float({}f64)", x),
        Expr::TextLit(s) => format!("J2Value::text({:?})", s),
        Expr::Bool(b) => format!("J2Value::Bool({})", b),
        Expr::Null => "J2Value::Null".to_string(),
        Expr::Underscore => "__get(env, \"_\")?".to_string(),
        Expr::Const(c) => match c {
            BuiltinConst::Pi => "J2Value::Float(std::f64::consts::PI)".into(),
            BuiltinConst::E => "J2Value::Float(std::f64::consts::E)".into(),
            BuiltinConst::Tau => "J2Value::Float(std::f64::consts::TAU)".into(),
            BuiltinConst::Inf => "J2Value::Float(f64::INFINITY)".into(),
            BuiltinConst::Nan => "J2Value::Float(f64::NAN)".into(),
            BuiltinConst::MaxVal => "J2Value::Float(f64::MAX)".into(),
            BuiltinConst::MinVal => "J2Value::Float(f64::MIN)".into(),
        },
        Expr::Ident(name) => format!("__get(env, \"{}\")?", name),
        Expr::Binary { op, lhs, rhs } => {
            let l = lower_expr(lhs);
            let r = lower_expr(rhs);
            match op {
                BinOp::Add => format!("({}).add(&{})?", l, r),
                BinOp::Sub => format!("({}).sub(&{})?", l, r),
                BinOp::Mul => format!("({}).mul(&{})?", l, r),
                BinOp::Div => format!("({}).div(&{})?", l, r),
                BinOp::Rem => format!("({}).rem(&{})?", l, r),
                BinOp::Pow => format!("({}).pow(&{})?", l, r),
                BinOp::Eq => format!("J2Value::Bool(({}).eq(&{}))", l, r),
                BinOp::NotEq => format!("J2Value::Bool(!({}).eq(&{}))", l, r),
                BinOp::Lt => format!("J2Value::Bool(({}).cmp_lt(&{})?)", l, r),
                BinOp::Gt => format!("J2Value::Bool(({}).cmp_lt(&({}))? && !({}).eq(&{}))", &r, &l, &l, &r),
                BinOp::LtEq => format!("{{ let __a = {}; let __b = {}; J2Value::Bool(__a.cmp_lt(&__b)? || __a.eq(&__b)) }}", l, r),
                BinOp::GtEq => format!("{{ let __a = {}; let __b = {}; J2Value::Bool(__b.cmp_lt(&__a)? || __a.eq(&__b)) }}", l, r),
                BinOp::And => format!("{{ let __a = {}; if __a.truthy() {{ {} }} else {{ __a }} }}", l, r),
                BinOp::Or => format!("{{ let __a = {}; if __a.truthy() {{ __a }} else {{ {} }} }}", l, r),
                BinOp::BAnd => format!("({}).band(&{})?", l, r),
                BinOp::BOr  => format!("({}).bor(&{})?",  l, r),
                BinOp::BXor => format!("({}).bxor(&{})?", l, r),
                BinOp::Shl  => format!("({}).shl(&{})?",  l, r),
                BinOp::Shr  => format!("({}).shr(&{})?",  l, r),
            }
        }
        Expr::Unary { op, operand } => match op {
            UnaryOp::Neg => format!("({}).neg()?", lower_expr(operand)),
            UnaryOp::Pos => format!("({})", lower_expr(operand)),
            UnaryOp::Not => format!("J2Value::Bool(!({}).truthy())", lower_expr(operand)),
            UnaryOp::BNot => format!("({}).bnot()?", lower_expr(operand)),
        },
        Expr::Call { callee, args } => {
            let cs = lower_expr(callee);
            let mut argbuf = String::new();
            for (i, a) in args.iter().enumerate() {
                if i > 0 { argbuf.push(','); }
                argbuf.push_str(&lower_expr(a));
            }
            format!("__call({}, &[{}])?", cs, argbuf)
        }
        Expr::Index { coll, idx } => format!("__index(&{}, &{})?", lower_expr(coll), lower_expr(idx)),
        Expr::Member { obj, field } => format!("__member(&{}, \"{}\")?", lower_expr(obj), field),
        Expr::Range { start, end } => match end {
            Some(e) => format!("__range({}, Some({}))?", lower_expr(start), lower_expr(e)),
            None => format!("__range({}, None)?", lower_expr(start)),
        },
        Expr::SeqLit(items) => {
            let mut s = String::from("J2Value::seq(vec![");
            for (i, it) in items.iter().enumerate() {
                if i > 0 { s.push(','); }
                s.push_str(&lower_expr(it));
            }
            s.push_str("])");
            s
        }
        Expr::MapLit(entries) => {
            let mut s = String::from("J2Value::map(vec![");
            for (i, (k, v)) in entries.iter().enumerate() {
                if i > 0 { s.push(','); }
                let _ = write!(s, "(String::from(\"{}\"), {})", k, lower_expr(v));
            }
            s.push_str("])");
            s
        }
        Expr::Pair(a, b) => format!("J2Value::pair({}, {})", lower_expr(a), lower_expr(b)),
        Expr::Pipe { lhs, rhs } => {
            // map if seq/flow, else apply f(x)
            format!(
                "__pipe({}, {})?",
                lower_expr(lhs),
                lower_expr(rhs),
            )
        }
        Expr::Filter { lhs, rhs } => format!("__filter({}, {})?", lower_expr(lhs), lower_expr(rhs)),
        Expr::Block(b) => {
            let mut s = String::from("{\n");
            for (i, st) in b.stmts.iter().enumerate() {
                if i == b.stmts.len() - 1 {
                    if let Stmt::Expr(e) = st {
                        let _ = write!(s, "    {}\n", lower_expr(e));
                        continue;
                    }
                }
                lower_stmt_top(st, &mut s, 1);
            }
            s.push_str("}");
            s
        }
        Expr::CondLambda(inner) => {
            // The body uses `_`
            format!(
                "J2Value::Cond(std::sync::Arc::new(|__x: &J2Value| -> J2Result<bool> {{ \
                    let env_rc = new_env_with_parent(); \
                    let env: &EnvRef = &env_rc; \
                    __bind(env, \"_\", __x.clone(), true)?; \
                    Ok(({}).truthy()) \
                }}))",
                lower_expr(inner)
            )
        }
        Expr::Lambda { params, body } => {
            // lambda with lexical capture, matching native HOF
            lower_lambda_expr(params, body.as_ref())
        }
        Expr::NativeBlock(raw) => {
            format!(
                "(|| -> J2Result<J2Value> {{ Ok({{ {} }}) }})()?",
                raw
            )
        }
        Expr::ClassLit { extends, fields, methods, statics } => {
            let mut s = String::from("{\n");
            s.push_str("    let mut __defaults: std::collections::HashMap<String, J2Value> = std::collections::HashMap::new();\n");
            for f in fields {
                let v = match &f.default {
                    Some(d) => lower_expr(d),
                    None => "J2Value::Null".to_string(),
                };
                let _ = write!(s, "    __defaults.insert(String::from(\"{}\"), {});\n", f.name, v);
            }
            s.push_str("    let mut __methods: std::collections::HashMap<String, std::sync::Arc<j2_runtime::value::J2ClassMethod>> = std::collections::HashMap::new();\n");
            for m in methods {
                let body_src = lower_func_body(&m.params, &m.body);
                let _ = write!(s, "    __methods.insert(String::from(\"{}\"), std::sync::Arc::new(j2_runtime::value::J2ClassMethod {{ name: String::from(\"{}\"), mutating: {}, func: match {} {{ J2Value::Func(g) => g, _ => unreachable!() }} }}));\n", m.name, m.name, m.mutating, body_src);
            }
            s.push_str("    let mut __statics: std::collections::HashMap<String, J2Value> = std::collections::HashMap::new();\n");
            for st in statics {
                let _ = write!(s, "    __statics.insert(String::from(\"{}\"), {});\n", st.name, lower_expr(&st.value));
            }
            s.push_str("    let mut __extends: Vec<String> = Vec::new();\n");
            for e in extends {
                let _ = write!(s, "    __extends.push(String::from(\"{}\"));\n", e);
            }
            s.push_str("    let mut __field_order: Vec<String> = Vec::new();\n");
            for f in fields {
                let _ = write!(s, "    __field_order.push(String::from(\"{}\"));\n", f.name);
            }
            s.push_str("    J2Value::Class(std::sync::Arc::new(j2_runtime::value::J2Class {\n");
            s.push_str("        name: String::from(\"<class>\"),\n");
            s.push_str("        extends: __extends,\n");
            s.push_str("        field_order: __field_order,\n");
            s.push_str("        field_defaults: __defaults,\n");
            s.push_str("        methods: __methods,\n");
            s.push_str("        statics: std::sync::Mutex::new(__statics),\n");
            s.push_str("    }))\n");
            s.push_str("}");
            s
        }
        Expr::StaticAccess { class_name, member } => {
            // `Class::member` static lookup
            format!(
                "__static_get(&__get(env, \"{}\")?, \"{}\")?",
                class_name, member
            )
        }
    }
}

// native lowering ============================== The typed subset

/// native fn items plus boxing shim
struct NativeEmit {
    items: String,
    shim: String,
}

/// A native class field
#[derive(Clone)]
struct ClassFieldSig {
    name: String,
    ty: Ty,
    default: Option<Expr>,
}

/// A native class method signature.
#[derive(Clone)]
struct ClassMethodSig {
    params: Vec<(String, Ty)>,
    ret: Ty,
    /// `&mut self` for `:=`, `&self` for `=`
    borrow_self: Borrow,
    body: FuncBody,
}

/// The resolved signature of a native-lowered class.
#[derive(Clone)]
struct ClassSig {
    name: String,
    fields: Vec<ClassFieldSig>,
    methods: HashMap<String, ClassMethodSig>,
}

type Classes = HashMap<String, ClassSig>;

/// native lowering context; types, usize locals, sigs
struct NCtx<'a> {
    gamma: HashMap<String, Ty>,
    usize_vars: HashSet<String>,
    /// seq bindings borrowable as `&mut [T]`
    mut_seqs: HashSet<String>,
    fns: &'a NativeFns,
    /// Generic functions whose *both* monomorphizations
    safe_gen: &'a HashSet<String>,
    /// type of `_` in cond-lambda, else `None`
    underscore: Option<Ty>,
    /// Owned, non-`Copy` `String` locals
    owned_strings: HashSet<String>,
    /// Known native classes (struct lowering targets).
    classes: &'a Classes,
    /// enclosing class; bare field names become `self.field`
    self_fields: Option<&'a ClassSig>,
    /// True inside a `:=`
    self_mut: bool,
    ret: Ty,
    /// hoisted loop-invariant lets; MIR matchers need `Operand::Copy`
    prologue: Vec<String>,
    /// counter for unique hoisted names
    tmp_ctr: usize,
    /// Whether hoisting into `prologue` is allowed here
    can_hoist: bool,
    /// Names assigned anywhere in the fn body
    assigned: Option<HashSet<String>>,
    /// CSE cache of hoisted `as usize` casts
    cast_hoist: HashMap<String, String>,
}

type NRes<T> = Result<T, ()>;

/// let/return backend type; `Nil` gives `None`
fn rust_ty(ty: &Ty) -> NRes<Option<String>> {
    Ok(match ty {
        Ty::Int => Some("i64".into()),
        Ty::Float => Some("f64".into()),
        Ty::Bool => Some("bool".into()),
        Ty::Text => Some("String".into()),
        Ty::Nil => None,
        Ty::Seq(e) => match e.as_ref() {
            Ty::Float => Some("Vec<f64>".into()),
            Ty::Int => Some("Vec<i64>".into()),
            Ty::Text => Some("Vec<String>".into()),
            _ => return Err(()),
        },
        Ty::Pair(a, b) => {
            let ra = rust_ty(a)?.ok_or(())?;
            let rb = rust_ty(b)?.ok_or(())?;
            Some(format!("({}, {})", ra, rb))
        }
        Ty::Map(k, v) => Some(format!("HashMap<String, {}>", map_value_rust(k, v)?)),
        Ty::Class(n) => Some(n.clone()),
        _ => return Err(()),
    })
}

/// value type for native `map<text, V>`
fn map_value_rust(k: &Ty, v: &Ty) -> NRes<&'static str> {
    if *k != Ty::Text {
        return Err(());
    }
    match v {
        Ty::Float => Ok("f64"),
        Ty::Int => Ok("i64"),
        _ => Err(()),
    }
}

/// param backend type honoring borrow mode
fn param_rust_ty(ty: &Ty, borrow: Borrow) -> NRes<String> {
    Ok(match (ty, borrow) {
        (Ty::Int, Borrow::ByVal) => "i64".into(),
        (Ty::Float, Borrow::ByVal) => "f64".into(),
        (Ty::Bool, Borrow::ByVal) => "bool".into(),
        (Ty::Text, _) => "&str".into(),
        (Ty::Map(k, v), Borrow::Mut) => format!("&mut HashMap<String, {}>", map_value_rust(k, v)?),
        (Ty::Map(k, v), _) => format!("&HashMap<String, {}>", map_value_rust(k, v)?),
        (Ty::Class(n), Borrow::Mut) => format!("&mut {}", n),
        (Ty::Class(n), Borrow::ByVal) => n.clone(),
        (Ty::Class(n), Borrow::Shared) => format!("&{}", n),
        (Ty::Seq(e), Borrow::Shared) => match e.as_ref() {
            Ty::Float => "&[f64]".into(),
            Ty::Int => "&[i64]".into(),
            Ty::Text => "&[String]".into(),
            _ => return Err(()),
        },
        (Ty::Seq(e), Borrow::Mut) => match e.as_ref() {
            Ty::Float => "&mut [f64]".into(),
            Ty::Int => "&mut [i64]".into(),
            Ty::Text => "&mut [String]".into(),
            _ => return Err(()),
        },
        _ => return Err(()),
    })
}

fn is_numeric(t: &Ty) -> bool {
    matches!(t, Ty::Int | Ty::Float)
}

/// compound-assignment operator for foldable op
fn compound_op_str(op: &BinOp) -> Option<&'static str> {
    match op {
        BinOp::Add => Some("+="),
        BinOp::Sub => Some("-="),
        BinOp::Mul => Some("*="),
        BinOp::Div => Some("/="),
        BinOp::Rem => Some("%="),
        _ => None,
    }
}

/// box native scalar/text into `J2Value`
fn box_scalar(ty: &Ty, code: &str) -> NRes<String> {
    Ok(match ty {
        Ty::Float => format!("box_f64({code})"),
        Ty::Int => format!("box_i64({code})"),
        Ty::Bool => format!("box_bool({code})"),
        Ty::Text => format!("box_text({code})"),
        _ => return Err(()),
    })
}

/// coerce types; only int to float implicit
fn coerce(code: String, from: &Ty, to: &Ty) -> NRes<String> {
    if from == to {
        return Ok(code);
    }
    match (from, to) {
        (Ty::Int, Ty::Float) => Ok(format!("({} as f64)", code)),
        _ => Err(()),
    }
}

/// mangle backend reserved words (`move` to `move_`)
fn mangle(name: &str) -> String {
    matches!(
        name,
        "move" | "type" | "loop" | "match" | "fn" | "let" | "ref" | "self" | "struct" | "impl"
        | "trait" | "use" | "mod" | "pub" | "as" | "in" | "if" | "else" | "for" | "while"
        | "return" | "where" | "async" | "await" | "dyn" | "box" | "const" | "static" | "enum"
        | "true" | "false" | "crate" | "super" | "unsafe" | "extern"
    )
    .then(|| format!("{}_", name))
    .unwrap_or_else(|| name.to_string())
}

/// unbox field getter; bail unless class survives
fn unbox_field_expr(ty: &Ty, getter: &str, classes: &Classes) -> NRes<String> {
    Ok(match ty {
        Ty::Float => format!("unbox_f64({getter})?"),
        Ty::Int => format!("unbox_i64({getter})?"),
        Ty::Bool => format!("unbox_bool({getter})?"),
        Ty::Text => format!("unbox_text({getter})?"),
        Ty::Seq(e) => match e.as_ref() {
            Ty::Float => format!("unbox_seq_f64({getter})?"),
            Ty::Int => format!("unbox_seq_i64({getter})?"),
            Ty::Text => format!("unbox_seq_text({getter})?"),
            _ => return Err(()),
        },
        Ty::Map(_, v) => match v.as_ref() {
            Ty::Float => format!("unbox_map_f64({getter})?"),
            Ty::Int => format!("unbox_map_i64({getter})?"),
            _ => return Err(()),
        },
        Ty::Class(n) if classes.contains_key(n) => format!("unbox_instance_{n}({getter})?"),
        _ => return Err(()),
    })
}

/// Box a struct-field value
fn box_field_expr(ty: &Ty, code: &str, classes: &Classes) -> NRes<String> {
    Ok(match ty {
        Ty::Float => format!("box_f64({code})"),
        Ty::Int => format!("box_i64({code})"),
        Ty::Bool => format!("box_bool({code})"),
        Ty::Text => format!("box_text({code})"),
        Ty::Seq(e) => match e.as_ref() {
            Ty::Float => format!("box_seq_f64({code})"),
            Ty::Int => format!("box_seq_i64({code})"),
            Ty::Text => format!("box_seq_text({code})"),
            _ => return Err(()),
        },
        Ty::Map(_, v) => match v.as_ref() {
            Ty::Float => format!("box_map_f64({code})"),
            Ty::Int => format!("box_map_i64({code})"),
            _ => return Err(()),
        },
        Ty::Class(n) if classes.contains_key(n) => format!("box_instance_{n}({code})?"),
        _ => return Err(()),
    })
}

/// Emit a native class
fn lower_native_class(csig: &ClassSig, fns: &NativeFns, safe_gen: &HashSet<String>, classes: &Classes) -> NRes<String> {
    let cn = &csig.name;
    let mut out = String::new();

    // bail unless every class ref survives natively
    for f in &csig.fields {
        if !class_refs_resolved(&f.ty, classes) {
            return Err(());
        }
    }
    for m in csig.methods.values() {
        for (_, pt) in &m.params {
            if !class_refs_resolved(pt, classes) {
                return Err(());
            }
        }
        if !class_refs_resolved(&m.ret, classes) {
            return Err(());
        }
    }

    // (a) struct decl
    let _ = writeln!(out, "#[allow(non_snake_case, dead_code)]\n#[derive(Clone)]\nstruct {cn} {{");
    for f in &csig.fields {
        let rt = rust_ty(&f.ty)?.ok_or(())?;
        let _ = writeln!(out, "    {}: {},", mangle(&f.name), rt);
    }
    let _ = writeln!(out, "}}");

    // (b) impl with methods
    let _ = writeln!(out, "#[allow(unused_mut, unused_variables, unused_parens, non_snake_case, dead_code)]\nimpl {cn} {{");
    for (mname, m) in &csig.methods {
        let self_p = if m.borrow_self == Borrow::Mut { "&mut self" } else { "&self" };
        let mut gamma: HashMap<String, Ty> = HashMap::new();
        let mut mut_seqs: HashSet<String> = HashSet::new();
        let mut owned_strings: HashSet<String> = HashSet::new();
        let mut plist = String::new();
        for (pn, pt) in &m.params {
            let borrow = match pt {
                Ty::Seq(_) | Ty::Map(..) | Ty::Text => Borrow::Shared,
                _ => Borrow::ByVal,
            };
            let prt = param_rust_ty(pt, borrow)?;
            let _ = write!(plist, ", {}: {}", mangle(pn), prt);
            gamma.insert(pn.clone(), pt.clone());
            let _ = (&mut mut_seqs, &mut owned_strings);
        }
        let ret_str = match rust_ty(&m.ret)? { Some(rt) => format!(" -> {}", rt), None => String::new() };
        let mut ctx = NCtx {
            gamma,
            usize_vars: HashSet::new(),
            mut_seqs,
            fns,
            safe_gen,
            underscore: None,
            owned_strings,
            classes,
            self_fields: Some(csig),
            self_mut: m.borrow_self == Borrow::Mut,
            ret: m.ret.clone(),
            prologue: Vec::new(),
            tmp_ctr: 0,
            can_hoist: true,
            assigned: None,
            cast_hoist: HashMap::new(),
        };
        let mut body_src = String::new();
        match &m.body {
            FuncBody::Expr(e) => {
                let (code, ty) = nlower_expr(&mut ctx, e)?;
                if m.ret == Ty::Nil {
                    let _ = writeln!(body_src, "        let _ = {};", code);
                } else {
                    let c = coerce(code, &ty, &m.ret)?;
                    let _ = writeln!(body_src, "        {}", c);
                }
            }
            FuncBody::Block(b) => {
                { let mut __a = HashSet::new(); collect_assigned_set(&b.stmts, &mut __a); ctx.assigned = Some(__a); }
            nlower_fn_block(&mut ctx, &b.stmts, &mut body_src, 2)?;
            }
        }
        body_src = flush_prologue(&ctx, "        ", body_src);
        let _ = writeln!(out, "    fn {}({}{}){} {{\n{}    }}", mangle(mname), self_p, plist, ret_str, body_src);
    }
    let _ = writeln!(out, "}}");

    // (c) boundary: J2Instance <-> struct.
    let _ = writeln!(out, "#[allow(dead_code, non_snake_case)]\nfn unbox_instance_{cn}(__v: &J2Value) -> J2Result<{cn}> {{");
    let _ = writeln!(out, "    match __v {{");
    let _ = writeln!(out, "        J2Value::Instance(__inst) => {{ let __i = __inst.lock().unwrap(); Ok({cn} {{");
    for f in &csig.fields {
        let getter = format!("__i.fields.get({:?}).ok_or_else(|| J2Err::key(\"missing field {}\"))?", f.name, f.name);
        let conv = unbox_field_expr(&f.ty, &getter, classes)?;
        let _ = writeln!(out, "            {}: {},", mangle(&f.name), conv);
    }
    let _ = writeln!(out, "        }}) }}");
    let _ = writeln!(out, "        _ => Err(J2Err::type_err(format!(\"expected {cn}, got {{}}\", __v.type_name()))),");
    let _ = writeln!(out, "    }}\n}}");

    let _ = writeln!(out, "#[allow(dead_code, non_snake_case)]\nfn box_instance_{cn}(__p: {cn}) -> J2Result<J2Value> {{");
    let _ = writeln!(out, "    let mut __fields: HashMap<String, J2Value> = HashMap::new();");
    for f in &csig.fields {
        let boxed = box_field_expr(&f.ty, &format!("__p.{}", mangle(&f.name)), classes)?;
        let _ = writeln!(out, "    __fields.insert(String::from({:?}), {});", f.name, boxed);
    }
    let _ = writeln!(out, "    let __class = __class_for({:?})?;", cn);
    let _ = writeln!(out, "    Ok(J2Value::Instance(std::sync::Arc::new(std::sync::Mutex::new(j2_runtime::value::J2Instance {{ class: __class, fields: __fields }}))))");
    let _ = writeln!(out, "}}");

    let _ = writeln!(out, "#[allow(dead_code, non_snake_case)]\nfn rebox_instance_{cn}(__orig: &J2Value, __p: {cn}) -> J2Result<()> {{");
    let _ = writeln!(out, "    match __orig {{");
    let _ = writeln!(out, "        J2Value::Instance(__inst) => {{ let mut __i = __inst.lock().unwrap();");
    for f in &csig.fields {
        let boxed = box_field_expr(&f.ty, &format!("__p.{}", mangle(&f.name)), classes)?;
        let _ = writeln!(out, "            __i.fields.insert(String::from({:?}), {});", f.name, boxed);
    }
    let _ = writeln!(out, "            Ok(()) }}");
    let _ = writeln!(out, "        _ => Err(J2Err::type_err(\"cannot rebox non-instance\")),");
    let _ = writeln!(out, "    }}\n}}");

    Ok(out)
}

/// HOF as generic `Fn` item, zero-cost
fn lower_native_hof(
    native_name: &str,
    body: &FuncBody,
    sig: &FnSig,
    fns: &NativeFns,
    safe_gen: &HashSet<String>,
    classes: &Classes,
) -> NRes<String> {
    // No mixing element-genericity (`T`) with function-genericity.
    if first_var_name(sig).is_some() {
        return Err(());
    }
    let mut gamma: HashMap<String, Ty> = HashMap::new();
    let mut mut_seqs: HashSet<String> = HashSet::new();
    let mut generics: Vec<String> = Vec::new();
    let mut param_list: Vec<String> = Vec::new();
    let mut fi = 0usize;
    for ((n, t), b) in sig
        .param_names
        .iter()
        .zip(sig.params.iter())
        .zip(sig.borrows.iter())
    {
        gamma.insert(n.clone(), t.clone());
        match t {
            Ty::Func(ps, r) => {
                let mut arg_tys = Vec::with_capacity(ps.len());
                for p in ps {
                    arg_tys.push(scalar_rust(p)?.to_string());
                }
                let ret = match r.as_ref() {
                    Ty::Nil => "()".to_string(),
                    o => scalar_rust(o)?.to_string(),
                };
                let g = format!("F{fi}");
                // `Copy` first avoids parse ambiguity; allows reuse
                generics.push(format!("{g}: Copy + Fn({}) -> {}", arg_tys.join(", "), ret));
                param_list.push(format!("{n}: {g}"));
                fi += 1;
            }
            _ => {
                if matches!(t, Ty::Seq(_)) && *b == Borrow::Mut {
                    mut_seqs.insert(n.clone());
                }
                let mutkw = if *b == Borrow::ByVal { "mut " } else { "" };
                param_list.push(format!("{mutkw}{n}: {}", param_rust_ty(t, *b)?));
            }
        }
    }
    if generics.is_empty() {
        return Err(());  // not actually a HOF - shouldn't happen
    }
    let mut ctx = NCtx {
        gamma,
        usize_vars: HashSet::new(),
        mut_seqs,
        fns,
        safe_gen,
        underscore: None,
        owned_strings: HashSet::new(),
        classes,
        self_fields: None,
        self_mut: false,
        ret: sig.ret.clone(),
        prologue: Vec::new(),
        tmp_ctr: 0,
        can_hoist: true,
        assigned: None,
        cast_hoist: HashMap::new(),
    };
    let mut body_src = String::new();
    match body {
        FuncBody::Expr(e) => {
            let (code, ty) = nlower_expr(&mut ctx, e)?;
            if sig.ret == Ty::Nil {
                let _ = writeln!(body_src, "    let _ = {};", code);
            } else {
                let c = coerce(code, &ty, &sig.ret)?;
                let _ = writeln!(body_src, "    {}", c);
            }
        }
        FuncBody::Block(b) => {
            { let mut __a = HashSet::new(); collect_assigned_set(&b.stmts, &mut __a); ctx.assigned = Some(__a); }
            nlower_fn_block(&mut ctx, &b.stmts, &mut body_src, 1)?;
        }
    }
    body_src = flush_prologue(&ctx, "    ", body_src);
    let ret_str = match rust_ty(&sig.ret)? {
        Some(rt) => format!(" -> {}", rt),
        None => String::new(),
    };
    Ok(format!(
        "#[allow(dead_code, unused_mut, unused_variables, unused_parens, non_snake_case)]\nfn {native_name}<{generics}>({params}){ret_str} {{\n{body_src}}}\n",
        generics = generics.join(", "),
        params = param_list.join(", "),
    ))
}

fn lower_native_fn(
    name: &str,
    _params: &[Param],
    body: &FuncBody,
    sig: &FnSig,
    type_params: &[String],
    fns: &NativeFns,
    safe_gen: &HashSet<String>,
    classes: &Classes,
) -> NRes<NativeEmit> {
    // instance-aliasing guard; class in and out bails
    if matches!(sig.ret, Ty::Class(_)) && sig.params.iter().any(|t| matches!(t, Ty::Class(_))) {
        return Err(());
    }
    // HOFs get generic item, no boxing shim
    if is_hof_sig(sig) {
        if !type_params.is_empty() {
            return Err(()); // no element-genericity mixed with function params
        }
        let items = lower_native_hof(&format!("{name}__native"), body, sig, fns, safe_gen, classes)?;
        return Ok(NativeEmit { items, shim: String::new() });
    }
    if !type_params.is_empty() && type_params.len() != 1 {
        return Err(()); // only single explicit type params supported
    }
    // The effective generic variable
    let var: Option<String> = if !type_params.is_empty() {
        Some(type_params[0].clone())
    } else {
        first_var_name(sig)
    };
    let var = match var {
        None => {
            // non-generic; one item plus shim
            let item = lower_native_fn_item(&format!("{name}__native"), body, sig, fns, safe_gen, classes)?;
            let shim = build_shim(name, sig, classes)?;
            return Ok(NativeEmit { items: item, shim });
        }
        Some(v) => v,
    };
    let var = &var;
    // generic; monomorphize f64/i64, shim dispatches runtime type
    let rep_idx = sig
        .params
        .iter()
        .position(|t| ty_mentions_var(t, var))
        .ok_or(())?;
    // Emit only the monomorphizations that actually lower
    let mut items = String::new();
    let mut have_f64 = false;
    let mut have_i64 = false;
    for (suffix, concrete) in [("f64", Ty::Float), ("i64", Ty::Int)] {
        let msig = subst_sig(sig, var, &concrete);
        if let Ok(item) = lower_native_fn_item(&format!("{name}__native_{suffix}"), body, &msig, fns, safe_gen, classes) {
            items.push_str(&item);
            items.push('\n');
            if suffix == "f64" { have_f64 = true; } else { have_i64 = true; }
        }
    }
    if !have_f64 && !have_i64 {
        return Err(());
    }
    let shim = build_generic_shim(name, sig, rep_idx, var, have_f64, have_i64, classes)?;
    Ok(NativeEmit { items, shim })
}

/// emit native fn item for concrete sig
fn lower_native_fn_item(
    native_name: &str,
    body: &FuncBody,
    sig: &FnSig,
    fns: &NativeFns,
    safe_gen: &HashSet<String>,
    classes: &Classes,
) -> NRes<String> {
    let mut gamma: HashMap<String, Ty> = HashMap::new();
    let mut mut_seqs: HashSet<String> = HashSet::new();
    let mut owned_strings: HashSet<String> = HashSet::new();
    for ((n, t), b) in sig
        .param_names
        .iter()
        .zip(sig.params.iter())
        .zip(sig.borrows.iter())
    {
        gamma.insert(n.clone(), t.clone());
        if matches!(t, Ty::Seq(_)) && *b == Borrow::Mut {
            mut_seqs.insert(n.clone());
        }
        // A by-value `String` param would be non-Copy
        let _ = &mut owned_strings;
    }
    let mut ctx = NCtx {
        gamma,
        usize_vars: HashSet::new(),
        mut_seqs,
        fns,
        safe_gen,
        underscore: None,
        owned_strings,
        classes,
        self_fields: None,
        self_mut: false,
        ret: sig.ret.clone(),
        prologue: Vec::new(),
        tmp_ctr: 0,
        can_hoist: true,
        assigned: None,
        cast_hoist: HashMap::new(),
    };

    let mut body_src = String::new();
    match body {
        FuncBody::Expr(e) => {
            let (code, ty) = nlower_expr(&mut ctx, e)?;
            if sig.ret == Ty::Nil {
                let _ = writeln!(body_src, "    let _ = {};", code);
            } else {
                let c = coerce(code, &ty, &sig.ret)?;
                let _ = writeln!(body_src, "    {}", c);
            }
        }
        FuncBody::Block(b) => {
            { let mut __a = HashSet::new(); collect_assigned_set(&b.stmts, &mut __a); ctx.assigned = Some(__a); }
            nlower_fn_block(&mut ctx, &b.stmts, &mut body_src, 1)?;
        }
    }
    body_src = flush_prologue(&ctx, "    ", body_src);

    let mut param_list = String::new();
    for (i, ((n, t), b)) in sig
        .param_names
        .iter()
        .zip(sig.params.iter())
        .zip(sig.borrows.iter())
        .enumerate()
    {
        if i > 0 {
            param_list.push_str(", ");
        }
        // by-value scalars `mut` for accumulators, reassignment
        let mutkw = if *b == Borrow::ByVal { "mut " } else { "" };
        let _ = write!(param_list, "{}{}: {}", mutkw, n, param_rust_ty(t, *b)?);
    }
    let ret_str = match rust_ty(&sig.ret)? {
        Some(rt) => format!(" -> {}", rt),
        None => String::new(),
    };
    // parallelize attr behind cfg; J2_NO_NESTED kill-switch
    let suppress_nested = std::env::var("J2_NO_NESTED").map(|v| v == "1").unwrap_or(false);
    let wants_attr = sig
        .params
        .iter()
        .any(|t| matches!(t, Ty::Seq(e) if **e == Ty::Float))
        && (!suppress_nested || !body_has_nested_loop(body));
    let attr = if wants_attr {
        "#[cfg_attr(j2_parallelize, parallelize_associative_float)]\n"
    } else {
        ""
    };
    Ok(format!(
        "{attr}#[allow(unused_mut, unused_variables, unused_parens, non_snake_case)]\nfn {native_name}({param_list}){ret_str} {{\n{body_src}}}\n",
    ))
}

/// first generic var name in sig
fn first_var_name(sig: &FnSig) -> Option<String> {
    fn find(ty: &Ty) -> Option<String> {
        match ty {
            Ty::Var(v) => Some(v.clone()),
            Ty::Seq(e) => find(e),
            Ty::Pair(a, b) => find(a).or_else(|| find(b)),
            Ty::Map(k, v) => find(k).or_else(|| find(v)),
            Ty::Func(ps, r) => ps.iter().find_map(find).or_else(|| find(r)),
            _ => None,
        }
    }
    sig.params.iter().find_map(find).or_else(|| find(&sig.ret))
}

/// nested loop present; withholds parallelize attr
fn body_has_nested_loop(body: &FuncBody) -> bool {
    fn loop_in_stmts(stmts: &[Stmt]) -> bool {
        stmts.iter().any(stmt_has_loop)
    }
    fn stmt_has_loop(s: &Stmt) -> bool {
        match s {
            Stmt::ForLoop { .. } | Stmt::Repeat { .. } | Stmt::DoRepeat { .. } | Stmt::Loop { .. } => true,
            Stmt::If { then_block, else_block, .. } => {
                loop_in_stmts(&then_block.stmts)
                    || else_block.as_ref().map_or(false, |b| loop_in_stmts(&b.stmts))
            }
            _ => false,
        }
    }
    fn stmt_has_nested(s: &Stmt) -> bool {
        match s {
            Stmt::ForLoop { body, .. } => loop_in_stmts(&body.stmts) || body.stmts.iter().any(stmt_has_nested),
            Stmt::Repeat { body, .. } | Stmt::DoRepeat { body, .. } | Stmt::Loop { body } => {
                loop_in_stmts(&body.stmts) || body.stmts.iter().any(stmt_has_nested)
            }
            Stmt::If { then_block, else_block, .. } => {
                then_block.stmts.iter().any(stmt_has_nested)
                    || else_block.as_ref().map_or(false, |b| b.stmts.iter().any(stmt_has_nested))
            }
            _ => false,
        }
    }
    match body {
        FuncBody::Expr(_) => false,
        FuncBody::Block(b) => b.stmts.iter().any(stmt_has_nested),
    }
}

/// Whether a type contains any generic variable.
fn ty_has_var(ty: &Ty) -> bool {
    match ty {
        Ty::Var(_) => true,
        Ty::Seq(e) => ty_has_var(e),
        Ty::Pair(a, b) => ty_has_var(a) || ty_has_var(b),
        Ty::Map(k, v) => ty_has_var(k) || ty_has_var(v),
        Ty::Func(ps, r) => ps.iter().any(ty_has_var) || ty_has_var(r),
        _ => false,
    }
}

/// does type mention generic `var`
fn ty_mentions_var(ty: &Ty, var: &str) -> bool {
    match ty {
        Ty::Var(v) => v == var,
        Ty::Seq(e) => ty_mentions_var(e, var),
        Ty::Pair(a, b) => ty_mentions_var(a, var) || ty_mentions_var(b, var),
        Ty::Map(k, v) => ty_mentions_var(k, var) || ty_mentions_var(v, var),
        Ty::Func(ps, r) => ps.iter().any(|p| ty_mentions_var(p, var)) || ty_mentions_var(r, var),
        _ => false,
    }
}

/// substitute `var` with `concrete` in `ty`
fn subst_ty(ty: &Ty, var: &str, concrete: &Ty) -> Ty {
    match ty {
        Ty::Var(v) if v == var => concrete.clone(),
        Ty::Seq(e) => Ty::Seq(Box::new(subst_ty(e, var, concrete))),
        Ty::Pair(a, b) => Ty::Pair(Box::new(subst_ty(a, var, concrete)), Box::new(subst_ty(b, var, concrete))),
        Ty::Map(k, v) => Ty::Map(Box::new(subst_ty(k, var, concrete)), Box::new(subst_ty(v, var, concrete))),
        Ty::Func(ps, r) => Ty::Func(
            ps.iter().map(|p| subst_ty(p, var, concrete)).collect(),
            Box::new(subst_ty(r, var, concrete)),
        ),
        other => other.clone(),
    }
}

fn subst_sig(sig: &FnSig, var: &str, concrete: &Ty) -> FnSig {
    FnSig {
        params: sig.params.iter().map(|t| subst_ty(t, var, concrete)).collect(),
        param_names: sig.param_names.clone(),
        borrows: sig.borrows.clone(),
        ret: subst_ty(&sig.ret, var, concrete),
    }
}

/// unbox/call/rebox/return statements of a shim
fn shim_body(native_name: &str, sig: &FnSig, ind: &str, classes: &Classes) -> NRes<String> {
    let mut s = String::new();
    for (i, (t, b)) in sig.params.iter().zip(sig.borrows.iter()).enumerate() {
        match (t, b) {
            (Ty::Int, _) => { let _ = writeln!(s, "{ind}let a{i} = unbox_i64(&__args[{i}])?;"); }
            (Ty::Float, _) => { let _ = writeln!(s, "{ind}let a{i} = unbox_f64(&__args[{i}])?;"); }
            (Ty::Bool, _) => { let _ = writeln!(s, "{ind}let a{i} = unbox_bool(&__args[{i}])?;"); }
            (Ty::Text, _) => { let _ = writeln!(s, "{ind}let a{i} = unbox_text(&__args[{i}])?;"); }
            (Ty::Seq(e), _) => match e.as_ref() {
                Ty::Float => { let _ = writeln!(s, "{ind}let mut a{i} = unbox_seq_f64(&__args[{i}])?;"); }
                Ty::Int => { let _ = writeln!(s, "{ind}let mut a{i} = unbox_seq_i64(&__args[{i}])?;"); }
                Ty::Text => { let _ = writeln!(s, "{ind}let mut a{i} = unbox_seq_text(&__args[{i}])?;"); }
                _ => return Err(()),
            },
            (Ty::Map(_, v), _) => match v.as_ref() {
                Ty::Float => { let _ = writeln!(s, "{ind}let mut a{i} = unbox_map_f64(&__args[{i}])?;"); }
                Ty::Int => { let _ = writeln!(s, "{ind}let mut a{i} = unbox_map_i64(&__args[{i}])?;"); }
                _ => return Err(()),
            },
            (Ty::Class(n), _) if classes.contains_key(n) => {
                let _ = writeln!(s, "{ind}let mut a{i} = unbox_instance_{n}(&__args[{i}])?;");
            }
            _ => return Err(()),
        }
    }
    let mut call_args = String::new();
    for (i, (t, b)) in sig.params.iter().zip(sig.borrows.iter()).enumerate() {
        if i > 0 { call_args.push_str(", "); }
        match (t, b) {
            (Ty::Seq(_), Borrow::Mut) | (Ty::Map(..), Borrow::Mut) | (Ty::Class(_), Borrow::Mut) => { let _ = write!(call_args, "&mut a{i}"); }
            (Ty::Seq(_), _) | (Ty::Map(..), _) | (Ty::Class(_), Borrow::Shared) => { let _ = write!(call_args, "&a{i}"); }
            (Ty::Text, _) => { let _ = write!(call_args, "&a{i}"); }
            _ => { let _ = write!(call_args, "a{i}"); }
        }
    }
    if sig.ret == Ty::Nil {
        let _ = writeln!(s, "{ind}{native_name}({call_args});");
    } else {
        let _ = writeln!(s, "{ind}let __ret = {native_name}({call_args});");
    }
    for (i, (t, b)) in sig.params.iter().zip(sig.borrows.iter()).enumerate() {
        match (t, b) {
            (Ty::Seq(e), Borrow::Mut) => match e.as_ref() {
                Ty::Float => { let _ = writeln!(s, "{ind}rebox_seq_f64(&__args[{i}], a{i})?;"); }
                Ty::Int => { let _ = writeln!(s, "{ind}rebox_seq_i64(&__args[{i}], a{i})?;"); }
                Ty::Text => { let _ = writeln!(s, "{ind}rebox_seq_text(&__args[{i}], a{i})?;"); }
                _ => return Err(()),
            },
            (Ty::Map(_, v), Borrow::Mut) => match v.as_ref() {
                Ty::Float => { let _ = writeln!(s, "{ind}rebox_map_f64(&__args[{i}], a{i})?;"); }
                Ty::Int => { let _ = writeln!(s, "{ind}rebox_map_i64(&__args[{i}], a{i})?;"); }
                _ => return Err(()),
            },
            (Ty::Class(n), Borrow::Mut) if classes.contains_key(n) => {
                let _ = writeln!(s, "{ind}rebox_instance_{n}(&__args[{i}], a{i})?;");
            }
            _ => {}
        }
    }
    let ret_box = match &sig.ret {
        Ty::Nil => "Ok(J2Value::Null)".to_string(),
        Ty::Float => "Ok(box_f64(__ret))".to_string(),
        Ty::Int => "Ok(box_i64(__ret))".to_string(),
        Ty::Bool => "Ok(box_bool(__ret))".to_string(),
        Ty::Text => "Ok(box_text(__ret))".to_string(),
        Ty::Seq(e) => match e.as_ref() {
            Ty::Float => "Ok(box_seq_f64(__ret))".to_string(),
            Ty::Int => "Ok(box_seq_i64(__ret))".to_string(),
            Ty::Text => "Ok(box_seq_text(__ret))".to_string(),
            _ => return Err(()),
        },
        Ty::Map(_, v) => match v.as_ref() {
            Ty::Float => "Ok(box_map_f64(__ret))".to_string(),
            Ty::Int => "Ok(box_map_i64(__ret))".to_string(),
            _ => return Err(()),
        },
        Ty::Class(n) if classes.contains_key(n) => format!("box_instance_{n}(__ret)"),
        Ty::Pair(a, b) => {
            let ba = box_scalar(a, "__ret.0")?;
            let bb = box_scalar(b, "__ret.1")?;
            format!("Ok(J2Value::pair({ba}, {bb}))")
        }
        _ => return Err(()),
    };
    let _ = writeln!(s, "{ind}{ret_box}");
    Ok(s)
}

fn build_shim(name: &str, sig: &FnSig, classes: &Classes) -> NRes<String> {
    let mut s = String::new();
    let n = sig.params.len();
    let _ = writeln!(
        s,
        "#[allow(unused_mut, non_snake_case)]\nfn {name}__shim(__args: &[J2Value]) -> J2Result<J2Value> {{",
    );
    let _ = writeln!(
        s,
        "    if __args.len() != {n} {{ return Err(J2Err::type_err(\"{name} expects {n} argument(s)\")); }}",
    );
    s.push_str(&shim_body(&format!("{name}__native"), sig, "    ", classes)?);
    let _ = writeln!(s, "}}");
    Ok(s)
}

/// The dispatching shim for a generic fn
fn build_generic_shim(
    name: &str,
    sig: &FnSig,
    rep_idx: usize,
    var: &str,
    have_f64: bool,
    have_i64: bool,
    classes: &Classes,
) -> NRes<String> {
    let n = sig.params.len();
    let branch = |width: &str, have: bool| -> NRes<String> {
        if have {
            let msig = subst_sig(sig, var, if width == "f64" { &Ty::Float } else { &Ty::Int });
            shim_body(&format!("{name}__native_{width}"), &msig, "        ", classes)
        } else {
            Ok(format!("        Err(J2Err::type_err(\"{name}: no {width} instantiation\"))\n"))
        }
    };
    let body_f64 = branch("f64", have_f64)?;
    let body_i64 = branch("i64", have_i64)?;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "#[allow(unused_mut, non_snake_case)]\nfn {name}__shim(__args: &[J2Value]) -> J2Result<J2Value> {{",
    );
    let _ = writeln!(
        s,
        "    if __args.len() != {n} {{ return Err(J2Err::type_err(\"{name} expects {n} argument(s)\")); }}",
    );
    let _ = writeln!(s, "    let __is_float = match &__args[{rep_idx}] {{");
    let _ = writeln!(s, "        J2Value::Seq(__s) => matches!(__s.lock().unwrap().items.first(), Some(J2Value::Float(_))),");
    let _ = writeln!(s, "        J2Value::Float(_) => true,");
    let _ = writeln!(s, "        _ => false,");
    let _ = writeln!(s, "    }};");
    let _ = writeln!(s, "    if __is_float {{");
    s.push_str(&body_f64);
    let _ = writeln!(s, "    }} else {{");
    s.push_str(&body_i64);
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    Ok(s)
}

/// Lower a native function block
fn nlower_fn_block(ctx: &mut NCtx<'_>, stmts: &[Stmt], out: &mut String, depth: usize) -> NRes<()> {
    let last = stmts.len().wrapping_sub(1);
    for (i, s) in stmts.iter().enumerate() {
        if i == last && ctx.ret != Ty::Nil {
            if let Stmt::Expr(e) = s {
                let (code, ty) = nlower_expr(ctx, e)?;
                let c = coerce(code, &ty, &ctx.ret.clone())?;
                let ind = "    ".repeat(depth);
                let _ = writeln!(out, "{ind}{c}");
                return Ok(());
            }
        }
        nlower_stmt(ctx, s, out, depth)?;
    }
    // non-nil return without value or `give` bails
    if ctx.ret != Ty::Nil && !block_returns(stmts) {
        return Err(());
    }
    Ok(())
}

/// conservative check all paths return via `give`
fn block_returns(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|s| match s {
        Stmt::Give { .. } => true,
        Stmt::If { then_block, else_block: Some(eb), .. } => {
            block_returns(&then_block.stmts) && block_returns(&eb.stmts)
        }
        _ => false,
    })
}

fn nlower_block_scoped(ctx: &mut NCtx<'_>, b: &Block, out: &mut String, depth: usize) -> NRes<()> {
    // snapshot/restore gamma; block bindings don't leak
    let saved_g = ctx.gamma.clone();
    let saved_u = ctx.usize_vars.clone();
    for s in &b.stmts {
        nlower_stmt(ctx, s, out, depth)?;
    }
    ctx.gamma = saved_g;
    ctx.usize_vars = saved_u;
    Ok(())
}

fn nlower_stmt(ctx: &mut NCtx<'_>, s: &Stmt, out: &mut String, depth: usize) -> NRes<()> {
    let ind = "    ".repeat(depth);
    match s {
        Stmt::Bind { name, value, mutable } => {
            // field write in method; mutating `:=` only
            if !ctx.gamma.contains_key(name) {
                if let Some(csig) = ctx.self_fields {
                    if let Some(f) = csig.fields.iter().find(|f| f.name == *name) {
                        if !ctx.self_mut {
                            return Err(()); // pure method may not mutate fields
                        }
                        let fty = f.ty.clone();
                        let (code, ty) = nlower_expr(ctx, value)?;
                        let c = coerce(code, &ty, &fty)?;
                        let _ = writeln!(out, "{ind}self.{} = {c};", mangle(name));
                        return Ok(());
                    }
                }
            }
            if ctx.gamma.contains_key(name) {
                // Reassignment of an existing native local.
                let target = ctx.gamma.get(name).cloned().unwrap();
                // peephole to `acc OP= rhs` for reductions
                if let Expr::Binary { op, lhs, rhs } = value {
                    if let Some(opc) = compound_op_str(op) {
                        let is_self = |e: &Expr| matches!(e, Expr::Ident(n) if n == name);
                        let operand = if is_self(lhs) {
                            Some(rhs.as_ref())
                        } else if matches!(op, BinOp::Add | BinOp::Mul) && is_self(rhs) {
                            Some(lhs.as_ref())
                        } else {
                            None
                        };
                        if let Some(operand) = operand {
                            let (rc, rt) = nlower_expr(ctx, operand)?;
                            let rc = coerce(rc, &rt, &target)?;
                            let _ = writeln!(out, "{ind}{name} {opc} {rc};");
                            return Ok(());
                        }
                    }
                }
                let (code, ty) = nlower_expr(ctx, value)?;
                let c = coerce(code, &ty, &target)?;
                let _ = writeln!(out, "{ind}{name} = {c};");
            } else {
                // `n = len(s)` makes usize length local
                if let Some(seq_code) = match_len_call(ctx, value)? {
                    let _ = writeln!(out, "{ind}let mut {name} = {seq_code}.len();");
                    ctx.gamma.insert(name.clone(), Ty::Int);
                    ctx.usize_vars.insert(name.clone());
                } else {
                    let (code, ty) = nlower_expr(ctx, value)?;
                    let rt = rust_ty(&ty)?.ok_or(())?;
                    let _ = writeln!(out, "{ind}let mut {name}: {rt} = {code};");
                    // owned seq local is `&mut`-borrowable
                    if matches!(ty, Ty::Seq(_)) {
                        ctx.mut_seqs.insert(name.clone());
                    }
                    // owned String is non-Copy; clone on read
                    if ty == Ty::Text {
                        ctx.owned_strings.insert(name.clone());
                    }
                    ctx.gamma.insert(name.clone(), ty);
                }
                let _ = mutable;
            }
            Ok(())
        }
        Stmt::CompoundAssign { name, op, value } => {
            let target = ctx.gamma.get(name).cloned().ok_or(())?;
            let (code, ty) = nlower_expr(ctx, value)?;
            let op_s = match op {
                BinOp::Add => "+=", BinOp::Sub => "-=", BinOp::Mul => "*=",
                BinOp::Div => "/=", BinOp::Rem => "%=",
                _ => return Err(()),
            };
            let c = coerce(code, &ty, &target)?;
            let _ = writeln!(out, "{ind}{name} {op_s} {c};");
            Ok(())
        }
        Stmt::IncDec { name, inc } => {
            let _ = ctx.gamma.get(name).cloned().ok_or(())?;
            let _ = writeln!(out, "{ind}{name} {} 1;", if *inc { "+=" } else { "-=" });
            Ok(())
        }
        Stmt::IndexedAssign { target, idx, value } => {
            // target must be seq or map identifier
            let tname = match target {
                Expr::Ident(n) => n.clone(),
                _ => return Err(()),
            };
            // map<text,V> index write is insert
            if let Some(Ty::Map(_, v)) = ctx.gamma.get(&tname).cloned() {
                let key = nlower_map_key_owned(ctx, idx)?;
                let (vc, vt) = nlower_expr(ctx, value)?;
                let vc = coerce(vc, &vt, &v)?;
                let _ = writeln!(out, "{ind}{tname}.insert({key}, {vc});");
                return Ok(());
            }
            let elem = match ctx.gamma.get(&tname) {
                Some(Ty::Seq(e)) => (**e).clone(),
                _ => return Err(()),
            };
            let idx_code = nlower_index(ctx, idx)?;
            // seq<text> index write stores owned String
            if elem == Ty::Text {
                let (vc, vt) = nlower_expr(ctx, value)?;
                if vt != Ty::Text { return Err(()); }
                let _ = writeln!(out, "{ind}{tname}[{idx_code}] = ({vc}).to_string();");
                return Ok(());
            }
            // peephole to `s[i] OP= rhs`; slice matchers
            if let Expr::Binary { op, lhs, rhs } = value {
                if let Some(opc) = compound_op_str(op) {
                    // is `e` exactly `s[idx]`
                    let mut is_self = |e: &Expr, ctx: &mut NCtx<'_>| -> bool {
                        if let Expr::Index { coll, idx: lidx } = e {
                            if matches!(coll.as_ref(), Expr::Ident(n) if *n == tname) {
                                if let Ok(li) = nlower_index(ctx, lidx) {
                                    return li == idx_code;
                                }
                            }
                        }
                        false
                    };
                    let operand: Option<&Expr> = if is_self(lhs, ctx) {
                        Some(rhs)
                    } else if matches!(op, BinOp::Add | BinOp::Mul) && is_self(rhs, ctx) {
                        Some(lhs)
                    } else {
                        None
                    };
                    if let Some(operand) = operand {
                        let (rc, rt) = nlower_expr(ctx, operand)?;
                        let rc = coerce(rc, &rt, &elem)?;
                        let _ = writeln!(out, "{ind}{tname}[{idx_code}] {opc} {rc};");
                        return Ok(());
                    }
                }
            }
            let (vc, vt) = nlower_expr(ctx, value)?;
            let vc = coerce(vc, &vt, &elem)?;
            let _ = writeln!(out, "{ind}{tname}[{idx_code}] = {vc};");
            Ok(())
        }
        Stmt::ForLoop { bindings, iter, filter, until, body } => {
            if bindings.len() != 1 || filter.is_some() || until.is_some() {
                return Err(());
            }
            let i = &bindings[0];
            let saved_g = ctx.gamma.clone();
            let saved_u = ctx.usize_vars.clone();
            match iter {
                // for-range over usize, J-inclusive bounds
                Expr::Range { start, end: Some(end) } => {
                    let lo = nlower_index(ctx, start)?;
                    let hi = inclusive_upper(ctx, end)?;
                    let _ = writeln!(out, "{ind}for {i} in {lo}..{hi} {{");
                    ctx.gamma.insert(i.clone(), Ty::Int);
                    ctx.usize_vars.insert(i.clone());
                    for st in &body.stmts { nlower_stmt(ctx, st, out, depth + 1)?; }
                }
                // for-each over a seq
                _ => {
                    let (sc, st) = nlower_expr(ctx, iter)?;
                    let elem = match st { Ty::Seq(e) => *e, _ => return Err(()) };
                    let slicev = format!("__fe_s_{i}");
                    let idxv = format!("__fe_{i}");
                    // Non-`Copy` (String) elements are cloned per iteration.
                    let clone = if elem == Ty::Text { ".clone()" } else { "" };
                    let _ = writeln!(out, "{ind}let {slicev} = &{sc}[..];");
                    let _ = writeln!(out, "{ind}for {idxv} in 0..{slicev}.len() {{");
                    let _ = writeln!(out, "{ind}    let {i} = {slicev}[{idxv}]{clone};");
                    if elem == Ty::Text { ctx.owned_strings.insert(i.clone()); }
                    ctx.gamma.insert(i.clone(), elem);
                    for st in &body.stmts { nlower_stmt(ctx, st, out, depth + 1)?; }
                }
            }
            ctx.gamma = saved_g;
            ctx.usize_vars = saved_u;
            let _ = writeln!(out, "{ind}}}");
            Ok(())
        }
        Stmt::Repeat { cond, body } => {
            let (cc, ct) = nlower_expr(ctx, cond)?;
            if ct != Ty::Bool { return Err(()); }
            let _ = writeln!(out, "{ind}while {cc} {{");
            nlower_block_scoped(ctx, body, out, depth + 1)?;
            let _ = writeln!(out, "{ind}}}");
            Ok(())
        }
        Stmt::If { cond, then_block, else_block } => {
            let (cc, ct) = nlower_expr(ctx, cond)?;
            if ct != Ty::Bool { return Err(()); }
            let _ = writeln!(out, "{ind}if {cc} {{");
            nlower_block_scoped(ctx, then_block, out, depth + 1)?;
            if let Some(eb) = else_block {
                let _ = writeln!(out, "{ind}}} else {{");
                nlower_block_scoped(ctx, eb, out, depth + 1)?;
            }
            let _ = writeln!(out, "{ind}}}");
            Ok(())
        }
        Stmt::Give { value } => {
            match value {
                Some(v) => {
                    let (code, ty) = nlower_expr(ctx, v)?;
                    let c = coerce(code, &ty, &ctx.ret.clone())?;
                    let _ = writeln!(out, "{ind}return {c};");
                }
                None => {
                    if ctx.ret != Ty::Nil { return Err(()); }
                    let _ = writeln!(out, "{ind}return;");
                }
            }
            Ok(())
        }
        Stmt::Stop { cond } => {
            match cond {
                Some(c) => {
                    let (cc, ct) = nlower_expr(ctx, c)?;
                    if ct != Ty::Bool { return Err(()); }
                    let _ = writeln!(out, "{ind}if {cc} {{ break; }}");
                }
                None => { let _ = writeln!(out, "{ind}break;"); }
            }
            Ok(())
        }
        Stmt::Skip { cond } => {
            match cond {
                Some(c) => {
                    let (cc, ct) = nlower_expr(ctx, c)?;
                    if ct != Ty::Bool { return Err(()); }
                    let _ = writeln!(out, "{ind}if {cc} {{ continue; }}");
                }
                None => { let _ = writeln!(out, "{ind}continue;"); }
            }
            Ok(())
        }
        // map<text,V> field write is insert
        Stmt::MemberAssign { target, field, value } => {
            let tname = match target { Expr::Ident(n) => n.clone(), _ => return Err(()) };
            let v = match ctx.gamma.get(&tname).cloned() {
                Some(Ty::Map(_, v)) => *v,
                _ => return Err(()),
            };
            let (vc, vt) = nlower_expr(ctx, value)?;
            let vc = coerce(vc, &vt, &v)?;
            let _ = writeln!(out, "{ind}{tname}.insert(String::from({:?}), {vc});", field);
            Ok(())
        }
        Stmt::Expr(e) => {
            let (code, _ty) = nlower_expr(ctx, e)?;
            let _ = writeln!(out, "{ind}let _ = {code};");
            Ok(())
        }
        // everything else bails to dynamic
        _ => Err(()),
    }
}

/// map read key as `&str` operand
fn nlower_map_key(ctx: &mut NCtx<'_>, idx: &Expr) -> NRes<String> {
    if let Expr::TextLit(s) = idx {
        return Ok(format!("{:?}", s));
    }
    let (kc, kt) = nlower_expr(ctx, idx)?;
    if kt != Ty::Text {
        return Err(());
    }
    Ok(format!("&({})[..]", kc))
}

/// map insert key as owned `String`
fn nlower_map_key_owned(ctx: &mut NCtx<'_>, idx: &Expr) -> NRes<String> {
    if let Expr::TextLit(s) = idx {
        return Ok(format!("String::from({:?})", s));
    }
    let (kc, kt) = nlower_expr(ctx, idx)?;
    if kt != Ty::Text {
        return Err(());
    }
    Ok(format!("({}).to_string()", kc))
}

/// type peek without emission; picks generic monomorph
fn peek_ty(ctx: &NCtx<'_>, e: &Expr) -> Ty {
    match e {
        Expr::IntLit(_) => Ty::Int,
        Expr::FloatLit(_) => Ty::Float,
        Expr::Bool(_) => Ty::Bool,
        Expr::TextLit(_) => Ty::Text,
        Expr::Const(_) => Ty::Float,
        Expr::Ident(n) => {
            if ctx.usize_vars.contains(n) { Ty::Int }
            else { ctx.gamma.get(n).cloned().unwrap_or(Ty::Dyn) }
        }
        Expr::Unary { operand, .. } => peek_ty(ctx, operand),
        Expr::Binary { op, lhs, rhs } => {
            use BinOp::*;
            match op {
                Eq | NotEq | Lt | Gt | LtEq | GtEq | And | Or => Ty::Bool,
                BAnd | BOr | BXor | Shl | Shr => Ty::Int,
                Div | Pow => Ty::Float,
                _ => {
                    let (l, r) = (peek_ty(ctx, lhs), peek_ty(ctx, rhs));
                    if l == Ty::Float || r == Ty::Float { Ty::Float } else { Ty::Int }
                }
            }
        }
        Expr::Index { coll, idx } => {
            // Sub-slice `coll[a..b]` keeps the seq type
            let is_slice = matches!(idx.as_ref(), Expr::Range { .. });
            match peek_ty(ctx, coll) {
                Ty::Seq(e) => if is_slice { Ty::Seq(e) } else { *e },
                Ty::Text => Ty::Text,
                _ => Ty::Dyn,
            }
        }
        Expr::Call { callee, args } => {
            if let Expr::Ident(f) = callee.as_ref() {
                match f.as_str() {
                    "len" => return Ty::Int,
                    "make_seq" if args.len() == 2 => return Ty::Seq(Box::new(peek_ty(ctx, &args[1]))),
                    "copy" if args.len() == 1 => return peek_ty(ctx, &args[0]),
                    "sqrt" | "abs" | "floor" | "ceil" | "round" | "exp" | "ln" | "sin"
                    | "cos" | "tan" | "pow" => return Ty::Float,
                    _ => {
                        if let Some(sig) = ctx.fns.get(f) {
                            return sig.ret.clone();
                        }
                    }
                }
            }
            Ty::Dyn
        }
        Expr::SeqLit(items) => {
            let elem = if items.iter().any(|it| peek_ty(ctx, it) == Ty::Float) { Ty::Float } else { Ty::Int };
            Ty::Seq(Box::new(elem))
        }
        _ => Ty::Dyn,
    }
}

/// if `e` is `len(seq)`, return seq code
fn match_len_call(ctx: &mut NCtx<'_>, e: &Expr) -> NRes<Option<String>> {
    if let Expr::Call { callee, args } = e {
        if let Expr::Ident(f) = callee.as_ref() {
            if f == "len" && args.len() == 1 {
                let (sc, st) = nlower_expr(ctx, &args[0])?;
                if matches!(st, Ty::Seq(_) | Ty::Text) {
                    return Ok(Some(sc));
                }
            }
        }
    }
    Ok(None)
}

/// Collect every name assigned anywhere in `stmts`
fn collect_assigned_set(stmts: &[Stmt], out: &mut HashSet<String>) {
    for s in stmts {
        match s {
            Stmt::Bind { name, .. }
            | Stmt::CompoundAssign { name, .. }
            | Stmt::IncDec { name, .. }
            | Stmt::Global { name, .. } => { out.insert(name.clone()); }
            Stmt::ForLoop { bindings, body, .. } => {
                for b in bindings { out.insert(b.clone()); }
                collect_assigned_set(&body.stmts, out);
            }
            Stmt::Repeat { body, .. } | Stmt::DoRepeat { body, .. } | Stmt::Loop { body } => {
                collect_assigned_set(&body.stmts, out);
            }
            Stmt::If { then_block, else_block, .. } => {
                collect_assigned_set(&then_block.stmts, out);
                if let Some(e) = else_block { collect_assigned_set(&e.stmts, out); }
            }
            Stmt::Try { body, typed_handlers, default_handler, .. } => {
                collect_assigned_set(&body.stmts, out);
                for h in typed_handlers { collect_assigned_set(&h.handler.stmts, out); }
                if let Some(d) = default_handler { collect_assigned_set(&d.stmts, out); }
            }
            Stmt::Func { body: FuncBody::Block(b), .. } => collect_assigned_set(&b.stmts, out),
            _ => {}
        }
    }
}

/// lower index/bound expr to usize arithmetic
fn nlower_index(ctx: &mut NCtx<'_>, e: &Expr) -> NRes<String> {
    match e {
        Expr::IntLit(n) => Ok(format!("{}", n)),
        Expr::Ident(name) => {
            if ctx.usize_vars.contains(name) {
                Ok(name.clone())
            } else if matches!(ctx.gamma.get(name), Some(Ty::Int)) {
                // hoist invariant casts into the preheader
                let invariant = ctx.assigned.as_ref().map_or(false, |a| !a.contains(name));
                if ctx.can_hoist && invariant {
                    if let Some(h) = ctx.cast_hoist.get(name) {
                        return Ok(h.clone());
                    }
                    let h = format!("__ucast_{}", name);
                    ctx.prologue.push(format!("let {h}: usize = {name} as usize;"));
                    ctx.cast_hoist.insert(name.clone(), h.clone());
                    Ok(h)
                } else {
                    Ok(format!("({} as usize)", name))
                }
            } else {
                Err(())
            }
        }
        Expr::Binary { op: op @ (BinOp::Add | BinOp::Sub | BinOp::Mul), lhs, rhs } => {
            let l = nlower_index(ctx, lhs)?;
            let r = nlower_index(ctx, rhs)?;
            let o = match op { BinOp::Add => "+", BinOp::Sub => "-", _ => "*" };
            Ok(format!("({} {} {})", l, o, r))
        }
        Expr::Call { callee, args } => {
            if let Expr::Ident(f) = callee.as_ref() {
                if f == "len" && args.len() == 1 {
                    let (sc, st) = nlower_expr(ctx, &args[0])?;
                    if matches!(st, Ty::Seq(_) | Ty::Text) {
                        return Ok(format!("{}.len()", sc));
                    }
                }
            }
            Err(())
        }
        _ => Err(()),
    }
}

/// exclusive upper bound for inclusive range end
fn inclusive_upper(ctx: &mut NCtx<'_>, end: &Expr) -> NRes<String> {
    match end {
        Expr::Binary { op: BinOp::Sub, lhs, rhs } if matches!(rhs.as_ref(), Expr::IntLit(1)) => {
            nlower_index(ctx, lhs)
        }
        _ => Ok(format!("({} + 1)", nlower_index(ctx, end)?)),
    }
}

/// seq arg borrow, whole seq or sub-slice
fn nlower_seq_arg(ctx: &mut NCtx<'_>, arg: &Expr, want_mut: bool) -> NRes<String> {
    let amp = if want_mut { "&mut " } else { "&" };
    match arg {
        Expr::Ident(name) => {
            if !matches!(ctx.gamma.get(name), Some(Ty::Seq(_))) {
                return Err(());
            }
            if want_mut && !ctx.mut_seqs.contains(name) {
                return Err(());
            }
            // A shared slice *parameter*
            if !want_mut && !ctx.mut_seqs.contains(name) {
                return Ok(name.clone());
            }
            Ok(format!("{amp}{name}[..]"))
        }
        // Sub-slice `coll[start..end]` (inclusive).
        Expr::Index { coll, idx } => {
            let (start, end) = match idx.as_ref() {
                Expr::Range { start, end: Some(end) } => (start.as_ref(), end.as_ref()),
                _ => return Err(()),
            };
            let cname = match coll.as_ref() {
                Expr::Ident(n) if matches!(ctx.gamma.get(n), Some(Ty::Seq(_))) => n.clone(),
                _ => return Err(()),
            };
            if want_mut && !ctx.mut_seqs.contains(&cname) {
                return Err(());
            }
            let lo = nlower_index(ctx, start)?;
            let hi = inclusive_upper(ctx, end)?;
            Ok(format!("{amp}{cname}[{lo}..{hi}]"))
        }
        _ => Err(()),
    }
}

/// Lower an expression to `(rust_code, type)`
fn nlower_expr(ctx: &mut NCtx<'_>, e: &Expr) -> NRes<(String, Ty)> {
    match e {
        Expr::IntLit(n) => Ok((format!("{}i64", n), Ty::Int)),
        Expr::FloatLit(x) => Ok((fmt_f64(*x), Ty::Float)),
        Expr::Bool(b) => Ok((format!("{}", b), Ty::Bool)),
        Expr::TextLit(s) => Ok((format!("String::from({:?})", s), Ty::Text)),
        Expr::Const(c) => {
            let s = match c {
                BuiltinConst::Pi => "std::f64::consts::PI",
                BuiltinConst::E => "std::f64::consts::E",
                BuiltinConst::Tau => "std::f64::consts::TAU",
                BuiltinConst::Inf => "f64::INFINITY",
                BuiltinConst::Nan => "f64::NAN",
                BuiltinConst::MaxVal => "f64::MAX",
                BuiltinConst::MinVal => "f64::MIN",
            };
            Ok((s.to_string(), Ty::Float))
        }
        Expr::Ident(name) => {
            // gamma first; unbound field names become `self.field`
            if let Some(ty) = ctx.gamma.get(name).cloned() {
                // usize length local reads back as i64
                if ctx.usize_vars.contains(name) {
                    Ok((format!("({} as i64)", name), Ty::Int))
                } else if ctx.owned_strings.contains(name) {
                    // clone-on-read for owned String
                    Ok((format!("{}.clone()", name), ty))
                } else {
                    Ok((name.clone(), ty))
                }
            } else if let Some(csig) = ctx.self_fields {
                let f = csig.fields.iter().find(|f| f.name == *name).ok_or(())?;
                // Clone any non-Copy field
                let clone = if is_copy_ty(&f.ty) { "" } else { ".clone()" };
                Ok((format!("self.{}{}", mangle(name), clone), f.ty.clone()))
            } else {
                Err(())
            }
        }
        // `_` in cond-lambda is iteration element
        Expr::Underscore => {
            let ty = ctx.underscore.clone().ok_or(())?;
            Ok(("__x".to_string(), ty))
        }
        Expr::Unary { op, operand } => {
            let (code, ty) = nlower_expr(ctx, operand)?;
            match op {
                UnaryOp::Neg if is_numeric(&ty) => Ok((format!("(-{})", code), ty)),
                UnaryOp::Pos if is_numeric(&ty) => Ok((format!("({})", code), ty)),
                UnaryOp::Not if ty == Ty::Bool => Ok((format!("(!{})", code), Ty::Bool)),
                UnaryOp::BNot if ty == Ty::Int => Ok((format!("(!{})", code), Ty::Int)),
                _ => Err(()),
            }
        }
        Expr::Binary { op, lhs, rhs } => nlower_binary(ctx, op, lhs, rhs),
        Expr::Index { coll, idx } => {
            let (cc, ct) = nlower_expr(ctx, coll)?;
            match ct {
                Ty::Seq(e) => {
                    let idx_code = nlower_index(ctx, idx)?;
                    // String element can't move out; clone
                    let clone = if *e == Ty::Text { ".clone()" } else { "" };
                    Ok((format!("{}[{}]{}", cc, idx_code, clone), *e))
                }
                // `m[key]` on a map<text,V> reads a value
                Ty::Map(_, v) => {
                    let key = nlower_map_key(ctx, idx)?;
                    Ok((format!("{}[{}]", cc, key), *v))
                }
                // text index gives 1-char `String`
                Ty::Text => {
                    let i = nlower_index(ctx, idx)?;
                    Ok((
                        format!("(&({})[..]).chars().nth({}).map(|__c| __c.to_string()).unwrap_or_default()", cc, i),
                        Ty::Text,
                    ))
                }
                _ => Err(()),
            }
        }
        // map field read or class field read
        Expr::Member { obj, field } => {
            let (oc, ot) = nlower_expr(ctx, obj)?;
            match ot {
                Ty::Map(_, v) => Ok((format!("{}[{:?}]", oc, field), *v)),
                Ty::Class(cn) => {
                    let csig = ctx.classes.get(&cn).ok_or(())?;
                    let f = csig.fields.iter().find(|f| f.name == *field).ok_or(())?;
                    // clone non-Copy field from borrowed instance
                    let clone = if is_copy_ty(&f.ty) { "" } else { ".clone()" };
                    Ok((format!("{}.{}{}", oc, mangle(field), clone), f.ty.clone()))
                }
                _ => Err(()),
            }
        }
        // map literal to owned `HashMap<String, V>`
        Expr::MapLit(entries) => {
            if entries.is_empty() {
                return Err(());
            }
            let lowered: Vec<(String, (String, Ty))> = entries
                .iter()
                .map(|(k, v)| nlower_expr(ctx, v).map(|lv| (k.clone(), lv)))
                .collect::<NRes<Vec<_>>>()?;
            let vty = if lowered.iter().any(|(_, (_, t))| *t == Ty::Float) {
                Ty::Float
            } else if lowered.iter().all(|(_, (_, t))| *t == Ty::Int) {
                Ty::Int
            } else {
                return Err(());
            };
            let vrust = match vty { Ty::Float => "f64", _ => "i64" };
            let mut s = format!("{{ let mut __m: HashMap<String, {}> = HashMap::new(); ", vrust);
            for (k, (vc, vt)) in lowered {
                let vc = coerce(vc, &vt, &vty)?;
                let _ = write!(s, "__m.insert(String::from({:?}), {}); ", k, vc);
            }
            s.push_str("__m }");
            Ok((s, Ty::Map(Box::new(Ty::Text), Box::new(vty))))
        }
        Expr::Call { callee, args } => nlower_call(ctx, callee, args),
        // seq literal, ints widen if any float
        Expr::SeqLit(items) => {
            if items.is_empty() {
                return Err(()); // can't infer element type from `[]`
            }
            let lowered: Vec<(String, Ty)> = items
                .iter()
                .map(|it| nlower_expr(ctx, it))
                .collect::<NRes<Vec<_>>>()?;
            // All-text literal -> `Vec<String>` (each element owned).
            if lowered.iter().all(|(_, t)| *t == Ty::Text) {
                let parts: Vec<String> = lowered.iter().map(|(c, _)| format!("({}).to_string()", c)).collect();
                return Ok((format!("vec![{}]", parts.join(", ")), Ty::Seq(Box::new(Ty::Text))));
            }
            let elem = if lowered.iter().any(|(_, t)| *t == Ty::Float) {
                Ty::Float
            } else if lowered.iter().all(|(_, t)| *t == Ty::Int) {
                Ty::Int
            } else {
                return Err(());
            };
            let mut parts = Vec::with_capacity(lowered.len());
            for (c, t) in lowered {
                parts.push(coerce(c, &t, &elem)?);
            }
            Ok((format!("vec![{}]", parts.join(", ")), Ty::Seq(Box::new(elem))))
        }
        // `(a, b)` native 2-tuple for multi-value returns
        Expr::Pair(a, b) => {
            let (ca, ta) = nlower_expr(ctx, a)?;
            let (cb, tb) = nlower_expr(ctx, b)?;
            Ok((format!("({}, {})", ca, cb), Ty::Pair(Box::new(ta), Box::new(tb))))
        }
        // map unary fn over seq; result chains
        Expr::Pipe { lhs, rhs } => {
            let (lc, lt) = nlower_expr(ctx, lhs)?;
            let elem = match lt { Ty::Seq(e) => *e, _ => return Err(()) };
            let fname = match rhs.as_ref() { Expr::Ident(n) => n.as_str(), _ => return Err(()) };
            let (body, ret_elem) = map_apply(ctx, fname, "__x", &elem)?;
            Ok((
                format!("{lc}.iter().map(|&__x| {body}).collect::<Vec<_>>()"),
                Ty::Seq(Box::new(ret_elem)),
            ))
        }
        // filter by bool fn or inline cond-lambda
        Expr::Filter { lhs, rhs } => {
            let (lc, lt) = nlower_expr(ctx, lhs)?;
            let elem = match lt { Ty::Seq(e) => *e, _ => return Err(()) };
            let pred = match rhs.as_ref() {
                // inline cond-lambda; lower body with `_` element
                Expr::CondLambda(inner) => {
                    let saved = ctx.underscore.take();
                    ctx.underscore = Some(elem.clone());
                    let res = nlower_expr(ctx, inner);
                    ctx.underscore = saved;
                    let (code, t) = res?;
                    if t != Ty::Bool { return Err(()); }
                    code
                }
                // Named predicate (a native bool fn).
                Expr::Ident(fname) => {
                    let (code, prt) = map_apply(ctx, fname, "__x", &elem)?;
                    if prt != Ty::Bool { return Err(()); }
                    code
                }
                _ => return Err(()),
            };
            Ok((
                format!("{lc}.iter().filter(|&&__x| {pred}).cloned().collect::<Vec<_>>()"),
                Ty::Seq(Box::new(elem)),
            ))
        }
        _ => Err(()),
    }
}

/// apply unary `fname` to pre-lowered arg
fn map_apply(ctx: &NCtx<'_>, fname: &str, argcode: &str, argty: &Ty) -> NRes<(String, Ty)> {
    // user/inferred native fn with one scalar param
    if let Some(sig) = ctx.fns.get(fname).cloned() {
        if sig.params.len() != 1 || matches!(sig.params[0], Ty::Seq(_) | Ty::Text) {
            return Err(());
        }
        let (callee, msig) = match first_var_name(&sig) {
            None => (format!("{fname}__native"), sig),
            Some(var) => {
                if !ctx.safe_gen.contains(fname) { return Err(()); }
                let concrete = match argty { Ty::Float => Ty::Float, Ty::Int => Ty::Int, _ => return Err(()) };
                let suffix = if concrete == Ty::Float { "f64" } else { "i64" };
                (format!("{fname}__native_{suffix}"), subst_sig(&sig, &var, &concrete))
            }
        };
        let arg = coerce(argcode.to_string(), argty, &msig.params[0])?;
        return Ok((format!("{callee}({arg})"), msig.ret));
    }
    // Math builtins: float -> float.
    match fname {
        "sqrt" | "abs" | "floor" | "ceil" | "round" | "exp" | "ln" | "sin" | "cos" | "tan" => {
            let a = coerce(argcode.to_string(), argty, &Ty::Float)?;
            let method = if fname == "ln" { "ln" } else { fname };
            Ok((format!("({}).{}()", a, method), Ty::Float))
        }
        _ => Err(()),
    }
}

fn nlower_binary(ctx: &mut NCtx<'_>, op: &BinOp, lhs: &Expr, rhs: &Expr) -> NRes<(String, Ty)> {
    use BinOp::*;
    let (lc, lt) = nlower_expr(ctx, lhs)?;
    let (rc, rt) = nlower_expr(ctx, rhs)?;
    match op {
        // text `+` text gives owned String
        Add if lt == Ty::Text && rt == Ty::Text => {
            Ok((format!("format!(\"{{}}{{}}\", {}, {})", lc, rc), Ty::Text))
        }
        Add | Sub | Mul | Rem => {
            if !is_numeric(&lt) || !is_numeric(&rt) {
                return Err(());
            }
            let rty = if lt == Ty::Float || rt == Ty::Float { Ty::Float } else { Ty::Int };
            let l = coerce(lc, &lt, &rty)?;
            let r = coerce(rc, &rt, &rty)?;
            // CSE self-multiply to one temp for matcher
            if *op == Mul && l == r && (l.contains('[') || l.contains('(')) {
                if let Some(rt_s) = rust_ty(&rty)? {
                    let n = ctx.tmp_ctr;
                    ctx.tmp_ctr += 1;
                    return Ok((
                        format!("{{ let __sq_{n}: {rt_s} = {l}; (__sq_{n} * __sq_{n}) }}"),
                        rty,
                    ));
                }
            }
            let o = match op { Add => "+", Sub => "-", Mul => "*", Rem => "%", _ => unreachable!() };
            Ok((format!("({} {} {})", l, o, r), rty))
        }
        Div => {
            if !is_numeric(&lt) || !is_numeric(&rt) { return Err(()); }
            let l = coerce(lc, &lt, &Ty::Float)?;
            let r = coerce(rc, &rt, &Ty::Float)?;
            Ok((format!("({} / {})", l, r), Ty::Float))
        }
        Pow => {
            if !is_numeric(&lt) || !is_numeric(&rt) { return Err(()); }
            let l = coerce(lc, &lt, &Ty::Float)?;
            let r = coerce(rc, &rt, &Ty::Float)?;
            Ok((format!("({}).powf({})", l, r), Ty::Float))
        }
        Eq | NotEq | Lt | Gt | LtEq | GtEq => {
            // numeric cmp (int to float), text/bool eq
            let o = match op { Eq => "==", NotEq => "!=", Lt => "<", Gt => ">", LtEq => "<=", GtEq => ">=", _ => unreachable!() };
            if is_numeric(&lt) && is_numeric(&rt) {
                let common = if lt == Ty::Float || rt == Ty::Float { Ty::Float } else { Ty::Int };
                let l = coerce(lc, &lt, &common)?;
                let r = coerce(rc, &rt, &common)?;
                Ok((format!("({} {} {})", l, o, r), Ty::Bool))
            } else if lt == Ty::Text && rt == Ty::Text {
                // text cmp; `[..]` unifies String and &str
                Ok((format!("(&({})[..] {} &({})[..])", lc, o, rc), Ty::Bool))
            } else if lt == rt && matches!(op, Eq | NotEq) {
                Ok((format!("({} {} {})", lc, o, rc), Ty::Bool))
            } else {
                Err(())
            }
        }
        And | Or => {
            if lt != Ty::Bool || rt != Ty::Bool { return Err(()); }
            let o = if *op == And { "&&" } else { "||" };
            Ok((format!("({} {} {})", lc, o, rc), Ty::Bool))
        }
        BAnd | BOr | BXor | Shl | Shr => {
            if lt != Ty::Int || rt != Ty::Int { return Err(()); }
            let o = match op { BAnd => "&", BOr => "|", BXor => "^", Shl => "<<", Shr => ">>", _ => unreachable!() };
            Ok((format!("({} {} {})", lc, o, rc), Ty::Int))
        }
    }
}

/// Lower fn-valued arg to satisfy `impl Fn`
fn nlower_fn_arg(ctx: &mut NCtx<'_>, arg: &Expr, ps: &[Ty], r: &Ty) -> NRes<String> {
    match arg {
        Expr::Ident(g) => {
            // A function-typed parameter in scope
            if let Some(Ty::Func(gps, gr)) = ctx.gamma.get(g) {
                if gps.as_slice() == ps && gr.as_ref() == r {
                    return Ok(g.clone());
                }
                return Err(());
            }
            // known non-generic, non-HOF native fn
            let gsig = ctx.fns.get(g).ok_or(())?;
            if first_var_name(gsig).is_some() || is_hof_sig(gsig) {
                return Err(());
            }
            if gsig.params.as_slice() != ps || &gsig.ret != r {
                return Err(());
            }
            // scalar by-value params keep fn-item type
            if gsig.borrows.iter().any(|b| *b != Borrow::ByVal) {
                return Err(());
            }
            for p in &gsig.params {
                scalar_rust(p)?;
            }
            Ok(format!("{}__native", g))
        }
        Expr::Lambda { params, body } => {
            let FuncBody::Expr(be) = body.as_ref() else { return Err(()) };
            if params.len() != ps.len() {
                return Err(());
            }
            // Captures must be `Copy` scalars
            let mut refs = HashSet::new();
            gather_all_idents(be, &mut refs);
            let lp_names: HashSet<&str> = params.iter().map(|p| p.name.as_str()).collect();
            for name in &refs {
                if lp_names.contains(name.as_str()) {
                    continue;
                }
                if let Some(t) = ctx.gamma.get(name) {
                    if !is_copy_ty(t) {
                        return Err(());
                    }
                }
            }
            let mut cgamma = ctx.gamma.clone();
            let mut plist = Vec::with_capacity(params.len());
            for (p, pt) in params.iter().zip(ps.iter()) {
                if p.pattern.is_some() {
                    return Err(());
                }
                cgamma.insert(p.name.clone(), pt.clone());
                plist.push(format!("{}: {}", p.name, scalar_rust(pt)?));
            }
            let mut cctx = NCtx {
                gamma: cgamma,
                usize_vars: ctx.usize_vars.clone(),
                mut_seqs: ctx.mut_seqs.clone(),
                fns: ctx.fns,
                safe_gen: ctx.safe_gen,
                underscore: None,
                owned_strings: ctx.owned_strings.clone(),
                classes: ctx.classes,
                self_fields: ctx.self_fields,
                self_mut: false,
                ret: r.clone(),
                // single-expr closure has no prologue; hoisting off
                prologue: Vec::new(),
                tmp_ctr: 0,
                can_hoist: false,
                assigned: None,
                cast_hoist: HashMap::new(),
            };
            let (code, ty) = nlower_expr(&mut cctx, be)?;
            let code = coerce(code, &ty, r)?;
            let ret = match r {
                Ty::Nil => "()".to_string(),
                o => scalar_rust(o)?.to_string(),
            };
            Ok(format!("move |{}| -> {} {{ {} }}", plist.join(", "), ret, code))
        }
        _ => Err(()),
    }
}

/// Whether `e` is a loop-invariant constant scalar
fn is_const_scalar_expr(e: &Expr) -> bool {
    match e {
        Expr::IntLit(_) | Expr::FloatLit(_) | Expr::Bool(_) | Expr::Const(_) => true,
        Expr::Unary { op, operand } if matches!(op, UnaryOp::Neg | UnaryOp::Pos) => {
            is_const_scalar_expr(operand)
        }
        _ => false,
    }
}

/// Hoist const scalar operand into prologue local
fn hoist_const_operand(ctx: &mut NCtx<'_>, arg: &Expr, code: String, rust_ty: &str) -> String {
    if ctx.can_hoist && is_const_scalar_expr(arg) {
        let name = format!("__pconst_{}", ctx.tmp_ctr);
        ctx.tmp_ctr += 1;
        ctx.prologue.push(format!("let {name}: {rust_ty} = {code};"));
        name
    } else {
        code
    }
}

/// Prepend the accumulated prologue bindings
fn flush_prologue(ctx: &NCtx<'_>, ind: &str, body: String) -> String {
    if ctx.prologue.is_empty() {
        return body;
    }
    let mut pro = String::new();
    for line in &ctx.prologue {
        let _ = writeln!(pro, "{ind}{line}");
    }
    format!("{pro}{body}")
}

/// Construct a native class
fn nlower_construct(ctx: &mut NCtx<'_>, csig: &ClassSig, args: &[Expr]) -> NRes<(String, Ty)> {
    let cn = csig.name.clone();
    let field_val = |ctx: &mut NCtx<'_>, f: &ClassFieldSig, ve: &Expr| -> NRes<String> {
        let (vc, vt) = nlower_expr(ctx, ve)?;
        // storing instance copies value; bail to dynamic
        if matches!(vt, Ty::Class(_)) {
            return Err(());
        }
        coerce(vc, &vt, &f.ty)
    };
    // Named form: a single MapLit arg.
    if args.len() == 1 {
        if let Expr::MapLit(entries) = &args[0] {
            let mut parts = Vec::with_capacity(csig.fields.len());
            for f in &csig.fields {
                let val = if let Some(e) = entries.iter().find(|e| e.0 == f.name) {
                    field_val(ctx, f, &e.1)?
                } else if let Some(def) = f.default.clone() {
                    field_val(ctx, f, &def)?
                } else {
                    return Err(());
                };
                parts.push(format!("{}: {}", mangle(&f.name), val));
            }
            return Ok((format!("{} {{ {} }}", cn, parts.join(", ")), Ty::Class(cn.clone())));
        }
    }
    // positional args; Map-first bails to match `__call`
    if args.len() == csig.fields.len() {
        if let Some(a0) = args.first() {
            if matches!(peek_ty(ctx, a0), Ty::Map(..)) {
                return Err(());
            }
        }
        let mut parts = Vec::with_capacity(args.len());
        for (f, a) in csig.fields.iter().zip(args) {
            let val = field_val(ctx, f, a)?;
            parts.push(format!("{}: {}", mangle(&f.name), val));
        }
        return Ok((format!("{} {{ {} }}", cn, parts.join(", ")), Ty::Class(cn.clone())));
    }
    Err(())
}

fn nlower_call(ctx: &mut NCtx<'_>, callee: &Expr, args: &[Expr]) -> NRes<(String, Ty)> {
    // Method call `obj.method(args)` on a class instance
    if let Expr::Member { obj, field } = callee {
        let (oc, ot) = nlower_expr(ctx, obj)?;
        let Ty::Class(cn) = ot else { return Err(()) };
        let csig = ctx.classes.get(&cn).ok_or(())?;
        let m = csig.methods.get(field).ok_or(())?;
        if m.borrow_self == Borrow::Mut {
            return Err(()); // mutating call in value position -> dynamic
        }
        if args.len() != m.params.len() {
            return Err(());
        }
        let mut acode = Vec::new();
        for ((_, pty), a) in m.params.iter().zip(args) {
            let (ac, at) = nlower_expr(ctx, a)?;
            acode.push(coerce(ac, &at, pty)?);
        }
        return Ok((format!("{}.{}({})", oc, mangle(field), acode.join(", ")), m.ret.clone()));
    }
    let fname = match callee {
        Expr::Ident(n) => n.as_str(),
        _ => return Err(()),
    };
    // call Fn param inside HOF; coerce args
    if let Some(Ty::Func(ps, r)) = ctx.gamma.get(fname).cloned() {
        if args.len() != ps.len() {
            return Err(());
        }
        let mut acode = Vec::with_capacity(args.len());
        for (a, pt) in args.iter().zip(ps.iter()) {
            let (ac, at) = nlower_expr(ctx, a)?;
            acode.push(coerce(ac, &at, pt)?);
        }
        return Ok((format!("{}({})", fname, acode.join(", ")), (*r).clone()));
    }
    // native class construction to struct literal
    if let Some(csig) = ctx.classes.get(fname).cloned() {
        return nlower_construct(ctx, &csig, args);
    }
    // Native -> native direct call
    if let Some(sig) = ctx.fns.get(fname).cloned() {
        if args.len() != sig.params.len() {
            return Err(());
        }
        // resolve generic monomorph from actual arg types
        let (sig, native_name) = match first_var_name(&sig) {
            None => (sig, format!("{}__native", fname)),
            Some(var) => {
                // generic callee needs both monomorphs
                if !ctx.safe_gen.contains(fname) {
                    return Err(());
                }
                // T-bearing args must agree on width
                let mut concrete: Option<Ty> = None;
                for (a, pt) in args.iter().zip(sig.params.iter()) {
                    let bound = match pt {
                        Ty::Var(v) if *v == var => Some(peek_ty(ctx, a)),
                        Ty::Seq(e) if matches!(e.as_ref(), Ty::Var(v) if *v == var) => {
                            match peek_ty(ctx, a) { Ty::Seq(el) => Some(*el), _ => None }
                        }
                        _ => None,
                    };
                    if let Some(t) = bound {
                        if t != Ty::Float && t != Ty::Int { return Err(()); }
                        match &concrete {
                            Some(c) if *c != t => return Err(()), // inconsistent -> bail
                            _ => concrete = Some(t),
                        }
                    }
                }
                let concrete = concrete.ok_or(())?;
                let suffix = if concrete == Ty::Float { "f64" } else { "i64" };
                (subst_sig(&sig, &var, &concrete), format!("{}__native_{}", fname, suffix))
            }
        };
        let mut call_args = String::new();
        for (i, ((a, pt), b)) in args.iter().zip(sig.params.iter()).zip(sig.borrows.iter()).enumerate() {
            if i > 0 { call_args.push_str(", "); }
            match pt {
                Ty::Seq(_) => {
                    let want_mut = *b == Borrow::Mut;
                    call_args.push_str(&nlower_seq_arg(ctx, a, want_mut)?);
                }
                // Function argument to a HOF
                Ty::Func(ps, r) => {
                    call_args.push_str(&nlower_fn_arg(ctx, a, ps, r)?);
                }
                // text/map reference passing across calls deferred
                Ty::Text | Ty::Map(..) => return Err(()),
                _ => {
                    let (ac, at) = nlower_expr(ctx, a)?;
                    call_args.push_str(&coerce(ac, &at, pt)?);
                }
            }
        }
        return Ok((format!("{}({})", native_name, call_args), sig.ret.clone()));
    }
    // Seq-producing builtins.
    match fname {
        // `make_seq(n, init)` -> `vec![init; n]`.
        "make_seq" => {
            if args.len() != 2 { return Err(()); }
            let n_usize = nlower_index(ctx, &args[0])?;
            let (ic, it) = nlower_expr(ctx, &args[1])?;
            if !is_numeric(&it) { return Err(()); }
            return Ok((format!("vec![{}; {}]", ic, n_usize), Ty::Seq(Box::new(it))));
        }
        // `copy(s)` -> an owned `Vec<T>`
        "copy" => {
            if args.len() != 1 { return Err(()); }
            let (sc, st) = nlower_expr(ctx, &args[0])?;
            if matches!(st, Ty::Seq(_)) {
                return Ok((format!("{}.to_vec()", sc), st));
            }
            return Err(());
        }
        // Native text predicates / search
        "contains" | "starts_with" | "ends_with" if args.len() == 2 => {
            let (sc, st) = nlower_expr(ctx, &args[0])?;
            let (subc, subt) = nlower_expr(ctx, &args[1])?;
            if st != Ty::Text || subt != Ty::Text { return Err(()); }
            let method = match fname { "contains" => "contains", "starts_with" => "starts_with", _ => "ends_with" };
            return Ok((format!("(&({})[..]).{}(&({})[..])", sc, method, subc), Ty::Bool));
        }
        "find" if args.len() == 2 => {
            let (sc, st) = nlower_expr(ctx, &args[0])?;
            let (subc, subt) = nlower_expr(ctx, &args[1])?;
            if st != Ty::Text || subt != Ty::Text { return Err(()); }
            return Ok((
                format!("((&({})[..]).find(&({})[..]).map(|__i| __i as i64).unwrap_or(-1))", sc, subc),
                Ty::Int,
            ));
        }
        // String-producing text builtins -> owned `String`
        "upper" | "lower" | "trim" if args.len() == 1 => {
            let (sc, st) = nlower_expr(ctx, &args[0])?;
            if st != Ty::Text { return Err(()); }
            let expr = match fname {
                "upper" => format!("(&({})[..]).to_uppercase()", sc),
                "lower" => format!("(&({})[..]).to_lowercase()", sc),
                _ => format!("(&({})[..]).trim().to_string()", sc),
            };
            return Ok((expr, Ty::Text));
        }
        // `replace(s, find, with)` -> owned String.
        "replace" if args.len() == 3 => {
            let (sc, st) = nlower_expr(ctx, &args[0])?;
            let (fc, ft) = nlower_expr(ctx, &args[1])?;
            let (wc, wt) = nlower_expr(ctx, &args[2])?;
            if st != Ty::Text || ft != Ty::Text || wt != Ty::Text { return Err(()); }
            return Ok((format!("(&({})[..]).replace(&({})[..], &({})[..])", sc, fc, wc), Ty::Text));
        }
        // `split(s, sep)` -> seq<text>
        "split" if args.len() == 2 => {
            let (sc, st) = nlower_expr(ctx, &args[0])?;
            let (sepc, sept) = nlower_expr(ctx, &args[1])?;
            if st != Ty::Text || sept != Ty::Text { return Err(()); }
            return Ok((
                format!(
                    "{{ let __s = &({})[..]; let __sep = &({})[..]; if __sep.is_empty() {{ __s.chars().map(|__c| __c.to_string()).collect::<Vec<String>>() }} else {{ __s.split(__sep).map(|__p| __p.to_string()).collect::<Vec<String>>() }} }}",
                    sc, sepc
                ),
                Ty::Seq(Box::new(Ty::Text)),
            ));
        }
        // `join(words, sep)` -> owned String.
        "join" if args.len() == 2 => {
            let (sc, st) = nlower_expr(ctx, &args[0])?;
            let (sepc, sept) = nlower_expr(ctx, &args[1])?;
            if !matches!(st, Ty::Seq(e) if *e == Ty::Text) || sept != Ty::Text { return Err(()); }
            return Ok((format!("({}).join(&({})[..])", sc, sepc), Ty::Text));
        }
        _ => {}
    }
    // Native math builtins: float -> float.
    match fname {
        "len" => {
            if args.len() != 1 { return Err(()); }
            let (sc, st) = nlower_expr(ctx, &args[0])?;
            if matches!(st, Ty::Seq(_) | Ty::Text | Ty::Map(..)) {
                Ok((format!("({}.len() as i64)", sc), Ty::Int))
            } else {
                Err(())
            }
        }
        "sqrt" | "abs" | "floor" | "ceil" | "round" | "exp" | "ln" | "sin" | "cos" | "tan" => {
            if args.len() != 1 { return Err(()); }
            let (ac, at) = nlower_expr(ctx, &args[0])?;
            let ac = coerce(ac, &at, &Ty::Float)?;
            let method = if fname == "ln" { "ln" } else { fname };
            Ok((format!("({}).{}()", ac, method), Ty::Float))
        }
        "pow" => {
            if args.len() != 2 { return Err(()); }
            let (ac, at) = nlower_expr(ctx, &args[0])?;
            let (bc, bt) = nlower_expr(ctx, &args[1])?;
            let ac = coerce(ac, &at, &Ty::Float)?;
            let bc = coerce(bc, &bt, &Ty::Float)?;
            Ok((format!("({}).powf({})", ac, bc), Ty::Float))
        }
        // Binary min/max -> `a.max(b)` / `a.min(b)`
        "max" | "min" if args.len() == 2 => {
            let (ac, at) = nlower_expr(ctx, &args[0])?;
            let (bc, bt) = nlower_expr(ctx, &args[1])?;
            if !is_numeric(&at) || !is_numeric(&bt) { return Err(()); }
            let common = if at == Ty::Float || bt == Ty::Float { Ty::Float } else { Ty::Int };
            let ac = coerce(ac, &at, &common)?;
            let bc = coerce(bc, &bt, &common)?;
            Ok((format!("({}).{}({})", ac, fname, bc), common))
        }
        // `clamp(x, lo, hi)` -> `x.clamp(lo, hi)`
        "clamp" if args.len() == 3 => {
            let (xc, xt) = nlower_expr(ctx, &args[0])?;
            let (lc, lt) = nlower_expr(ctx, &args[1])?;
            let (hc, ht) = nlower_expr(ctx, &args[2])?;
            if !is_numeric(&xt) || !is_numeric(&lt) || !is_numeric(&ht) { return Err(()); }
            let common = if [&xt, &lt, &ht].iter().any(|t| **t == Ty::Float) { Ty::Float } else { Ty::Int };
            let xc = coerce(xc, &xt, &common)?;
            let lc = coerce(lc, &lt, &common)?;
            let hc = coerce(hc, &ht, &common)?;
            let rt = rust_ty(&common)?.ok_or(())?;
            let lc = hoist_const_operand(ctx, &args[1], lc, &rt);
            let hc = hoist_const_operand(ctx, &args[2], hc, &rt);
            Ok((format!("({}).clamp({}, {})", xc, lc, hc), common))
        }
        _ => Err(()),
    }
}

/// Format f64 as unambiguous native float literal
fn fmt_f64(x: f64) -> String {
    format!("{}f64", x)
}

// ------------------------------ runtime preamble ------------------------------

/// Preamble prepended to every transpiled J program
pub const PREAMBLE: &str = r###"
use j2_runtime::prelude::*;
use j2_runtime::value::{J2Value, J2Func, J2FuncClause};
use j2_runtime::error::{J2Err, J2ErrKind, J2Result};
use j2_runtime::flow::J2Flow;
#[allow(unused_imports)]
use j2_runtime::convert::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Clone)]
struct Env {
    table: HashMap<String, (J2Value, bool)>,
    /// names declared in this scope, not inherited
    locals: std::collections::HashSet<String>,
}

type EnvRef = Rc<RefCell<Env>>;

thread_local! {
    /// registry of user top-level bindings
    static GLOBALS: RefCell<HashMap<String, (J2Value, bool)>> = RefCell::new(HashMap::new());
    /// holds a `give` value during unwinding
    static GIVE_SLOT: RefCell<Vec<J2Value>> = const { RefCell::new(Vec::new()) };
}

/// push `give` value, return unwind signal
fn __give(v: J2Value) -> J2Err {
    GIVE_SLOT.with(|g| g.borrow_mut().push(v));
    J2Err::give_signal()
}

/// pop the latest `give` value
fn __take_give() -> J2Value {
    GIVE_SLOT.with(|g| g.borrow_mut().pop()).unwrap_or(J2Value::Null)
}

thread_local! {
    /// current top-level source line, 0 unknown
    static CURRENT_LINE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
fn __set_line(n: usize) { CURRENT_LINE.with(|c| c.set(n)); }
fn __current_line() -> usize { CURRENT_LINE.with(|c| c.get()) }

thread_local! {
    /// last panic message from the quiet hook
    static PANIC_MSG: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}
/// quiet panic hook, capture message, no backtrace
fn __install_panic_hook() {
    let verbose = std::env::var("RUST_BACKTRACE").map(|v| v != "0").unwrap_or(false);
    std::panic::set_hook(Box::new(move |info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic".to_string());
        PANIC_MSG.with(|m| *m.borrow_mut() = msg);
        if verbose { eprintln!("{}", info); }
    }));
}
fn __take_panic_msg() -> String {
    PANIC_MSG.with(|m| std::mem::take(&mut *m.borrow_mut()))
}

fn new_env() -> EnvRef {
    let mut t: HashMap<String, (J2Value, bool)> = HashMap::new();
    let mut staging: HashMap<String, J2Value> = HashMap::new();
    j2_builtins::install(&mut staging);
    j2_math::install(&mut staging);
    j2_stats::install(&mut staging);
    j2_time::install(&mut staging);
    j2_rand::install(&mut staging);
    j2_fs::install(&mut staging);
    j2_proc::install(&mut staging);
    j2_str::install(&mut staging);
    j2_regex::install(&mut staging);
    j2_json::install(&mut staging);
    j2_date::install(&mut staging);
    j2_hash::install(&mut staging);
    j2_base64::install(&mut staging);
    j2_hex::install(&mut staging);
    j2_http::install(&mut staging);
    j2_sys::install(&mut staging);
    j2_bits::install(&mut staging);
    j2_async::install(&mut staging);
    for (k, v) in staging { t.insert(k, (v, false)); }
    Rc::new(RefCell::new(Env { table: t, locals: std::collections::HashSet::new() }))
}

fn new_env_with_parent() -> EnvRef {
    // fresh env seeded with builtins and globals
    let env = new_env();
    GLOBALS.with(|g| {
        let g = g.borrow();
        let mut env = env.borrow_mut();
        for (k, v) in g.iter() {
            env.table.entry(k.clone()).or_insert_with(|| v.clone());
        }
    });
    env
}

/// bind in env, register funcs and globals
fn __bind(env: &EnvRef, name: &str, value: J2Value, mutable: bool) -> J2Result<()> {
    // inside a method, write fields through `this`
    let wrote_to_instance = METHOD_THIS.with(|t| {
        if let Some(inst) = t.borrow().last() {
            let mut i = inst.lock().unwrap();
            if i.fields.contains_key(name) {
                i.fields.insert(name.to_string(), value.clone());
                return true;
            }
        }
        false
    });
    if wrote_to_instance { return Ok(()); }
    // same-name funcs merge into multi-clause
    let existing = env.borrow().table.get(name).cloned();
    if let (Some((J2Value::Func(old), _)), J2Value::Func(new)) = (existing.as_ref(), &value) {
        let mut clauses: Vec<J2FuncClause> = Vec::with_capacity(old.clauses.len() + new.clauses.len());
        for c in &old.clauses {
            clauses.push(J2FuncClause {
                patterns: c.patterns.clone(),
                param_names: c.param_names.clone(),
                body: c.body.clone(),
            });
        }
        for c in &new.clauses {
            clauses.push(J2FuncClause {
                patterns: c.patterns.clone(),
                param_names: c.param_names.clone(),
                body: c.body.clone(),
            });
        }
        let merged = J2Value::Func(std::sync::Arc::new(J2Func {
            name: name.to_string(),
            arity: new.arity,
            clauses,
        }));
        GLOBALS.with(|g| {
            g.borrow_mut().insert(name.to_string(), (merged.clone(), mutable));
        });
        {
            let mut e = env.borrow_mut();
            e.table.insert(name.to_string(), (merged, mutable));
            e.locals.insert(name.to_string());
        }
        return Ok(());
    }
    // constant reassignment check, this scope only
    let in_locals = env.borrow().locals.contains(name);
    let effective_mutable = match existing {
        Some((_, true)) if in_locals => true,  // rebinding a mutable local stays mutable
        Some((_, false)) if in_locals => {
            return Err(J2Err::mutability(format!("\"{}\" is a constant and cannot be reassigned", name)));
        }
        // absent or inherited, declare fresh here
        _ => mutable,
    };
    // funcs go to GLOBALS for recursion
    if matches!(value, J2Value::Func(_)) {
        GLOBALS.with(|g| {
            g.borrow_mut().insert(name.to_string(), (value.clone(), effective_mutable));
        });
    }
    {
        let mut e = env.borrow_mut();
        e.table.insert(name.to_string(), (value, effective_mutable));
        e.locals.insert(name.to_string());
    }
    Ok(())
}

/// bind a parameter as fresh mutable local
fn __bind_param(env: &EnvRef, name: &str, value: J2Value) {
    let mut e = env.borrow_mut();
    e.table.insert(name.to_string(), (value, true));
    e.locals.insert(name.to_string()); // a parameter is declared in this scope
}

/// promote a name to GLOBALS
fn __globalize(env: &EnvRef, name: &str) {
    let v = env.borrow().table.get(name).cloned();
    if let Some(entry) = v {
        GLOBALS.with(|g| { g.borrow_mut().insert(name.to_string(), entry); });
    }
}

/// runtime `J2Class` for a native class name
#[allow(dead_code)]
fn __class_for(name: &str) -> J2Result<std::sync::Arc<j2_runtime::value::J2Class>> {
    let v = GLOBALS.with(|g| g.borrow().get(name).map(|(v, _)| v.clone()));
    match v {
        Some(J2Value::Class(c)) => Ok(c),
        _ => Err(J2Err::key(format!("class {} not found", name))),
    }
}

fn __rebind(env: &EnvRef, name: &str, value: J2Value) -> J2Result<()> {
    // inside a method, write fields through `this`
    let wrote_to_instance = METHOD_THIS.with(|t| {
        if let Some(inst) = t.borrow().last() {
            let mut i = inst.lock().unwrap();
            if i.fields.contains_key(name) {
                i.fields.insert(name.to_string(), value.clone());
                return true;
            }
        }
        false
    });
    if wrote_to_instance { return Ok(()); }
    let mut env = env.borrow_mut();
    let entry = env.table.get(name).cloned();
    match entry {
        Some((_, mutable)) => {
            if !mutable {
                return Err(J2Err::mutability(format!("\"{}\" is a constant and cannot be reassigned", name)));
            }
            env.table.insert(name.to_string(), (value.clone(), true));
            drop(env);
            // mirror top-level updates into GLOBALS
            GLOBALS.with(|g| {
                let mut g = g.borrow_mut();
                if g.contains_key(name) {
                    g.insert(name.to_string(), (value, true));
                }
            });
            Ok(())
        }
        None => Err(J2Err::name_err(format!("\"{}\" is not defined", name))),
    }
}

fn __get(env: &EnvRef, name: &str) -> J2Result<J2Value> {
    {
        let env = env.borrow();
        if let Some((v, _)) = env.table.get(name) { return Ok(v.clone()); }
    }
    // in a method, bare names are fields
    if let Some(v) = METHOD_THIS.with(|t| {
        t.borrow().last().and_then(|inst| {
            inst.lock().unwrap().fields.get(name).cloned()
        })
    }) {
        return Ok(v);
    }
    Err(J2Err::name_err(format!("\"{}\" is not defined", name)))
}

thread_local! {
    /// stack of active method `this` references
    static METHOD_THIS: std::cell::RefCell<Vec<std::sync::Arc<std::sync::Mutex<j2_runtime::value::J2Instance>>>>
        = const { std::cell::RefCell::new(Vec::new()) };
}

fn __call(callee: J2Value, args: &[J2Value]) -> J2Result<J2Value> {
    match &callee {
        // calling a Class builds an Instance
        J2Value::Class(c) => {
            let mut fields = c.field_defaults.clone();
            if let Some(J2Value::Map(m)) = args.get(0) {
                let m = m.lock().unwrap();
                for (k, v) in m.iter() { fields.insert(k.clone(), v.clone()); }
            } else if !args.is_empty() {
                // positional args zip to fields in order
                for (name, v) in c.field_order.iter().zip(args.iter()) {
                    fields.insert(name.clone(), v.clone());
                }
            }
            Ok(J2Value::Instance(std::sync::Arc::new(std::sync::Mutex::new(
                j2_runtime::value::J2Instance { class: c.clone(), fields }
            ))))
        }
        J2Value::Builtin(f, _) => f(args),
        J2Value::Func(g) => {
            // Pick first clause whose patterns all match.
            for clause in &g.clauses {
                if clause.patterns.len() != args.len() { continue; }
                let mut ok = true;
                for (pat, arg) in clause.patterns.iter().zip(args.iter()) {
                    if let Some(p) = pat {
                        if !p.eq(arg) { ok = false; break; }
                    }
                }
                if ok {
                    return (clause.body)(args);
                }
            }
            Err(J2Err::type_err(format!("no matching clause for {}", g.name)))
        }
        J2Value::Cond(p) => {
            if args.len() != 1 {
                return Err(J2Err::type_err("cond takes exactly 1 argument"));
            }
            Ok(J2Value::Bool(p(&args[0])?))
        }
        _ => Err(J2Err::type_err(format!("not callable: {}", callee.type_name()))),
    }
}

fn __pipe(lhs: J2Value, rhs: J2Value) -> J2Result<J2Value> {
    match &lhs {
        J2Value::Seq(s) => {
            let items = s.lock().unwrap().items.clone();
            let mut out = Vec::with_capacity(items.len());
            for v in items {
                out.push(__call(rhs.clone(), &[v])?);
            }
            Ok(J2Value::seq(out))
        }
        J2Value::Flow(_) => {
            // materialize flow via __pipe per value
            let items = __materialize(&lhs)?;
            let mut out = Vec::with_capacity(items.len());
            for v in items { out.push(__call(rhs.clone(), &[v])?); }
            Ok(J2Value::seq(out))
        }
        _ => __call(rhs, &[lhs]),
    }
}

fn __filter(lhs: J2Value, rhs: J2Value) -> J2Result<J2Value> {
    // filter `lhs` by predicate `rhs`
    let pred = |v: &J2Value| -> J2Result<bool> {
        let r = __call(rhs.clone(), &[v.clone()])?;
        Ok(r.truthy())
    };
    match &lhs {
        J2Value::Seq(s) => {
            let items = s.lock().unwrap().items.clone();
            let mut out = Vec::new();
            for v in items {
                if pred(&v)? { out.push(v); }
            }
            Ok(J2Value::seq(out))
        }
        J2Value::Flow(_) => {
            let items = __materialize(&lhs)?;
            let mut out = Vec::new();
            for v in items {
                if pred(&v)? { out.push(v); }
            }
            Ok(J2Value::seq(out))
        }
        _ => {
            if pred(&lhs)? { Ok(lhs) } else { Ok(J2Value::Null) }
        }
    }
}

fn __index(coll: &J2Value, idx: &J2Value) -> J2Result<J2Value> {
    // map indexed by runtime text key
    if let (J2Value::Map(m), J2Value::Text(k)) = (coll, idx) {
        let m = m.lock().unwrap();
        return m.get(k.as_str()).cloned()
            .ok_or_else(|| J2Err::key(format!("key {:?} not found in map", k.as_str())));
    }
    // slice or gather, ranges are inclusive
    if matches!(idx, J2Value::Flow(_) | J2Value::Seq(_) | J2Value::SeqF64(_)) {
        let indices = __materialize(idx)?;
        match coll {
            J2Value::Seq(s) => {
                let s = s.lock().unwrap();
                let mut out = Vec::with_capacity(indices.len());
                for iv in &indices {
                    let i = iv.as_num_i64()?;
                    if i < 0 { return Err(J2Err::index("negative indices are not supported")); }
                    out.push(s.items.get(i as usize).cloned().ok_or_else(|| {
                        J2Err::index(format!("index {} is out of range for seq of length {}", i, s.items.len()))
                    })?);
                }
                return Ok(J2Value::seq(out));
            }
            J2Value::SeqF64(s) => {
                let s = s.lock().unwrap();
                let mut out = Vec::with_capacity(indices.len());
                for iv in &indices {
                    let i = iv.as_num_i64()?;
                    if i < 0 { return Err(J2Err::index("negative indices are not supported")); }
                    out.push(J2Value::Float(*s.get(i as usize).ok_or_else(|| {
                        J2Err::index(format!("index {} is out of range for seq of length {}", i, s.len()))
                    })?));
                }
                return Ok(J2Value::seq(out));
            }
            J2Value::Text(t) => {
                let chars: Vec<char> = t.chars().collect();
                let mut out = String::new();
                for iv in &indices {
                    let i = iv.as_num_i64()?;
                    if i < 0 { return Err(J2Err::index("negative indices are not supported")); }
                    out.push(*chars.get(i as usize).ok_or_else(|| {
                        J2Err::index(format!("index {} is out of range for text", i))
                    })?);
                }
                return Ok(J2Value::text(out));
            }
            _ => return Err(J2Err::type_err("slice requires a seq or text")),
        }
    }
    let i = idx.as_num_i64()?;
    if i < 0 { return Err(J2Err::index("negative indices are not supported")); }
    match coll {
        J2Value::Seq(s) => {
            let s = s.lock().unwrap();
            s.items.get(i as usize).cloned().ok_or_else(|| J2Err::index(format!("index {} is out of range for seq of length {}", i, s.items.len())))
        }
        J2Value::SeqF64(s) => {
            let s = s.lock().unwrap();
            s.get(i as usize).map(|x| J2Value::Float(*x)).ok_or_else(|| J2Err::index(format!("index {} is out of range for seq of length {}", i, s.len())))
        }
        J2Value::Text(t) => {
            t.chars().nth(i as usize).map(|c| J2Value::text(c.to_string()))
                .ok_or_else(|| J2Err::index(format!("index {} is out of range for text", i)))
        }
        _ => Err(J2Err::type_err("indexing requires seq, text, or map")),
    }
}

fn __member(obj: &J2Value, field: &str) -> J2Result<J2Value> {
    match obj {
        J2Value::Map(m) => {
            let m = m.lock().unwrap();
            m.get(field).cloned().ok_or_else(|| J2Err::key(format!("key {:?} not found in map", field)))
        }
        J2Value::Instance(inst) => {
            let inst_lock = inst.lock().unwrap();
            if let Some(v) = inst_lock.fields.get(field) {
                return Ok(v.clone());
            }
            if let Some(m) = inst_lock.class.methods.get(field).cloned() {
                drop(inst_lock);
                let inst_clone = inst.clone();
                return Ok(J2Value::Builtin(
                    std::sync::Arc::new(move |args: &[J2Value]| -> J2Result<J2Value> {
                        // push `this` so bare names hit fields
                        METHOD_THIS.with(|t| t.borrow_mut().push(inst_clone.clone()));
                        let f = &m.func;
                        let result = if let Some(clause) = f.clauses.first() {
                            (clause.body)(args)
                        } else {
                            Err(J2Err::runtime("method has no body"))
                        };
                        METHOD_THIS.with(|t| { t.borrow_mut().pop(); });
                        result
                    }),
                    "<method>",
                ));
            }
            Err(J2Err::key(format!("no field or method {:?}", field)))
        }
        _ => Err(J2Err::type_err(format!("member access requires a map or instance, got {}", obj.type_name()))),
    }
}

fn __static_get(class_val: &J2Value, member: &str) -> J2Result<J2Value> {
    match class_val {
        J2Value::Class(c) => {
            let s = c.statics.lock().unwrap();
            s.get(member).cloned().ok_or_else(|| J2Err::key(format!("no static member {:?} on class {}", member, c.name)))
        }
        _ => Err(J2Err::type_err(format!("`::` requires a class, got {}", class_val.type_name()))),
    }
}

/// snapshot env keys at scope start
fn __scope_enter(env: &EnvRef) -> (Vec<String>, Vec<String>) {
    let e = env.borrow();
    (e.table.keys().cloned().collect(), e.locals.iter().cloned().collect())
}

/// drop bindings not in snapshot, restore locals
fn __scope_leave(env: &EnvRef, snapshot: (Vec<String>, Vec<String>)) {
    use std::collections::HashSet;
    let (keys, locals) = snapshot;
    let keep: HashSet<&str> = keys.iter().map(|s| s.as_str()).collect();
    let mut env = env.borrow_mut();
    env.table.retain(|k, _| keep.contains(k.as_str()));
    env.locals = locals.into_iter().collect();
}

/// does raised error class match catch class
#[allow(dead_code)]
fn __kind_matches(kind: &str, name: &str) -> bool {
    match name {
        "BaseError" => true, // catches everything
        "Error" => kind != "SystemExit" && kind != "KeyboardInterrupt",
        "ArithmeticError" => matches!(kind, "ArithmeticError" | "ZeroDivisionError" | "OverflowError"),
        "LookupError" => matches!(kind, "LookupError" | "IndexError" | "KeyError"),
        "FlowError" => matches!(kind, "FlowError" | "InfiniteFlowError"),
        "NameError" => matches!(kind, "NameError" | "MutabilityError"),
        "ValueError" => matches!(kind, "ValueError" | "ConversionError"),
        "RuntimeError" => matches!(kind, "RuntimeError" | "RecursionError"),
        _ => kind == name,
    }
}

fn __index_set(coll: &J2Value, idx: &J2Value, value: J2Value) -> J2Result<()> {
    let i = idx.as_num_i64()?;
    if i < 0 { return Err(J2Err::index("negative indices are not supported")); }
    match coll {
        J2Value::Seq(s) => {
            let mut s = s.lock().unwrap();
            let len = s.items.len();
            if (i as usize) < len {
                s.items[i as usize] = value;
                Ok(())
            } else {
                Err(J2Err::index(format!("index {} is out of range for seq of length {}", i, len)))
            }
        }
        J2Value::SeqF64(s) => {
            let mut s = s.lock().unwrap();
            let len = s.len();
            if (i as usize) < len {
                s[i as usize] = value.as_num_f64()?;
                Ok(())
            } else {
                Err(J2Err::index(format!("index {} is out of range for seq of length {}", i, len)))
            }
        }
        _ => Err(J2Err::type_err("indexed assignment requires a seq")),
    }
}

fn __member_set(obj: &J2Value, field: &str, value: J2Value) -> J2Result<()> {
    match obj {
        J2Value::Map(m) => {
            let mut m = m.lock().unwrap();
            m.insert(field.to_string(), value);
            Ok(())
        }
        _ => Err(J2Err::type_err("member assignment requires a map")),
    }
}

fn __range(start: J2Value, end: Option<J2Value>) -> J2Result<J2Value> {
    let s = start.as_num_i64()?;
    match end {
        Some(e) => {
            let e = e.as_num_i64()?;
            Ok(J2Value::flow(J2Flow::range(s, e)))
        }
        None => Ok(J2Value::flow(J2Flow::open_range(s))),
    }
}

fn __materialize(v: &J2Value) -> J2Result<Vec<J2Value>> {
    match v {
        J2Value::Seq(s) => Ok(s.lock().unwrap().items.clone()),
        J2Value::SeqF64(s) => Ok(s.lock().unwrap().iter().map(|x| J2Value::Float(*x)).collect()),
        J2Value::Flow(f) => {
            if f.lock().unwrap().is_infinite() {
                return Err(J2Err::infinite_flow("cannot consume an infinite flow without until"));
            }
            let mut flow = std::mem::replace(&mut *f.lock().unwrap(), J2Flow::range(0, -1));
            flow.collect_all()
        }
        J2Value::Text(t) => Ok(t.chars().map(|c| J2Value::text(c.to_string())).collect()),
        _ => Err(J2Err::type_err("not iterable")),
    }
}
"###;
