// hash module, cryptographic and fast hashes

use crate::value::J2Value;
use crate::error::{J2Err, J2Result};
use sha2::{Sha256, Sha512, Digest};
use md5::Md5;

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

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b { s.push_str(&format!("{:02x}", x)); }
    s
}

pub fn sha256(args: &[J2Value]) -> J2Result<J2Value> {
    let b = extract_bytes(args.get(0).ok_or_else(|| J2Err::type_err("hash.sha256: missing arg"))?, "hash.sha256")?;
    let mut h = Sha256::new();
    h.update(&b);
    Ok(J2Value::text(hex(&h.finalize())))
}

pub fn sha512(args: &[J2Value]) -> J2Result<J2Value> {
    let b = extract_bytes(args.get(0).ok_or_else(|| J2Err::type_err("hash.sha512: missing arg"))?, "hash.sha512")?;
    let mut h = Sha512::new();
    h.update(&b);
    Ok(J2Value::text(hex(&h.finalize())))
}

pub fn md5(args: &[J2Value]) -> J2Result<J2Value> {
    let b = extract_bytes(args.get(0).ok_or_else(|| J2Err::type_err("hash.md5: missing arg"))?, "hash.md5")?;
    let mut h = Md5::new();
    h.update(&b);
    Ok(J2Value::text(hex(&h.finalize())))
}

pub fn xxhash(args: &[J2Value]) -> J2Result<J2Value> {
    use xxhash_rust::xxh3::xxh3_64;
    let b = extract_bytes(args.get(0).ok_or_else(|| J2Err::type_err("hash.xxhash: missing arg"))?, "hash.xxhash")?;
    Ok(J2Value::Int(xxh3_64(&b) as i64))
}

pub fn install(env: &mut std::collections::HashMap<String, J2Value>) {
    let mut m = std::collections::HashMap::<String, J2Value>::new();
    macro_rules! ins { ($n:expr, $f:path) => {
        m.insert($n.to_string(), J2Value::Builtin(std::sync::Arc::new($f), $n));
    } }
    ins!("sha256", sha256);
    ins!("sha512", sha512);
    ins!("md5", md5);
    ins!("xxhash", xxhash);
    env.insert("hash".into(), J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m))));
}
