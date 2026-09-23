// bits module, bitwise ops on integer val

use crate::value::J2Value;
use crate::error::{J2Err, J2Result};

fn pair_i64(args: &[J2Value], who: &str) -> J2Result<(i64, i64)> {
    if args.len() < 2 { return Err(J2Err::type_err(format!("{}: takes 2 args", who))); }
    let a = args[0].as_num_i64().map_err(|_| J2Err::type_err(format!("{}: arg 0 must be int", who)))?;
    let b = args[1].as_num_i64().map_err(|_| J2Err::type_err(format!("{}: arg 1 must be int", who)))?;
    Ok((a, b))
}

fn one_i64(args: &[J2Value], who: &str) -> J2Result<i64> {
    if args.is_empty() { return Err(J2Err::type_err(format!("{}: takes 1 arg", who))); }
    args[0].as_num_i64().map_err(|_| J2Err::type_err(format!("{}: arg 0 must be int", who)))
}

pub fn band(a: &[J2Value]) -> J2Result<J2Value> { let (a,b) = pair_i64(a, "bits.band")?; Ok(J2Value::Int(a & b)) }
pub fn bor(a: &[J2Value])  -> J2Result<J2Value> { let (a,b) = pair_i64(a, "bits.bor")?;  Ok(J2Value::Int(a | b)) }
pub fn bxor(a: &[J2Value]) -> J2Result<J2Value> { let (a,b) = pair_i64(a, "bits.bxor")?; Ok(J2Value::Int(a ^ b)) }
pub fn bnot(a: &[J2Value]) -> J2Result<J2Value> { Ok(J2Value::Int(!one_i64(a, "bits.bnot")?)) }
pub fn shl(a: &[J2Value])  -> J2Result<J2Value> { let (a,b) = pair_i64(a, "bits.shl")?; Ok(J2Value::Int(((a as u64).wrapping_shl(b as u32)) as i64)) }
pub fn shr(a: &[J2Value])  -> J2Result<J2Value> { let (a,b) = pair_i64(a, "bits.shr")?; Ok(J2Value::Int(((a as u64).wrapping_shr(b as u32)) as i64)) }
pub fn popcount(a: &[J2Value]) -> J2Result<J2Value> { Ok(J2Value::Int(one_i64(a, "bits.popcount")?.count_ones() as i64)) }
pub fn leading_zeros(a: &[J2Value]) -> J2Result<J2Value> { Ok(J2Value::Int(one_i64(a, "bits.leading_zeros")?.leading_zeros() as i64)) }
pub fn trailing_zeros(a: &[J2Value]) -> J2Result<J2Value> { Ok(J2Value::Int(one_i64(a, "bits.trailing_zeros")?.trailing_zeros() as i64)) }

pub fn install(env: &mut std::collections::HashMap<String, J2Value>) {
    let mut m = std::collections::HashMap::<String, J2Value>::new();
    macro_rules! ins { ($n:expr, $f:path) => {
        m.insert($n.to_string(), J2Value::Builtin(std::sync::Arc::new($f), $n));
    } }
    ins!("band", band); ins!("bor", bor); ins!("bxor", bxor); ins!("bnot", bnot);
    ins!("shl", shl); ins!("shr", shr);
    ins!("popcount", popcount); ins!("leading_zeros", leading_zeros); ins!("trailing_zeros", trailing_zeros);
    env.insert("bits".into(), J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m))));
}
