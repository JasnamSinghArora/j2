// date module, calendar dates and durations

use crate::value::J2Value;
use crate::error::{J2Err, J2Result};
use chrono::{Local, Utc, DateTime, Datelike, Timelike, NaiveDateTime, TimeZone};

fn date_map<Tz: TimeZone>(dt: &DateTime<Tz>) -> J2Value
where Tz::Offset: std::fmt::Display {
    let mut m = std::collections::HashMap::<String, J2Value>::new();
    m.insert("epoch_secs".into(), J2Value::Int(dt.timestamp()));
    m.insert("epoch_millis".into(), J2Value::Int(dt.timestamp_millis()));
    m.insert("year".into(), J2Value::Int(dt.year() as i64));
    m.insert("month".into(), J2Value::Int(dt.month() as i64));
    m.insert("day".into(), J2Value::Int(dt.day() as i64));
    m.insert("hour".into(), J2Value::Int(dt.hour() as i64));
    m.insert("minute".into(), J2Value::Int(dt.minute() as i64));
    m.insert("second".into(), J2Value::Int(dt.second() as i64));
    m.insert("weekday".into(), J2Value::Int(dt.weekday().num_days_from_monday() as i64));
    m.insert("iso".into(), J2Value::text(dt.to_rfc3339()));
    J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m)))
}

pub fn now(_args: &[J2Value]) -> J2Result<J2Value> { Ok(date_map(&Local::now())) }
pub fn now_utc(_args: &[J2Value]) -> J2Result<J2Value> { Ok(date_map(&Utc::now())) }

pub fn from_epoch(args: &[J2Value]) -> J2Result<J2Value> {
    let secs = args.get(0).and_then(|v| v.as_num_i64().ok())
        .ok_or_else(|| J2Err::type_err("date.from_epoch: arg 0 must be int"))?;
    let dt = chrono::DateTime::<Utc>::from_timestamp(secs, 0)
        .ok_or_else(|| J2Err::value("date.from_epoch: out of range"))?;
    Ok(date_map(&dt))
}

pub fn format(args: &[J2Value]) -> J2Result<J2Value> {
    // Accept a date map
    let m = match args.get(0) {
        Some(J2Value::Map(m)) => m.clone(),
        _ => return Err(J2Err::type_err("date.format: arg 0 must be a date map")),
    };
    let fmt = match args.get(1) {
        Some(J2Value::Text(t)) => (**t).clone(),
        _ => return Err(J2Err::type_err("date.format: arg 1 must be text")),
    };
    let secs = {
        let m = m.lock().unwrap_or_else(|p| p.into_inner());
        m.get("epoch_secs").and_then(|v| v.as_num_i64().ok())
            .ok_or_else(|| J2Err::value("date.format: missing epoch_secs"))?
    };
    let dt = chrono::DateTime::<Utc>::from_timestamp(secs, 0)
        .ok_or_else(|| J2Err::value("date.format: out of range"))?;
    Ok(J2Value::text(dt.format(&fmt).to_string()))
}

pub fn parse(args: &[J2Value]) -> J2Result<J2Value> {
    let s = match args.get(0) {
        Some(J2Value::Text(t)) => (**t).clone(),
        _ => return Err(J2Err::type_err("date.parse: arg 0 must be text")),
    };
    let fmt = match args.get(1) {
        Some(J2Value::Text(t)) => (**t).clone(),
        _ => return Err(J2Err::type_err("date.parse: arg 1 must be text")),
    };
    let ndt = NaiveDateTime::parse_from_str(&s, &fmt).map_err(|e| J2Err::value(format!("date.parse: {}", e)))?;
    let dt = Utc.from_utc_datetime(&ndt);
    Ok(date_map(&dt))
}

pub fn add_seconds(args: &[J2Value]) -> J2Result<J2Value> {
    let m = match args.get(0) {
        Some(J2Value::Map(m)) => m.clone(),
        _ => return Err(J2Err::type_err("date.add_seconds: arg 0 must be a date map")),
    };
    let n = args.get(1).and_then(|v| v.as_num_i64().ok())
        .ok_or_else(|| J2Err::type_err("date.add_seconds: arg 1 must be int"))?;
    let secs = {
        let m = m.lock().unwrap_or_else(|p| p.into_inner());
        m.get("epoch_secs").and_then(|v| v.as_num_i64().ok())
            .ok_or_else(|| J2Err::value("date.add_seconds: missing epoch_secs"))?
    };
    let dt = chrono::DateTime::<Utc>::from_timestamp(secs + n, 0)
        .ok_or_else(|| J2Err::value("date.add_seconds: out of range"))?;
    Ok(date_map(&dt))
}

pub fn add_days(args: &[J2Value]) -> J2Result<J2Value> {
    let n = args.get(1).and_then(|v| v.as_num_i64().ok())
        .ok_or_else(|| J2Err::type_err("date.add_days: arg 1 must be int"))?;
    let new_args = [args[0].clone(), J2Value::Int(n * 86400)];
    add_seconds(&new_args)
}

pub fn diff_seconds(args: &[J2Value]) -> J2Result<J2Value> {
    let a = match args.get(0) {
        Some(J2Value::Map(m)) => { let m = m.lock().unwrap_or_else(|p| p.into_inner()); m.get("epoch_secs").and_then(|v| v.as_num_i64().ok()).unwrap_or(0) }
        _ => return Err(J2Err::type_err("date.diff_seconds: arg 0 must be a date map")),
    };
    let b = match args.get(1) {
        Some(J2Value::Map(m)) => { let m = m.lock().unwrap_or_else(|p| p.into_inner()); m.get("epoch_secs").and_then(|v| v.as_num_i64().ok()).unwrap_or(0) }
        _ => return Err(J2Err::type_err("date.diff_seconds: arg 1 must be a date map")),
    };
    Ok(J2Value::Int(a - b))
}

pub fn install(env: &mut std::collections::HashMap<String, J2Value>) {
    let mut m = std::collections::HashMap::<String, J2Value>::new();
    macro_rules! ins { ($n:expr, $f:path) => {
        m.insert($n.to_string(), J2Value::Builtin(std::sync::Arc::new($f), $n));
    } }
    ins!("now", now);
    ins!("now_utc", now_utc);
    ins!("from_epoch", from_epoch);
    ins!("format", format);
    ins!("parse", parse);
    ins!("add_seconds", add_seconds);
    ins!("add_days", add_days);
    ins!("diff_seconds", diff_seconds);
    env.insert("date".into(), J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m))));
}
