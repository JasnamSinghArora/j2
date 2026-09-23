// proc module, process and OS interaction

use crate::value::J2Value;
use crate::error::{J2Err, J2Result};

fn arg_text(args: &[J2Value], i: usize, who: &str) -> J2Result<String> {
    match args.get(i) {
        Some(J2Value::Text(t)) => Ok((**t).clone()),
        Some(other) => Err(J2Err::type_err(format!("{}: arg {} expected text, got {}", who, i, other.type_name()))),
        None => Err(J2Err::type_err(format!("{}: missing arg {}", who, i))),
    }
}

pub fn argv(_args: &[J2Value]) -> J2Result<J2Value> {
    let v: Vec<J2Value> = std::env::args().map(J2Value::text).collect();
    Ok(J2Value::seq(v))
}

pub fn env_var(args: &[J2Value]) -> J2Result<J2Value> {
    let name = arg_text(args, 0, "proc.env")?;
    match std::env::var(&name) {
        Ok(v) => Ok(J2Value::text(v)),
        Err(_) => Ok(J2Value::Null),
    }
}

pub fn set_env(args: &[J2Value]) -> J2Result<J2Value> {
    let name = arg_text(args, 0, "proc.set_env")?;
    let val = arg_text(args, 1, "proc.set_env")?;
    unsafe { std::env::set_var(&name, &val); }
    Ok(J2Value::Null)
}

pub fn exit_proc(args: &[J2Value]) -> J2Result<J2Value> {
    let code = args.get(0).and_then(|v| v.as_num_i64().ok()).unwrap_or(0) as i32;
    std::process::exit(code);
}

pub fn cwd(_args: &[J2Value]) -> J2Result<J2Value> {
    let p = std::env::current_dir().map_err(|e| J2Err::runtime(format!("proc.cwd: {}", e)))?;
    Ok(J2Value::text(p.to_string_lossy().into_owned()))
}

pub fn chdir(args: &[J2Value]) -> J2Result<J2Value> {
    let path = arg_text(args, 0, "proc.chdir")?;
    std::env::set_current_dir(&path).map_err(|e| J2Err::runtime(format!("proc.chdir: {}", e)))?;
    Ok(J2Value::Null)
}

pub fn run(args: &[J2Value]) -> J2Result<J2Value> {
    let cmd = arg_text(args, 0, "proc.run")?;
    // Spawning arbitrary processes is gated
    crate::require_cap("J2_ALLOW_PROC", "proc.run")?;
    let mut cmd_args: Vec<String> = Vec::new();
    if let Some(J2Value::Seq(s)) = args.get(1) {
        for v in &s.lock().unwrap_or_else(|p| p.into_inner()).items {
            if let J2Value::Text(t) = v {
                cmd_args.push((**t).clone());
            }
        }
    }
    let out = std::process::Command::new(&cmd).args(&cmd_args).output()
        .map_err(|e| J2Err::runtime(format!("proc.run: {}", e)))?;
    let mut m = std::collections::HashMap::<String, J2Value>::new();
    m.insert("stdout".into(), J2Value::text(String::from_utf8_lossy(&out.stdout).into_owned()));
    m.insert("stderr".into(), J2Value::text(String::from_utf8_lossy(&out.stderr).into_owned()));
    m.insert("status".into(), J2Value::Int(out.status.code().unwrap_or(-1) as i64));
    Ok(J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m))))
}

pub fn install(env: &mut std::collections::HashMap<String, J2Value>) {
    let mut m = std::collections::HashMap::<String, J2Value>::new();
    macro_rules! ins { ($n:expr, $f:path) => {
        m.insert($n.to_string(), J2Value::Builtin(std::sync::Arc::new($f), $n));
    } }
    ins!("argv", argv);
    ins!("env", env_var);
    ins!("set_env", set_env);
    ins!("exit", exit_proc);
    ins!("cwd", cwd);
    ins!("chdir", chdir);
    ins!("run", run);
    env.insert("proc".into(), J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m))));
}
