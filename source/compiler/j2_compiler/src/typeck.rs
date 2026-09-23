// bidirectional local type checker for native subset

use crate::ast::*;
use std::collections::HashMap;

/// How a seq/text param is borrowed natively
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Borrow {
    /// Scalar passed by value (`f64`, `i64`, `bool`).
    ByVal,
    /// `&[T]` / `&str` - read-only
    Shared,
    /// `&mut [T]`, body writes `param[i]`
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

/// function name to resolved native signature
pub type NativeFns = HashMap<String, FnSig>;

/// Native candidate iff annotated, no pattern params
pub fn is_native_candidate(params: &[Param], ret_ty: &Option<Ty>) -> bool {
    if ret_ty.is_none() { return false; }
    if params.is_empty() {
        // zero-arg annotated fn still a candidate
        return true;
    }
    params.iter().all(|p| p.pattern.is_none() && p.ty.is_some())
}

/// candidate signature, borrows inferred from writes
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

// type inference makes native lowering the default

/// Parameter usage accumulated in one walk
#[derive(Default, Clone, Copy)]
struct ParamUsage {
    used: bool,
    as_seq: bool,    // indexed / len() / sliced
    as_int: bool,    // loop bound, index position, %, bitwise
    as_text: bool,  // passed where text is expected
}

/// Infer native signature, None if not lowerable
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
                    return None; // unused param -> can't/needn't infer
                }
                if u.as_seq {
                    // element width stays generic unless pinned
                    Ty::Seq(Box::new(Ty::Var("T".into())))
                } else if u.as_int {
                    Ty::Int
                } else if u.as_text {
                    Ty::Text
                } else {
                    // Scalar numeric of undetermined width -> generic.
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

/// Walk expr accumulating how `name` is used
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
            if let Expr::Ident(f) = callee.as_ref() {
                // `len(p)`/`copy(p)` marks p a seq
                if (f == "len" || f == "copy") && args.len() == 1 {
                    if let Expr::Ident(n) = &args[0] {
                        if n == name { u.used = true; u.as_seq = true; }
                    }
                }
                // Text predicates/search -> both args are text.
                if matches!(f.as_str(), "contains" | "starts_with" | "ends_with" | "find") && args.len() == 2 {
                    for a in args {
                        if let Expr::Ident(n) = a {
                            if n == name { u.used = true; u.as_text = true; }
                        }
                    }
                }
                // String-producing text builtins -> arg is text.
                if matches!(f.as_str(), "upper" | "lower" | "trim") && args.len() == 1 {
                    if let Expr::Ident(n) = &args[0] {
                        if n == name { u.used = true; u.as_text = true; }
                    }
                }
                // interprocedural, inherit callee param's shape
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
        Expr::Range { start, end } => {
            usage_expr(start, name, true, u, fns);
            if let Some(e) = end { usage_expr(e, name, true, u, fns); }
        }
        Expr::Member { obj, .. } => usage_expr(obj, name, false, u, fns),
        Expr::Pair(a, b) => { usage_expr(a, name, false, u, fns); usage_expr(b, name, false, u, fns); }
        Expr::SeqLit(items) => { for it in items { usage_expr(it, name, false, u, fns); } }
        Expr::Pipe { lhs, rhs } | Expr::Filter { lhs, rhs } => {
            // pipe/filter lhs is iterated, so seq
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

/// Infer return type; None if not native
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
            // track top-level locals for trailing expr
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
            // no trailing expr or `give`, unit return
            Some(Ty::Nil)
        }
    }
}

/// Does body index-write `name`? Drives borrow inference
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
        // `m.field = v` mutates a map
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

// ------------------------- synthesis / checking -------------------------

/// Local typing context plus native fn signatures
pub struct TyCtx<'a> {
    pub gamma: HashMap<String, Ty>,
    pub fns: &'a NativeFns,
}

impl<'a> TyCtx<'a> {
    pub fn new(fns: &'a NativeFns) -> Self {
        TyCtx { gamma: HashMap::new(), fns }
    }
}

/// Bottom-up synthesis; `Ty::Dyn` means bail to dynamic
pub fn synth(ctx: &TyCtx<'_>, e: &Expr) -> Ty {
    match e {
        Expr::IntLit(_) => Ty::Int,
        Expr::FloatLit(_) => Ty::Float,
        Expr::Bool(_) => Ty::Bool,
        Expr::TextLit(_) => Ty::Text,
        Expr::Const(_) => Ty::Float, // PI, E, INF, ... are all f64
        Expr::Ident(name) => ctx.gamma.get(name).cloned().unwrap_or(Ty::Dyn),
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
        // map member read yields the value type
        Expr::Member { obj, .. } => match synth(ctx, obj) {
            Ty::Map(_, v) => *v,
            _ => Ty::Dyn,
        },
        // map literal gives `map<text, V>`, V joined
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
        // pipe yields seq of f's result type
        Expr::Pipe { lhs, rhs } => {
            if let Ty::Seq(_) = synth(ctx, lhs) {
                if let Expr::Ident(f) = rhs.as_ref() {
                    if let Some(sig) = ctx.fns.get(f) {
                        if sig.params.len() == 1 {
                            return Ty::Seq(Box::new(sig.ret.clone()));
                        }
                    }
                    match f.as_str() {
                        "sqrt" | "abs" | "floor" | "ceil" | "round" | "exp" | "ln" | "sin"
                        | "cos" | "tan" => return Ty::Seq(Box::new(Ty::Float)),
                        _ => {}
                    }
                }
            }
            Ty::Dyn
        }
        // filter keeps elem type; pred bool fn/cond-lambda
        Expr::Filter { lhs, rhs } => {
            if let Ty::Seq(elem) = synth(ctx, lhs) {
                match rhs.as_ref() {
                    Expr::CondLambda(_) => return Ty::Seq(elem),
                    Expr::Ident(f) => {
                        if let Some(sig) = ctx.fns.get(f) {
                            if sig.params.len() == 1 && sig.ret == Ty::Bool {
                                return Ty::Seq(elem);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ty::Dyn
        }
        Expr::SeqLit(items) => {
            if items.is_empty() {
                return Ty::Dyn;
            }
            // All-text literal -> seq<text>; else numeric join.
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

fn synth_binary(ctx: &TyCtx<'_>, op: &BinOp, lhs: &Expr, rhs: &Expr) -> Ty {
    use BinOp::*;
    match op {
        // Comparisons / logical -> bool.
        Eq | NotEq | Lt | Gt | LtEq | GtEq | And | Or => Ty::Bool,
        // Bitwise / shifts -> int.
        BAnd | BOr | BXor | Shl | Shr => Ty::Int,
        // Arithmetic -> numeric join.
        Add | Sub | Mul | Div | Rem | Pow => {
            let lt = synth(ctx, lhs);
            let rt = synth(ctx, rhs);
            num_join(&lt, &rt, op)
        }
    }
}

/// Numeric type join with J's coercion rule
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
        // generic joined with int stays generic
        lt.clone()
    } else if matches!(rt, Ty::Var(_)) {
        rt.clone()
    } else {
        Ty::Int
    }
}

fn synth_call(ctx: &TyCtx<'_>, callee: &Expr, args: &[Expr]) -> Ty {
    if let Expr::Ident(name) = callee {
        // Known native function -> its return type.
        if let Some(sig) = ctx.fns.get(name) {
            return sig.ret.clone();
        }
        // Native built-ins recognized by the typed path.
        match name.as_str() {
            "len" => return Ty::Int,
            "make_seq" if args.len() == 2 => return Ty::Seq(Box::new(synth(ctx, &args[1]))),
            "copy" if args.len() == 1 => return synth(ctx, &args[0]),
            "sqrt" | "abs" | "floor" | "ceil" | "round" | "exp" | "ln"
            | "log" | "sin" | "cos" | "tan" | "pow" => {
                // math builtins are float to float natively
                let _ = args;
                return Ty::Float;
            }
            // min/max/clamp keep numeric join of operands
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

/// Can `from` coerce to `to`; int-to-float allowed
pub fn coercible(from: &Ty, to: &Ty) -> bool {
    if from == to { return true; }
    matches!((from, to), (Ty::Int, Ty::Float))
}
