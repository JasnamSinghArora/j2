// J type checker: bidirectional, local inference for the native subset.
//
// See `J2_NATIVE_BACKEND_DESIGN.md` §4. The discipline is exactly backend's:
// function signatures are fully annotated; bodies are locally inferred.
// There is no global Hindley–Milner inference and no unification across
// function boundaries.
//
// This module decides, per top-level function, whether it is a *native*
// candidate (all params + return annotated) and gathers the signature the
// native lowerer needs. The lowerer (`lower.rs`) re-uses `synth` to infer
// local types while emitting backend, so the type rules live here once.

use crate::ast::*;
use std::collections::HashMap;

/// How a `seq`/`text` parameter is borrowed in the emitted backend fn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Borrow {
    /// Scalar passed by value (`f64`, `i64`, `bool`).
    ByVal,
    /// `&[T]` / `&str`: read-only.
    Shared,
    /// `&mut [T]`: the body writes `param[i] = ...`.
    Mut,
}

/// The resolved signature of a native-candidate function.
#[derive(Debug, Clone)]
pub struct FnSig {
    pub params: Vec<Ty>,
    pub param_names: Vec<String>,
    pub borrows: Vec<Borrow>,
    pub ret: Ty,
}

/// Map from function name → resolved native signature. Only functions that
/// are native candidates *and* whose bodies type-check appear here.
pub type NativeFns = HashMap<String, FnSig>;

/// A function is a *native candidate* iff it has no pattern params, every
/// param carries a type annotation, and the return type is annotated.
pub fn is_native_candidate(params: &[Param], ret_ty: &Option<Ty>) -> bool {
    if ret_ty.is_none() { return false; }
    if params.is_empty() {
        // A zero-arg annotated fn is still a candidate (ret annotated).
        return true;
    }
    params.iter().all(|p| p.pattern.is_none() && p.ty.is_some())
}

/// Build the signature for a candidate function, inferring borrow modes by
/// scanning the body for indexed writes to each `seq` parameter.
pub fn build_sig(params: &[Param], ret_ty: &Option<Ty>, body: &FuncBody) -> FnSig {
    let mut tys = Vec::with_capacity(params.len());
    let mut names = Vec::with_capacity(params.len());
    let mut borrows = Vec::with_capacity(params.len());
    for p in params {
        let ty = p.ty.clone().unwrap_or(Ty::Dyn);
        let borrow = match &ty {
            Ty::Seq(_) | Ty::Map(..) => {
                if body_writes_index(body, &p.name) { Borrow::Mut } else { Borrow::Shared }
            }
            Ty::Text => Borrow::Shared,
            _ => Borrow::ByVal,
        };
        tys.push(ty);
        names.push(p.name.clone());
        borrows.push(borrow);
    }
    FnSig {
        params: tys,
        param_names: names,
        borrows,
        ret: ret_ty.clone().unwrap_or(Ty::Nil),
    }
}

// ───────────────────────── type inference (no annotations) ─────────────────────────
//
// Makes native lowering the *default*: an unannotated function is given a
// concrete native signature inferred from how its parameters are used in
// the body. Numeric parameters whose width can't be pinned become a shared
// generic `T` (monomorphized f64/i64 by the backend). If inference can't
// determine a parameter's shape, the function stays dynamic. Soundness is
// guaranteed downstream by (a) the lowerer's coerce-or-bail discipline and
// (b) the runtime error-fallback dispatcher emitted for inferred fns.

/// How a parameter is used in the body, accumulated by a single walk.
#[derive(Default, Clone, Copy)]
struct ParamUsage {
    used: bool,
    as_seq: bool,    // indexed / len() / sliced
    as_int: bool,    // loop bound, index position, %, bitwise
    as_text: bool,   // passed where text is expected (conservative: unused for now)
}

/// Infer a native signature for an (possibly unannotated) function, or
/// `None` if it can't be lowered natively. Annotated parameters are
/// respected; unannotated ones are inferred structurally.
pub fn infer_fn_sig(params: &[Param], body: &FuncBody, fns: &NativeFns) -> Option<FnSig> {
    if params.is_empty() {
        return None;
    }
    if params.iter().any(|p| p.pattern.is_some()) {
        return None;
    }
    let mut tys = Vec::with_capacity(params.len());
    let mut names = Vec::with_capacity(params.len());
    let mut borrows = Vec::with_capacity(params.len());
    for p in params {
        let ty = match &p.ty {
            Some(t) => t.clone(),
            None => {
                let u = param_usage(body, &p.name, fns);
                if !u.used {
                    return None; // unused param → can't/needn't infer
                }
                if u.as_seq {
                    // Element width is left generic unless pinned elsewhere.
                    Ty::Seq(Box::new(Ty::Var("T".into())))
                } else if u.as_int {
                    Ty::Int
                } else if u.as_text {
                    Ty::Text
                } else {
                    // Scalar numeric of undetermined width → generic.
                    Ty::Var("T".into())
                }
            }
        };
        let borrow = match &ty {
            Ty::Seq(_) | Ty::Map(..) => {
                if body_writes_index(body, &p.name) { Borrow::Mut } else { Borrow::Shared }
            }
            Ty::Text => Borrow::Shared,
            _ => Borrow::ByVal,
        };
        tys.push(ty);
        names.push(p.name.clone());
        borrows.push(borrow);
    }
    let ret = infer_ret(&names, &tys, body, fns)?;
    Some(FnSig { params: tys, param_names: names, borrows, ret })
}

fn param_usage(body: &FuncBody, name: &str, fns: &NativeFns) -> ParamUsage {
    let mut u = ParamUsage::default();
    match body {
        FuncBody::Expr(e) => usage_expr(e, name, false, &mut u, fns),
        FuncBody::Block(b) => usage_block(b, name, &mut u, fns),
    }
    u
}

fn usage_block(b: &Block, name: &str, u: &mut ParamUsage, fns: &NativeFns) {
    for s in &b.stmts {
        usage_stmt(s, name, u, fns);
    }
}

fn usage_stmt(s: &Stmt, name: &str, u: &mut ParamUsage, fns: &NativeFns) {
    match s {
        Stmt::Bind { value, .. } | Stmt::Global { value, .. } => usage_expr(value, name, false, u, fns),
        Stmt::CompoundAssign { name: n, value, .. } => {
            if n == name { u.used = true; }
            usage_expr(value, name, false, u, fns);
        }
        Stmt::IndexedAssign { target, idx, value } => {
            if let Expr::Ident(t) = target {
                if t == name { u.used = true; u.as_seq = true; }
            } else {
                usage_expr(target, name, false, u, fns);
            }
            usage_expr(idx, name, true, u, fns);
            usage_expr(value, name, false, u, fns);
        }
        Stmt::MemberAssign { target, value, .. } => {
            usage_expr(target, name, false, u, fns);
            usage_expr(value, name, false, u, fns);
        }
        Stmt::IncDec { name: n, .. } => { if n == name { u.used = true; } }
        Stmt::ForLoop { iter, filter, until, body, .. } => {
            usage_expr(iter, name, true, u, fns);
            if let Some(f) = filter { usage_expr(f, name, false, u, fns); }
            if let Some(t) = until { usage_expr(t, name, false, u, fns); }
            usage_block(body, name, u, fns);
        }
        Stmt::Repeat { cond, body } | Stmt::DoRepeat { body, cond } => {
            usage_expr(cond, name, false, u, fns);
            usage_block(body, name, u, fns);
        }
        Stmt::Loop { body } => usage_block(body, name, u, fns),
        Stmt::If { cond, then_block, else_block } => {
            usage_expr(cond, name, false, u, fns);
            usage_block(then_block, name, u, fns);
            if let Some(eb) = else_block { usage_block(eb, name, u, fns); }
        }
        Stmt::Stop { cond } | Stmt::Skip { cond } => {
            if let Some(c) = cond { usage_expr(c, name, false, u, fns); }
        }
        Stmt::Give { value } => { if let Some(v) = value { usage_expr(v, name, false, u, fns); } }
        Stmt::Try { body, typed_handlers, default_handler } => {
            usage_block(body, name, u, fns);
            for h in typed_handlers { usage_block(&h.handler, name, u, fns); }
            if let Some(d) = default_handler { usage_block(d, name, u, fns); }
        }
        Stmt::Assert { cond, msg } => {
            usage_expr(cond, name, false, u, fns);
            if let Some(m) = msg { usage_expr(m, name, false, u, fns); }
        }
        Stmt::Expr(e) => usage_expr(e, name, false, u, fns),
        Stmt::Func { .. } => {}
    }
}

/// Walk an expression accumulating how `name` is used. `int_ctx` is true
/// when the surrounding position requires an integer (index/bound).
fn usage_expr(e: &Expr, name: &str, int_ctx: bool, u: &mut ParamUsage, fns: &NativeFns) {
    match e {
        Expr::Ident(n) => {
            if n == name {
                u.used = true;
                if int_ctx { u.as_int = true; }
            }
        }
        Expr::Binary { op, lhs, rhs } => {
            let int_op = matches!(op, BinOp::Rem | BinOp::BAnd | BinOp::BOr | BinOp::BXor | BinOp::Shl | BinOp::Shr);
            usage_expr(lhs, name, int_ctx || int_op, u, fns);
            usage_expr(rhs, name, int_ctx || int_op, u, fns);
        }
        Expr::Unary { operand, .. } => usage_expr(operand, name, int_ctx, u, fns),
        Expr::Index { coll, idx } => {
            if let Expr::Ident(n) = coll.as_ref() {
                if n == name { u.used = true; u.as_seq = true; }
            } else {
                usage_expr(coll, name, false, u, fns);
            }
            usage_expr(idx, name, true, u, fns);
        }
        Expr::Call { callee, args } => {
            if matches!(callee.as_ref(), Expr::Ident(f) if matches!(f.as_str(), "map" | "filter")) && args.len() == 2 {
                if matches!(&args[0], Expr::Ident(n) if n == name) {
                    u.used = true;
                    u.as_seq = true;
                }
            }
            if let Expr::Ident(f) = callee.as_ref() {
                // `len(p)` / `copy(p)` → p is a seq.
                if (f == "len" || f == "copy") && args.len() == 1 {
                    if let Expr::Ident(n) = &args[0] {
                        if n == name { u.used = true; u.as_seq = true; }
                    }
                }
                // Text predicates/search → both args are text.
                if matches!(f.as_str(), "contains" | "starts_with" | "ends_with" | "find") && args.len() == 2 {
                    for a in args {
                        if let Expr::Ident(n) = a {
                            if n == name { u.used = true; u.as_text = true; }
                        }
                    }
                }
                // String-producing text builtins → arg is text.
                if matches!(f.as_str(), "upper" | "lower" | "trim") && args.len() == 1 {
                    if let Expr::Ident(n) = &args[0] {
                        if n == name { u.used = true; u.as_text = true; }
                    }
                }
                // Interprocedural: a param passed to a known native fn's
                // parameter inherits that parameter's shape (seq/int/text).
                if let Some(sig) = fns.get(f) {
                    for (k, a) in args.iter().enumerate() {
                        if let Expr::Ident(n) = a {
                            if n == name {
                                match sig.params.get(k) {
                                    Some(Ty::Seq(_)) => { u.used = true; u.as_seq = true; }
                                    Some(Ty::Int) => { u.used = true; u.as_int = true; }
                                    Some(Ty::Text) => { u.used = true; u.as_text = true; }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
            usage_expr(callee, name, false, u, fns);
            for a in args { usage_expr(a, name, false, u, fns); }
        }
        Expr::Lambda { params, body } if !params.iter().any(|p| p.name == name) => {
            match body.as_ref() {
                FuncBody::Expr(e) => usage_expr(e, name, false, u, fns),
                FuncBody::Block(b) => usage_block(b, name, u, fns),
            }
        }
        Expr::CondLambda(e) => usage_expr(e, name, false, u, fns),
        Expr::Range { start, end } => {
            usage_expr(start, name, true, u, fns);
            if let Some(e) = end { usage_expr(e, name, true, u, fns); }
        }
        Expr::Member { obj, .. } => usage_expr(obj, name, false, u, fns),
        Expr::Pair(a, b) => { usage_expr(a, name, false, u, fns); usage_expr(b, name, false, u, fns); }
        Expr::SeqLit(items) => { for it in items { usage_expr(it, name, false, u, fns); } }
        Expr::Pipe { lhs, rhs } | Expr::Filter { lhs, rhs } => {
            // The left side of a pipe/filter is iterated → a seq.
            if let Expr::Ident(n) = lhs.as_ref() {
                if n == name { u.used = true; u.as_seq = true; }
            } else {
                usage_expr(lhs, name, false, u, fns);
            }
            usage_expr(rhs, name, false, u, fns);
        }
        _ => {}
    }
}

/// Infer the return type of a function under inferred parameter types by
/// tracking top-level local bindings and synthesizing the trailing
/// expression / `give`. Returns `None` if it can't be determined natively.
fn infer_ret(names: &[String], tys: &[Ty], body: &FuncBody, fns: &NativeFns) -> Option<Ty> {
    let mut ctx = TyCtx::new(fns);
    for (n, t) in names.iter().zip(tys.iter()) {
        ctx.gamma.insert(n.clone(), t.clone());
    }
    match body {
        FuncBody::Expr(e) => {
            let t = synth(&ctx, e);
            if t == Ty::Dyn { None } else { Some(t) }
        }
        FuncBody::Block(b) => {
            // Track top-level locals so the trailing expr can reference them.
            for (i, s) in b.stmts.iter().enumerate() {
                match s {
                    Stmt::Bind { name, value, .. } => {
                        let t = synth(&ctx, value);
                        ctx.gamma.insert(name.clone(), t);
                    }
                    Stmt::Give { value: Some(v) } => {
                        let t = synth(&ctx, v);
                        return if t == Ty::Dyn { None } else { Some(t) };
                    }
                    Stmt::Give { value: None } => return Some(Ty::Nil),
                    Stmt::Expr(e) if i == b.stmts.len() - 1 => {
                        let t = synth(&ctx, e);
                        return if t == Ty::Dyn { None } else { Some(t) };
                    }
                    _ => {}
                }
            }
            // No trailing value expression and no `give` → unit return.
            Some(Ty::Nil)
        }
    }
}

/// Does the function body ever perform `name[i] = ...` (indexed write) on
/// the binding `name`? Used for borrow inference (`&mut [T]` vs `&[T]`).
fn body_writes_index(body: &FuncBody, name: &str) -> bool {
    match body {
        FuncBody::Expr(_) => false,
        FuncBody::Block(b) => block_writes_index(b, name),
    }
}

fn block_writes_index(b: &Block, name: &str) -> bool {
    b.stmts.iter().any(|s| stmt_writes_index(s, name))
}

fn stmt_writes_index(s: &Stmt, name: &str) -> bool {
    match s {
        Stmt::IndexedAssign { target, .. } => {
            matches!(target, Expr::Ident(n) if n == name)
        }
        // `m.field = v` mutates a map (→ `&mut` borrow).
        Stmt::MemberAssign { target, .. } => {
            matches!(target, Expr::Ident(n) if n == name)
        }
        Stmt::ForLoop { body, .. } => block_writes_index(body, name),
        Stmt::Repeat { body, .. } | Stmt::DoRepeat { body, .. } | Stmt::Loop { body } => {
            block_writes_index(body, name)
        }
        Stmt::If { then_block, else_block, .. } => {
            block_writes_index(then_block, name)
                || else_block.as_ref().map_or(false, |b| block_writes_index(b, name))
        }
        _ => false,
    }
}

// ───────────────────────── synthesis / checking ─────────────────────────

/// Local typing context: name → type, plus the known native fn signatures.
pub struct TyCtx<'a> {
    pub gamma: HashMap<String, Ty>,
    pub fns: &'a NativeFns,
}

impl<'a> TyCtx<'a> {
    pub fn new(fns: &'a NativeFns) -> Self {
        TyCtx { gamma: HashMap::new(), fns }
    }
}

/// Bottom-up type synthesis. Returns `Ty::Dyn` when the expression cannot
/// be given a native type (which, in a native context, signals a bail-out
/// to dynamic lowering / a type error).
pub fn synth(ctx: &TyCtx<'_>, e: &Expr) -> Ty {
    match e {
        Expr::IntLit(_) => Ty::Int,
        Expr::FloatLit(_) => Ty::Float,
        Expr::Bool(_) => Ty::Bool,
        Expr::TextLit(_) => Ty::Text,
        Expr::Const(_) => Ty::Float, // PI, E, INF, … are all f64
        Expr::Ident(name) => ctx.gamma.get(name).cloned().unwrap_or(Ty::Dyn),
        Expr::Underscore => ctx.gamma.get("_").cloned().unwrap_or(Ty::Dyn),
        Expr::Unary { op, operand } => match op {
            UnaryOp::Neg | UnaryOp::Pos => synth(ctx, operand),
            UnaryOp::Not => Ty::Bool,
            UnaryOp::BNot => Ty::Int,
        },
        Expr::Binary { op, lhs, rhs } => synth_binary(ctx, op, lhs, rhs),
        Expr::Index { coll, .. } => match synth(ctx, coll) {
            Ty::Seq(elem) => *elem,
            Ty::Map(_, v) => *v,
            Ty::Text => Ty::Text,
            _ => Ty::Dyn,
        },
        // `m.field` on a map reads a value of the map's value type.
        Expr::Member { obj, .. } => match synth(ctx, obj) {
            Ty::Map(_, v) => *v,
            _ => Ty::Dyn,
        },
        // `{a: 1, b: 2}` → map<text, V> where V is the numeric join of values.
        Expr::MapLit(entries) => {
            if entries.is_empty() {
                return Ty::Dyn;
            }
            let v = entries.iter().fold(Ty::Int, |acc, (_, e)| num_join(&acc, &synth(ctx, e), &BinOp::Add));
            if matches!(v, Ty::Int | Ty::Float) {
                Ty::Map(Box::new(Ty::Text), Box::new(v))
            } else {
                Ty::Dyn
            }
        }
        Expr::Call { callee, args } => synth_call(ctx, callee, args),
        Expr::Pipe { lhs, rhs } => synth_transform(ctx, lhs, rhs, false),
        Expr::Filter { lhs, rhs } => synth_transform(ctx, lhs, rhs, true),
        Expr::SeqLit(items) => {
            if items.is_empty() {
                return Ty::Dyn;
            }
            // All-text literal → seq<text>; else numeric join.
            if items.iter().all(|it| synth(ctx, it) == Ty::Text) {
                return Ty::Seq(Box::new(Ty::Text));
            }
            let elem = items.iter().fold(Ty::Int, |acc, it| num_join(&acc, &synth(ctx, it), &BinOp::Add));
            if matches!(elem, Ty::Int | Ty::Float | Ty::Var(_)) {
                Ty::Seq(Box::new(elem))
            } else {
                Ty::Dyn
            }
        }
        _ => Ty::Dyn,
    }
}

fn synth_collection_input(ctx: &TyCtx<'_>, input: &Expr) -> Ty {
    if let Expr::Range { start, end: Some(end) } = input {
        if synth(ctx, start) == Ty::Int && synth(ctx, end) == Ty::Int {
            return Ty::Seq(Box::new(Ty::Int));
        }
    }
    synth(ctx, input)
}

/// Infer collection callback results in their own parameter scope. Generic
/// unary numeric functions specialize to the input element type here, just as
/// they do in native lowering; propagating an unbound T loses top-level maps.
fn synth_transform(ctx: &TyCtx<'_>, input: &Expr, callback: &Expr, filter: bool) -> Ty {
    let Ty::Seq(elem) = synth_collection_input(ctx, input) else { return Ty::Dyn; };
    let result = match callback {
        Expr::Ident(name) if !ctx.gamma.contains_key(name) => {
            if let Some(sig) = ctx.fns.get(name) {
                if sig.params.len() != 1 { return Ty::Dyn; }
                if matches!(sig.ret, Ty::Var(_)) { (*elem).clone() } else { sig.ret.clone() }
            } else if name == "abs" {
                (*elem).clone()
            } else if matches!(name.as_str(), "floor" | "ceil" | "round") {
                Ty::Int
            } else if matches!(name.as_str(), "sqrt" | "exp" | "ln" | "sin" | "cos" | "tan") {
                Ty::Float
            } else {
                Ty::Dyn
            }
        }
        Expr::Lambda { params, body } if params.len() == 1 && params[0].pattern.is_none() => {
            let mut child = TyCtx { gamma: ctx.gamma.clone(), fns: ctx.fns };
            child.gamma.insert(params[0].name.clone(), (*elem).clone());
            match body.as_ref() {
                FuncBody::Expr(expr) => synth(&child, expr),
                FuncBody::Block(block) => synth_callback_block(&child, block),
            }
        }
        Expr::CondLambda(expr) => {
            let mut child = TyCtx { gamma: ctx.gamma.clone(), fns: ctx.fns };
            child.gamma.insert("_".into(), (*elem).clone());
            synth(&child, expr)
        }
        _ => Ty::Dyn,
    };
    if filter {
        if result == Ty::Bool { Ty::Seq(elem) } else { Ty::Dyn }
    } else if result != Ty::Dyn {
        Ty::Seq(Box::new(result))
    } else {
        Ty::Dyn
    }
}

/// Callback blocks may return from either branch before reaching their tail.
/// Keep every reachable return's exact scalar type: coercing int to float
/// here would change dynamic maps whose branches return different types.
fn synth_callback_block(ctx: &TyCtx<'_>, block: &Block) -> Ty {
    fn scalar(ty: &Ty) -> bool {
        matches!(ty, Ty::Int | Ty::Float | Ty::Bool | Ty::Var(_))
    }

    fn record(result: &mut Option<Ty>, ty: Ty) -> Result<(), ()> {
        if !scalar(&ty) || result.as_ref().is_some_and(|previous| *previous != ty) {
            return Err(());
        }
        *result = Some(ty);
        Ok(())
    }

    // Only the outermost block has an implicit tail return. An expression
    // at the end of an if arm is still just an expression statement.
    fn walk(
        ctx: &mut TyCtx<'_>,
        block: &Block,
        tail_returns: bool,
        result: &mut Option<Ty>,
    ) -> Result<bool, ()> {
        for (index, stmt) in block.stmts.iter().enumerate() {
            match stmt {
                Stmt::Bind { name, value, .. } => {
                    let ty = synth(ctx, value);
                    if !scalar(&ty)
                        || ctx.gamma.get(name).is_some_and(|previous| *previous != ty)
                    {
                        return Err(());
                    }
                    ctx.gamma.insert(name.clone(), ty);
                }
                Stmt::CompoundAssign { name, op, value } => {
                    let previous = ctx.gamma.get(name).ok_or(())?;
                    let ty = num_join(previous, &synth(ctx, value), op);
                    if !scalar(&ty) || ty != *previous { return Err(()); }
                }
                Stmt::IncDec { name, .. } => {
                    if !matches!(ctx.gamma.get(name), Some(Ty::Int | Ty::Float | Ty::Var(_))) {
                        return Err(());
                    }
                }
                Stmt::Give { value: Some(value) } => {
                    record(result, synth(ctx, value))?;
                    return Ok(true);
                }
                Stmt::If { cond, then_block, else_block } => {
                    if synth(ctx, cond) != Ty::Bool { return Err(()); }
                    let mut then_ctx = TyCtx { gamma: ctx.gamma.clone(), fns: ctx.fns };
                    let then_returns = walk(&mut then_ctx, then_block, false, result)?;
                    let mut branch_names = then_ctx.gamma.keys()
                        .filter(|name| !ctx.gamma.contains_key(*name)).cloned().collect::<Vec<_>>();
                    let else_returns = if let Some(else_block) = else_block {
                        let mut else_ctx = TyCtx { gamma: ctx.gamma.clone(), fns: ctx.fns };
                        let returns = walk(&mut else_ctx, else_block, false, result)?;
                        branch_names.extend(else_ctx.gamma.keys()
                            .filter(|name| !ctx.gamma.contains_key(*name)).cloned());
                        returns
                    } else {
                        false
                    };
                    if then_returns && else_returns { return Ok(true); }
                    // Dynamic if arms share their surrounding binding scope.
                    // A conditionally introduced name cannot become a fresh
                    // native declaration after the branch (even when both
                    // arms happen to infer the same type).
                    for name in branch_names { ctx.gamma.insert(name, Ty::Dyn); }
                }
                Stmt::Expr(value) if tail_returns && index + 1 == block.stmts.len() => {
                    record(result, synth(ctx, value))?;
                    return Ok(true);
                }
                Stmt::Expr(value) => {
                    if !scalar(&synth(ctx, value)) { return Err(()); }
                }
                _ => return Err(()),
            }
        }
        Ok(false)
    }

    let mut child = TyCtx { gamma: ctx.gamma.clone(), fns: ctx.fns };
    let mut result = None;
    match walk(&mut child, block, true, &mut result) {
        Ok(true) => result.unwrap_or(Ty::Dyn),
        _ => Ty::Dyn,
    }
}

fn synth_binary(ctx: &TyCtx<'_>, op: &BinOp, lhs: &Expr, rhs: &Expr) -> Ty {
    use BinOp::*;
    match op {
        // Comparisons / logical → bool.
        Eq | NotEq | Lt | Gt | LtEq | GtEq | And | Or => Ty::Bool,
        // Bitwise / shifts → int.
        BAnd | BOr | BXor | Shl | Shr => Ty::Int,
        // Arithmetic → numeric join.
        Add | Sub | Mul | Div | Rem | Pow => {
            let lt = synth(ctx, lhs);
            let rt = synth(ctx, rhs);
            num_join(&lt, &rt, op)
        }
    }
}

/// Numeric type join with J's coercion rule: if either side is float, the
/// result is float; if both int, result is int (except `/` and `**`, which
/// always produce float, matching `f64` division / `powf`). Anything
/// non-numeric ⇒ Dyn (bail).
fn num_join(lt: &Ty, rt: &Ty, op: &BinOp) -> Ty {
    let numeric = |t: &Ty| matches!(t, Ty::Int | Ty::Float | Ty::Var(_));
    if !numeric(lt) || !numeric(rt) {
        return Ty::Dyn;
    }
    if matches!(op, BinOp::Div | BinOp::Pow) {
        return Ty::Float;
    }
    if *lt == Ty::Float || *rt == Ty::Float {
        Ty::Float
    } else if matches!(lt, Ty::Var(_)) {
        // Generic carries through (a generic param joined with int stays
        // generic - it monomorphizes to the concrete width later).
        lt.clone()
    } else if matches!(rt, Ty::Var(_)) {
        rt.clone()
    } else {
        Ty::Int
    }
}

fn synth_call(ctx: &TyCtx<'_>, callee: &Expr, args: &[Expr]) -> Ty {
    if let Expr::Ident(name) = callee {
        // Known native function → its return type.
        if let Some(sig) = ctx.fns.get(name) {
            return sig.ret.clone();
        }
        if matches!(name.as_str(), "map" | "filter") && args.len() == 2 && !ctx.gamma.contains_key(name) {
            return synth_transform(ctx, &args[0], &args[1], name == "filter");
        }
        // Native built-ins recognized by the typed path.
        match name.as_str() {
            "len" => return Ty::Int,
            "make_seq" if args.len() == 2 => return Ty::Seq(Box::new(synth(ctx, &args[1]))),
            "copy" if args.len() == 1 => return synth(ctx, &args[0]),
            "collect" if args.len() == 1 => return synth_collection_input(ctx, &args[0]),
            "abs" if args.len() == 1 => return synth(ctx, &args[0]),
            "floor" | "ceil" | "round" if args.len() == 1 => return Ty::Int,
            "sqrt" | "exp" | "ln" | "log" | "sin" | "cos" | "tan" | "pow" => {
                // Math builtins are float → float in native code.
                let _ = args;
                return Ty::Float;
            }
            // min/max/clamp preserve the numeric join of their operands.
            "max" | "min" if args.len() == 2 => {
                return num_join(&synth(ctx, &args[0]), &synth(ctx, &args[1]), &BinOp::Add);
            }
            "clamp" if args.len() == 3 => {
                let j = num_join(&synth(ctx, &args[0]), &synth(ctx, &args[1]), &BinOp::Add);
                return num_join(&j, &synth(ctx, &args[2]), &BinOp::Add);
            }
            // Text predicates / search.
            "contains" | "starts_with" | "ends_with" if args.len() == 2 => return Ty::Bool,
            "find" if args.len() == 2 => return Ty::Int,
            // String-producing text builtins.
            "upper" | "lower" | "trim" if args.len() == 1 => return Ty::Text,
            "replace" if args.len() == 3 => return Ty::Text,
            "split" if args.len() == 2 => return Ty::Seq(Box::new(Ty::Text)),
            "join" if args.len() == 2 => return Ty::Text,
            _ => {}
        }
    }
    Ty::Dyn
}

/// Whether an expression of type `from` can flow into an expected `to`
/// position (used for `check`). `int`→`float` is an allowed coercion.
pub fn coercible(from: &Ty, to: &Ty) -> bool {
    if from == to { return true; }
    matches!((from, to), (Ty::Int, Ty::Float))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transform_type(source: &str, elem: Ty) -> Ty {
        let tokens = crate::lexer::tokenize(source).unwrap();
        let program = crate::parser::parse(&tokens).unwrap();
        let Stmt::Bind { value, .. } = &program.items[0] else { panic!("expected binding"); };
        let fns = NativeFns::new();
        let mut ctx = TyCtx::new(&fns);
        ctx.gamma.insert("xs".into(), Ty::Seq(Box::new(elem)));
        synth(&ctx, value)
    }

    #[test]
    fn callback_blocks_infer_locals_and_all_early_returns() {
        let source = "out = map(xs, func(x) = {\n value := x * 2\n value += 1\n if x > 0 {\n offset = value + 10\n give offset\n }\n value\n})";
        assert_eq!(transform_type(source, Ty::Int), Ty::Seq(Box::new(Ty::Int)));
        let source = "out = filter(xs, func(x) = {\n if x > 0 {\n if x > 10 { give true }\n give false\n } else { give true }\n})";
        assert_eq!(transform_type(source, Ty::Int), Ty::Seq(Box::new(Ty::Int)));
        let source = "out = map(xs, func(x) = {\n doubled = x * 2\n doubled\n})";
        let generic = Ty::Var("T".into());
        assert_eq!(transform_type(source, generic.clone()), Ty::Seq(Box::new(generic)));
    }

    #[test]
    fn callback_blocks_reject_mixed_unknown_and_missing_returns() {
        for source in [
            "out = map(xs, func(x) = {\n if x > 0 { give 1.5 }\n x\n})",
            "out = map(xs, func(x) = {\n if x > 0 { give x }\n})",
            "out = map(xs, func(x) = {\n if x > 0 { give missing }\n x\n})",
            "out = map(xs, func(x) = {\n value := x\n if x > 0 { value = 1.5 }\n value\n})",
            "out = map(xs, func(x) = {\n if x > 0 { give }\n x\n})",
        ] {
            assert_eq!(transform_type(source, Ty::Int), Ty::Dyn, "{source}");
        }
    }

    #[test]
    fn callback_branch_declarations_cannot_escape_or_be_redeclared() {
        for source in [
            "out = map(xs, func(x) = {\n if x > 0 { local = 1 }\n local\n})",
            "out = map(xs, func(x) = {\n if x > 0 { local = 1 } else { local = 2 }\n local\n})",
            "out = map(xs, func(x) = {\n if x > 0 { local = 1 }\n local = 2\n local\n})",
        ] {
            assert_eq!(transform_type(source, Ty::Int), Ty::Dyn, "{source}");
        }
    }
}
