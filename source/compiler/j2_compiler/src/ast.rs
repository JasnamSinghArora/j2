// J AST node definitions

#[derive(Debug, Clone)]
pub struct Program {
    pub items: Vec<Stmt>,
    /// item start lines (1-based) for error reports
    pub item_lines: Vec<usize>,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    /// constant `=` or mutable `:=` binding
    Bind { name: String, value: Expr, mutable: bool },
    /// `name op= expr` compound assignment
    CompoundAssign { name: String, op: BinOp, value: Expr },
    /// `s[i] = expr` on mutable seq/map
    IndexedAssign { target: Expr, idx: Expr, value: Expr },
    /// `m.key = expr` on mutable map
    MemberAssign { target: Expr, field: String, value: Expr },
    /// `name++` / `name--`
    IncDec { name: String, inc: bool },
    /// func definition; expr/block body, pattern or typed
    Func {
        name: String,
        params: Vec<Param>,
        body: FuncBody,
        /// return type; `None` when untyped
        ret_ty: Option<Ty>,
        /// generic params; monomorphized in native backend
        type_params: Vec<String>,
    },
    /// `global name = ...`
    Global { name: String, value: Expr, mutable: bool },
    /// `for i in range_expr {
    ForLoop {
        bindings: Vec<String>,
        iter: Expr,
        filter: Option<Expr>,
        until: Option<Expr>,
        body: Block,
    },
    /// `repeat cond { ... }`
    Repeat { cond: Expr, body: Block },
    /// `do { ... } repeat cond`
    DoRepeat { body: Block, cond: Expr },
    /// `loop { ... }`
    Loop { body: Block },
    /// `if cond {} else {}`, else optional
    If {
        cond: Expr,
        then_block: Block,
        else_block: Option<Block>,
    },
    /// `stop if cond` / unconditional `stop`
    Stop { cond: Option<Expr> },
    /// `skip if cond` / unconditional `skip`
    Skip { cond: Option<Expr> },
    /// `give expr` early return from block
    Give { value: Option<Expr> },
    /// `try` with typed handlers and default else
    Try {
        body: Block,
        typed_handlers: Vec<TryHandler>,
        default_handler: Option<Block>,
    },
    /// `assert cond [, msg]`
    Assert { cond: Expr, msg: Option<Expr> },
    /// Expression-as-statement (e.g. a `print(x)` call)
    Expr(Expr),
}

/// class-typed `else KIND {}` handler in try
#[derive(Debug, Clone)]
pub struct TryHandler {
    pub class_name: String,
    pub handler: Block,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    /// If the parameter is a pattern literal
    pub pattern: Option<Expr>,
    /// type annotation; all typed enables native lowering
    pub ty: Option<Ty>,
}

/// J type lattice; `Dyn` is top
#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    Int,            // i64
    Float,          // f64
    Bool,           // bool
    Text,           // String / &str
    Nil,            // ()
    Seq(Box<Ty>),   // Vec<ty>, borrowed as &[ty] / &mut [ty]
    Map(Box<Ty>, Box<Ty>),
    Pair(Box<Ty>, Box<Ty>),
    Func(Vec<Ty>, Box<Ty>),
    /// A generic type variable
    Var(String),
    /// A nominal user class type
    Class(String),
    Dyn,  // J2Value - dynamic fallback / boundary type
}

#[derive(Debug, Clone)]
pub enum FuncBody {
    Expr(Expr),
    Block(Block),
}

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    /// statement lines for error reports, 0 unknown
    pub stmt_lines: Vec<usize>,
}

#[derive(Debug, Clone)]
pub enum Expr {
    IntLit(i64),
    FloatLit(f64),
    TextLit(String),
    Bool(bool),
    Null,
    /// `_` implicit var in cond contexts
    Underscore,
    /// built-in constants like PI, E, INF
    Const(BuiltinConst),
    /// Reference to a binding.
    Ident(String),
    /// `expr op expr`
    Binary { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr> },
    /// `unary op` (-x, +x, not x)
    Unary { op: UnaryOp, operand: Box<Expr> },
    /// `f(arg1, arg2, ...)` - function call
    Call { callee: Box<Expr>, args: Vec<Expr> },
    /// `coll[idx]`
    Index { coll: Box<Expr>, idx: Box<Expr> },
    /// `obj.field` - map member access
    Member { obj: Box<Expr>, field: String },
    /// `start..end` seq or flow; `start..` open flow
    Range { start: Box<Expr>, end: Option<Box<Expr>> },
    /// `[a, b, c]` - seq literal
    SeqLit(Vec<Expr>),
    /// `{key: val}` map literal, identifier keys
    MapLit(Vec<(String, Expr)>),
    /// `(a, b)` - pair literal
    Pair(Box<Expr>, Box<Expr>),
    /// `expr >> f` pipe; call or map
    Pipe { lhs: Box<Expr>, rhs: Box<Expr> },
    /// `expr ? f` filter then apply
    Filter { lhs: Box<Expr>, rhs: Box<Expr> },
    /// anonymous block yielding last expression
    Block(Block),
    /// fmt/input are ordinary builtin calls
    // (No special variant needed
    /// inline `_` condition; parser wraps `_` RHS
    CondLambda(Box<Expr>),
    /// anonymous `func(...)` expr; captures by clone snapshot
    Lambda { params: Vec<Param>, body: Box<FuncBody> },
    /// `backend { <raw backend>
    NativeBlock(String),
    /// class literal; fields, methods, statics; no `self`
    ClassLit {
        extends: Vec<String>,
        fields: Vec<ClassField>,
        methods: Vec<ClassMethod>,
        statics: Vec<ClassStatic>,
    },
    /// `Obj::name` static lookup via class table
    StaticAccess { class_name: String, member: String },
}

#[derive(Debug, Clone)]
pub struct ClassField {
    pub name: String,
    /// type annotation; all typed makes native struct
    pub ty: Option<Ty>,
    /// default value, overridable by named arg
    pub default: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct ClassMethod {
    pub name: String,
    /// `true` -> method was defined with `:=`
    pub mutating: bool,
    pub params: Vec<Param>,
    /// return type; enables native lowering
    pub ret_ty: Option<Ty>,
    pub body: FuncBody,
}

#[derive(Debug, Clone)]
pub struct ClassStatic {
    pub name: String,
    /// static fn or value, stored as J2Value
    pub value: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    Eq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    And,
    Or,
    BAnd,   // & integer
    BOr,    // | integer
    BXor,   // ^ integer
    Shl,    // << integer
    Shr,  // `>>` shift, disambiguated from pipe at parse
}

#[derive(Debug, Clone, PartialEq)]
pub enum UnaryOp {
    Neg,
    Pos,
    Not,
    BNot,   // ~ integer bitwise-not
}

#[derive(Debug, Clone, PartialEq)]
pub enum BuiltinConst {
    Pi,
    E,
    Tau,
    Inf,
    Nan,
    MaxVal,
    MinVal,
}
