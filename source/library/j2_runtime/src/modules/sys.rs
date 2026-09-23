// sys module, system and OS info

use crate::value::J2Value;
use crate::error::J2Result;

pub fn os(_: &[J2Value]) -> J2Result<J2Value> { Ok(J2Value::text(std::env::consts::OS.to_string())) }
pub fn arch(_: &[J2Value]) -> J2Result<J2Value> { Ok(J2Value::text(std::env::consts::ARCH.to_string())) }
pub fn hostname(_: &[J2Value]) -> J2Result<J2Value> {
    Ok(J2Value::text(whoami::fallible::hostname().unwrap_or_else(|_| "unknown".to_string())))
}
pub fn username(_: &[J2Value]) -> J2Result<J2Value> {
    Ok(J2Value::text(whoami::username()))
}
pub fn cpu_count(_: &[J2Value]) -> J2Result<J2Value> {
    Ok(J2Value::Int(num_cpus::get() as i64))
}

pub fn install(env: &mut std::collections::HashMap<String, J2Value>) {
    let mut m = std::collections::HashMap::<String, J2Value>::new();
    macro_rules! ins { ($n:expr, $f:path) => {
        m.insert($n.to_string(), J2Value::Builtin(std::sync::Arc::new($f), $n));
    } }
    ins!("os", os); ins!("arch", arch); ins!("hostname", hostname);
    ins!("username", username); ins!("cpu_count", cpu_count);
    env.insert("sys".into(), J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m))));
}
