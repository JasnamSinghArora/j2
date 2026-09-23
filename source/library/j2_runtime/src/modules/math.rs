// math module (spec numeric and math modules)

use crate::builtins;
use crate::error::{J2Err, J2Result};
use crate::value::J2Value;

fn arity(args: &[J2Value], n: usize, who: &str) -> J2Result<()> {
    if args.len() != n {
        Err(J2Err::type_err(format!("math.{}: expected {} args, got {}", who, n, args.len())))
    } else { Ok(()) }
}

pub fn abs(args: &[J2Value]) -> J2Result<J2Value> { builtins::abs(args) }
pub fn sign(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "sign")?;
    match &args[0] {
        J2Value::Int(n) => Ok(J2Value::Int(n.signum())),
        J2Value::Float(x) => Ok(J2Value::Int(if *x > 0.0 { 1 } else if *x < 0.0 { -1 } else { 0 })),
        _ => Err(J2Err::type_err("math.sign: number expected")),
    }
}
pub fn clamp(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 3, "clamp")?;
    let x = args[0].as_num_f64()?;
    let lo = args[1].as_num_f64()?;
    let hi = args[2].as_num_f64()?;
    let r = x.max(lo).min(hi);
    if let (J2Value::Int(_), J2Value::Int(_), J2Value::Int(_)) = (&args[0], &args[1], &args[2]) {
        return Ok(J2Value::Int(r as i64));
    }
    Ok(J2Value::Float(r))
}
pub fn round(args: &[J2Value]) -> J2Result<J2Value> { builtins::round(args) }
pub fn floor(args: &[J2Value]) -> J2Result<J2Value> { builtins::floor(args) }
pub fn ceil(args: &[J2Value]) -> J2Result<J2Value> { builtins::ceil(args) }
pub fn trunc(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "trunc")?;
    let x = args[0].as_num_f64()?;
    Ok(J2Value::Int(builtins::f64_to_i64(x.trunc(), "trunc")?))
}
pub fn pow(args: &[J2Value]) -> J2Result<J2Value> { builtins::pow(args) }
pub fn sqrt(args: &[J2Value]) -> J2Result<J2Value> { builtins::sqrt(args) }
pub fn cbrt(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "cbrt")?;
    Ok(J2Value::Float(args[0].as_num_f64()?.cbrt()))
}
pub fn log(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "log")?;
    let x = args[0].as_num_f64()?;
    if x <= 0.0 { return Err(J2Err::value("math.log: argument must be positive")); }
    Ok(J2Value::Float(x.ln()))
}
pub fn log2(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "log2")?;
    let x = args[0].as_num_f64()?;
    if x <= 0.0 { return Err(J2Err::value("math.log2: argument must be positive")); }
    Ok(J2Value::Float(x.log2()))
}
pub fn log10(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "log10")?;
    let x = args[0].as_num_f64()?;
    if x <= 0.0 { return Err(J2Err::value("math.log10: argument must be positive")); }
    Ok(J2Value::Float(x.log10()))
}
pub fn sin(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "sin")?; Ok(J2Value::Float(args[0].as_num_f64()?.sin()))
}
pub fn cos(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "cos")?; Ok(J2Value::Float(args[0].as_num_f64()?.cos()))
}
pub fn tan(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "tan")?; Ok(J2Value::Float(args[0].as_num_f64()?.tan()))
}
pub fn torad(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "torad")?;
    Ok(J2Value::Float(args[0].as_num_f64()? * std::f64::consts::PI / 180.0))
}
pub fn todeg(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "todeg")?;
    Ok(J2Value::Float(args[0].as_num_f64()? * 180.0 / std::f64::consts::PI))
}

pub fn exp(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "exp")?;
    Ok(J2Value::Float(args[0].as_num_f64()?.exp()))
}
pub fn ln(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 1, "ln")?;
    Ok(J2Value::Float(args[0].as_num_f64()?.ln()))
}
pub fn log_n(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args, 2, "log_n")?;
    Ok(J2Value::Float(args[0].as_num_f64()?.log(args[1].as_num_f64()?)))
}
pub fn asin(args: &[J2Value]) -> J2Result<J2Value> { arity(args,1,"asin")?; Ok(J2Value::Float(args[0].as_num_f64()?.asin())) }
pub fn acos(args: &[J2Value]) -> J2Result<J2Value> { arity(args,1,"acos")?; Ok(J2Value::Float(args[0].as_num_f64()?.acos())) }
pub fn atan(args: &[J2Value]) -> J2Result<J2Value> { arity(args,1,"atan")?; Ok(J2Value::Float(args[0].as_num_f64()?.atan())) }
pub fn atan2(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args,2,"atan2")?; Ok(J2Value::Float(args[0].as_num_f64()?.atan2(args[1].as_num_f64()?)))
}
pub fn sinh(args: &[J2Value]) -> J2Result<J2Value> { arity(args,1,"sinh")?; Ok(J2Value::Float(args[0].as_num_f64()?.sinh())) }
pub fn cosh(args: &[J2Value]) -> J2Result<J2Value> { arity(args,1,"cosh")?; Ok(J2Value::Float(args[0].as_num_f64()?.cosh())) }
pub fn tanh(args: &[J2Value]) -> J2Result<J2Value> { arity(args,1,"tanh")?; Ok(J2Value::Float(args[0].as_num_f64()?.tanh())) }
pub fn gcd(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args,2,"gcd")?;
    let mut a = args[0].as_num_i64()?.unsigned_abs();
    let mut b = args[1].as_num_i64()?.unsigned_abs();
    while b != 0 { let t = b; b = a % b; a = t; }
    Ok(J2Value::Int(a as i64))
}
pub fn lcm(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args,2,"lcm")?;
    let a = args[0].as_num_i64()?.unsigned_abs();
    let b = args[1].as_num_i64()?.unsigned_abs();
    if a == 0 || b == 0 { return Ok(J2Value::Int(0)); }
    let (mut x, mut y) = (a, b);
    while y != 0 { let t = y; y = x % y; x = t; }
    // lcm = (a/gcd)*b with overflow checks
    let prod = (a / x)
        .checked_mul(b)
        .ok_or_else(|| crate::error::J2Err::overflow("lcm: overflow"))?;
    let r = i64::try_from(prod)
        .map_err(|_| crate::error::J2Err::overflow("lcm: result exceeds i64 range"))?;
    Ok(J2Value::Int(r))
}
pub fn factorial(args: &[J2Value]) -> J2Result<J2Value> {
    arity(args,1,"factorial")?;
    let n = args[0].as_num_i64()?;
    if n < 0 { return Err(crate::error::J2Err::value("factorial: negative input")); }
    let mut r: i64 = 1;
    for i in 2..=n { r = r.checked_mul(i).ok_or_else(|| crate::error::J2Err::overflow("factorial: overflow"))?; }
    Ok(J2Value::Int(r))
}
pub fn is_finite(args: &[J2Value]) -> J2Result<J2Value> { arity(args,1,"is_finite")?; Ok(J2Value::Bool(args[0].as_num_f64()?.is_finite())) }
pub fn is_nan(args: &[J2Value]) -> J2Result<J2Value> { arity(args,1,"is_nan")?; Ok(J2Value::Bool(args[0].as_num_f64()?.is_nan())) }

pub fn install(env: &mut std::collections::HashMap<String, J2Value>) {
    // Expose as `math.X` by inserting a map.
    let mut m = std::collections::HashMap::<String, J2Value>::new();
    macro_rules! ins { ($n:expr, $f:path) => { m.insert($n.to_string(), J2Value::Builtin(std::sync::Arc::new($f), $n)); } }
    ins!("abs", abs); ins!("sign", sign); ins!("clamp", clamp);
    ins!("round", round); ins!("floor", floor); ins!("ceil", ceil); ins!("trunc", trunc);
    ins!("pow", pow); ins!("sqrt", sqrt); ins!("cbrt", cbrt);
    ins!("log", log); ins!("log2", log2); ins!("log10", log10);
    ins!("sin", sin); ins!("cos", cos); ins!("tan", tan);
    ins!("torad", torad); ins!("todeg", todeg);
    ins!("exp", exp); ins!("ln", ln); ins!("log_n", log_n);
    ins!("asin", asin); ins!("acos", acos); ins!("atan", atan); ins!("atan2", atan2);
    ins!("sinh", sinh); ins!("cosh", cosh); ins!("tanh", tanh);
    ins!("gcd", gcd); ins!("lcm", lcm); ins!("factorial", factorial);
    ins!("is_finite", is_finite); ins!("is_nan", is_nan);
    env.insert("math".into(), J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m))));
}
