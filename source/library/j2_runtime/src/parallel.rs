// parallel hooks; mirrors std runtime, compiles standalone

use std::sync::Arc;
use std::thread;

use crate::error::{J2Err, J2Result};
use crate::value::J2Value;

/// Threshold matching `library/std/src/parallel_runtime.rs:PARALLEL_REDUCE_THRESHOLD`.
const PAR_THRESHOLD: usize = 32_768;
const N_WORKERS: usize = 8;

/// Sum a seq into a single number
pub fn sum_seq(items: &[J2Value]) -> J2Result<J2Value> {
    if items.is_empty() {
        return Err(J2Err::value("sum requires a non-empty seq"));
    }
    // fast path for uniform ints or floats
    let all_int = items.iter().all(|v| matches!(v, J2Value::Int(_)));
    if all_int {
        if items.len() >= PAR_THRESHOLD {
            return par_sum_i64(items);
        }
        let mut s: i64 = 0;
        for v in items {
            if let J2Value::Int(n) = v {
                s = s.checked_add(*n).ok_or_else(|| J2Err::overflow("sum overflow"))?;
            }
        }
        return Ok(J2Value::Int(s));
    }
    let all_num = items.iter().all(|v| matches!(v, J2Value::Int(_) | J2Value::Float(_) | J2Value::Bool(_)));
    if all_num {
        if items.len() >= PAR_THRESHOLD {
            return par_sum_f64(items);
        }
        let mut s = 0.0;
        for v in items { s += v.as_num_f64()?; }
        return Ok(J2Value::Float(s));
    }
    Err(J2Err::type_err("sum requires a seq of numbers"))
}

fn par_sum_i64(items: &[J2Value]) -> J2Result<J2Value> {
    let items: Arc<Vec<i64>> = Arc::new(items.iter().map(|v| match v {
        J2Value::Int(n) => *n,
        _ => 0,
    }).collect());
    let n = items.len();
    let chunk = (n + N_WORKERS - 1) / N_WORKERS;
    let mut handles = Vec::with_capacity(N_WORKERS);
    for w in 0..N_WORKERS {
        let lo = w * chunk;
        let hi = ((w + 1) * chunk).min(n);
        if lo >= hi { break; }
        let items = items.clone();
        handles.push(thread::spawn(move || {
            let mut s: i64 = 0;
            for i in lo..hi { s = s.wrapping_add(items[i]); }
            s
        }));
    }
    let mut total: i64 = 0;
    for h in handles {
        total = total.wrapping_add(h.join().map_err(|_| J2Err::runtime("worker panicked"))?);
    }
    Ok(J2Value::Int(total))
}

fn par_sum_f64(items: &[J2Value]) -> J2Result<J2Value> {
    let items: Arc<Vec<f64>> = Arc::new(
        items.iter().map(|v| match v {
            J2Value::Int(n) => *n as f64,
            J2Value::Float(x) => *x,
            J2Value::Bool(b) => if *b { 1.0 } else { 0.0 },
            _ => 0.0,
        }).collect()
    );
    let n = items.len();
    let chunk = (n + N_WORKERS - 1) / N_WORKERS;
    let mut handles = Vec::with_capacity(N_WORKERS);
    for w in 0..N_WORKERS {
        let lo = w * chunk;
        let hi = ((w + 1) * chunk).min(n);
        if lo >= hi { break; }
        let items = items.clone();
        handles.push(thread::spawn(move || {
            let mut s = 0.0f64;
            for i in lo..hi { s += items[i]; }
            s
        }));
    }
    let mut total = 0.0;
    for h in handles {
        total += h.join().map_err(|_| J2Err::runtime("worker panicked"))?;
    }
    Ok(J2Value::Float(total))
}

/// Parallel map over items; serial below threshold
pub fn map_seq<F>(items: Vec<J2Value>, f: F) -> J2Result<Vec<J2Value>>
where F: Fn(&J2Value) -> J2Result<J2Value> + Send + Sync + 'static
{
    let n = items.len();
    if n < PAR_THRESHOLD {
        let mut out = Vec::with_capacity(n);
        for v in &items { out.push(f(v)?); }
        return Ok(out);
    }
    let items = Arc::new(items);
    let f = Arc::new(f);
    let chunk = (n + N_WORKERS - 1) / N_WORKERS;
    let mut handles = Vec::with_capacity(N_WORKERS);
    for w in 0..N_WORKERS {
        let lo = w * chunk;
        let hi = ((w + 1) * chunk).min(n);
        if lo >= hi { break; }
        let items = items.clone();
        let f = f.clone();
        handles.push(thread::spawn(move || {
            let mut out = Vec::with_capacity(hi - lo);
            for i in lo..hi {
                out.push(f(&items[i]));
            }
            out
        }));
    }
    let mut out = Vec::with_capacity(n);
    for h in handles {
        let chunk_results = h.join().map_err(|_| J2Err::runtime("worker panicked"))?;
        for r in chunk_results { out.push(r?); }
    }
    Ok(out)
}

/// Parallel filter+count: count items satisfying pred.
pub fn filter_count<F>(items: &[J2Value], pred: F) -> J2Result<i64>
where F: Fn(&J2Value) -> J2Result<bool> + Send + Sync + 'static
{
    let n = items.len();
    if n < PAR_THRESHOLD {
        let mut c: i64 = 0;
        for v in items { if pred(v)? { c += 1; } }
        return Ok(c);
    }
    let items: Arc<Vec<J2Value>> = Arc::new(items.to_vec());
    let pred = Arc::new(pred);
    let chunk = (n + N_WORKERS - 1) / N_WORKERS;
    let mut handles = Vec::with_capacity(N_WORKERS);
    for w in 0..N_WORKERS {
        let lo = w * chunk;
        let hi = ((w + 1) * chunk).min(n);
        if lo >= hi { break; }
        let items = items.clone();
        let pred = pred.clone();
        handles.push(thread::spawn(move || -> J2Result<i64> {
            let mut c: i64 = 0;
            for i in lo..hi {
                if pred(&items[i])? { c += 1; }
            }
            Ok(c)
        }));
    }
    let mut total: i64 = 0;
    for h in handles {
        total += h.join().map_err(|_| J2Err::runtime("worker panicked"))??;
    }
    Ok(total)
}
