// async module; Tokio-backed blocking fns, no `await`

use crate::value::J2Value;
use crate::error::{J2Err, J2Result};
use std::sync::OnceLock;
use tokio::runtime::Runtime;

fn rt() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(num_cpus::get())
            .build()
            .expect("init tokio runtime")
    })
}

fn call_zero_arg(callee: &J2Value) -> J2Result<J2Value> {
    match callee {
        J2Value::Func(f) => {
            for c in &f.clauses {
                if c.patterns.is_empty() { return (c.body)(&[]); }
            }
            Err(J2Err::type_err("async.parallel: zero-arg callee required"))
        }
        J2Value::Builtin(f, _) => f(&[]),
        _ => Err(J2Err::type_err("async.parallel: callee must be a function")),
    }
}

/// Run zero-arg fns concurrently, results in order
pub fn parallel(args: &[J2Value]) -> J2Result<J2Value> {
    let xs = match args.get(0) {
        Some(J2Value::Seq(s)) => s.lock().unwrap_or_else(|p| p.into_inner()).items.clone(),
        _ => return Err(J2Err::type_err("async.parallel: arg 0 must be a seq of functions")),
    };
    let r = rt();
    // Submit each task; collect handles.
    let n = xs.len();
    let mut futs = Vec::with_capacity(n);
    for f in xs.into_iter() {
        futs.push(r.spawn_blocking(move || call_zero_arg(&f)));
    }
    // Await all.
    let mut out = Vec::with_capacity(n);
    for fut in futs {
        let r = r.block_on(fut).map_err(|e| J2Err::runtime(format!("async.parallel: join: {}", e)))??;
        out.push(r);
    }
    Ok(J2Value::seq(out))
}

pub fn sleep_ms(args: &[J2Value]) -> J2Result<J2Value> {
    let ms = args.get(0).and_then(|v| v.as_num_i64().ok()).unwrap_or(0).max(0) as u64;
    rt().block_on(async move {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    });
    Ok(J2Value::Null)
}

pub fn http_get(args: &[J2Value]) -> J2Result<J2Value> {
    let url = match args.get(0) {
        Some(J2Value::Text(t)) => (**t).clone(),
        _ => return Err(J2Err::type_err("async.http_get: arg 0 must be text")),
    };
    rt().block_on(async move {
        let client = reqwest::Client::new();
        match client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16() as i64;
                let body = resp.text().await.unwrap_or_default();
                let mut m = std::collections::HashMap::<String, J2Value>::new();
                m.insert("status".into(), J2Value::Int(status));
                m.insert("body".into(), J2Value::text(body));
                Ok(J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m))))
            }
            Err(e) => Err(J2Err::runtime(format!("async.http_get: {}", e))),
        }
    })
}

pub fn read_file(args: &[J2Value]) -> J2Result<J2Value> {
    let path = match args.get(0) {
        Some(J2Value::Text(t)) => (**t).clone(),
        _ => return Err(J2Err::type_err("async.read_file: arg 0 must be text")),
    };
    rt().block_on(async move {
        match tokio::fs::read_to_string(&path).await {
            Ok(s) => Ok(J2Value::text(s)),
            Err(e) => Err(J2Err::runtime(format!("async.read_file: {}", e))),
        }
    })
}

pub fn write_file(args: &[J2Value]) -> J2Result<J2Value> {
    let path = match args.get(0) {
        Some(J2Value::Text(t)) => (**t).clone(),
        _ => return Err(J2Err::type_err("async.write_file: arg 0 must be text")),
    };
    let content = match args.get(1) {
        Some(J2Value::Text(t)) => (**t).clone(),
        _ => return Err(J2Err::type_err("async.write_file: arg 1 must be text")),
    };
    rt().block_on(async move {
        match tokio::fs::write(&path, content).await {
            Ok(()) => Ok(J2Value::Null),
            Err(e) => Err(J2Err::runtime(format!("async.write_file: {}", e))),
        }
    })
}

pub fn install(env: &mut std::collections::HashMap<String, J2Value>) {
    let mut m = std::collections::HashMap::<String, J2Value>::new();
    macro_rules! ins { ($n:expr, $f:path) => {
        m.insert($n.to_string(), J2Value::Builtin(std::sync::Arc::new($f), $n));
    } }
    ins!("parallel", parallel);
    ins!("sleep_ms", sleep_ms);
    ins!("http_get", http_get);
    ins!("read_file", read_file);
    ins!("write_file", write_file);
    env.insert("async".into(), J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m))));
}
