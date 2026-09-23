// json module, parsing and stringification

use crate::value::J2Value;
use crate::error::{J2Err, J2Result};
use serde_json::Value as SV;

fn from_serde(v: SV) -> J2Value {
    match v {
        SV::Null => J2Value::Null,
        SV::Bool(b) => J2Value::Bool(b),
        SV::Number(n) => {
            if let Some(i) = n.as_i64() { J2Value::Int(i) }
            else if let Some(f) = n.as_f64() { J2Value::Float(f) }
            else { J2Value::Null }
        }
        SV::String(s) => J2Value::text(s),
        SV::Array(a) => J2Value::seq(a.into_iter().map(from_serde).collect()),
        SV::Object(o) => {
            let mut m = std::collections::HashMap::<String, J2Value>::new();
            for (k, v) in o { m.insert(k, from_serde(v)); }
            J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m)))
        }
    }
}

fn to_serde(v: &J2Value) -> J2Result<SV> {
    match v {
        J2Value::Null => Ok(SV::Null),
        J2Value::Bool(b) => Ok(SV::Bool(*b)),
        J2Value::Int(i) => Ok(SV::Number((*i).into())),
        J2Value::Float(f) => {
            serde_json::Number::from_f64(*f).map(SV::Number)
                .ok_or_else(|| J2Err::value("json.stringify: cannot encode NaN/inf"))
        }
        J2Value::Text(t) => Ok(SV::String((**t).clone())),
        J2Value::Seq(s) => {
            let s = s.lock().unwrap_or_else(|p| p.into_inner());
            let mut out = Vec::with_capacity(s.items.len());
            for v in &s.items { out.push(to_serde(v)?); }
            Ok(SV::Array(out))
        }
        J2Value::SeqF64(_) => to_serde(&v.to_boxed_seq()),
        J2Value::Map(m) => {
            let m = m.lock().unwrap_or_else(|p| p.into_inner());
            let mut out = serde_json::Map::new();
            for (k, v) in m.iter() { out.insert(k.clone(), to_serde(v)?); }
            Ok(SV::Object(out))
        }
        _ => Err(J2Err::type_err(format!("json.stringify: cannot encode {}", v.type_name()))),
    }
}

pub fn parse(args: &[J2Value]) -> J2Result<J2Value> {
    let s = match args.get(0) {
        Some(J2Value::Text(t)) => (**t).clone(),
        _ => return Err(J2Err::type_err("json.parse: arg 0 must be text")),
    };
    let v: SV = serde_json::from_str(&s).map_err(|e| J2Err::value(format!("json.parse: {}", e)))?;
    Ok(from_serde(v))
}

pub fn stringify(args: &[J2Value]) -> J2Result<J2Value> {
    let v = args.get(0).ok_or_else(|| J2Err::type_err("json.stringify: missing arg"))?;
    let s = to_serde(v)?;
    Ok(J2Value::text(serde_json::to_string(&s).map_err(|e| J2Err::runtime(format!("json.stringify: {}", e)))?))
}

pub fn stringify_pretty(args: &[J2Value]) -> J2Result<J2Value> {
    let v = args.get(0).ok_or_else(|| J2Err::type_err("json.stringify_pretty: missing arg"))?;
    let s = to_serde(v)?;
    Ok(J2Value::text(serde_json::to_string_pretty(&s).map_err(|e| J2Err::runtime(format!("json.stringify_pretty: {}", e)))?))
}

pub fn install(env: &mut std::collections::HashMap<String, J2Value>) {
    let mut m = std::collections::HashMap::<String, J2Value>::new();
    macro_rules! ins { ($n:expr, $f:path) => {
        m.insert($n.to_string(), J2Value::Builtin(std::sync::Arc::new($f), $n));
    } }
    ins!("parse", parse);
    ins!("stringify", stringify);
    ins!("stringify_pretty", stringify_pretty);
    env.insert("json".into(), J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m))));
}
