// Arc/Mutex so J2Value is Send + Sync

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use crate::error::{J2Err, J2Result};
use crate::flow::J2Flow;
use crate::seq::J2Seq;

#[derive(Clone)]
pub enum J2Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(Arc<String>),
    Seq(Arc<Mutex<J2Seq>>),
    /// Packed float seq, native-only; avoids per-element boxing
    SeqF64(Arc<Mutex<Vec<f64>>>),
    Flow(Arc<Mutex<J2Flow>>),
    Pair(Arc<(J2Value, J2Value)>),
    Map(Arc<Mutex<HashMap<String, J2Value>>>),
    Cond(Arc<dyn Fn(&J2Value) -> J2Result<bool> + Send + Sync>),
    Builtin(Arc<dyn Fn(&[J2Value]) -> J2Result<J2Value> + Send + Sync>, &'static str),
    Func(Arc<J2Func>),
    /// User class; calling it constructs an instance
    Class(Arc<J2Class>),
    /// Instance of a Class
    Instance(Arc<Mutex<J2Instance>>),
}

/// Definition of a user-defined class.
#[derive(Debug)]
pub struct J2Class {
    pub name: String,
    /// Names of base classes
    pub extends: Vec<String>,
    /// Field names in declaration order
    pub field_order: Vec<String>,
    /// Field default values (cloned per-instance on construction).
    pub field_defaults: HashMap<String, J2Value>,
    /// Instance methods; `mutating` flag tracked for optimizations
    pub methods: HashMap<String, Arc<J2ClassMethod>>,
    /// Static (`::`) members stored on the class
    pub statics: Mutex<HashMap<String, J2Value>>,
}

pub struct J2ClassMethod {
    pub name: String,
    pub mutating: bool,
    pub func: Arc<J2Func>,
}

impl std::fmt::Debug for J2ClassMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "J2ClassMethod({}, mut={})", self.name, self.mutating)
    }
}

/// Instance state, fields plus back-pointer to Class
#[derive(Debug)]
pub struct J2Instance {
    pub class: Arc<J2Class>,
    pub fields: HashMap<String, J2Value>,
}

pub struct J2Func {
    pub name: String,
    pub arity: usize,
    pub clauses: Vec<J2FuncClause>,
}

pub struct J2FuncClause {
    pub patterns: Vec<Option<J2Value>>,
    pub param_names: Vec<String>,
    pub body: Arc<dyn Fn(&[J2Value]) -> J2Result<J2Value> + Send + Sync>,
}

impl fmt::Debug for J2Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => write!(f, "null"),
            Self::Bool(b) => write!(f, "{}", b),
            Self::Int(n) => write!(f, "{}", n),
            Self::Float(x) => write!(f, "{}", x),
            Self::Text(s) => write!(f, "{:?}", s),
            Self::Seq(s) => write!(f, "{:?}", s.lock().unwrap_or_else(|p| p.into_inner())),
            Self::SeqF64(s) => write!(f, "{:?}", s.lock().unwrap_or_else(|p| p.into_inner())),
            Self::Flow(_) => write!(f, "<flow>"),
            Self::Pair(p) => write!(f, "({:?}, {:?})", p.0, p.1),
            Self::Map(m) => write!(f, "{:?}", m.lock().unwrap_or_else(|p| p.into_inner())),
            Self::Cond(_) => write!(f, "<cond>"),
            Self::Builtin(_, n) => write!(f, "<builtin {}>", n),
            Self::Func(g) => write!(f, "<func {}>", g.name),
            Self::Class(c) => write!(f, "<class {}>", c.name),
            Self::Instance(_) => write!(f, "<instance>"),
        }
    }
}

impl fmt::Display for J2Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => write!(f, "null"),
            Self::Bool(b) => write!(f, "{}", b),
            Self::Int(n) => write!(f, "{}", n),
            Self::Float(x) => fmt_jfloat(*x, f),
            Self::Text(s) => write!(f, "{}", s),
            Self::Seq(s) => {
                let s = s.lock().unwrap_or_else(|p| p.into_inner());
                write!(f, "[")?;
                for (i, v) in s.items.iter().enumerate() {
                    if i > 0 { write!(f, ",")?; }
                    write!(f, "{}", v)?;
                }
                write!(f, "]")
            }
            // same rendering as boxed Float seq, byte-for-byte
            Self::SeqF64(s) => {
                let s = s.lock().unwrap_or_else(|p| p.into_inner());
                write!(f, "[")?;
                for (i, x) in s.iter().enumerate() {
                    if i > 0 { write!(f, ",")?; }
                    fmt_jfloat(*x, f)?;
                }
                write!(f, "]")
            }
            Self::Flow(_) => write!(f, "<flow>"),
            Self::Pair(p) => write!(f, "({}, {})", p.0, p.1),
            Self::Map(m) => {
                let m = m.lock().unwrap_or_else(|p| p.into_inner());
                write!(f, "{{")?;
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}: {}", k, v)?;
                }
                write!(f, "}}")
            }
            Self::Cond(_) => write!(f, "<cond>"),
            Self::Builtin(_, n) => write!(f, "<builtin {}>", n),
            Self::Func(g) => write!(f, "<func {}>", g.name),
            Self::Class(c) => write!(f, "<class {}>", c.name),
            Self::Instance(inst) => {
                let inst = inst.lock().unwrap_or_else(|p| p.into_inner());
                write!(f, "{}{{", inst.class.name)?;
                for (i, (k, v)) in inst.fields.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}: {}", k, v)?;
                }
                write!(f, "}}")
            }
        }
    }
}

impl J2Value {
    pub fn text<S: Into<String>>(s: S) -> Self { Self::Text(Arc::new(s.into())) }
    pub fn seq(items: Vec<J2Value>) -> Self { Self::Seq(Arc::new(Mutex::new(J2Seq { items }))) }
    /// Packed float-sequence constructor (the native fast path).
    pub fn seq_f64(v: Vec<f64>) -> Self { Self::SeqF64(Arc::new(Mutex::new(v))) }
    /// Materialize any sequence form
    pub fn seq_elems(&self) -> Option<Vec<J2Value>> {
        match self {
            Self::Seq(s) => Some(s.lock().unwrap_or_else(|p| p.into_inner()).items.clone()),
            Self::SeqF64(s) => Some(
                s.lock().unwrap_or_else(|p| p.into_inner()).iter().map(|x| J2Value::Float(*x)).collect(),
            ),
            _ => None,
        }
    }

    /// Normalize any seq to a boxed `Seq`
    pub fn to_boxed_seq(&self) -> J2Value {
        match self.seq_elems() {
            Some(items) => J2Value::seq(items),
            None => self.clone(),
        }
    }
    pub fn map(entries: Vec<(String, J2Value)>) -> Self {
        let mut m = HashMap::new();
        for (k, v) in entries { m.insert(k, v); }
        Self::Map(Arc::new(Mutex::new(m)))
    }
    pub fn pair(a: J2Value, b: J2Value) -> Self { Self::Pair(Arc::new((a, b))) }
    pub fn flow(f: J2Flow) -> Self { Self::Flow(Arc::new(Mutex::new(f))) }

    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) | Self::Int(_) | Self::Float(_) | Self::Text(_) => "val",
            Self::Seq(_) | Self::SeqF64(_) => "seq",
            Self::Flow(_) => "flow",
            Self::Pair(_) => "pair",
            Self::Map(_) => "map",
            Self::Cond(_) => "cond",
            Self::Builtin(_, _) | Self::Func(_) => "func",
            Self::Class(_) => "class",
            Self::Instance(_) => "instance",
        }
    }

    pub fn truthy(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(b) => *b,
            Self::Int(n) => *n != 0,
            Self::Float(x) => *x != 0.0 && !x.is_nan(),
            Self::Text(s) => !s.is_empty(),
            Self::Seq(s) => !s.lock().unwrap_or_else(|p| p.into_inner()).items.is_empty(),
            Self::SeqF64(s) => !s.lock().unwrap_or_else(|p| p.into_inner()).is_empty(),
            Self::Map(m) => !m.lock().unwrap_or_else(|p| p.into_inner()).is_empty(),
            _ => true,
        }
    }

    pub fn as_num_f64(&self) -> J2Result<f64> {
        match self {
            Self::Int(n) => Ok(*n as f64),
            Self::Float(x) => Ok(*x),
            Self::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            _ => Err(J2Err::type_err(format!("expected number, got {}", self.type_name()))),
        }
    }
    pub fn as_num_i64(&self) -> J2Result<i64> {
        match self {
            Self::Int(n) => Ok(*n),
            Self::Float(x) => {
                if x.fract() == 0.0 && x.is_finite() { Ok(*x as i64) }
                else { Err(J2Err::type_err("expected integer, got non-integer float")) }
            }
            Self::Bool(b) => Ok(if *b { 1 } else { 0 }),
            _ => Err(J2Err::type_err(format!("expected integer, got {}", self.type_name()))),
        }
    }
    pub fn as_text(&self) -> J2Result<String> {
        match self {
            Self::Text(s) => Ok(s.as_ref().clone()),
            _ => Err(J2Err::type_err(format!("expected text, got {}", self.type_name()))),
        }
    }

    pub fn band(&self, other: &Self) -> J2Result<Self> {
        Ok(J2Value::Int(self.as_num_i64()? & other.as_num_i64()?))
    }
    pub fn bor(&self, other: &Self) -> J2Result<Self> {
        Ok(J2Value::Int(self.as_num_i64()? | other.as_num_i64()?))
    }
    pub fn bxor(&self, other: &Self) -> J2Result<Self> {
        Ok(J2Value::Int(self.as_num_i64()? ^ other.as_num_i64()?))
    }
    pub fn shl(&self, other: &Self) -> J2Result<Self> {
        let a = self.as_num_i64()? as u64;
        let b = other.as_num_i64()? as u32;
        Ok(J2Value::Int(a.wrapping_shl(b) as i64))
    }
    pub fn shr(&self, other: &Self) -> J2Result<Self> {
        let a = self.as_num_i64()? as u64;
        let b = other.as_num_i64()? as u32;
        Ok(J2Value::Int(a.wrapping_shr(b) as i64))
    }
    pub fn bnot(&self) -> J2Result<Self> {
        Ok(J2Value::Int(!self.as_num_i64()?))
    }

    pub fn add(&self, other: &Self) -> J2Result<Self> {
        // seq + seq concatenates; text too
        if let (Some(mut out), Some(b)) = (self.seq_elems(), other.seq_elems()) {
            out.extend(b);
            return Ok(J2Value::seq(out));
        }
        if let (J2Value::Text(a), J2Value::Text(b)) = (self, other) {
            let mut out = String::with_capacity(a.len() + b.len());
            out.push_str(a);
            out.push_str(b);
            return Ok(J2Value::Text(std::sync::Arc::new(out)));
        }
        numeric_binop(self, other, "+", |a,b| a+b, i64::checked_add)
    }
    pub fn sub(&self, other: &Self) -> J2Result<Self> { numeric_binop(self, other, "-", |a,b| a-b, i64::checked_sub) }
    pub fn mul(&self, other: &Self) -> J2Result<Self> { numeric_binop(self, other, "*", |a,b| a*b, i64::checked_mul) }
    pub fn div(&self, other: &Self) -> J2Result<Self> {
        let b = other.as_num_f64()?;
        if b == 0.0 { return Err(J2Err::zero_div("division by zero")); }
        let a = self.as_num_f64()?;
        let r = a / b;
        if let (Self::Int(x), Self::Int(y)) = (self, other) {
            if *y != 0 && x % y == 0 { return Ok(Self::Int(x / y)); }
        }
        Ok(Self::Float(r))
    }
    pub fn rem(&self, other: &Self) -> J2Result<Self> {
        let b = other.as_num_f64()?;
        if b == 0.0 { return Err(J2Err::zero_div("modulo by zero")); }
        if let (Self::Int(x), Self::Int(y)) = (self, other) {
            return Ok(Self::Int(x.rem_euclid(*y)));
        }
        Ok(Self::Float(self.as_num_f64()? % b))
    }
    pub fn pow(&self, other: &Self) -> J2Result<Self> {
        let b = other.as_num_f64()?;
        let a = self.as_num_f64()?;
        if a == 0.0 && b < 0.0 {
            return Err(J2Err::value("zero cannot be raised to a negative power"));
        }
        if let (Self::Int(x), Self::Int(y)) = (self, other) {
            if *y >= 0 {
                let mut r = 1i64;
                let mut e = *y as u32;
                let mut base = *x;
                while e > 0 {
                    if e & 1 == 1 { r = r.checked_mul(base).ok_or_else(|| J2Err::overflow("result exceeds MAX_VAL"))?; }
                    e >>= 1;
                    if e > 0 { base = base.checked_mul(base).ok_or_else(|| J2Err::overflow("result exceeds MAX_VAL"))?; }
                }
                return Ok(Self::Int(r));
            }
        }
        Ok(Self::Float(a.powf(b)))
    }
    pub fn neg(&self) -> J2Result<Self> {
        match self {
            Self::Int(n) => Ok(Self::Int(-n)),
            Self::Float(x) => Ok(Self::Float(-x)),
            _ => Err(J2Err::type_err(format!("unary minus on non-number ({})", self.type_name()))),
        }
    }

    pub fn eq(&self, other: &Self) -> bool {
        // seq comparison element-wise; packed equals boxed twin
        if let (Some(a), Some(b)) = (self.seq_elems(), other.seq_elems()) {
            return a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.eq(y));
        }
        match (self, other) {
            (Self::Null, Self::Null) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Int(a), Self::Int(b)) => a == b,
            (Self::Float(a), Self::Float(b)) => {
                if a.is_nan() || b.is_nan() { return false; }
                a == b
            }
            (Self::Int(a), Self::Float(b)) | (Self::Float(b), Self::Int(a)) => {
                if b.is_nan() { return false; }
                (*a as f64) == *b
            }
            (Self::Text(a), Self::Text(b)) => a == b,
            // (seq-vs-seq handled above via seq_elems)
            (Self::Pair(a), Self::Pair(b)) => a.0.eq(&b.0) && a.1.eq(&b.1),
            _ => false,
        }
    }

    pub fn cmp_lt(&self, other: &Self) -> J2Result<bool> {
        match (self, other) {
            (Self::Int(a), Self::Int(b)) => Ok(a < b),
            (Self::Float(a), Self::Float(b)) => Ok(a < b),
            (Self::Int(a), Self::Float(b)) => Ok((*a as f64) < *b),
            (Self::Float(a), Self::Int(b)) => Ok(*a < (*b as f64)),
            (Self::Text(a), Self::Text(b)) => Ok(a < b),
            _ => Err(J2Err::type_err(format!(
                "cannot compare {} with {}", self.type_name(), other.type_name(),
            ))),
        }
    }
}

/// Render a float exactly as J does
fn fmt_jfloat(x: f64, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    if x.is_nan() {
        write!(f, "NaN")
    } else if x.is_infinite() {
        write!(f, "{}", if x > 0.0 { "inf" } else { "-inf" })
    } else {
        write!(f, "{}", x)
    }
}

fn numeric_binop(
    a: &J2Value,
    b: &J2Value,
    op_name: &str,
    f_op: fn(f64, f64) -> f64,
    i_op: fn(i64, i64) -> Option<i64>,
) -> J2Result<J2Value> {
    match (a, b) {
        (J2Value::Int(x), J2Value::Int(y)) => {
            if let Some(r) = i_op(*x, *y) { Ok(J2Value::Int(r)) }
            else { Err(J2Err::overflow(format!("integer overflow in {} {} {}", x, op_name, y))) }
        }
        (J2Value::Float(_), _) | (_, J2Value::Float(_))
        | (J2Value::Int(_), _) | (_, J2Value::Int(_))
        | (J2Value::Bool(_), _) | (_, J2Value::Bool(_)) => {
            let x = a.as_num_f64()?;
            let y = b.as_num_f64()?;
            Ok(J2Value::Float(f_op(x, y)))
        }
        _ => Err(J2Err::type_err(format!(
            "cannot {} {} and {}", op_name, a.type_name(), b.type_name(),
        ))),
    }
}
