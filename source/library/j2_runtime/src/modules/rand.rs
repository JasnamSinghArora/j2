// rand module, thread-local xorshift64*; seed(0) becomes seed(1)

use crate::value::J2Value;
use crate::error::J2Err as J2Error;
use std::cell::Cell;

thread_local! {
    static STATE: Cell<u64> = const { Cell::new(0xCAFE_BABE_DEAD_BEEFu64) };
}

fn next_u64() -> u64 {
    STATE.with(|s| {
        let mut x = s.get();
        if x == 0 { x = 1; }
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.set(x);
        x.wrapping_mul(0x2545_F491_4F6C_DD1Du64)
    })
}

pub fn seed(args: &[J2Value]) -> Result<J2Value, J2Error> {
    if args.len() != 1 {
        return Err(J2Error::type_err("rand.seed(s) takes 1 argument"));
    }
    let s = match &args[0] {
        J2Value::Int(i) => (*i as u64).max(1),
        J2Value::Float(f) => (f.to_bits()).max(1),
        _ => return Err(J2Error::type_err("rand.seed expects a number")),
    };
    STATE.with(|st| st.set(s));
    Ok(J2Value::Null)
}

pub fn next_int(args: &[J2Value]) -> Result<J2Value, J2Error> {
    if !args.is_empty() {
        return Err(J2Error::type_err("rand.next_int() takes no arguments"));
    }
    let raw = next_u64();
    Ok(J2Value::Int(raw as i64))
}

pub fn next_float(args: &[J2Value]) -> Result<J2Value, J2Error> {
    if !args.is_empty() {
        return Err(J2Error::type_err("rand.next_float() takes no arguments"));
    }
    let v = next_u64() >> 11; // 53 bits of randomness
    let f = (v as f64) / ((1u64 << 53) as f64);
    Ok(J2Value::Float(f))
}

pub fn next_range(args: &[J2Value]) -> Result<J2Value, J2Error> {
    if args.len() != 2 {
        return Err(J2Error::type_err("rand.next_range(lo, hi) takes 2 arguments"));
    }
    let lo = match &args[0] {
        J2Value::Int(i) => *i,
        _ => return Err(J2Error::type_err("rand.next_range expects int arguments")),
    };
    let hi = match &args[1] {
        J2Value::Int(i) => *i,
        _ => return Err(J2Error::type_err("rand.next_range expects int arguments")),
    };
    if hi <= lo {
        return Err(J2Error::value("rand.next_range: hi must be > lo"));
    }
    let span = (hi - lo) as u64;
    let r = next_u64() % span;
    Ok(J2Value::Int(lo + r as i64))
}

pub fn install(env: &mut std::collections::HashMap<String, J2Value>) {
    let mut m = std::collections::HashMap::<String, J2Value>::new();
    macro_rules! ins { ($n:expr, $f:path) => { m.insert($n.to_string(), J2Value::Builtin(std::sync::Arc::new($f), $n)); } }
    ins!("seed", seed);
    ins!("next_int", next_int);
    ins!("next_float", next_float);
    ins!("next_range", next_range);
    env.insert("rand".into(), J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m))));
}
