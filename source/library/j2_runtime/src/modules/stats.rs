// stats module (spec numeric and math modules)

use std::collections::HashMap;

use crate::builtins::{self, collect_finite_items};
use crate::error::{J2Err, J2Result};
use crate::value::J2Value;

fn arity(args: &[J2Value], n: usize, who: &str) -> J2Result<()> {
    if args.len() != n {
        Err(J2Err::type_err(format!("stats.{}: expected {} args, got {}", who, n, args.len())))
    } else { Ok(()) }
}

pub fn sum(args: &[J2Value]) -> J2Result<J2Value> { builtins::sum(args) }
pub fn max(args: &[J2Value]) -> J2Result<J2Value> { builtins::max(args) }
pub fn min(args: &[J2Value]) -> J2Result<J2Value> { builtins::min(args) }

pub fn count(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "count")?;
    let items = match &args[0] {
        J2Value::Seq(s) => s.lock().unwrap_or_else(|p| p.into_inner()).items.clone(),
        J2Value::SeqF64(_) | J2Value::Flow(_) => collect_finite_items(&args[0], "stats.count")?,
        _ => return Err(J2Err::type_err("stats.count: seq or flow expected")),
    };
    Ok(J2Value::Int(items.len() as i64))
}

pub fn avg(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "avg")?;
    let items = collect_finite_items(&args[0], "stats.avg")?;
    if items.is_empty() { return Err(J2Err::value("stats.avg: empty seq")); }
    let mut s = 0.0;
    for v in &items { s += v.as_num_f64()?; }
    Ok(J2Value::Float(s / items.len() as f64))
}

pub fn median(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "median")?;
    let items = collect_finite_items(&args[0], "stats.median")?;
    if items.is_empty() { return Err(J2Err::value("stats.median: empty seq")); }
    let mut nums: Vec<f64> = Vec::with_capacity(items.len());
    for v in &items { nums.push(v.as_num_f64()?); }
    nums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = nums.len();
    if n % 2 == 1 { Ok(J2Value::Float(nums[n / 2])) }
    else {
        let m = (nums[n / 2 - 1] + nums[n / 2]) / 2.0;
        Ok(J2Value::Float(m))
    }
}

pub fn mode(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "mode")?;
    let items = collect_finite_items(&args[0], "stats.mode")?;
    if items.is_empty() { return Err(J2Err::value("stats.mode: empty seq")); }
    // Count by display-string key
    let mut counts: HashMap<String, (usize, J2Value)> = HashMap::new();
    for v in &items {
        let k = format!("{}", v);
        counts.entry(k).and_modify(|e| e.0 += 1).or_insert((1, v.clone()));
    }
    // Most frequent; tie-break by first-seen.
    let mut best: Option<(usize, J2Value, usize)> = None;
    for (idx, v) in items.iter().enumerate() {
        let k = format!("{}", v);
        let (c, _) = counts[&k];
        let candidate = (c, v.clone(), idx);
        match &best {
            None => best = Some(candidate),
            Some((bc, _, bi)) => {
                if c > *bc || (c == *bc && idx < *bi) {
                    best = Some(candidate);
                }
            }
        }
    }
    Ok(best.unwrap().1)
}

pub fn range(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "range")?;
    let mx = max(args)?;
    let mn = min(args)?;
    mx.sub(&mn)
}

pub fn install(env: &mut std::collections::HashMap<String, J2Value>) {
    let mut m = std::collections::HashMap::<String, J2Value>::new();
    macro_rules! ins { ($n:expr, $f:path) => { m.insert($n.to_string(), J2Value::Builtin(std::sync::Arc::new($f), $n)); } }
    ins!("sum", sum); ins!("max", max); ins!("min", min); ins!("count", count);
    ins!("avg", avg); ins!("median", median); ins!("mode", mode); ins!("range", range);
    env.insert("stats".into(), J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m))));
}
