//! Ordered, independent indexed updates which do not fit the MIR idiom matchers.
//!
//! The proof is deliberately structural: one destination, exactly the induction
//! index, a scalar-only call graph, and at most one terminal write per path.
//! Reads of that destination become the worker's original element. Other arrays
//! are read-only (the native boundary separately rules out aliased arguments).
use super::*;

fn scalar(ty: &Ty) -> bool {
    matches!(ty, Ty::Int | Ty::Float | Ty::Bool)
}

/// Keep the existing allocation-free float scale/affine MIR fast paths. These
/// expressions cannot raise scalar errors and have no other array to bounds
/// check. More general expressions continue through the ordered helper.
fn existing_float_kernel(ctx: &NCtx<'_>, stmt: &Stmt, dst: &str, i: &str) -> bool {
    let Stmt::ForLoop {
        iter: Expr::Range { start, end: Some(end) }, filter: None, body, ..
    } = stmt else { return false; };
    if !matches!(start.as_ref(), Expr::IntLit(0)) { return false; }
    let Expr::Binary { op: BinOp::Sub, lhs, rhs } = end.as_ref() else { return false; };
    if !matches!(rhs.as_ref(), Expr::IntLit(1)) { return false; }
    if !matches!(lhs.as_ref(), Expr::Call { callee, args }
        if matches!(callee.as_ref(), Expr::Ident(n) if n == "len")
        && matches!(args.as_slice(), [Expr::Ident(n)] if n == dst)) { return false; }
    let [Stmt::IndexedAssign { value, .. }] = body.stmts.as_slice() else { return false; };
    let atom = |e: &Expr| matches!(e, Expr::IntLit(_) | Expr::FloatLit(_) | Expr::Const(_))
        || matches!(e, Expr::Ident(n) if matches!(ctx.gamma.get(n), Some(Ty::Int | Ty::Float)));
    let own = |e: &Expr| matches!(e, Expr::Index { coll, idx }
        if matches!(coll.as_ref(), Expr::Ident(n) if n == dst)
        && matches!(idx.as_ref(), Expr::Ident(n) if n == i));
    let scale = |e: &Expr| match e {
        Expr::Binary { op: BinOp::Mul, lhs, rhs } => (own(lhs) && atom(rhs)) || (atom(lhs) && own(rhs)),
        _ => false,
    };
    match value {
        Expr::Binary { op: BinOp::Add | BinOp::Sub, lhs, rhs } => {
            ((own(lhs) || scale(lhs)) && atom(rhs))
                || (atom(lhs) && (own(rhs) || scale(rhs)))
        }
        _ => scale(value),
    }
}

/// Check effects and replace the only legal destination read with the worker's
/// element. No whole-destination capture may escape into the closure.
fn rewrite(
    ctx: &NCtx<'_>, expr: &Expr, dst: &str, induction: &str, old: &str,
) -> NRes<Expr> {
    let recurse = |e| rewrite(ctx, e, dst, induction, old);
    Ok(match expr {
        Expr::IntLit(_) | Expr::FloatLit(_) | Expr::Bool(_) | Expr::Const(_) => expr.clone(),
        Expr::Ident(name) if name != dst && ctx.gamma.get(name).is_some_and(scalar) => expr.clone(),
        Expr::Unary { op, operand } => Expr::Unary { op: op.clone(), operand: Box::new(recurse(operand)?) },
        Expr::Binary { op, lhs, rhs } => Expr::Binary {
            op: op.clone(), lhs: Box::new(recurse(lhs)?), rhs: Box::new(recurse(rhs)?),
        },
        Expr::Index { coll, idx } => {
            let Expr::Ident(name) = coll.as_ref() else { return Err(()); };
            if name == dst {
                if !matches!(idx.as_ref(), Expr::Ident(i) if i == induction) { return Err(()); }
                Expr::Ident(old.into())
            } else {
                if !matches!(ctx.gamma.get(name), Some(Ty::Seq(e)) if scalar(e)) { return Err(()); }
                Expr::Index { coll: coll.clone(), idx: Box::new(recurse(idx)?) }
            }
        }
        Expr::Call { callee, args } => {
            let Expr::Ident(name) = callee.as_ref() else { return Err(()); };
            if ctx.gamma.contains_key(name) { return Err(()); }
            if name == "len" && !ctx.fns.contains_key(name) && args.len() == 1 {
                // A destination length inside the callback would capture the
                // mutable destination. It can always be bound before the loop.
                if !matches!(&args[0], Expr::Ident(n) if n != dst && matches!(ctx.gamma.get(n), Some(Ty::Seq(_)))) {
                    return Err(());
                }
                expr.clone()
            } else {
                let callable = if let Some(sig) = ctx.fns.get(name) {
                    sig.params.iter().all(|t| scalar(t) || matches!(t, Ty::Var(_)))
                        && (scalar(&sig.ret) || matches!(sig.ret, Ty::Var(_)))
                        && sig.borrows.iter().all(|b| *b == Borrow::ByVal)
                } else {
                    matches!(name.as_str(), "sqrt" | "abs" | "floor" | "ceil" | "round"
                        | "exp" | "ln" | "sin" | "cos" | "tan" | "pow" | "min" | "max" | "clamp")
                };
                if !callable { return Err(()); }
                Expr::Call { callee: callee.clone(), args: args.iter().map(recurse).collect::<NRes<_>>()? }
            }
        }
        _ => return Err(()),
    })
}

/// All paths must end at their sole write. An operation after a write could
/// fail after modifying that element, which cannot be represented by a staged
/// callback result. Local scalar bindings before the write are safe.
fn destination<'a>(block: &'a Block, induction: &str, found: &mut Option<&'a str>) -> NRes<()> {
    for (pos, stmt) in block.stmts.iter().enumerate() {
        match stmt {
            Stmt::Bind { .. } => {}
            Stmt::IndexedAssign { target: Expr::Ident(name), idx: Expr::Ident(i), .. }
                if i == induction && pos + 1 == block.stmts.len() => {
                    if found.is_some_and(|previous| previous != name) { return Err(()); }
                    *found = Some(name);
                }
            Stmt::If { then_block, else_block, .. } if pos + 1 == block.stmts.len() => {
                destination(then_block, induction, found)?;
                if let Some(other) = else_block { destination(other, induction, found)?; }
            }
            _ => return Err(()),
        }
    }
    Ok(())
}

struct Loop<'a> {
    dst: &'a str,
    induction: &'a str,
    old: &'a str,
    value: &'a str,
    elem: &'a Ty,
}

impl Loop<'_> {
    fn expr(&self, ctx: &mut NCtx<'_>, expr: &Expr, parallel: bool) -> NRes<(String, Ty)> {
        let rewritten = rewrite(ctx, expr, self.dst, self.induction, self.old)?;
        nlower_expr(ctx, if parallel { &rewritten } else { expr })
    }

    fn block(&self, ctx: &mut NCtx<'_>, block: &Block, parallel: bool) -> NRes<String> {
        let mut out = String::from("{\n");
        let mut terminal = false;
        for stmt in &block.stmts {
            match stmt {
                Stmt::Bind { name, value, .. } => {
                    // Rebinding an outer variable is a loop-carried effect.
                    if ctx.gamma.contains_key(name) { return Err(()); }
                    let (code, ty) = self.expr(ctx, value, parallel)?;
                    if !scalar(&ty) { return Err(()); }
                    writeln!(out, "let {name} = {code};").unwrap();
                    ctx.gamma.insert(name.clone(), ty);
                }
                Stmt::IndexedAssign { value, .. } => {
                    let (code, ty) = self.expr(ctx, value, parallel)?;
                    // A dynamic sequence can retain an integer element even
                    // among floats. Do not widen a newly admitted assignment.
                    if ty != *self.elem { return Err(()); }
                    if parallel {
                        writeln!(out, "{code}").unwrap();
                    } else {
                        let Self { dst, induction: i, value: v, .. } = self;
                        // J evaluates the RHS before validating the target.
                        writeln!(out, "let {v} = {code};").unwrap();
                        writeln!(out, "if {i} < 0 {{ std::panic::panic_any(J2Err::index(\"negative indices are not supported\")); }}").unwrap();
                        writeln!(out, "if ({i} as usize) >= {dst}.len() {{ std::panic::panic_any(J2Err::index(format!(\"index {{}} is out of range for seq of length {{}}\", {i}, {dst}.len()))); }}").unwrap();
                        writeln!(out, "{dst}[{i} as usize] = {v};").unwrap();
                    }
                    terminal = true;
                }
                Stmt::If { cond, then_block, else_block } => {
                    let (cond, ty) = self.expr(ctx, cond, parallel)?;
                    if ty != Ty::Bool { return Err(()); }
                    let then = self.block(&mut ctx.clone(), then_block, parallel)?;
                    let other = match else_block {
                        Some(b) => self.block(&mut ctx.clone(), b, parallel)?,
                        None if parallel => format!("{{ {} }}", self.old),
                        None => "{}".into(),
                    };
                    writeln!(out, "if {cond} {then} else {other}").unwrap();
                    terminal = true;
                }
                _ => return Err(()),
            }
        }
        if parallel && !terminal { writeln!(out, "{}", self.old).unwrap(); }
        out.push('}');
        Ok(out)
    }
}

pub(super) fn try_lower(ctx: &mut NCtx<'_>, stmt: &Stmt, out: &mut String, depth: usize) -> bool {
    let mut child = ctx.clone();
    match lower(&mut child, stmt, depth) {
        Ok(code) => {
            // Only fresh-name allocation escapes this lexical loop scope.
            ctx.tmp_ctr = child.tmp_ctr;
            out.push_str(&code);
            true
        }
        Err(()) => false,
    }
}

fn lower(ctx: &mut NCtx<'_>, stmt: &Stmt, depth: usize) -> NRes<String> {
    let Stmt::ForLoop { bindings, iter: Expr::Range { start, end: Some(end) }, filter, until: None, body } = stmt else { return Err(()); };
    if bindings.len() != 1 { return Err(()); }
    let induction = &bindings[0];
    let mut dst = None;
    destination(body, induction, &mut dst)?;
    let dst = dst.ok_or(())?;
    if dst == induction || !ctx.mut_seqs.contains(dst) { return Err(()); }
    let elem = match ctx.gamma.get(dst) {
        Some(Ty::Seq(e)) if matches!(e.as_ref(), Ty::Int | Ty::Float) => (**e).clone(),
        _ => return Err(()),
    };
    if elem == Ty::Float && existing_float_kernel(ctx, stmt, dst, induction) { return Err(()); }
    // Include local bindings and function identifiers when reserving names.
    let mut reserved: HashSet<String> = ctx.gamma.keys().cloned().collect();
    gather_all_idents_stmt(stmt, &mut reserved);
    let (mut writes, mut scalars, mut locals) = (HashSet::new(), HashSet::new(), HashSet::new());
    gather_stmt_refs(stmt, &mut reserved, &mut writes, &mut scalars, &mut locals);
    reserved.extend(locals);
    reserved.insert(induction.clone());
    let mut fresh = |stem: &str| {
        let mut name = format!("__j2_loop_{stem}_{}", ctx.tmp_ctr);
        ctx.tmp_ctr += 1;
        while reserved.contains(&name) { name.push('_'); }
        reserved.insert(name.clone());
        name
    };
    let (lo, hi, old, index, value) = (fresh("lo"), fresh("hi"), fresh("old"), fresh("index"), fresh("value"));
    ctx.can_hoist = false;
    ctx.checked_scalars = true;
    ctx.self_fields = None;
    // Range bounds are evaluated once in source order before any writes. They
    // may read the destination here, before its mutable worker borrow exists.
    rewrite(ctx, start, "", "", "")?;
    rewrite(ctx, end, "", "", "")?;
    let (start, start_ty) = nlower_expr(ctx, start)?;
    let (end, end_ty) = nlower_expr(ctx, end)?;
    if start_ty != Ty::Int || end_ty != Ty::Int { return Err(()); }
    ctx.gamma.insert(induction.clone(), Ty::Int);
    ctx.usize_vars.remove(induction);
    ctx.gamma.insert(old.clone(), elem.clone());
    let plan = Loop { dst, induction, old: &old, value: &value, elem: &elem };
    let mut parallel = plan.block(&mut ctx.clone(), body, true)?;
    let mut serial = plan.block(&mut ctx.clone(), body, false)?;
    if let Some(filter) = filter {
        let (pc, pt) = plan.expr(&mut ctx.clone(), filter, true)?;
        let (sc, st) = plan.expr(&mut ctx.clone(), filter, false)?;
        if pt != Ty::Bool || st != Ty::Bool { return Err(()); }
        parallel = format!("{{ if {pc} {parallel} else {{ {old} }} }}");
        serial = format!("{{ if {sc} {serial} }}");
    }
    let enabled = std::env::var("J2_PARALLEL").as_deref() != Ok("0");
    let ind = "    ".repeat(depth);
    // The guard avoids reading an out-of-bounds old element before a filter
    // or RHS which would otherwise run first. Signed inclusive serial ranges
    // also preserve descending iteration and negative-index diagnostics.
    Ok(format!("{ind}{{\n{ind}    let {lo}: i64 = {start};\n{ind}    let {hi}: i64 = {end};\n{ind}    if {enabled} && {lo} >= 0 && {lo} <= {hi} && ({hi} as u64) < ({dst}.len() as u64) {{\n{ind}        j2_runtime::parallel::update_native(&mut {dst}[..], {lo} as usize, ({hi} as usize) + 1, |{index}, {old}| {{ let {induction} = {index} as i64; {parallel} }});\n{ind}    }} else if {lo} <= {hi} {{\n{ind}        for {induction} in {lo}..={hi} {serial}\n{ind}    }} else {{\n{ind}        for {induction} in ({hi}..={lo}).rev() {serial}\n{ind}    }}\n{ind}}}\n"))
}
