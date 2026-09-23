// hex module, hex encoding and decoding

use crate::value::J2Value;
use crate::error::{J2Err, J2Result};

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
    let bytes = extract_bytes(args.get(0).ok_or_else(|| J2Err::type_err("hex.encode: missing arg"))?, "hex.encode")?;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{:02x}", b));
    }
    Ok(J2Value::text(out))
}

pub fn decode(args: &[J2Value]) -> J2Result<J2Value> {
    let s = match args.get(0) {
        Some(J2Value::Text(t)) => (**t).clone(),
        _ => return Err(J2Err::type_err("hex.decode: arg 0 must be text")),
    };
    if s.len() % 2 != 0 { return Err(J2Err::value("hex.decode: odd-length input")); }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = hexval(bytes[i]).ok_or_else(|| J2Err::value("hex.decode: invalid char"))?;
        let lo = hexval(bytes[i+1]).ok_or_else(|| J2Err::value("hex.decode: invalid char"))?;
        out.push(((hi << 4) | lo) as u8);
        i += 2;
    }
    Ok(J2Value::seq(out.into_iter().map(|b| J2Value::Int(b as i64)).collect()))
}

fn hexval(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

pub fn install(env: &mut std::collections::HashMap<String, J2Value>) {
    let mut m = std::collections::HashMap::<String, J2Value>::new();
    macro_rules! ins { ($n:expr, $f:path) => {
        m.insert($n.to_string(), J2Value::Builtin(std::sync::Arc::new($f), $n));
    } }
    ins!("encode", encode);
    ins!("decode", decode);
    env.insert("hex".into(), J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m))));
}
