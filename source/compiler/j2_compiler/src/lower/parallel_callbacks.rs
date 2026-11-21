//! Pure multi-statement collection callbacks. Keep the proof and lowering
//! together: normal native statement lowering intentionally recognizes faster
//! unchecked arithmetic idioms, which are unsuitable for these callbacks.
use super::*;

fn scalar(t: &Ty) -> bool { matches!(t, Ty::Int | Ty::Float | Ty::Bool) }

pub(super) fn binding_names(body: &FuncBody, names: &mut HashSet<String>) {
    fn walk(b: &Block, names: &mut HashSet<String>) {
        for stmt in &b.stmts {
            match stmt {
                Stmt::Bind { name, .. } => { names.insert(name.clone()); }
                Stmt::If { then_block, else_block, .. } => {
                    walk(then_block, names);
                    if let Some(other) = else_block { walk(other, names); }
                }
                _ => {}
            }
        }
    }
    if let FuncBody::Block(b) = body { walk(b, names); }
}

fn expr(ctx: &mut NCtx<'_>, e: &Expr) -> NRes<(String, Ty)> {
    if !scalar_callback_expr(ctx, e) { return Err(()); }
    let (code, ty) = nlower_expr(ctx, e)?;
    if !scalar(&ty) { return Err(()); }
    Ok((code, ty))
}

fn record_return(ret: &mut Option<Ty>, ty: Ty) -> NRes<()> {
    if ret.as_ref().is_some_and(|previous| *previous != ty) { return Err(()); }
    *ret = Some(ty);
    Ok(())
}

fn assign(ctx: &mut NCtx<'_>, locals: &HashMap<String, bool>, name: &str, value: &Expr) -> NRes<String> {
    if locals.get(name) != Some(&true) || name == "_" { return Err(()); }
    let (code, ty) = expr(ctx, value)?;
    if ctx.gamma.get(name) != Some(&ty) { return Err(()); }
    Ok(format!("{name} = {code};\n"))
}

/// Return whether every reachable path gives a value. Only the outermost
/// block's trailing expression is a return; an if-body expression is a
/// statement in J2. Branch-created bindings are poisoned after the branch:
/// J2 conditionals share a scope, so pretending they simply vanish is unsound.
fn block(
    ctx: &mut NCtx<'_>, locals: &mut HashMap<String, bool>, b: &Block,
    tail: bool, ret: &mut Option<Ty>, out: &mut String,
) -> NRes<bool> {
    for (i, stmt) in b.stmts.iter().enumerate() {
        match stmt {
            Stmt::Bind { name, value, mutable } => {
                if ctx.gamma.contains_key(name) {
                    out.push_str(&assign(ctx, locals, name, value)?);
                } else {
                    if name == "_" { return Err(()); }
                    let (code, ty) = expr(ctx, value)?;
                    let mutability = if *mutable { "mut " } else { "" };
                    writeln!(out, "let {mutability}{name} = {code};").unwrap();
                    ctx.gamma.insert(name.clone(), ty);
                    ctx.usize_vars.remove(name);
                    locals.insert(name.clone(), *mutable);
                }
            }
            Stmt::CompoundAssign { name, op, value } => {
                let e = Expr::Binary { op: op.clone(), lhs: Box::new(Expr::Ident(name.clone())), rhs: Box::new(value.clone()) };
                out.push_str(&assign(ctx, locals, name, &e)?);
            }
            Stmt::IncDec { name, inc } => {
                let e = Expr::Binary { op: if *inc { BinOp::Add } else { BinOp::Sub }, lhs: Box::new(Expr::Ident(name.clone())), rhs: Box::new(Expr::IntLit(1)) };
                out.push_str(&assign(ctx, locals, name, &e)?);
            }
            Stmt::If { cond, then_block, else_block } => {
                let (cond, ty) = expr(ctx, cond)?;
                if ty != Ty::Bool { return Err(()); }
                let mut then_ctx = ctx.clone();
                let mut then_locals = locals.clone();
                writeln!(out, "if {cond} {{").unwrap();
                let then_returns = block(&mut then_ctx, &mut then_locals, then_block, false, ret, out)?;
                out.push_str("}");
                let mut else_ctx = ctx.clone();
                let else_returns = if let Some(other) = else_block {
                    out.push_str(" else {\n");
                    let r = block(&mut else_ctx, &mut locals.clone(), other, false, ret, out)?;
                    out.push_str("}");
                    r
                } else { false };
                out.push('\n');
                if then_returns && else_returns { return Ok(true); }
                let names: Vec<_> = then_ctx.gamma.keys().chain(else_ctx.gamma.keys())
                    .filter(|n| !ctx.gamma.contains_key(*n)).cloned().collect();
                for name in names { ctx.gamma.insert(name, Ty::Dyn); }
            }
            Stmt::Give { value: Some(e) } => {
                let (code, ty) = expr(ctx, e)?;
                record_return(ret, ty)?;
                writeln!(out, "return {code};").unwrap();
                return Ok(true);
            }
            Stmt::Expr(e) => {
                let (code, ty) = expr(ctx, e)?;
                if tail && i + 1 == b.stmts.len() {
                    record_return(ret, ty)?;
                    writeln!(out, "return {code};").unwrap();
                    return Ok(true);
                }
                writeln!(out, "let _ = {code};").unwrap();
            }
            _ => return Err(()),
        }
    }
    Ok(false)
}

pub(super) fn lower(ctx: &mut NCtx<'_>, parameter: &str, arg: &str, body: &FuncBody) -> NRes<(String, Ty)> {
    match body {
        FuncBody::Expr(e) => expr(ctx, e),
        FuncBody::Block(b) => {
            let mut locals = HashMap::from([(parameter.to_string(), true)]);
            // Mutating a parameter modifies the per-invocation scalar copy.
            let mut code = format!("let mut {arg} = {arg};\n");
            let mut ret = None;
            if !block(ctx, &mut locals, b, true, &mut ret, &mut code)? { return Err(()); }
            Ok((code, ret.ok_or(())?))
        }
    }
}
