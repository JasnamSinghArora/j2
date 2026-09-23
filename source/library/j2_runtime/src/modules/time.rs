// time module, monotonic timing for benchmarks

use crate::value::J2Value;
use crate::error::J2Err as J2Error;
use std::time::Instant;
use std::cell::RefCell;

thread_local! {
    // slot table; avoids adding Instant to J2Value
    static SLOTS: RefCell<Vec<Instant>> = const { RefCell::new(Vec::new()) };
}

pub fn now(args: &[J2Value]) -> Result<J2Value, J2Error> {
    if !args.is_empty() {
        return Err(J2Error::type_err("time.now() takes no arguments"));
    }
    let idx = SLOTS.with(|s| {
        let mut v = s.borrow_mut();
        v.push(Instant::now());
        v.len() as i64 - 1
    });
    Ok(J2Value::Int(idx))
}

pub fn elapsed_ms(args: &[J2Value]) -> Result<J2Value, J2Error> {
    if args.len() != 1 {
        return Err(J2Error::type_err("time.elapsed_ms(t) takes 1 argument"));
    }
    let idx = match &args[0] {
        J2Value::Int(i) => *i as usize,
        _ => return Err(J2Error::type_err("time.elapsed_ms expects a token returned by time.now()")),
    };
    let ms = SLOTS.with(|s| {
        let v = s.borrow();
        v.get(idx).map(|t| t.elapsed().as_secs_f64() * 1000.0)
    });
    match ms {
        Some(ms) => Ok(J2Value::Float(ms)),
        None => Err(J2Error::value("invalid time token")),
    }
}

pub fn install(env: &mut std::collections::HashMap<String, J2Value>) {
    let mut m = std::collections::HashMap::<String, J2Value>::new();
    macro_rules! ins { ($n:expr, $f:path) => { m.insert($n.to_string(), J2Value::Builtin(std::sync::Arc::new($f), $n)); } }
    ins!("now", now);
    ins!("elapsed_ms", elapsed_ms);
    env.insert("time".into(), J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m))));
}
