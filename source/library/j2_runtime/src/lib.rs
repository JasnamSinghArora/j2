// runtime types and built-ins for transpiled J

pub mod value;
pub mod error;
pub mod seq;
pub mod flow;
pub mod builtins;
pub mod modules;
pub mod parallel;
pub mod convert;

pub use error::{J2Err, J2ErrKind};
pub use value::J2Value;

/// Deny-by-default capability gate via driver-set env var
pub fn require_cap(env_key: &str, who: &str) -> error::J2Result<()> {
    if std::env::var(env_key).map(|v| v == "1").unwrap_or(false) {
        return Ok(());
    }
    let flag = match env_key {
        "J2_ALLOW_FS" => "--allow-fs",
        "J2_ALLOW_PROC" => "--allow-proc",
        "J2_ALLOW_NET" => "--allow-net",
        _ => "--allow-all",
    };
    Err(error::J2Err::runtime(format!(
        "{}: capability denied by the deny-by-default sandbox, re-run with {} to grant it",
        who, flag
    )))
}

/// Best-effort SSRF guard, string-level, no DNS
pub fn check_url_allowed(url: &str, who: &str) -> error::J2Result<()> {
    if std::env::var("J2_ALLOW_LOCAL_NET").map(|v| v == "1").unwrap_or(false) {
        return Ok(());
    }
    let after_scheme = url.split("://").nth(1).unwrap_or(url);
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    // strip userinfo@ and :port / [ipv6]
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = host.trim_start_matches('[');
    let host_lc = host.split([']', ':']).next().unwrap_or(host).to_lowercase();
    let blocked = host_lc == "localhost"
        || host_lc == "::1"
        || host_lc.is_empty()
        || host_lc.starts_with("127.")
        || host_lc.starts_with("0.")
        || host_lc.starts_with("10.")
        || host_lc.starts_with("192.168.")
        || host_lc.starts_with("169.254.")
        || host_lc.starts_with("fc")
        || host_lc.starts_with("fd")
        || (host_lc.starts_with("172.")
            && host_lc.split('.').nth(1).and_then(|o| o.parse::<u8>().ok())
                .map(|o| (16..=31).contains(&o)).unwrap_or(false));
    if blocked {
        return Err(error::J2Err::runtime(format!(
            "{}: blocked request to private/loopback host {:?} (set J2_ALLOW_LOCAL_NET=1 to override)",
            who, host_lc
        )));
    }
    Ok(())
}

/// Prelude, every name transpiled J may reference
pub mod prelude {
    pub use crate::value::J2Value;
    pub use crate::error::{J2Err, J2ErrKind, J2Result};
    pub use crate::seq::J2Seq;
    pub use crate::flow::J2Flow;
    pub use crate::builtins as j2_builtins;
    pub use crate::modules::math as j2_math;
    pub use crate::modules::stats as j2_stats;
    pub use crate::modules::time as j2_time;
    pub use crate::modules::rand as j2_rand;
    pub use crate::modules::fs as j2_fs;
    pub use crate::modules::proc as j2_proc;
    pub use crate::modules::str_mod as j2_str;
    pub use crate::modules::regex_mod as j2_regex;
    pub use crate::modules::json as j2_json;
    pub use crate::modules::date as j2_date;
    pub use crate::modules::hash as j2_hash;
    pub use crate::modules::base64_mod as j2_base64;
    pub use crate::modules::hex as j2_hex;
    pub use crate::modules::http as j2_http;
    pub use crate::modules::sys as j2_sys;
    pub use crate::modules::bits as j2_bits;
    pub use crate::modules::async_mod as j2_async;
    pub use crate::parallel as j2_parallel;
    pub use crate::convert as j2_convert;
}
