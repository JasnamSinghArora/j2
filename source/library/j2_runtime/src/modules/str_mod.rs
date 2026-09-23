// J `str` module - extended string operations

use crate::value::J2Value;
use crate::error::{J2Err, J2Result};

fn arg_text(args: &[J2Value], i: usize, who: &str) -> J2Result<String> {
    match args.get(i) {
        Some(J2Value::Text(t)) => Ok((**t).clone()),
        Some(other) => Err(J2Err::type_err(format!("{}: arg {} expected text, got {}", who, i, other.type_name()))),
        None => Err(J2Err::type_err(format!("{}: missing arg {}", who, i))),
    }
}

pub fn replace(args: &[J2Value]) -> J2Result<J2Value> {
    let s = arg_text(args, 0, "str.replace")?;
    let find = arg_text(args, 1, "str.replace")?;
    let with_ = arg_text(args, 2, "str.replace")?;
    Ok(J2Value::text(s.replace(&find, &with_)))
}

pub fn replace_n(args: &[J2Value]) -> J2Result<J2Value> {
    let s = arg_text(args, 0, "str.replace_n")?;
    let find = arg_text(args, 1, "str.replace_n")?;
    let with_ = arg_text(args, 2, "str.replace_n")?;
    let n = args.get(3).and_then(|v| v.as_num_i64().ok()).unwrap_or(0).max(0) as usize;
    Ok(J2Value::text(s.replacen(&find, &with_, n)))
}

pub fn find(args: &[J2Value]) -> J2Result<J2Value> {
    let s = arg_text(args, 0, "str.find")?;
    let sub = arg_text(args, 1, "str.find")?;
    Ok(match s.find(&sub) { Some(n) => J2Value::Int(n as i64), None => J2Value::Null })
}

pub fn rfind(args: &[J2Value]) -> J2Result<J2Value> {
    let s = arg_text(args, 0, "str.rfind")?;
    let sub = arg_text(args, 1, "str.rfind")?;
    Ok(match s.rfind(&sub) { Some(n) => J2Value::Int(n as i64), None => J2Value::Null })
}

pub fn starts_with(args: &[J2Value]) -> J2Result<J2Value> {
    let s = arg_text(args, 0, "str.starts_with")?;
    let p = arg_text(args, 1, "str.starts_with")?;
    Ok(J2Value::Bool(s.starts_with(&p)))
}

pub fn ends_with(args: &[J2Value]) -> J2Result<J2Value> {
    let s = arg_text(args, 0, "str.ends_with")?;
    let p = arg_text(args, 1, "str.ends_with")?;
    Ok(J2Value::Bool(s.ends_with(&p)))
}

pub fn pad_left(args: &[J2Value]) -> J2Result<J2Value> {
    let s = arg_text(args, 0, "str.pad_left")?;
    let n = args.get(1).and_then(|v| v.as_num_i64().ok()).unwrap_or(0).max(0) as usize;
    let ch = args.get(2).and_then(|v| if let J2Value::Text(t) = v { t.chars().next() } else { None }).unwrap_or(' ');
    if s.chars().count() >= n { return Ok(J2Value::text(s)); }
    let pad = std::iter::repeat(ch).take(n - s.chars().count()).collect::<String>();
    Ok(J2Value::text(format!("{}{}", pad, s)))
}

pub fn pad_right(args: &[J2Value]) -> J2Result<J2Value> {
    let s = arg_text(args, 0, "str.pad_right")?;
    let n = args.get(1).and_then(|v| v.as_num_i64().ok()).unwrap_or(0).max(0) as usize;
    let ch = args.get(2).and_then(|v| if let J2Value::Text(t) = v { t.chars().next() } else { None }).unwrap_or(' ');
    if s.chars().count() >= n { return Ok(J2Value::text(s)); }
    let pad = std::iter::repeat(ch).take(n - s.chars().count()).collect::<String>();
    Ok(J2Value::text(format!("{}{}", s, pad)))
}

pub fn repeat_str(args: &[J2Value]) -> J2Result<J2Value> {
    let s = arg_text(args, 0, "str.repeat")?;
    let n = args.get(1).and_then(|v| v.as_num_i64().ok()).unwrap_or(0).max(0) as usize;
    Ok(J2Value::text(s.repeat(n)))
}

pub fn chars(args: &[J2Value]) -> J2Result<J2Value> {
    let s = arg_text(args, 0, "str.chars")?;
    Ok(J2Value::seq(s.chars().map(|c| J2Value::text(c.to_string())).collect()))
}

pub fn bytes(args: &[J2Value]) -> J2Result<J2Value> {
    let s = arg_text(args, 0, "str.bytes")?;
    Ok(J2Value::seq(s.bytes().map(|b| J2Value::Int(b as i64)).collect()))
}

pub fn from_bytes(args: &[J2Value]) -> J2Result<J2Value> {
    match args.get(0) {
        Some(J2Value::Seq(s)) => {
            let s = s.lock().unwrap_or_else(|p| p.into_inner());
            let mut out = Vec::with_capacity(s.items.len());
            for v in &s.items {
                let n = v.as_num_i64().map_err(|_| J2Err::type_err("str.from_bytes: seq must contain ints"))?;
                if !(0..=255).contains(&n) { return Err(J2Err::value("str.from_bytes: byte out of range")); }
                out.push(n as u8);
            }
            String::from_utf8(out).map(J2Value::text).map_err(|_| J2Err::value("str.from_bytes: invalid UTF-8"))
        }
        Some(v @ J2Value::SeqF64(_)) => from_bytes(&[v.to_boxed_seq()]),
        _ => Err(J2Err::type_err("str.from_bytes: arg 0 must be seq<int>")),
    }
}

pub fn parse_int(args: &[J2Value]) -> J2Result<J2Value> {
    let s = arg_text(args, 0, "str.parse_int")?;
    s.trim().parse::<i64>().map(J2Value::Int).map_err(|_| J2Err::conversion(format!("str.parse_int: cannot parse {:?}", s)))
}

pub fn parse_float(args: &[J2Value]) -> J2Result<J2Value> {
    let s = arg_text(args, 0, "str.parse_float")?;
    s.trim().parse::<f64>().map(J2Value::Float).map_err(|_| J2Err::conversion(format!("str.parse_float: cannot parse {:?}", s)))
}

pub fn is_empty(args: &[J2Value]) -> J2Result<J2Value> {
    let s = arg_text(args, 0, "str.is_empty")?;
    Ok(J2Value::Bool(s.is_empty()))
}

pub fn is_alpha(args: &[J2Value]) -> J2Result<J2Value> {
    let s = arg_text(args, 0, "str.is_alpha")?;
    Ok(J2Value::Bool(!s.is_empty() && s.chars().all(|c| c.is_alphabetic())))
}

pub fn is_digit(args: &[J2Value]) -> J2Result<J2Value> {
    let s = arg_text(args, 0, "str.is_digit")?;
    Ok(J2Value::Bool(!s.is_empty() && s.chars().all(|c| c.is_ascii_digit())))
}

pub fn is_alnum(args: &[J2Value]) -> J2Result<J2Value> {
    let s = arg_text(args, 0, "str.is_alnum")?;
    Ok(J2Value::Bool(!s.is_empty() && s.chars().all(|c| c.is_alphanumeric())))
}

pub fn install(env: &mut std::collections::HashMap<String, J2Value>) {
    let mut m = std::collections::HashMap::<String, J2Value>::new();
    macro_rules! ins { ($n:expr, $f:path) => {
        m.insert($n.to_string(), J2Value::Builtin(std::sync::Arc::new($f), $n));
    } }
    ins!("replace", replace);
    ins!("replace_n", replace_n);
    ins!("find", find);
    ins!("rfind", rfind);
    ins!("starts_with", starts_with);
    ins!("ends_with", ends_with);
    ins!("pad_left", pad_left);
    ins!("pad_right", pad_right);
    ins!("repeat", repeat_str);
    ins!("chars", chars);
    ins!("bytes", bytes);
    ins!("from_bytes", from_bytes);
    ins!("parse_int", parse_int);
    ins!("parse_float", parse_float);
    ins!("is_empty", is_empty);
    ins!("is_alpha", is_alpha);
    ins!("is_digit", is_digit);
    ins!("is_alnum", is_alnum);
    env.insert("text".into(), J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m))));
}
