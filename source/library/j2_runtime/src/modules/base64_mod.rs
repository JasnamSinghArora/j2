// J `base64` module.

use crate::value::J2Value;
use crate::error::{J2Err, J2Result};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE};

fn extract_bytes(v: &J2Value, who: &str) -> J2Result<Vec<u8>> {
    match v {
        J2Value::Text(t) => Ok(t.as_bytes().to_vec()),
        J2Value::Seq(s) => {
            let s = s.lock().unwrap_or_else(|p| p.into_inner());
            let mut out = Vec::with_capacity(s.items.len());
            for v in &s.items {
                let n = v.as_num_i64().map_err(|_| J2Err::type_err(format!("{}: seq must contain ints", who)))?;
                if !(0..=255).contains(&n) { return Err(J2Err::value(format!("{}: byte out of range", who))); }
                out.push(n as u8);
            }
            Ok(out)
        }
        J2Value::SeqF64(_) => extract_bytes(&v.to_boxed_seq(), who),
        _ => Err(J2Err::type_err(format!("{}: arg 0 must be text or seq<int>", who))),
    }
}

pub fn encode(args: &[J2Value]) -> J2Result<J2Value> {
    let b = extract_bytes(args.get(0).ok_or_else(|| J2Err::type_err("base64.encode: missing arg"))?, "base64.encode")?;
    Ok(J2Value::text(STANDARD.encode(&b)))
}

pub fn decode(args: &[J2Value]) -> J2Result<J2Value> {
    let s = match args.get(0) {
        Some(J2Value::Text(t)) => (**t).clone(),
        _ => return Err(J2Err::type_err("base64.decode: arg 0 must be text")),
    };
    let bytes = STANDARD.decode(&s).map_err(|e| J2Err::value(format!("base64.decode: {}", e)))?;
    Ok(J2Value::seq(bytes.into_iter().map(|b| J2Value::Int(b as i64)).collect()))
}

pub fn encode_url(args: &[J2Value]) -> J2Result<J2Value> {
    let b = extract_bytes(args.get(0).ok_or_else(|| J2Err::type_err("base64.encode_url: missing arg"))?, "base64.encode_url")?;
    Ok(J2Value::text(URL_SAFE.encode(&b)))
}

pub fn decode_url(args: &[J2Value]) -> J2Result<J2Value> {
    let s = match args.get(0) {
        Some(J2Value::Text(t)) => (**t).clone(),
        _ => return Err(J2Err::type_err("base64.decode_url: arg 0 must be text")),
    };
    let bytes = URL_SAFE.decode(&s).map_err(|e| J2Err::value(format!("base64.decode_url: {}", e)))?;
    Ok(J2Value::seq(bytes.into_iter().map(|b| J2Value::Int(b as i64)).collect()))
}

pub fn install(env: &mut std::collections::HashMap<String, J2Value>) {
    let mut m = std::collections::HashMap::<String, J2Value>::new();
    macro_rules! ins { ($n:expr, $f:path) => {
        m.insert($n.to_string(), J2Value::Builtin(std::sync::Arc::new($f), $n));
    } }
    ins!("encode", encode);
    ins!("decode", decode);
    ins!("encode_url", encode_url);
    ins!("decode_url", decode_url);
    env.insert("base64".into(), J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m))));
}
