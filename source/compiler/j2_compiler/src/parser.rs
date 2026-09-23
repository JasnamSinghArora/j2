// recursive descent J parser; grammar in spec

use crate::ast::*;
use crate::error::J2Error;
use crate::lexer::{Tok, Token};

pub fn parse(tokens: &[Token]) -> Result<Program, J2Error> {
    let mut p = Parser { toks: tokens, pos: 0, angle_pending: 0 };
    p.parse_program()
}

/// Whether expr references implicit `_`
fn expr_uses_underscore(e: &Expr) -> bool {
    match e {
        Expr::Underscore => true,
        Expr::Binary { lhs, rhs, .. } => expr_uses_underscore(lhs) || expr_uses_underscore(rhs),
        Expr::Unary { operand, .. } => expr_uses_underscore(operand),
        Expr::Index { coll, idx } => expr_uses_underscore(coll) || expr_uses_underscore(idx),
        Expr::Call { callee, args } => {
            expr_uses_underscore(callee) || args.iter().any(expr_uses_underscore)
        }
        Expr::Member { obj, .. } => expr_uses_underscore(obj),
        Expr::Range { start, end } => {
            expr_uses_underscore(start) || end.as_ref().map_or(false, |e| expr_uses_underscore(e))
        }
        Expr::Pair(a, b) => expr_uses_underscore(a) || expr_uses_underscore(b),
        Expr::SeqLit(items) => items.iter().any(expr_uses_underscore),
        // nested pipe/filter owns its `_` scope
        _ => false,
    }
}

struct Parser<'a> {
    toks: &'a [Token],
    pos: usize,
    /// Leftover `>` closes banked from `>>` tokens
    angle_pending: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].kind
    }
    fn peek_at(&self, n: usize) -> &Tok {
        let i = self.pos + n;
        if i < self.toks.len() {
            &self.toks[i].kind
        } else {
            &Tok::Eof
        }
    }
    fn cur_loc(&self) -> (usize, usize) {
        let t = &self.toks[self.pos];
        (t.line, t.col)
    }
    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].kind.clone();
        if !matches!(t, Tok::Eof) {
            self.pos += 1;
        }
        t
    }
    fn expect(&mut self, want: &Tok) -> Result<(), J2Error> {
        if std::mem::discriminant(self.peek()) == std::mem::discriminant(want) {
            self.bump();
            Ok(())
        } else {
            let (line, col) = self.cur_loc();
            Err(J2Error::syntax(
                format!("expected {:?}, found {:?}", want, self.peek()),
                line,
                col,
            ))
        }
    }
    fn eat_newlines(&mut self) {
        while matches!(self.peek(), Tok::Newline) {
            self.bump();
        }
    }
    fn eat(&mut self, t: &Tok) -> bool {
        if std::mem::discriminant(self.peek()) == std::mem::discriminant(t) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn parse_program(&mut self) -> Result<Program, J2Error> {
        let mut items = Vec::new();
        let mut item_lines = Vec::new();
        self.eat_newlines();
        while !matches!(self.peek(), Tok::Eof) {
            let line = self.cur_loc().0;
            let stmt = self.parse_stmt()?;
            items.push(stmt);
            item_lines.push(line);
            self.eat_newlines();
        }
        Ok(Program { items, item_lines })
    }

    fn parse_stmt(&mut self) -> Result<Stmt, J2Error> {
        match self.peek() {
            Tok::Func => self.parse_func_def(),
            Tok::For => self.parse_for_loop(),
            Tok::Repeat => self.parse_repeat(),
            Tok::Do => self.parse_do_repeat(),
            Tok::Loop => self.parse_loop_inf(),
            Tok::If => self.parse_if(),
            Tok::Stop => self.parse_stop(),
            Tok::Skip => self.parse_skip(),
            Tok::Give => self.parse_give(),
            Tok::Try => self.parse_try(),
            Tok::Global => self.parse_global(),
            Tok::Assert => self.parse_assert(),
            Tok::Ident(_) => self.parse_bind_or_expr(),
            _ => {
                let expr = self.parse_expr()?;
                Ok(Stmt::Expr(expr))
            }
        }
    }

    fn parse_func_def(&mut self) -> Result<Stmt, J2Error> {
        self.expect(&Tok::Func)?;
        let name = self.expect_ident()?;
        // Optional generic type parameters
        let mut type_params: Vec<String> = Vec::new();
        if self.eat(&Tok::Lt) {
            loop {
                let tp = self.expect_ident()?;
                // optional `: name` bound, ignored for now
                if self.eat(&Tok::Colon) {
                    let _ = self.expect_ident()?;
                }
                type_params.push(tp);
                if !self.eat(&Tok::Comma) { break; }
            }
            self.expect_close_angle()?;
        }
        self.expect(&Tok::LParen)?;
        let mut params = Vec::new();
        if !matches!(self.peek(), Tok::RParen) {
            loop {
                let param = self.parse_param()?;
                params.push(param);
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
        }
        self.expect(&Tok::RParen)?;
        // Optional return-type annotation: `-> ty`.
        let ret_ty = if self.eat(&Tok::Arrow) {
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(&Tok::Eq)?;
        let body = if matches!(self.peek(), Tok::LBrace) {
            FuncBody::Block(self.parse_block()?)
        } else {
            FuncBody::Expr(self.parse_expr()?)
        };
        Ok(Stmt::Func { name, params, body, ret_ty, type_params })
    }

    /// Parse a J type annotation
    fn parse_type(&mut self) -> Result<Ty, J2Error> {
        match self.peek().clone() {
            Tok::Func => {
                self.bump();
                self.expect(&Tok::LParen)?;
                let mut params = Vec::new();
                if !matches!(self.peek(), Tok::RParen) {
                    loop {
                        params.push(self.parse_type()?);
                        if !self.eat(&Tok::Comma) { break; }
                    }
                }
                self.expect(&Tok::RParen)?;
                self.expect(&Tok::Arrow)?;
                let ret = self.parse_type()?;
                Ok(Ty::Func(params, Box::new(ret)))
            }
            Tok::Ident(name) => {
                self.bump();
                match name.as_str() {
                    "int" => Ok(Ty::Int),
                    "float" => Ok(Ty::Float),
                    "bool" => Ok(Ty::Bool),
                    "text" => Ok(Ty::Text),
                    "nil" => Ok(Ty::Nil),
                    "dyn" | "val" => Ok(Ty::Dyn),
                    "seq" => {
                        self.expect(&Tok::Lt)?;
                        let inner = self.parse_type()?;
                        self.expect_close_angle()?;
                        Ok(Ty::Seq(Box::new(inner)))
                    }
                    "map" => {
                        self.expect(&Tok::Lt)?;
                        let k = self.parse_type()?;
                        self.expect(&Tok::Comma)?;
                        let v = self.parse_type()?;
                        self.expect_close_angle()?;
                        Ok(Ty::Map(Box::new(k), Box::new(v)))
                    }
                    "pair" => {
                        self.expect(&Tok::Lt)?;
                        let a = self.parse_type()?;
                        self.expect(&Tok::Comma)?;
                        let b = self.parse_type()?;
                        self.expect_close_angle()?;
                        Ok(Ty::Pair(Box::new(a), Box::new(b)))
                    }
                    // other idents become type vars
                    _ => Ok(Ty::Var(name)),
                }
            }
            other => {
                let (l, c) = self.cur_loc();
                Err(J2Error::syntax(format!("expected type, found {:?}", other), l, c))
            }
        }
    }

    /// Consume closing `>`, banking leftover from `>>`
    fn expect_close_angle(&mut self) -> Result<(), J2Error> {
        if self.angle_pending > 0 {
            self.angle_pending -= 1;
            return Ok(());
        }
        match self.peek() {
            Tok::Gt => { self.bump(); Ok(()) }
            Tok::PipePipe => { self.bump(); self.angle_pending = 1; Ok(()) }
            _ => {
                let (l, c) = self.cur_loc();
                Err(J2Error::syntax(format!("expected `>` to close generic type, found {:?}", self.peek()), l, c))
            }
        }
    }

    /// Parse the body of a class literal
    fn parse_class_lit(&mut self) -> Result<Expr, J2Error> {
        let mut extends = Vec::new();
        if self.eat(&Tok::Extends) {
            loop {
                match self.peek().clone() {
                    Tok::Ident(n) => { self.bump(); extends.push(n); }
                    _ => {
                        let (l, c) = self.cur_loc();
                        return Err(J2Error::syntax("expected class name after `extends`", l, c));
                    }
                }
                if !self.eat(&Tok::Comma) { break; }
            }
        }
        self.expect(&Tok::LBrace)?;
        self.eat_newlines();
        let mut fields = Vec::new();
        let mut methods = Vec::new();
        let mut statics = Vec::new();
        while !matches!(self.peek(), Tok::RBrace | Tok::Eof) {
            // Static member: `::name ... `
            if matches!(self.peek(), Tok::ColonColon) {
                self.bump();
                let name = self.expect_ident()?;
                if matches!(self.peek(), Tok::LParen) {
                    // static method: `::name(params) = body`
                    self.bump();
                    let mut params = Vec::new();
                    if !matches!(self.peek(), Tok::RParen) {
                        loop {
                            params.push(self.parse_param()?);
                            if !self.eat(&Tok::Comma) { break; }
                        }
                    }
                    self.expect(&Tok::RParen)?;
                    self.expect(&Tok::Eq)?;
                    let body = if matches!(self.peek(), Tok::LBrace) {
                        FuncBody::Block(self.parse_block()?)
                    } else {
                        FuncBody::Expr(self.parse_expr()?)
                    };
                    // static method is lambda stored on class
                    statics.push(ClassStatic {
                        name,
                        value: Expr::Lambda { params, body: Box::new(body) },
                    });
                } else {
                    // static value: `::name = expr`
                    self.expect(&Tok::Eq)?;
                    let v = self.parse_expr()?;
                    statics.push(ClassStatic { name, value: v });
                }
                self.eat_newlines();
                continue;
            }
            // instance member; look-ahead picks field vs method
            let name = self.expect_ident()?;
            if matches!(self.peek(), Tok::LParen) {
                // method form, `=` expr or `:=` block
                self.bump();
                let mut params = Vec::new();
                if !matches!(self.peek(), Tok::RParen) {
                    loop {
                        params.push(self.parse_param()?);
                        if !self.eat(&Tok::Comma) { break; }
                    }
                }
                self.expect(&Tok::RParen)?;
                // optional return type annotation
                let ret_ty = if self.eat(&Tok::Arrow) { Some(self.parse_type()?) } else { None };
                let mutating = if self.eat(&Tok::ColonEq) { true }
                               else if self.eat(&Tok::Eq) { false }
                               else {
                                   let (l, c) = self.cur_loc();
                                   return Err(J2Error::syntax("expected `=` (pure) or `:=` (mutating) after method params", l, c));
                               };
                let body = if matches!(self.peek(), Tok::LBrace) {
                    FuncBody::Block(self.parse_block()?)
                } else {
                    FuncBody::Expr(self.parse_expr()?)
                };
                methods.push(ClassMethod { name, mutating, params, ret_ty, body });
            } else if self.eat(&Tok::Colon) {
                // typed field, optional default
                let ty = self.parse_type()?;
                let default = if self.eat(&Tok::Eq) { Some(self.parse_expr()?) } else { None };
                fields.push(ClassField { name, ty: Some(ty), default });
            } else if self.eat(&Tok::Eq) {
                // Field with default value (untyped).
                let default = self.parse_expr()?;
                fields.push(ClassField { name, ty: None, default: Some(default) });
            } else {
                // Field declared, no default. Defaults to null.
                fields.push(ClassField { name, ty: None, default: None });
            }
            self.eat_newlines();
        }
        self.expect(&Tok::RBrace)?;
        Ok(Expr::ClassLit { extends, fields, methods, statics })
    }

    fn parse_param(&mut self) -> Result<Param, J2Error> {
        // pattern-match form, param is a literal
        match self.peek().clone() {
            Tok::IntLit(_) | Tok::FloatLit(_) | Tok::TextLit(_)
            | Tok::True | Tok::False | Tok::Null => {
                let lit = self.parse_atom()?;
                Ok(Param { name: "_pat".to_string(), pattern: Some(lit), ty: None })
            }
            Tok::Ident(name) => {
                self.bump();
                // Optional type annotation: `name: ty`.
                let ty = if self.eat(&Tok::Colon) {
                    Some(self.parse_type()?)
                } else {
                    None
                };
                Ok(Param { name, pattern: None, ty })
            }
            Tok::Underscore => {
                self.bump();
                let ty = if self.eat(&Tok::Colon) {
                    Some(self.parse_type()?)
                } else {
                    None
                };
                Ok(Param { name: "_".to_string(), pattern: None, ty })
            }
            _ => {
                let (l, c) = self.cur_loc();
                Err(J2Error::syntax(format!("expected parameter, got {:?}", self.peek()), l, c))
            }
        }
    }

    fn parse_for_loop(&mut self) -> Result<Stmt, J2Error> {
        self.expect(&Tok::For)?;
        let mut bindings = vec![self.parse_loop_binding()?];
        while self.eat(&Tok::Comma) {
            bindings.push(self.parse_loop_binding()?);
        }
        self.expect(&Tok::In)?;
        let iter = self.parse_expr()?;
        let filter = if self.eat(&Tok::If) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        let until = if self.eat(&Tok::Until) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        let body = self.parse_block()?;
        Ok(Stmt::ForLoop { bindings, iter, filter, until, body })
    }

    fn parse_loop_binding(&mut self) -> Result<String, J2Error> {
        match self.peek().clone() {
            Tok::Ident(n) => {
                self.bump();
                Ok(n)
            }
            Tok::Underscore => {
                self.bump();
                Ok("_".to_string())
            }
            _ => {
                let (l, c) = self.cur_loc();
                Err(J2Error::syntax(format!("expected loop binding, got {:?}", self.peek()), l, c))
            }
        }
    }

    fn parse_repeat(&mut self) -> Result<Stmt, J2Error> {
        self.expect(&Tok::Repeat)?;
        let cond = self.parse_expr()?;
        let body = self.parse_block()?;
        Ok(Stmt::Repeat { cond, body })
    }

    fn parse_do_repeat(&mut self) -> Result<Stmt, J2Error> {
        self.expect(&Tok::Do)?;
        let body = self.parse_block()?;
        self.expect(&Tok::Repeat)?;
        let cond = self.parse_expr()?;
        Ok(Stmt::DoRepeat { body, cond })
    }

    fn parse_loop_inf(&mut self) -> Result<Stmt, J2Error> {
        self.expect(&Tok::Loop)?;
        let body = self.parse_block()?;
        Ok(Stmt::Loop { body })
    }

    fn parse_if(&mut self) -> Result<Stmt, J2Error> {
        self.expect(&Tok::If)?;
        let cond = self.parse_expr()?;
        let then_block = self.parse_block()?;
        self.eat_newlines();
        let else_block = if self.eat(&Tok::Else) {
            if matches!(self.peek(), Tok::If) {
                // else-if wraps nested If in synthetic block
                let l = self.cur_loc().0;
                let inner = self.parse_if()?;
                Some(Block { stmts: vec![inner], stmt_lines: vec![l] })
            } else {
                Some(self.parse_block()?)
            }
        } else {
            None
        };
        Ok(Stmt::If { cond, then_block, else_block })
    }

    fn parse_stop(&mut self) -> Result<Stmt, J2Error> {
        self.expect(&Tok::Stop)?;
        let cond = if self.eat(&Tok::If) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(Stmt::Stop { cond })
    }

    fn parse_skip(&mut self) -> Result<Stmt, J2Error> {
        self.expect(&Tok::Skip)?;
        let cond = if self.eat(&Tok::If) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(Stmt::Skip { cond })
    }

    fn parse_give(&mut self) -> Result<Stmt, J2Error> {
        self.expect(&Tok::Give)?;
        let value = if matches!(self.peek(), Tok::Newline | Tok::RBrace | Tok::Eof) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        Ok(Stmt::Give { value })
    }

    fn parse_try(&mut self) -> Result<Stmt, J2Error> {
        self.expect(&Tok::Try)?;
        let body = self.parse_block()?;
        self.eat_newlines();
        let mut typed_handlers = Vec::new();
        let mut default_handler = None;
        // parse else-KIND chain, then optional else
        while self.eat(&Tok::Else) {
            self.eat_newlines();
            if let Tok::Ident(name) = self.peek().clone() {
                // class-typed handler
                self.bump();
                let block = self.parse_block()?;
                typed_handlers.push(crate::ast::TryHandler { class_name: name, handler: block });
                self.eat_newlines();
            } else if matches!(self.peek(), Tok::LBrace) {
                default_handler = Some(self.parse_block()?);
                break;
            } else {
                let (l, c) = self.cur_loc();
                return Err(J2Error::syntax("expected error class name or `{` after `else`", l, c));
            }
        }
        Ok(Stmt::Try { body, typed_handlers, default_handler })
    }

    fn parse_assert(&mut self) -> Result<Stmt, J2Error> {
        self.expect(&Tok::Assert)?;
        let cond = self.parse_expr()?;
        let msg = if self.eat(&Tok::Comma) {
            Some(self.parse_expr()?)
        } else { None };
        Ok(Stmt::Assert { cond, msg })
    }

    fn parse_global(&mut self) -> Result<Stmt, J2Error> {
        self.expect(&Tok::Global)?;
        let name = self.expect_ident()?;
        let mutable = if self.eat(&Tok::ColonEq) {
            true
        } else if self.eat(&Tok::Eq) {
            false
        } else {
            let (l, c) = self.cur_loc();
            return Err(J2Error::syntax("expected = or := after global name", l, c));
        };
        let value = self.parse_expr()?;
        Ok(Stmt::Global { name, value, mutable })
    }

    fn parse_bind_or_expr(&mut self) -> Result<Stmt, J2Error> {
        // We need to look ahead to decide
        let name = if let Tok::Ident(n) = self.peek().clone() {
            n
        } else {
            unreachable!()
        };
        match self.peek_at(1) {
            Tok::Eq => {
                self.bump(); // name
                self.bump(); // =
                let value = self.parse_expr()?;
                Ok(Stmt::Bind { name, value, mutable: false })
            }
            Tok::ColonEq => {
                self.bump();
                self.bump();
                let value = self.parse_expr()?;
                Ok(Stmt::Bind { name, value, mutable: true })
            }
            Tok::PlusEq | Tok::MinusEq | Tok::StarEq | Tok::SlashEq | Tok::PercentEq => {
                self.bump();
                let op_tok = self.bump();
                let op = match op_tok {
                    Tok::PlusEq => BinOp::Add,
                    Tok::MinusEq => BinOp::Sub,
                    Tok::StarEq => BinOp::Mul,
                    Tok::SlashEq => BinOp::Div,
                    Tok::PercentEq => BinOp::Rem,
                    _ => unreachable!(),
                };
                let value = self.parse_expr()?;
                Ok(Stmt::CompoundAssign { name, op, value })
            }
            Tok::PlusPlus => {
                self.bump();
                self.bump();
                Ok(Stmt::IncDec { name, inc: true })
            }
            Tok::MinusMinus => {
                self.bump();
                self.bump();
                Ok(Stmt::IncDec { name, inc: false })
            }
            _ => {
                // parse expr first; `=` means indexed/member assign
                let lhs = self.parse_expr()?;
                if matches!(self.peek(), Tok::Eq) {
                    self.bump();
                    let value = self.parse_expr()?;
                    return match lhs {
                        Expr::Index { coll, idx } => Ok(Stmt::IndexedAssign {
                            target: *coll, idx: *idx, value,
                        }),
                        Expr::Member { obj, field } => Ok(Stmt::MemberAssign {
                            target: *obj, field, value,
                        }),
                        _ => {
                            let (l, c) = self.cur_loc();
                            Err(J2Error::syntax(
                                "left-hand side of `=` must be a name, `s[i]`, or `m.field`",
                                l, c,
                            ))
                        }
                    };
                }
                Ok(Stmt::Expr(lhs))
            }
        }
    }

    fn parse_block(&mut self) -> Result<Block, J2Error> {
        self.expect(&Tok::LBrace)?;
        self.eat_newlines();
        let mut stmts = Vec::new();
        let mut stmt_lines = Vec::new();
        while !matches!(self.peek(), Tok::RBrace | Tok::Eof) {
            let line = self.cur_loc().0;
            stmts.push(self.parse_stmt()?);
            stmt_lines.push(line);
            self.eat_newlines();
        }
        self.expect(&Tok::RBrace)?;
        Ok(Block { stmts, stmt_lines })
    }

    fn expect_ident(&mut self) -> Result<String, J2Error> {
        match self.peek().clone() {
            Tok::Ident(n) => {
                self.bump();
                Ok(n)
            }
            _ => {
                let (l, c) = self.cur_loc();
                Err(J2Error::syntax(format!("expected identifier, got {:?}", self.peek()), l, c))
            }
        }
    }

    // ---------- expressions, with precedence ladder ----------

    fn parse_expr(&mut self) -> Result<Expr, J2Error> {
        self.parse_pipe()
    }

    fn parse_pipe(&mut self) -> Result<Expr, J2Error> {
        let mut lhs = self.parse_filter()?;
        while self.eat(&Tok::PipePipe) {
            let rhs = self.parse_filter()?;
            lhs = Expr::Pipe { lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_filter(&mut self) -> Result<Expr, J2Error> {
        let mut lhs = self.parse_or()?;
        while self.eat(&Tok::Question) {
            let rhs = self.parse_or()?;
            // predicate using `_` becomes per-element cond-lambda
            let rhs = if expr_uses_underscore(&rhs) {
                Expr::CondLambda(Box::new(rhs))
            } else {
                rhs
            };
            lhs = Expr::Filter { lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_or(&mut self) -> Result<Expr, J2Error> {
        let mut lhs = self.parse_and()?;
        while self.eat(&Tok::Or) {
            let rhs = self.parse_and()?;
            lhs = Expr::Binary { op: BinOp::Or, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Expr, J2Error> {
        let mut lhs = self.parse_not()?;
        while self.eat(&Tok::And) {
            let rhs = self.parse_not()?;
            lhs = Expr::Binary { op: BinOp::And, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_not(&mut self) -> Result<Expr, J2Error> {
        if self.eat(&Tok::Not) {
            let operand = self.parse_not()?;
            Ok(Expr::Unary { op: UnaryOp::Not, operand: Box::new(operand) })
        } else {
            self.parse_cmp()
        }
    }

    fn parse_cmp(&mut self) -> Result<Expr, J2Error> {
        let mut lhs = self.parse_bor()?;
        loop {
            let op = match self.peek() {
                Tok::EqEq => BinOp::Eq,
                Tok::BangEq => BinOp::NotEq,
                Tok::Lt => BinOp::Lt,
                Tok::Gt => BinOp::Gt,
                Tok::LtEq => BinOp::LtEq,
                Tok::GtEq => BinOp::GtEq,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_bor()?;
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    // C-style bitwise precedence, OR lowest
    fn parse_bor(&mut self) -> Result<Expr, J2Error> {
        let mut lhs = self.parse_bxor()?;
        while self.eat(&Tok::Pipe) {
            let rhs = self.parse_bxor()?;
            lhs = Expr::Binary { op: BinOp::BOr, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_bxor(&mut self) -> Result<Expr, J2Error> {
        let mut lhs = self.parse_band()?;
        while self.eat(&Tok::Caret) {
            let rhs = self.parse_band()?;
            lhs = Expr::Binary { op: BinOp::BXor, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_band(&mut self) -> Result<Expr, J2Error> {
        let mut lhs = self.parse_shift()?;
        while self.eat(&Tok::Amp) {
            let rhs = self.parse_shift()?;
            lhs = Expr::Binary { op: BinOp::BAnd, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_shift(&mut self) -> Result<Expr, J2Error> {
        let mut lhs = self.parse_range()?;
        loop {
            let op = match self.peek() {
                Tok::Shl => BinOp::Shl,
                // `>>` is also the pipe token
                _ => break,
            };
            self.bump();
            let rhs = self.parse_range()?;
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_range(&mut self) -> Result<Expr, J2Error> {
        let lhs = self.parse_add()?;
        if self.eat(&Tok::DotDot) {
            // `n..` is open-ended unless an expr follows
            if expr_start(self.peek()) {
                let rhs = self.parse_add()?;
                Ok(Expr::Range { start: Box::new(lhs), end: Some(Box::new(rhs)) })
            } else {
                Ok(Expr::Range { start: Box::new(lhs), end: None })
            }
        } else {
            Ok(lhs)
        }
    }

    fn parse_add(&mut self) -> Result<Expr, J2Error> {
        let mut lhs = self.parse_mul()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_mul()?;
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_mul(&mut self) -> Result<Expr, J2Error> {
        let mut lhs = self.parse_pow()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinOp::Mul,
                Tok::Slash => BinOp::Div,
                Tok::Percent => BinOp::Rem,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_pow()?;
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_pow(&mut self) -> Result<Expr, J2Error> {
        let lhs = self.parse_unary()?;
        if self.eat(&Tok::StarStar) {
            // Right-associative.
            let rhs = self.parse_pow()?;
            Ok(Expr::Binary { op: BinOp::Pow, lhs: Box::new(lhs), rhs: Box::new(rhs) })
        } else {
            Ok(lhs)
        }
    }

    fn parse_unary(&mut self) -> Result<Expr, J2Error> {
        if self.eat(&Tok::Minus) {
            let operand = self.parse_unary()?;
            Ok(Expr::Unary { op: UnaryOp::Neg, operand: Box::new(operand) })
        } else if self.eat(&Tok::Plus) {
            let operand = self.parse_unary()?;
            Ok(Expr::Unary { op: UnaryOp::Pos, operand: Box::new(operand) })
        } else if self.eat(&Tok::Tilde) {
            let operand = self.parse_unary()?;
            Ok(Expr::Unary { op: UnaryOp::BNot, operand: Box::new(operand) })
        } else {
            self.parse_postfix()
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, J2Error> {
        let mut expr = self.parse_atom()?;
        loop {
            match self.peek() {
                Tok::LParen => {
                    self.bump();
                    // named args collected into one Map argument
                    let mut args: Vec<Expr> = Vec::new();
                    let mut named: Vec<(String, Expr)> = Vec::new();
                    let mut saw_named = false;
                    if !matches!(self.peek(), Tok::RParen) {
                        loop {
                            // Lookahead: `IDENT :` -> named arg
                            let is_named = matches!(self.peek(), Tok::Ident(_))
                                && matches!(self.peek_at(1), Tok::Colon);
                            if is_named {
                                saw_named = true;
                                let key = self.expect_ident()?;
                                self.expect(&Tok::Colon)?;
                                let val = self.parse_expr()?;
                                named.push((key, val));
                            } else {
                                args.push(self.parse_expr()?);
                            }
                            if !self.eat(&Tok::Comma) { break; }
                        }
                    }
                    self.expect(&Tok::RParen)?;
                    if saw_named {
                        if !args.is_empty() {
                            let (l, c) = self.cur_loc();
                            return Err(J2Error::syntax("cannot mix positional and named arguments", l, c));
                        }
                        args.push(Expr::MapLit(named));
                    }
                    expr = Expr::Call { callee: Box::new(expr), args };
                }
                Tok::LBracket => {
                    self.bump();
                    let idx = self.parse_expr()?;
                    self.expect(&Tok::RBracket)?;
                    expr = Expr::Index { coll: Box::new(expr), idx: Box::new(idx) };
                }
                Tok::Dot => {
                    // member access; require ident on right
                    self.bump();
                    let field = self.expect_ident()?;
                    expr = Expr::Member { obj: Box::new(expr), field };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn parse_atom(&mut self) -> Result<Expr, J2Error> {
        let tok = self.peek().clone();
        // class literal with optional extends
        if matches!(tok, Tok::Class) {
            self.bump();
            return self.parse_class_lit();
        }
        // lambda expression in expression position
        if matches!(tok, Tok::Func) {
            self.bump();
            self.expect(&Tok::LParen)?;
            let mut params = Vec::new();
            if !matches!(self.peek(), Tok::RParen) {
                loop {
                    let p = self.parse_param()?;
                    params.push(p);
                    if !self.eat(&Tok::Comma) { break; }
                }
            }
            self.expect(&Tok::RParen)?;
            self.expect(&Tok::Eq)?;
            let body = if matches!(self.peek(), Tok::LBrace) {
                FuncBody::Block(self.parse_block()?)
            } else {
                FuncBody::Expr(self.parse_expr()?)
            };
            return Ok(Expr::Lambda { params, body: Box::new(body) });
        }
        // native escape hatch; arbitrary code, needs J2_TRUSTED=1
        if matches!(tok, Tok::Native) {
            let trusted = std::env::var("J2_TRUSTED").map(|v| v == "1").unwrap_or(false);
            if !trusted {
                let (l, c) = self.cur_loc();
                return Err(J2Error::syntax(
                    "`backend { ... }` (raw-backend escape hatch) is disabled because it executes \
                     arbitrary native code. Re-run with `--allow-unsafe` (or set J2_TRUSTED=1) \
                     to enable it for trusted source.",
                    l, c,
                ));
            }
            self.bump();
            if let Tok::NativeBlockSource(s) = self.peek().clone() {
                self.bump();
                return Ok(Expr::NativeBlock(s));
            }
            let (l, c) = self.cur_loc();
            return Err(J2Error::syntax("expected `{` after `backend`", l, c));
        }
        match tok {
            Tok::IntLit(n) => {
                self.bump();
                Ok(Expr::IntLit(n))
            }
            Tok::FloatLit(f) => {
                self.bump();
                Ok(Expr::FloatLit(f))
            }
            Tok::TextLit(s) => {
                self.bump();
                Ok(Expr::TextLit(s))
            }
            Tok::True => {
                self.bump();
                Ok(Expr::Bool(true))
            }
            Tok::False => {
                self.bump();
                Ok(Expr::Bool(false))
            }
            Tok::Null => {
                self.bump();
                Ok(Expr::Null)
            }
            Tok::Pi => {
                self.bump();
                Ok(Expr::Const(BuiltinConst::Pi))
            }
            Tok::E => {
                self.bump();
                Ok(Expr::Const(BuiltinConst::E))
            }
            Tok::Tau => {
                self.bump();
                Ok(Expr::Const(BuiltinConst::Tau))
            }
            Tok::Inf => {
                self.bump();
                Ok(Expr::Const(BuiltinConst::Inf))
            }
            Tok::Nan => {
                self.bump();
                Ok(Expr::Const(BuiltinConst::Nan))
            }
            Tok::MaxVal => {
                self.bump();
                Ok(Expr::Const(BuiltinConst::MaxVal))
            }
            Tok::MinVal => {
                self.bump();
                Ok(Expr::Const(BuiltinConst::MinVal))
            }
            Tok::Ident(name) => {
                self.bump();
                // Static access: `ClassName::member`
                if self.eat(&Tok::ColonColon) {
                    let member = self.expect_ident()?;
                    return Ok(Expr::StaticAccess { class_name: name, member });
                }
                Ok(Expr::Ident(name))
            }
            Tok::Underscore => {
                self.bump();
                Ok(Expr::Underscore)
            }
            Tok::LParen => {
                self.bump();
                let first = self.parse_expr()?;
                if self.eat(&Tok::Comma) {
                    let second = self.parse_expr()?;
                    self.expect(&Tok::RParen)?;
                    Ok(Expr::Pair(Box::new(first), Box::new(second)))
                } else {
                    self.expect(&Tok::RParen)?;
                    Ok(first)
                }
            }
            Tok::LBracket => {
                self.bump();
                let mut items = Vec::new();
                if !matches!(self.peek(), Tok::RBracket) {
                    items.push(self.parse_expr()?);
                    while self.eat(&Tok::Comma) {
                        items.push(self.parse_expr()?);
                    }
                }
                self.expect(&Tok::RBracket)?;
                Ok(Expr::SeqLit(items))
            }
            Tok::LBrace => {
                // Disambiguate map literal vs block
                if self.is_map_literal() {
                    self.bump();
                    let mut entries = Vec::new();
                    if !matches!(self.peek(), Tok::RBrace) {
                        let key = self.expect_ident()?;
                        self.expect(&Tok::Colon)?;
                        let val = self.parse_expr()?;
                        entries.push((key, val));
                        while self.eat(&Tok::Comma) {
                            self.eat_newlines();
                            let k = self.expect_ident()?;
                            self.expect(&Tok::Colon)?;
                            let v = self.parse_expr()?;
                            entries.push((k, v));
                        }
                    }
                    self.expect(&Tok::RBrace)?;
                    Ok(Expr::MapLit(entries))
                } else {
                    let block = self.parse_block()?;
                    Ok(Expr::Block(block))
                }
            }
            _ => {
                let (l, c) = self.cur_loc();
                Err(J2Error::syntax(format!("unexpected token in expression: {:?}", self.peek()), l, c))
            }
        }
    }

    fn is_map_literal(&self) -> bool {
        // `{}` or `IDENT:` after brace means map
        if !matches!(self.peek(), Tok::LBrace) {
            return false;
        }
        let t1 = self.peek_at(1);
        if matches!(t1, Tok::RBrace) {
            return true;
        }
        if matches!(t1, Tok::Ident(_)) && matches!(self.peek_at(2), Tok::Colon) {
            return true;
        }
        false
    }
}

fn expr_start(t: &Tok) -> bool {
    matches!(
        t,
        Tok::IntLit(_)
            | Tok::FloatLit(_)
            | Tok::TextLit(_)
            | Tok::True
            | Tok::False
            | Tok::Null
            | Tok::Pi
            | Tok::E
            | Tok::Tau
            | Tok::Inf
            | Tok::Nan
            | Tok::MaxVal
            | Tok::MinVal
            | Tok::Ident(_)
            | Tok::Underscore
            | Tok::LParen
            | Tok::LBracket
            | Tok::LBrace
            | Tok::Minus
            | Tok::Plus
            | Tok::Not
    )
}
