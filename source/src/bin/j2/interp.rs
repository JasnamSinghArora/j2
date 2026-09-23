//! Tree-walking J interpreter, falls back to compilation

use std::collections::HashMap;

use j2_compiler::ast::*;
use j2_runtime::error::J2Err;
use j2_runtime::prelude::*;
use j2_runtime::value::J2Value;

/// Unsupported construct or genuine J runtime error
pub enum IErr {
    Unsupported,
    J(J2Err),
}
type IRes<T> = Result<T, IErr>;
impl From<J2Err> for IErr {
    fn from(e: J2Err) -> Self {
        IErr::J(e)
    }
}

/// Control-flow result of executing a statement.
enum Flow {
    Normal,
    Give(J2Value),
    Stop,
    Skip,
}

struct Interp {
    /// Scope stack; `frames[0]` global, bindings carry mutability
    frames: Vec<HashMap<String, (J2Value, bool)>>,
    builtins: HashMap<String, J2Value>,
    funcs: HashMap<String, (Vec<Param>, FuncBody)>,
    /// Source line of the statement currently executing
    cur_line: usize,
}

fn build_builtins() -> HashMap<String, J2Value> {
    let mut m = HashMap::new();
    j2_builtins::install(&mut m);
    j2_math::install(&mut m);
    j2_stats::install(&mut m);
    j2_time::install(&mut m);
    j2_rand::install(&mut m);
    j2_fs::install(&mut m);
    j2_proc::install(&mut m);
    j2_str::install(&mut m);
    j2_regex::install(&mut m);
    j2_json::install(&mut m);
    j2_date::install(&mut m);
    j2_hash::install(&mut m);
    j2_base64::install(&mut m);
    j2_hex::install(&mut m);
    j2_http::install(&mut m);
    j2_sys::install(&mut m);
    j2_bits::install(&mut m);
    j2_async::install(&mut m);
    m
}

impl Interp {
    fn new() -> Self {
        Interp {
            frames: vec![HashMap::new()],
            builtins: build_builtins(),
            funcs: HashMap::new(),
            cur_line: 0,
        }
    }

    fn lookup(&self, name: &str) -> Option<J2Value> {
        if let Some((v, _)) = self.frames.last().unwrap().get(name) {
            return Some(v.clone());
        }
        if let Some((v, _)) = self.frames[0].get(name) {
            return Some(v.clone());
        }
        self.builtins.get(name).cloned()
    }

    /// Bind var; same-scope constant rebind errors
    fn bind(&mut self, name: &str, v: J2Value, mutable: bool) -> IRes<()> {
        let frame = self.frames.last_mut().unwrap();
        let eff = match frame.get(name) {
            Some((_, true)) => true,  // re-bind a mutable local -> stays mutable
            Some((_, false)) => {
                return Err(IErr::J(J2Err::mutability(format!(
                    "\"{}\" is a constant and cannot be reassigned",
                    name
                ))));
            }
            None => mutable, // fresh declaration (shadows any inherited)
        };
        frame.insert(name.to_string(), (v, eff));
        Ok(())
    }

    /// Fresh mutable local, no constant check
    fn force_set(&mut self, name: &str, v: J2Value) {
        self.frames.last_mut().unwrap().insert(name.to_string(), (v, true));
    }

    /// Reassign an existing binding
    fn reassign(&mut self, name: &str, v: J2Value) -> IRes<()> {
        let top = self.frames.len() - 1;
        for idx in [top, 0] {
            if let Some((_, mutable)) = self.frames[idx].get(name) {
                if !*mutable {
                    return Err(IErr::J(J2Err::mutability(format!(
                        "\"{}\" is a constant and cannot be reassigned",
                        name
                    ))));
                }
                self.frames[idx].insert(name.to_string(), (v, true));
                return Ok(());
            }
            if top == 0 { break; }
        }
        Err(IErr::J(J2Err::name_err(format!("\"{}\" is not defined", name))))
    }

    fn eval(&mut self, e: &Expr) -> IRes<J2Value> {
        Ok(match e {
            Expr::IntLit(n) => J2Value::Int(*n),
            Expr::FloatLit(x) => J2Value::Float(*x),
            Expr::TextLit(s) => J2Value::text(s.clone()),
            Expr::Bool(b) => J2Value::Bool(*b),
            Expr::Null => J2Value::Null,
            Expr::Ident(name) => self
                .lookup(name)
                .ok_or_else(|| IErr::J(J2Err::name_err(format!("\"{}\" is not defined", name))))?,
            Expr::Unary { op, operand } => {
                let v = self.eval(operand)?;
                match op {
                    UnaryOp::Neg => v.neg()?,
                    UnaryOp::Pos => v,
                    UnaryOp::Not => J2Value::Bool(!v.truthy()),
                    UnaryOp::BNot => v.bnot()?,
                }
            }
            Expr::Binary { op, lhs, rhs } => return self.eval_binary(op, lhs, rhs),
            Expr::Index { coll, idx } => {
                let c = self.eval(coll)?;
                let i = self.eval(idx)?;
                self.index(&c, &i)?
            }
            Expr::Member { obj, field } => {
                let o = self.eval(obj)?;
                match &o {
                    J2Value::Map(m) => m
                        .lock()
                        .unwrap()
                        .get(field)
                        .cloned()
                        .ok_or_else(|| IErr::J(J2Err::key(format!("no field {}", field))))?,
                    // No classes, so non-map member access errors
                    _ => return Err(IErr::J(J2Err::type_err(format!(
                        "cannot access field {} on {}",
                        field,
                        o.type_name()
                    )))),
                }
            }
            Expr::SeqLit(items) => {
                let mut v = Vec::with_capacity(items.len());
                for it in items {
                    v.push(self.eval(it)?);
                }
                J2Value::seq(v)
            }
            Expr::MapLit(entries) => {
                let mut m = HashMap::new();
                for (k, ve) in entries {
                    m.insert(k.clone(), self.eval(ve)?);
                }
                J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m)))
            }
            Expr::Pair(a, b) => J2Value::pair(self.eval(a)?, self.eval(b)?),
            Expr::Range { start, end } => {
                // Eager inclusive range -> seq
                let Some(end) = end else { return Err(IErr::Unsupported) };
                let s = self.eval(start)?.as_num_i64()?;
                let e = self.eval(end)?.as_num_i64()?;
                let mut v = Vec::new();
                let mut i = s;
                while i <= e {
                    v.push(J2Value::Int(i));
                    i += 1;
                }
                J2Value::seq(v)
            }
            Expr::Call { callee, args } => return self.eval_call(callee, args),
            // Lambdas, classes, blocks etc need compile path
            _ => return Err(IErr::Unsupported),
        })
    }

    fn eval_binary(&mut self, op: &BinOp, lhs: &Expr, rhs: &Expr) -> IRes<J2Value> {
        // Short-circuiting boolean ops.
        if matches!(op, BinOp::And | BinOp::Or) {
            let l = self.eval(lhs)?;
            let lt = l.truthy();
            return Ok(match op {
                BinOp::And => if lt { self.eval(rhs)? } else { l },
                BinOp::Or => if lt { l } else { self.eval(rhs)? },
                _ => unreachable!(),
            });
        }
        let l = self.eval(lhs)?;
        let r = self.eval(rhs)?;
        Ok(match op {
            BinOp::Add => l.add(&r)?,
            BinOp::Sub => l.sub(&r)?,
            BinOp::Mul => l.mul(&r)?,
            BinOp::Div => l.div(&r)?,
            BinOp::Rem => l.rem(&r)?,
            BinOp::Pow => l.pow(&r)?,
            BinOp::Eq => J2Value::Bool(l.eq(&r)),
            BinOp::NotEq => J2Value::Bool(!l.eq(&r)),
            BinOp::Lt => J2Value::Bool(l.cmp_lt(&r)?),
            BinOp::Gt => J2Value::Bool(r.cmp_lt(&l)?),
            BinOp::LtEq => J2Value::Bool(l.cmp_lt(&r)? || l.eq(&r)),
            BinOp::GtEq => J2Value::Bool(r.cmp_lt(&l)? || l.eq(&r)),
            BinOp::BAnd => l.band(&r)?,
            BinOp::BOr => l.bor(&r)?,
            BinOp::BXor => l.bxor(&r)?,
            BinOp::Shl => l.shl(&r)?,
            BinOp::Shr => l.shr(&r)?,
            BinOp::And | BinOp::Or => unreachable!(),
        })
    }

    fn index(&self, c: &J2Value, i: &J2Value) -> IRes<J2Value> {
        match c {
            J2Value::Seq(s) => {
                let s = s.lock().unwrap();
                let n = s.items.len() as i64;
                let idx = i.as_num_i64()?;
                if idx < 0 || idx >= n {
                    return Err(IErr::J(J2Err::index(format!(
                        "index {} is out of range for seq of length {}",
                        idx, n
                    ))));
                }
                Ok(s.items[idx as usize].clone())
            }
            // Packed float seq indexes like boxed
            J2Value::SeqF64(s) => {
                let s = s.lock().unwrap();
                let n = s.len() as i64;
                let idx = i.as_num_i64()?;
                if idx < 0 || idx >= n {
                    return Err(IErr::J(J2Err::index(format!(
                        "index {} is out of range for seq of length {}",
                        idx, n
                    ))));
                }
                Ok(J2Value::Float(s[idx as usize]))
            }
            J2Value::Map(m) => {
                let key = i.as_text()?;
                m.lock()
                    .unwrap()
                    .get(&key)
                    .cloned()
                    .ok_or_else(|| IErr::J(J2Err::key(format!("no key {}", key))))
            }
            _ => Err(IErr::J(J2Err::type_err(format!("cannot index {}", c.type_name())))),
        }
    }

    fn eval_call(&mut self, callee: &Expr, args: &[Expr]) -> IRes<J2Value> {
        let Expr::Ident(name) = callee else { return Err(IErr::Unsupported) };
        let mut argv = Vec::with_capacity(args.len());
        for a in args {
            argv.push(self.eval(a)?);
        }
        // User-defined function?
        if let Some((params, body)) = self.funcs.get(name).cloned() {
            return self.call_user(&params, &body, argv);
        }
        // Builtin?
        if let Some(J2Value::Builtin(f, _)) = self.lookup(name) {
            return f(&argv).map_err(IErr::J);
        }
        Err(IErr::J(J2Err::name_err(format!("\"{}\" is not defined", name))))
    }

    fn call_user(&mut self, params: &[Param], body: &FuncBody, argv: Vec<J2Value>) -> IRes<J2Value> {
        if params.iter().any(|p| p.pattern.is_some()) {
            return Err(IErr::Unsupported); // pattern/multi-clause -> compile path
        }
        if argv.len() != params.len() {
            return Err(IErr::J(J2Err::type_err(format!(
                "expected {} argument(s), got {}",
                params.len(),
                argv.len()
            ))));
        }
        let mut frame: HashMap<String, (J2Value, bool)> = HashMap::new();
        for (p, v) in params.iter().zip(argv) {
            frame.insert(p.name.clone(), (v, true)); // params are mutable locals
        }
        self.frames.push(frame);
        let result = (|| match body {
            FuncBody::Expr(e) => self.eval(e),
            FuncBody::Block(b) => match self.exec_block(&b.stmts, &b.stmt_lines)? {
                Flow::Give(v) => Ok(v),
                _ => Ok(J2Value::Null),
            },
        })();
        self.frames.pop();
        result
    }

    /// Run block, return first non-Normal flow
    fn exec_block(&mut self, stmts: &[Stmt], lines: &[usize]) -> IRes<Flow> {
        for (i, s) in stmts.iter().enumerate() {
            if let Some(&ln) = lines.get(i) {
                if ln > 0 { self.cur_line = ln; }
            }
            match self.exec(s)? {
                Flow::Normal => {}
                other => return Ok(other),
            }
        }
        Ok(Flow::Normal)
    }

    fn exec(&mut self, s: &Stmt) -> IRes<Flow> {
        match s {
            Stmt::Func { name, params, body, .. } => {
                self.funcs.insert(name.clone(), (params.clone(), body.clone()));
            }
            Stmt::Bind { name, value, mutable } => {
                let v = self.eval(value)?;
                self.bind(name, v, *mutable)?;
            }
            Stmt::CompoundAssign { name, op, value } => {
                let cur = self
                    .lookup(name)
                    .ok_or_else(|| IErr::J(J2Err::name_err(format!("\"{}\" is not defined", name))))?;
                let rhs = self.eval(value)?;
                let nv = match op {
                    BinOp::Add => cur.add(&rhs)?,
                    BinOp::Sub => cur.sub(&rhs)?,
                    BinOp::Mul => cur.mul(&rhs)?,
                    BinOp::Div => cur.div(&rhs)?,
                    BinOp::Rem => cur.rem(&rhs)?,
                    _ => return Err(IErr::Unsupported),
                };
                self.reassign(name, nv)?;
            }
            Stmt::IncDec { name, inc } => {
                let cur = self
                    .lookup(name)
                    .ok_or_else(|| IErr::J(J2Err::name_err(format!("\"{}\" is not defined", name))))?;
                let one = J2Value::Int(1);
                let nv = if *inc { cur.add(&one)? } else { cur.sub(&one)? };
                self.reassign(name, nv)?;
            }
            Stmt::IndexedAssign { target, idx, value } => {
                let coll = self.eval(target)?;
                let i = self.eval(idx)?;
                let v = self.eval(value)?;
                match &coll {
                    J2Value::Seq(s) => {
                        let mut s = s.lock().unwrap();
                        let n = s.items.len() as i64;
                        let ix = i.as_num_i64()?;
                        if ix < 0 || ix >= n {
                            return Err(IErr::J(J2Err::index(format!(
                                "index {} is out of range for seq of length {}",
                                ix, n
                            ))));
                        }
                        s.items[ix as usize] = v;
                    }
                    J2Value::SeqF64(s) => {
                        let mut s = s.lock().unwrap();
                        let n = s.len() as i64;
                        let ix = i.as_num_i64()?;
                        if ix < 0 || ix >= n {
                            return Err(IErr::J(J2Err::index(format!(
                                "index {} is out of range for seq of length {}",
                                ix, n
                            ))));
                        }
                        s[ix as usize] = v.as_num_f64()?;
                    }
                    J2Value::Map(m) => {
                        m.lock().unwrap().insert(i.as_text()?, v);
                    }
                    _ => return Err(IErr::J(J2Err::type_err(format!(
                        "cannot index-assign {}",
                        coll.type_name()
                    )))),
                }
            }
            Stmt::MemberAssign { target, field, value } => {
                let o = self.eval(target)?;
                let v = self.eval(value)?;
                match &o {
                    J2Value::Map(m) => {
                        m.lock().unwrap().insert(field.clone(), v);
                    }
                    _ => return Err(IErr::J(J2Err::type_err(format!(
                        "cannot set field {} on {}",
                        field,
                        o.type_name()
                    )))),
                }
            }
            Stmt::If { cond, then_block, else_block } => {
                if self.eval(cond)?.truthy() {
                    return self.exec_scoped(&then_block.stmts, &then_block.stmt_lines);
                } else if let Some(eb) = else_block {
                    return self.exec_scoped(&eb.stmts, &eb.stmt_lines);
                }
            }
            Stmt::ForLoop { bindings, iter, filter, until, body } => {
                if filter.is_some() || until.is_some() || bindings.len() != 1 {
                    return Err(IErr::Unsupported);
                }
                let items = self.iter_items(iter)?;
                let var = &bindings[0];
                for item in items {
                    self.force_set(var, item);
                    match self.exec_scoped(&body.stmts, &body.stmt_lines)? {
                        Flow::Stop => break,
                        Flow::Skip | Flow::Normal => {}
                        g @ Flow::Give(_) => return Ok(g),
                    }
                }
            }
            Stmt::Repeat { cond, body } => {
                while self.eval(cond)?.truthy() {
                    match self.exec_scoped(&body.stmts, &body.stmt_lines)? {
                        Flow::Stop => break,
                        Flow::Skip | Flow::Normal => {}
                        g @ Flow::Give(_) => return Ok(g),
                    }
                }
            }
            Stmt::Give { value } => {
                let v = match value {
                    Some(e) => self.eval(e)?,
                    None => J2Value::Null,
                };
                return Ok(Flow::Give(v));
            }
            Stmt::Stop { cond } => {
                if cond.as_ref().map(|c| self.eval(c)).transpose()?.map(|v| v.truthy()).unwrap_or(true) {
                    return Ok(Flow::Stop);
                }
            }
            Stmt::Skip { cond } => {
                if cond.as_ref().map(|c| self.eval(c)).transpose()?.map(|v| v.truthy()).unwrap_or(true) {
                    return Ok(Flow::Skip);
                }
            }
            Stmt::Assert { cond, msg } => {
                if !self.eval(cond)?.truthy() {
                    let m = match msg {
                        Some(e) => self.eval(e)?.as_text().unwrap_or_default(),
                        None => "assertion failed".to_string(),
                    };
                    return Err(IErr::J(J2Err::runtime(format!("assertion failed: {}", m))));
                }
            }
            Stmt::Expr(e) => {
                self.eval(e)?;
            }
            // Loop, DoRepeat, Try, Global need compile path
            _ => return Err(IErr::Unsupported),
        }
        Ok(Flow::Normal)
    }

    /// Execute a nested block with per-block scoping
    fn exec_scoped(&mut self, stmts: &[Stmt], lines: &[usize]) -> IRes<Flow> {
        let snapshot: Vec<String> = self.frames.last().unwrap().keys().cloned().collect();
        let flow = self.exec_block(stmts, lines)?;
        let frame = self.frames.last_mut().unwrap();
        let keep: std::collections::HashSet<&str> = snapshot.iter().map(|s| s.as_str()).collect();
        frame.retain(|k, _| keep.contains(k.as_str()));
        Ok(flow)
    }

    fn iter_items(&mut self, iter: &Expr) -> IRes<Vec<J2Value>> {
        // `for i in a..b`
        if let Expr::Range { start, end: Some(end) } = iter {
            let s = self.eval(start)?.as_num_i64()?;
            let e = self.eval(end)?.as_num_i64()?;
            let mut v = Vec::new();
            let mut i = s;
            while i <= e {
                v.push(J2Value::Int(i));
                i += 1;
            }
            return Ok(v);
        }
        match self.eval(iter)? {
            J2Value::Seq(s) => Ok(s.lock().unwrap().items.clone()),
            J2Value::SeqF64(s) => Ok(s.lock().unwrap().iter().map(|x| J2Value::Float(*x)).collect()),
            other => Err(IErr::J(J2Err::type_err(format!("cannot iterate {}", other.type_name())))),
        }
    }

    fn run(&mut self, prog: &Program) -> IRes<()> {
        // Register all functions first for forward refs
        for s in &prog.items {
            if let Stmt::Func { name, params, body, .. } = s {
                self.funcs.insert(name.clone(), (params.clone(), body.clone()));
            }
        }
        for (idx, s) in prog.items.iter().enumerate() {
            if matches!(s, Stmt::Func { .. }) {
                continue;
            }
            if let Some(&ln) = prog.item_lines.get(idx) {
                if ln > 0 { self.cur_line = ln; }
            }
            match self.exec(s)? {
                Flow::Normal => {}
                _ => return Err(IErr::Unsupported), // give/stop/skip at top level
            }
        }
        Ok(())
    }
}

/// Whole-program check; partial interpretation would duplicate effects
pub fn supported(prog: &Program) -> bool {
    let funcs: std::collections::HashSet<&str> = prog
        .items
        .iter()
        .filter_map(|s| match s {
            Stmt::Func { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    prog.items.iter().all(|s| stmt_ok(s, &funcs))
}

fn block_ok(b: &Block, f: &std::collections::HashSet<&str>) -> bool {
    b.stmts.iter().all(|s| stmt_ok(s, f))
}

fn stmt_ok(s: &Stmt, f: &std::collections::HashSet<&str>) -> bool {
    match s {
        Stmt::Bind { value, .. } | Stmt::CompoundAssign { value, .. } => expr_ok(value, f),
        Stmt::IncDec { .. } => true,
        Stmt::IndexedAssign { target, idx, value } => {
            expr_ok(target, f) && expr_ok(idx, f) && expr_ok(value, f)
        }
        Stmt::MemberAssign { target, value, .. } => expr_ok(target, f) && expr_ok(value, f),
        Stmt::Func { params, body, .. } => {
            params.iter().all(|p| p.pattern.is_none()) && funcbody_ok(body, f)
        }
        Stmt::If { cond, then_block, else_block } => {
            expr_ok(cond, f)
                && block_ok(then_block, f)
                && else_block.as_ref().map(|b| block_ok(b, f)).unwrap_or(true)
        }
        Stmt::ForLoop { bindings, iter, filter, until, body } => {
            bindings.len() == 1
                && filter.is_none()
                && until.is_none()
                && expr_ok(iter, f)
                && block_ok(body, f)
        }
        Stmt::Repeat { cond, body } => expr_ok(cond, f) && block_ok(body, f),
        Stmt::Give { value } => value.as_ref().map(|e| expr_ok(e, f)).unwrap_or(true),
        Stmt::Stop { cond } | Stmt::Skip { cond } => {
            cond.as_ref().map(|c| expr_ok(c, f)).unwrap_or(true)
        }
        Stmt::Assert { cond, msg } => {
            expr_ok(cond, f) && msg.as_ref().map(|m| expr_ok(m, f)).unwrap_or(true)
        }
        Stmt::Expr(e) => expr_ok(e, f),
        // Loop, DoRepeat, Try, Global need compile path
        _ => false,
    }
}

fn funcbody_ok(b: &FuncBody, f: &std::collections::HashSet<&str>) -> bool {
    match b {
        FuncBody::Expr(e) => expr_ok(e, f),
        FuncBody::Block(blk) => block_ok(blk, f),
    }
}

fn expr_ok(e: &Expr, f: &std::collections::HashSet<&str>) -> bool {
    match e {
        Expr::IntLit(_) | Expr::FloatLit(_) | Expr::TextLit(_) | Expr::Bool(_) | Expr::Null => true,
        // Function-as-value (HOF) unsupported, compile it
        Expr::Ident(n) => !f.contains(n.as_str()),
        Expr::Unary { operand, .. } => expr_ok(operand, f),
        Expr::Binary { lhs, rhs, .. } => expr_ok(lhs, f) && expr_ok(rhs, f),
        Expr::Index { coll, idx } => expr_ok(coll, f) && expr_ok(idx, f),
        Expr::Member { obj, .. } => expr_ok(obj, f),
        Expr::SeqLit(items) => items.iter().all(|x| expr_ok(x, f)),
        Expr::MapLit(es) => es.iter().all(|(_, v)| expr_ok(v, f)),
        Expr::Pair(a, b) => expr_ok(a, f) && expr_ok(b, f),
        // Closed ranges only; open flows need compiling
        Expr::Range { start, end } => {
            end.is_some() && expr_ok(start, f) && expr_ok(end.as_ref().unwrap(), f)
        }
        // Direct calls only; callee must be name
        Expr::Call { callee, args } => {
            matches!(callee.as_ref(), Expr::Ident(_)) && args.iter().all(|a| expr_ok(a, f))
        }
        // Lambda, ClassLit, etc need compile path
        _ => false,
    }
}

/// Run `prog` under the interpreter
pub fn try_run(prog: &Program) -> Option<i32> {
    let mut it = Interp::new();
    match it.run(prog) {
        Ok(()) => Some(0),
        Err(IErr::J(e)) => {
            match it.cur_line {
                0 => eprintln!("{}", e),
                n => eprintln!("{} (at line {})", e, n),
            }
            Some(1)
        }
        Err(IErr::Unsupported) => None,
    }
}
