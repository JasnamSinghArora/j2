// regex module; compiled patterns cached thread-locally

use crate::value::J2Value;
use crate::error::{J2Err, J2Result};
use regex::Regex;
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    static CACHE: RefCell<HashMap<String, Regex>> = RefCell::new(HashMap::new());
}

fn get_re(pattern: &str) -> J2Result<Regex> {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some(r) = c.get(pattern) { return Ok(r.clone()); }
        let r = Regex::new(pattern).map_err(|e| J2Err::value(format!("regex: invalid pattern: {}", e)))?;
        c.insert(pattern.to_string(), r.clone());
        Ok(r)
    })
}

fn arg_text(args: &[J2Value], i: usize, who: &str) -> J2Result<String> {
    match args.get(i) {
        Some(J2Value::Text(t)) => Ok((**t).clone()),
        Some(other) => Err(J2Err::type_err(format!("{}: arg {} expected text, got {}", who, i, other.type_name()))),
        None => Err(J2Err::type_err(format!("{}: missing arg {}", who, i))),
    }
}

pub fn matches_fn(args: &[J2Value]) -> J2Result<J2Value> {
    let pat = arg_text(args, 0, "regex.match")?;
    let s = arg_text(args, 1, "regex.match")?;
    let re = get_re(&pat)?;
    Ok(J2Value::Bool(re.is_match(&s)))
}

pub fn find_fn(args: &[J2Value]) -> J2Result<J2Value> {
    let pat = arg_text(args, 0, "regex.find")?;
    let s = arg_text(args, 1, "regex.find")?;
    let re = get_re(&pat)?;
    match re.find(&s) {
        Some(m) => {
            let mut map = HashMap::<String, J2Value>::new();
            map.insert("start".into(), J2Value::Int(m.start() as i64));
            map.insert("end".into(), J2Value::Int(m.end() as i64));
            map.insert("text".into(), J2Value::text(m.as_str().to_string()));
            Ok(J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(map))))
        }
        None => Ok(J2Value::Null),
    }
}

pub fn find_all(args: &[J2Value]) -> J2Result<J2Value> {
    let pat = arg_text(args, 0, "regex.find_all")?;
    let s = arg_text(args, 1, "regex.find_all")?;
    let re = get_re(&pat)?;
    let mut out = Vec::new();
    for m in re.find_iter(&s) {
        let mut map = HashMap::<String, J2Value>::new();
        map.insert("start".into(), J2Value::Int(m.start() as i64));
        map.insert("end".into(), J2Value::Int(m.end() as i64));
        map.insert("text".into(), J2Value::text(m.as_str().to_string()));
        out.push(J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(map))));
    }
    Ok(J2Value::seq(out))
}

pub fn replace(args: &[J2Value]) -> J2Result<J2Value> {
    let pat = arg_text(args, 0, "regex.replace")?;
    let s = arg_text(args, 1, "regex.replace")?;
    let with_ = arg_text(args, 2, "regex.replace")?;
    let re = get_re(&pat)?;
    Ok(J2Value::text(re.replace(&s, with_.as_str()).into_owned()))
}

pub fn replace_all(args: &[J2Value]) -> J2Result<J2Value> {
    let pat = arg_text(args, 0, "regex.replace_all")?;
    let s = arg_text(args, 1, "regex.replace_all")?;
    let with_ = arg_text(args, 2, "regex.replace_all")?;
    let re = get_re(&pat)?;
    Ok(J2Value::text(re.replace_all(&s, with_.as_str()).into_owned()))
}

pub fn split(args: &[J2Value]) -> J2Result<J2Value> {
    let pat = arg_text(args, 0, "regex.split")?;
    let s = arg_text(args, 1, "regex.split")?;
    let re = get_re(&pat)?;
    Ok(J2Value::seq(re.split(&s).map(|x| J2Value::text(x.to_string())).collect()))
}

pub fn groups(args: &[J2Value]) -> J2Result<J2Value> {
    let pat = arg_text(args, 0, "regex.groups")?;
    let s = arg_text(args, 1, "regex.groups")?;
    let re = get_re(&pat)?;
    match re.captures(&s) {
        Some(caps) => {
            let mut out = Vec::new();
            for i in 0..caps.len() {
                out.push(match caps.get(i) {
                    Some(m) => J2Value::text(m.as_str().to_string()),
                    None => J2Value::Null,
                });
            }
            Ok(J2Value::seq(out))
        }
        None => Ok(J2Value::Null),
    }
}

pub fn install(env: &mut std::collections::HashMap<String, J2Value>) {
    let mut m = std::collections::HashMap::<String, J2Value>::new();
    macro_rules! ins { ($n:expr, $f:path) => {
        m.insert($n.to_string(), J2Value::Builtin(std::sync::Arc::new($f), $n));
    } }
    ins!("match", matches_fn);
    ins!("find", find_fn);
    ins!("find_all", find_all);
    ins!("replace", replace);
    ins!("replace_all", replace_all);
    ins!("split", split);
    ins!("groups", groups);
    env.insert("regex".into(), J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m))));
}
