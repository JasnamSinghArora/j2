// http module, minimal client over `ureq`

use crate::value::J2Value;
use crate::error::{J2Err, J2Result};

fn arg_text(args: &[J2Value], i: usize, who: &str) -> J2Result<String> {
    match args.get(i) {
        Some(J2Value::Text(t)) => Ok((**t).clone()),
        Some(other) => Err(J2Err::type_err(format!("{}: arg {} expected text, got {}", who, i, other.type_name()))),
        None => Err(J2Err::type_err(format!("{}: missing arg {}", who, i))),
    }
}

fn response_map(resp: ureq::Response) -> J2Value {
    let status = resp.status() as i64;
    let mut hmap = std::collections::HashMap::<String, J2Value>::new();
    for h in resp.headers_names() {
        if let Some(v) = resp.header(&h) {
            hmap.insert(h.to_string(), J2Value::text(v.to_string()));
        }
    }
    let body = resp.into_string().unwrap_or_default();
    let mut m = std::collections::HashMap::<String, J2Value>::new();
    m.insert("status".into(), J2Value::Int(status));
    m.insert("body".into(), J2Value::text(body));
    m.insert("headers".into(), J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(hmap))));
    J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m)))
}

pub fn get(args: &[J2Value]) -> J2Result<J2Value> {
    let url = arg_text(args, 0, "http.get")?;
    crate::require_cap("J2_ALLOW_NET", "http.get")?;
    crate::check_url_allowed(&url, "http.get")?;
    match ureq::get(&url).call() {
        Ok(r) => Ok(response_map(r)),
        Err(ureq::Error::Status(_, r)) => Ok(response_map(r)),
        Err(e) => Err(J2Err::runtime(format!("http.get: {}", e))),
    }
}

pub fn post(args: &[J2Value]) -> J2Result<J2Value> {
    let url = arg_text(args, 0, "http.post")?;
    let body = arg_text(args, 1, "http.post")?;
    crate::require_cap("J2_ALLOW_NET", "http.post")?;
    crate::check_url_allowed(&url, "http.post")?;
    match ureq::post(&url).send_string(&body) {
        Ok(r) => Ok(response_map(r)),
        Err(ureq::Error::Status(_, r)) => Ok(response_map(r)),
        Err(e) => Err(J2Err::runtime(format!("http.post: {}", e))),
    }
}

pub fn post_json(args: &[J2Value]) -> J2Result<J2Value> {
    let url = arg_text(args, 0, "http.post_json")?;
    crate::require_cap("J2_ALLOW_NET", "http.post_json")?;
    crate::check_url_allowed(&url, "http.post_json")?;
    let body_val = args.get(1).ok_or_else(|| J2Err::type_err("http.post_json: missing body"))?;
    // Reuse json.stringify
    let body_text = super::json::stringify(&[body_val.clone()])?;
    let body = match body_text { J2Value::Text(t) => (*t).clone(), _ => unreachable!() };
    match ureq::post(&url).set("Content-Type", "application/json").send_string(&body) {
        Ok(r) => Ok(response_map(r)),
        Err(ureq::Error::Status(_, r)) => Ok(response_map(r)),
        Err(e) => Err(J2Err::runtime(format!("http.post_json: {}", e))),
    }
}

pub fn install(env: &mut std::collections::HashMap<String, J2Value>) {
    let mut m = std::collections::HashMap::<String, J2Value>::new();
    macro_rules! ins { ($n:expr, $f:path) => {
        m.insert($n.to_string(), J2Value::Builtin(std::sync::Arc::new($f), $n));
    } }
    ins!("get", get);
    ins!("post", post);
    ins!("post_json", post_json);
    env.insert("http".into(), J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m))));
}
