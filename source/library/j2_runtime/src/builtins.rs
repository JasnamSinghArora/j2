// spec built-ins, exact semantics including error cases

use std::io::{self, Write, BufRead};

use crate::error::{J2Err, J2Result};
use crate::flow::J2Flow;
use crate::value::J2Value;

/// Convert an `f64` to `i64`, erroring
pub fn f64_to_i64(x: f64, who: &str) -> J2Result<i64> {
    // -2^63 and 2^63 exactly representable as f64
    if x.is_finite() && x >= -9_223_372_036_854_775_808.0 && x < 9_223_372_036_854_775_808.0 {
        Ok(x as i64)
    } else {
        Err(J2Err::value(format!(
            "{}: {} is not a finite integer within i64 range",
            who, x
        )))
    }
}

// ------------------------- A -------------------------

/// abs(x): absolute value of a number.
pub fn abs(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "abs")?;
    match &args[0] {
        J2Value::Int(n) => Ok(J2Value::Int(n.wrapping_abs())),
        J2Value::Float(x) => Ok(J2Value::Float(x.abs())),
        _ => Err(J2Err::type_err("abs requires a number")),
    }
}

// ------------------------- C -------------------------

pub fn ceil(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "ceil")?;
    let x = args[0].as_num_f64()?;
    Ok(J2Value::Int(f64_to_i64(x.ceil(), "ceil")?))
}

/// collect(flow[, pred]), drain until pred true (inclusive)
pub fn collect(args: &[J2Value]) -> J2Result<J2Value> {
    if args.is_empty() || args.len() > 2 {
        return Err(J2Err::type_err("collect requires 1 or 2 arguments"));
    }
    let v = &args[0];
    match v {
        J2Value::Flow(f) => {
            let is_inf = f.lock().unwrap_or_else(|p| p.into_inner()).is_infinite();
            if args.len() == 1 {
                if is_inf {
                    return Err(J2Err::infinite_flow(
                        "collect requires an until condition on an infinite flow",
                    ));
                }
                let mut flow = std::mem::replace(&mut *f.lock().unwrap_or_else(|p| p.into_inner()), J2Flow::range(0, -1));
                let items = flow.collect_all()?;
                Ok(J2Value::seq(items))
            } else {
                let pred = match &args[1] {
                    J2Value::Cond(p) => p.clone(),
                    _ => return Err(J2Err::type_err("collect: second argument must be a cond")),
                };
                let mut flow = std::mem::replace(&mut *f.lock().unwrap_or_else(|p| p.into_inner()), J2Flow::range(0, -1));
                let items = flow.collect_until(|v| pred(v))?;
                Ok(J2Value::seq(items))
            }
        }
        J2Value::Seq(s) => Ok(J2Value::seq(s.lock().unwrap_or_else(|p| p.into_inner()).items.clone())),
        J2Value::SeqF64(_) => Ok(args[0].to_boxed_seq()),
        _ => Err(J2Err::type_err("collect requires a flow")),
    }
}

pub fn contains(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 2, "contains")?;
    match (&args[0], &args[1]) {
        (J2Value::Seq(s), needle) => {
            let s = s.lock().unwrap_or_else(|p| p.into_inner());
            Ok(J2Value::Bool(s.items.iter().any(|v| v.eq(needle))))
        }
        (J2Value::SeqF64(_), _) => contains(&[args[0].to_boxed_seq(), args[1].clone()]),
        (J2Value::Text(haystack), J2Value::Text(needle)) => {
            Ok(J2Value::Bool(haystack.contains(needle.as_str())))
        }
        _ => Err(J2Err::type_err("contains: unsupported argument types")),
    }
}

pub fn count(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "count")?;
    match &args[0] {
        J2Value::Seq(s) => Ok(J2Value::Int(s.lock().unwrap_or_else(|p| p.into_inner()).items.len() as i64)),
        J2Value::SeqF64(s) => Ok(J2Value::Int(s.lock().unwrap_or_else(|p| p.into_inner()).len() as i64)),
        J2Value::Text(s) => Ok(J2Value::Int(s.chars().count() as i64)),
        J2Value::Map(m) => Ok(J2Value::Int(m.lock().unwrap_or_else(|p| p.into_inner()).len() as i64)),
        J2Value::Flow(f) => {
            if f.lock().unwrap_or_else(|p| p.into_inner()).is_infinite() {
                return Err(J2Err::infinite_flow("count cannot be applied to an infinite flow"));
            }
            let mut flow = std::mem::replace(&mut *f.lock().unwrap_or_else(|p| p.into_inner()), J2Flow::range(0, -1));
            let mut n = 0i64;
            while flow.next()?.is_some() { n += 1; }
            Ok(J2Value::Int(n))
        }
        _ => Err(J2Err::type_err("count: unsupported argument type")),
    }
}

// ------------------------- F -------------------------

pub fn floor(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "floor")?;
    let x = args[0].as_num_f64()?;
    Ok(J2Value::Int(f64_to_i64(x.floor(), "floor")?))
}

pub fn fmt(args: &[J2Value]) -> J2Result<J2Value> {
    if args.is_empty() {
        return Err(J2Err::type_err("fmt requires at least the template"));
    }
    let tmpl = args[0].as_text()?;
    let rest = &args[1..];
    let placeholders = tmpl.matches("{}").count();
    if placeholders != rest.len() {
        return Err(J2Err::value(format!(
            "fmt: {} placeholders but {} arguments",
            placeholders,
            rest.len()
        )));
    }
    let mut out = String::with_capacity(tmpl.len());
    let mut i = 0;
    let mut chars = tmpl.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' && chars.peek() == Some(&'}') {
            chars.next();
            out.push_str(&format!("{}", rest[i]));
            i += 1;
        } else {
            out.push(c);
        }
    }
    Ok(J2Value::text(out))
}

// ------------------------- I -------------------------

pub fn input(args: &[J2Value]) -> J2Result<J2Value> {
    if !args.is_empty() {
        let prompt = args[0].as_text()?;
        print!("{}", prompt);
        io::stdout().flush().ok();
    }
    let mut s = String::new();
    io::stdin().lock().read_line(&mut s).map_err(|e| J2Err::runtime(e.to_string()))?;
    if s.ends_with('\n') { s.pop(); }
    if s.ends_with('\r') { s.pop(); }
    Ok(J2Value::text(s))
}

// ------------------------- J -------------------------

pub fn join(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 2, "join")?;
    let sep = args[1].as_text()?;
    match &args[0] {
        J2Value::Seq(s) => {
            let s = s.lock().unwrap_or_else(|p| p.into_inner());
            let mut out = String::new();
            for (i, v) in s.items.iter().enumerate() {
                if i > 0 { out.push_str(&sep); }
                out.push_str(&format!("{}", v));
            }
            Ok(J2Value::text(out))
        }
        J2Value::SeqF64(_) => join(&[args[0].to_boxed_seq(), args[1].clone()]),
        _ => Err(J2Err::type_err("join requires a seq")),
    }
}

// ------------------------- L -------------------------

pub fn len(args: &[J2Value]) -> J2Result<J2Value> { count(args) }

pub fn lower(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "lower")?;
    Ok(J2Value::text(args[0].as_text()?.to_lowercase()))
}

// ------------------------- M -------------------------

pub fn max(args: &[J2Value]) -> J2Result<J2Value> {
    // Binary form `max(a, b)`
    if args.len() == 2 {
        return Ok(binary_max_min(&args[0], &args[1], true)?);
    }
    arity(args, 1, "max")?;
    let items = collect_finite_items(&args[0], "max")?;
    if items.is_empty() { return Err(J2Err::value("max requires a non-empty seq")); }
    let mut best = items[0].clone();
    for v in &items[1..] {
        if v.cmp_lt(&best)? == false && !v.eq(&best) {
            best = v.clone();
        }
    }
    Ok(best)
}

pub fn min(args: &[J2Value]) -> J2Result<J2Value> {
    // Binary form `min(a, b)`
    if args.len() == 2 {
        return Ok(binary_max_min(&args[0], &args[1], false)?);
    }
    arity(args, 1, "min")?;
    let items = collect_finite_items(&args[0], "min")?;
    if items.is_empty() { return Err(J2Err::value("min requires a non-empty seq")); }
    let mut best = items[0].clone();
    for v in &items[1..] {
        if v.cmp_lt(&best)? { best = v.clone(); }
    }
    Ok(best)
}

/// Binary max/min, i64 for ints, else f64
fn binary_max_min(a: &J2Value, b: &J2Value, want_max: bool) -> J2Result<J2Value> {
    if let (J2Value::Int(x), J2Value::Int(y)) = (a, b) {
        return Ok(J2Value::Int(if want_max { *x.max(y) } else { *x.min(y) }));
    }
    let x = a.as_num_f64()?;
    let y = b.as_num_f64()?;
    Ok(J2Value::Float(if want_max { x.max(y) } else { x.min(y) }))
}

/// clamp via max-then-min; inverted range never panics
pub fn clamp(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 3, "clamp")?;
    if let (J2Value::Int(x), J2Value::Int(lo), J2Value::Int(hi)) = (&args[0], &args[1], &args[2]) {
        return Ok(J2Value::Int((*x).max(*lo).min(*hi)));
    }
    let x = args[0].as_num_f64()?;
    let lo = args[1].as_num_f64()?;
    let hi = args[2].as_num_f64()?;
    Ok(J2Value::Float(x.max(lo).min(hi)))
}

// ------------------------- N -------------------------

pub fn num(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "num")?;
    let s = args[0].as_text()?;
    if s.is_empty() { return Err(J2Err::conversion("empty text cannot be converted to a number")); }
    if let Ok(n) = s.parse::<i64>() { return Ok(J2Value::Int(n)); }
    if let Ok(x) = s.parse::<f64>() { return Ok(J2Value::Float(x)); }
    Err(J2Err::conversion(format!("{:?} cannot be converted to a number", s)))
}

// ------------------------- P -------------------------

pub fn pow(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 2, "pow")?;
    args[0].pow(&args[1])
}

pub fn print(args: &[J2Value]) -> J2Result<J2Value> {
    let mut parts = Vec::with_capacity(args.len());
    for v in args {
        parts.push(format!("{}", v));
    }
    println!("{}", parts.join(" "));
    Ok(J2Value::Null)
}

// ------------------------- R -------------------------

/// In-place append, avoids O(N) `s += [x]`
pub fn push(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 2, "push")?;
    match &args[0] {
        J2Value::Seq(s) => {
            s.lock().unwrap_or_else(|p| p.into_inner()).items.push(args[1].clone());
            Ok(J2Value::Null)
        }
        // packed seq, append as f64, keep representation
        J2Value::SeqF64(s) => {
            s.lock().unwrap_or_else(|p| p.into_inner()).push(args[1].as_num_f64()?);
            Ok(J2Value::Null)
        }
        _ => Err(J2Err::type_err("push requires a mutable seq")),
    }
}

/// `make_seq(n, init)`, n copies of init, preallocated
pub fn make_seq(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 2, "make_seq")?;
    let n = args[0].as_num_i64()?;
    if n < 0 { return Err(J2Err::value("make_seq: length must be non-negative")); }
    // Float init -> packed float seq
    if let J2Value::Float(x) = args[1] {
        return Ok(J2Value::seq_f64(vec![x; n as usize]));
    }
    let init = args[1].clone();
    let mut v: Vec<J2Value> = Vec::with_capacity(n as usize);
    for _ in 0..n { v.push(init.clone()); }
    Ok(J2Value::seq(v))
}

pub fn reverse(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "reverse")?;
    match &args[0] {
        J2Value::Seq(s) => {
            let mut items = s.lock().unwrap_or_else(|p| p.into_inner()).items.clone();
            items.reverse();
            Ok(J2Value::seq(items))
        }
        J2Value::Text(t) => Ok(J2Value::text(t.chars().rev().collect::<String>())),
        J2Value::Flow(f) => {
            if f.lock().unwrap_or_else(|p| p.into_inner()).is_infinite() {
                return Err(J2Err::infinite_flow("reverse: infinite flow"));
            }
            let mut flow = std::mem::replace(&mut *f.lock().unwrap_or_else(|p| p.into_inner()), J2Flow::range(0, -1));
            let mut items = flow.collect_all()?;
            items.reverse();
            Ok(J2Value::seq(items))
        }
        J2Value::SeqF64(_) => reverse(&[args[0].to_boxed_seq()]),
        _ => Err(J2Err::type_err("reverse: unsupported argument")),
    }
}

pub fn round(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "round")?;
    let x = args[0].as_num_f64()?;
    // banker's rounding (half to even) per spec
    let r = if (x - x.floor()).abs() == 0.5 {
        let lo = f64_to_i64(x.floor(), "round")?;
        if lo % 2 == 0 { lo } else { lo + 1 }
    } else {
        f64_to_i64(x.round(), "round")?
    };
    Ok(J2Value::Int(r))
}

// ------------------------- S -------------------------

pub fn slice(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 3, "slice")?;
    // Clamp negative indices to 0
    let i = args[1].as_num_i64()?.max(0) as usize;
    let j = args[2].as_num_i64()?.max(0) as usize;
    if i > j { return Err(J2Err::value("slice: start index greater than end index")); }
    match &args[0] {
        J2Value::Seq(s) => {
            let s = s.lock().unwrap_or_else(|p| p.into_inner());
            let n = s.items.len();
            let jj = j.min(n);
            let ii = i.min(jj);
            Ok(J2Value::seq(s.items[ii..jj].to_vec()))
        }
        J2Value::Text(t) => {
            let chars: Vec<char> = t.chars().collect();
            let n = chars.len();
            let jj = j.min(n);
            let ii = i.min(jj);
            Ok(J2Value::text(chars[ii..jj].iter().collect::<String>()))
        }
        J2Value::Flow(_) => {
            // For a flow, materialize.
            let items = collect_finite_items(&args[0], "slice")?;
            let n = items.len();
            let jj = j.min(n);
            let ii = i.min(jj);
            Ok(J2Value::seq(items[ii..jj].to_vec()))
        }
        J2Value::SeqF64(_) => slice(&[args[0].to_boxed_seq(), args[1].clone(), args[2].clone()]),
        _ => Err(J2Err::type_err("slice: unsupported argument")),
    }
}

pub fn sort(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "sort")?;
    match &args[0] {
        J2Value::Seq(s) => {
            let mut items = s.lock().unwrap_or_else(|p| p.into_inner()).items.clone();
            items.sort_by(|a, b| {
                match (a, b) {
                    (J2Value::Int(x), J2Value::Int(y)) => x.cmp(y),
                    (J2Value::Float(x), J2Value::Float(y)) => x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal),
                    (J2Value::Int(x), J2Value::Float(y)) => (*x as f64).partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal),
                    (J2Value::Float(x), J2Value::Int(y)) => x.partial_cmp(&(*y as f64)).unwrap_or(std::cmp::Ordering::Equal),
                    (J2Value::Text(x), J2Value::Text(y)) => x.cmp(y),
                    _ => std::cmp::Ordering::Equal,
                }
            });
            Ok(J2Value::seq(items))
        }
        J2Value::SeqF64(_) => sort(&[args[0].to_boxed_seq()]),
        _ => Err(J2Err::type_err("sort requires a seq")),
    }
}

pub fn split(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 2, "split")?;
    let t = args[0].as_text()?;
    let sep = args[1].as_text()?;
    let parts: Vec<J2Value> = if sep.is_empty() {
        t.chars().map(|c| J2Value::text(c.to_string())).collect()
    } else {
        t.split(sep.as_str()).map(|s| J2Value::text(s.to_string())).collect()
    };
    Ok(J2Value::seq(parts))
}

pub fn sqrt(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "sqrt")?;
    let x = args[0].as_num_f64()?;
    if x < 0.0 { return Err(J2Err::value("cannot take square root of negative number")); }
    Ok(J2Value::Float(x.sqrt()))
}

pub fn str_fn(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "str")?;
    Ok(J2Value::text(format!("{}", args[0])))
}

pub fn sum(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "sum")?;
    let items = collect_finite_items(&args[0], "sum")?;
    if items.is_empty() { return Err(J2Err::value("sum requires a non-empty seq")); }
    // parallel if numeric and len >= threshold
    crate::parallel::sum_seq(&items)
}

// ------------------------- T -------------------------

pub fn trim(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "trim")?;
    Ok(J2Value::text(args[0].as_text()?.trim().to_string()))
}

// ------------------------- U -------------------------

pub fn upper(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "upper")?;
    Ok(J2Value::text(args[0].as_text()?.to_uppercase()))
}

// ------------------------- helpers -------------------------

fn arity(args: &[J2Value], expected: usize, name: &str) -> J2Result<()> {
    if args.len() != expected {
        return Err(J2Err::type_err(format!(
            "{}: expected {} argument(s), got {}",
            name, expected, args.len()
        )));
    }
    Ok(())
}

/// Materialize a finite seq/flow into Vec<J2Value>
pub(crate) fn collect_finite_items(v: &J2Value, who: &str) -> J2Result<Vec<J2Value>> {
    match v {
        J2Value::Seq(s) => Ok(s.lock().unwrap_or_else(|p| p.into_inner()).items.clone()),
        J2Value::SeqF64(s) => Ok(s.lock().unwrap_or_else(|p| p.into_inner()).iter().map(|x| J2Value::Float(*x)).collect()),
        J2Value::Flow(f) => {
            if f.lock().unwrap_or_else(|p| p.into_inner()).is_infinite() {
                return Err(J2Err::infinite_flow(format!("{} cannot be applied to an infinite flow", who)));
            }
            let mut flow = std::mem::replace(&mut *f.lock().unwrap_or_else(|p| p.into_inner()), J2Flow::range(0, -1));
            flow.collect_all()
        }
        _ => Err(J2Err::type_err(format!("{}: expected a seq or flow, got {}", who, v.type_name()))),
    }
}

// ------------------------- extended (post-spec) -------------------------

/// type(x), runtime type name as text
pub fn type_of(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "type")?;
    Ok(J2Value::text(args[0].type_name().to_string()))
}

fn call_value(callee: &J2Value, args: Vec<J2Value>) -> J2Result<J2Value> {
    match callee {
        J2Value::Builtin(f, _) => f(&args),
        J2Value::Func(jf) => {
            for c in &jf.clauses {
                // Check literal patterns.
                let mut ok = true;
                for (i, pat) in c.patterns.iter().enumerate() {
                    if let Some(p) = pat {
                        if i >= args.len() || !args[i].eq(p) { ok = false; break; }
                    }
                }
                if !ok { continue; }
                if args.len() != jf.arity { continue; }
                return (c.body)(&args);
            }
            Err(J2Err::runtime("no matching function clause"))
        }
        _ => Err(J2Err::type_err("call_value: not callable")),
    }
}

/// reduce(seq, init, fn), left fold fn(acc, x)
pub fn reduce(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 3, "reduce")?;
    let items = collect_finite_items(&args[0], "reduce")?;
    let mut acc = args[1].clone();
    for v in items {
        acc = call_value(&args[2], vec![acc, v])?;
    }
    Ok(acc)
}

/// map(seq, fn), new seq of fn(x)
pub fn map_fn(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 2, "map")?;
    let items = collect_finite_items(&args[0], "map")?;
    let mut out = Vec::with_capacity(items.len());
    for v in items {
        out.push(call_value(&args[1], vec![v])?);
    }
    Ok(J2Value::seq(out))
}

/// filter(seq, pred), keep elements where pred truthy
pub fn filter_fn(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 2, "filter")?;
    let items = collect_finite_items(&args[0], "filter")?;
    let mut out = Vec::new();
    for v in items {
        if call_value(&args[1], vec![v.clone()])?.truthy() { out.push(v); }
    }
    Ok(J2Value::seq(out))
}

/// take(seq, n) - first n elements
pub fn take(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 2, "take")?;
    let n = args[1].as_num_i64()?.max(0) as usize;
    let items = collect_finite_items(&args[0], "take")?;
    Ok(J2Value::seq(items.into_iter().take(n).collect()))
}

/// skip(seq, n) - drop first n elements
pub fn skip(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 2, "skip")?;
    let n = args[1].as_num_i64()?.max(0) as usize;
    let items = collect_finite_items(&args[0], "skip")?;
    Ok(J2Value::seq(items.into_iter().skip(n).collect()))
}

/// zip(a, b) - seq of pairs
pub fn zip(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 2, "zip")?;
    let a = collect_finite_items(&args[0], "zip")?;
    let b = collect_finite_items(&args[1], "zip")?;
    let mut out = Vec::with_capacity(a.len().min(b.len()));
    for (x, y) in a.into_iter().zip(b.into_iter()) {
        out.push(J2Value::Pair(std::sync::Arc::new((x, y))));
    }
    Ok(J2Value::seq(out))
}

/// enumerate(seq) - seq of (index, value) pairs
pub fn enumerate(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "enumerate")?;
    let items = collect_finite_items(&args[0], "enumerate")?;
    let mut out = Vec::with_capacity(items.len());
    for (i, v) in items.into_iter().enumerate() {
        out.push(J2Value::Pair(std::sync::Arc::new((J2Value::Int(i as i64), v))));
    }
    Ok(J2Value::seq(out))
}

/// unique(seq), distinct elements, first occurrence order
pub fn unique(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "unique")?;
    let items = collect_finite_items(&args[0], "unique")?;
    let mut seen: Vec<J2Value> = Vec::new();
    for v in items {
        if !seen.iter().any(|s| s.eq(&v)) { seen.push(v); }
    }
    Ok(J2Value::seq(seen))
}

/// flatten(seq_of_seq) - concatenate sub-seqs into one
pub fn flatten(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "flatten")?;
    let items = collect_finite_items(&args[0], "flatten")?;
    let mut out = Vec::new();
    for v in items {
        match v {
            J2Value::Seq(s) => out.extend(s.lock().unwrap_or_else(|p| p.into_inner()).items.iter().cloned()),
            J2Value::SeqF64(s) => out.extend(s.lock().unwrap_or_else(|p| p.into_inner()).iter().map(|x| J2Value::Float(*x))),
            other => out.push(other),
        }
    }
    Ok(J2Value::seq(out))
}

/// sort_by(seq, cmp), cmp(a, b) returns int
pub fn sort_by(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 2, "sort_by")?;
    let mut items = collect_finite_items(&args[0], "sort_by")?;
    let cmp = args[1].clone();
    // bubble sort, error bail, keeps closures simple
    let n = items.len();
    let mut err: Option<J2Err> = None;
    for i in 0..n {
        for j in 0..n.saturating_sub(i + 1) {
            match call_value(&cmp, vec![items[j].clone(), items[j+1].clone()]) {
                Ok(r) => {
                    let c = r.as_num_i64().unwrap_or(0);
                    if c > 0 { items.swap(j, j+1); }
                }
                Err(e) => { err = Some(e); break; }
            }
        }
        if err.is_some() { break; }
    }
    if let Some(e) = err { return Err(e); }
    Ok(J2Value::seq(items))
}

/// assert_eq(a, b), error unless equal, returns null
pub fn assert_eq(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 2, "assert_eq")?;
    if !args[0].eq(&args[1]) {
        return Err(J2Err::runtime(format!("assert_eq failed: {} != {}", args[0], args[1])));
    }
    Ok(J2Value::Null)
}

/// Global namespace seeded with every built-in
pub fn install(env: &mut std::collections::HashMap<String, J2Value>) {
    macro_rules! ins {
        ($n:ident, $f:path) => {
            env.insert(stringify!($n).to_string(), J2Value::Builtin(std::sync::Arc::new($f), stringify!($n)));
        };
    }
    ins!(abs, abs);
    ins!(ceil, ceil);
    ins!(clamp, clamp);
    ins!(collect, collect);
    ins!(contains, contains);
    ins!(count, count);
    ins!(floor, floor);
    ins!(fmt, fmt);
    ins!(input, input);
    ins!(join, join);
    ins!(len, len);
    ins!(lower, lower);
    ins!(max, max);
    ins!(min, min);
    ins!(num, num);
    ins!(pow, pow);
    ins!(print, print);
    ins!(reverse, reverse);
    ins!(push, push);
    ins!(make_seq, make_seq);
    ins!(round, round);
    ins!(slice, slice);
    ins!(sort, sort);
    ins!(split, split);
    ins!(sqrt, sqrt);
    env.insert("str".into(), J2Value::Builtin(std::sync::Arc::new(str_fn), "str"));
    ins!(sum, sum);
    ins!(trim, trim);
    ins!(upper, upper);

    // Extended (post-spec) built-ins for general-purpose use.
    env.insert("type".into(), J2Value::Builtin(std::sync::Arc::new(type_of), "type"));
    env.insert("reduce".into(), J2Value::Builtin(std::sync::Arc::new(reduce), "reduce"));
    env.insert("map".into(), J2Value::Builtin(std::sync::Arc::new(map_fn), "map"));
    env.insert("filter".into(), J2Value::Builtin(std::sync::Arc::new(filter_fn), "filter"));
    env.insert("take".into(), J2Value::Builtin(std::sync::Arc::new(take), "take"));
    env.insert("skip".into(), J2Value::Builtin(std::sync::Arc::new(skip), "skip"));
    env.insert("zip".into(), J2Value::Builtin(std::sync::Arc::new(zip), "zip"));
    env.insert("enumerate".into(), J2Value::Builtin(std::sync::Arc::new(enumerate), "enumerate"));
    env.insert("unique".into(), J2Value::Builtin(std::sync::Arc::new(unique), "unique"));
    env.insert("flatten".into(), J2Value::Builtin(std::sync::Arc::new(flatten), "flatten"));
    env.insert("sort_by".into(), J2Value::Builtin(std::sync::Arc::new(sort_by), "sort_by"));
    env.insert("assert_eq".into(), J2Value::Builtin(std::sync::Arc::new(assert_eq), "assert_eq"));

    // Bare transcendental / log math functions
    macro_rules! insm {
        ($n:expr, $f:path) => {
            env.insert($n.to_string(), J2Value::Builtin(std::sync::Arc::new($f), $n));
        };
    }
    insm!("sin", crate::modules::math::sin);
    insm!("cos", crate::modules::math::cos);
    insm!("tan", crate::modules::math::tan);
    insm!("exp", crate::modules::math::exp);
    insm!("ln", crate::modules::math::ln);
    insm!("log", crate::modules::math::log);
    insm!("log2", crate::modules::math::log2);
    insm!("log10", crate::modules::math::log10);
    insm!("cbrt", crate::modules::math::cbrt);
    insm!("asin", crate::modules::math::asin);
    insm!("acos", crate::modules::math::acos);
    insm!("atan", crate::modules::math::atan);
    insm!("sinh", crate::modules::math::sinh);
    insm!("cosh", crate::modules::math::cosh);
    insm!("tanh", crate::modules::math::tanh);
}
