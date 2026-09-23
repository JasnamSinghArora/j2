// J `fs` module - filesystem operations

use crate::value::J2Value;
use crate::error::{J2Err, J2Result};
use std::fs;
use std::path::Path;

fn arg_text(args: &[J2Value], i: usize, who: &str) -> J2Result<String> {
    match args.get(i) {
        Some(J2Value::Text(t)) => Ok((**t).clone()),
        Some(other) => Err(J2Err::type_err(format!("{}: arg {} expected text, got {}", who, i, other.type_name()))),
        None => Err(J2Err::type_err(format!("{}: missing arg {}", who, i))),
    }
}

fn map_io(e: std::io::Error, who: &str) -> J2Err {
    J2Err::runtime(format!("{}: {}", who, e))
}

pub fn read_file(args: &[J2Value]) -> J2Result<J2Value> {
    let path = arg_text(args, 0, "fs.read_file")?;
    fs::read_to_string(&path).map(J2Value::text).map_err(|e| map_io(e, "fs.read_file"))
}

pub fn write_file(args: &[J2Value]) -> J2Result<J2Value> {
    let path = arg_text(args, 0, "fs.write_file")?;
    let content = arg_text(args, 1, "fs.write_file")?;
    fs::write(&path, content).map(|_| J2Value::Null).map_err(|e| map_io(e, "fs.write_file"))
}

pub fn append_file(args: &[J2Value]) -> J2Result<J2Value> {
    use std::io::Write;
    let path = arg_text(args, 0, "fs.append_file")?;
    let content = arg_text(args, 1, "fs.append_file")?;
    let mut f = fs::OpenOptions::new().append(true).create(true).open(&path)
        .map_err(|e| map_io(e, "fs.append_file"))?;
    f.write_all(content.as_bytes()).map_err(|e| map_io(e, "fs.append_file"))?;
    Ok(J2Value::Null)
}

pub fn read_bytes(args: &[J2Value]) -> J2Result<J2Value> {
    let path = arg_text(args, 0, "fs.read_bytes")?;
    let bytes = fs::read(&path).map_err(|e| map_io(e, "fs.read_bytes"))?;
    Ok(J2Value::seq(bytes.into_iter().map(|b| J2Value::Int(b as i64)).collect()))
}

pub fn write_bytes(args: &[J2Value]) -> J2Result<J2Value> {
    let path = arg_text(args, 0, "fs.write_bytes")?;
    let bytes = match args.get(1) {
        Some(J2Value::Seq(s)) => {
            let s = s.lock().unwrap_or_else(|p| p.into_inner());
            let mut out = Vec::with_capacity(s.items.len());
            for v in &s.items {
                let n = v.as_num_i64().map_err(|_| J2Err::type_err("fs.write_bytes: seq must contain ints"))?;
                if !(0..=255).contains(&n) { return Err(J2Err::value("fs.write_bytes: byte out of range")); }
                out.push(n as u8);
            }
            out
        }
        Some(J2Value::SeqF64(s)) => {
            let s = s.lock().unwrap_or_else(|p| p.into_inner());
            let mut out = Vec::with_capacity(s.len());
            for &x in s.iter() {
                let n = J2Value::Float(x).as_num_i64().map_err(|_| J2Err::type_err("fs.write_bytes: seq must contain ints"))?;
                if !(0..=255).contains(&n) { return Err(J2Err::value("fs.write_bytes: byte out of range")); }
                out.push(n as u8);
            }
            out
        }
        _ => return Err(J2Err::type_err("fs.write_bytes: arg 1 must be seq<int>")),
    };
    fs::write(&path, bytes).map(|_| J2Value::Null).map_err(|e| map_io(e, "fs.write_bytes"))
}

pub fn exists(args: &[J2Value]) -> J2Result<J2Value> {
    let path = arg_text(args, 0, "fs.exists")?;
    Ok(J2Value::Bool(Path::new(&path).exists()))
}

pub fn is_dir(args: &[J2Value]) -> J2Result<J2Value> {
    let path = arg_text(args, 0, "fs.is_dir")?;
    Ok(J2Value::Bool(Path::new(&path).is_dir()))
}

pub fn is_file(args: &[J2Value]) -> J2Result<J2Value> {
    let path = arg_text(args, 0, "fs.is_file")?;
    Ok(J2Value::Bool(Path::new(&path).is_file()))
}

pub fn list_dir(args: &[J2Value]) -> J2Result<J2Value> {
    let path = arg_text(args, 0, "fs.list_dir")?;
    let entries = fs::read_dir(&path).map_err(|e| map_io(e, "fs.list_dir"))?;
    let mut out = Vec::new();
    for e in entries {
        let e = e.map_err(|e| map_io(e, "fs.list_dir"))?;
        out.push(J2Value::text(e.file_name().to_string_lossy().into_owned()));
    }
    Ok(J2Value::seq(out))
}

pub fn remove(args: &[J2Value]) -> J2Result<J2Value> {
    let path = arg_text(args, 0, "fs.remove")?;
    fs::remove_file(&path).map(|_| J2Value::Null).map_err(|e| map_io(e, "fs.remove"))
}

pub fn remove_dir(args: &[J2Value]) -> J2Result<J2Value> {
    let path = arg_text(args, 0, "fs.remove_dir")?;
    fs::remove_dir_all(&path).map(|_| J2Value::Null).map_err(|e| map_io(e, "fs.remove_dir"))
}

pub fn mkdir(args: &[J2Value]) -> J2Result<J2Value> {
    let path = arg_text(args, 0, "fs.mkdir")?;
    fs::create_dir(&path).map(|_| J2Value::Null).map_err(|e| map_io(e, "fs.mkdir"))
}

pub fn mkdir_p(args: &[J2Value]) -> J2Result<J2Value> {
    let path = arg_text(args, 0, "fs.mkdir_p")?;
    fs::create_dir_all(&path).map(|_| J2Value::Null).map_err(|e| map_io(e, "fs.mkdir_p"))
}

pub fn rename(args: &[J2Value]) -> J2Result<J2Value> {
    let src = arg_text(args, 0, "fs.rename")?;
    let dst = arg_text(args, 1, "fs.rename")?;
    fs::rename(&src, &dst).map(|_| J2Value::Null).map_err(|e| map_io(e, "fs.rename"))
}

pub fn copy(args: &[J2Value]) -> J2Result<J2Value> {
    let src = arg_text(args, 0, "fs.copy")?;
    let dst = arg_text(args, 1, "fs.copy")?;
    fs::copy(&src, &dst).map(|n| J2Value::Int(n as i64)).map_err(|e| map_io(e, "fs.copy"))
}

pub fn read_lines(args: &[J2Value]) -> J2Result<J2Value> {
    let path = arg_text(args, 0, "fs.read_lines")?;
    let content = fs::read_to_string(&path).map_err(|e| map_io(e, "fs.read_lines"))?;
    Ok(J2Value::seq(content.lines().map(|l| J2Value::text(l.to_string())).collect()))
}

pub fn metadata(args: &[J2Value]) -> J2Result<J2Value> {
    let path = arg_text(args, 0, "fs.metadata")?;
    let md = fs::metadata(&path).map_err(|e| map_io(e, "fs.metadata"))?;
    let mut m = std::collections::HashMap::<String, J2Value>::new();
    m.insert("size".into(), J2Value::Int(md.len() as i64));
    m.insert("is_dir".into(), J2Value::Bool(md.is_dir()));
    m.insert("is_file".into(), J2Value::Bool(md.is_file()));
    if let Ok(modified) = md.modified() {
        if let Ok(d) = modified.duration_since(std::time::SystemTime::UNIX_EPOCH) {
            m.insert("modified_epoch".into(), J2Value::Int(d.as_secs() as i64));
        }
    }
    Ok(J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m))))
}

pub fn install(env: &mut std::collections::HashMap<String, J2Value>) {
    let mut m = std::collections::HashMap::<String, J2Value>::new();
    // fs builtins gated on `J2_ALLOW_FS`, deny-by-default
    macro_rules! ins { ($n:expr, $f:path) => {
        m.insert($n.to_string(), J2Value::Builtin(std::sync::Arc::new(|args: &[J2Value]| {
            crate::require_cap("J2_ALLOW_FS", $n)?;
            $f(args)
        }), $n));
    } }
    ins!("read_file", read_file);
    ins!("write_file", write_file);
    ins!("append_file", append_file);
    ins!("read_bytes", read_bytes);
    ins!("write_bytes", write_bytes);
    ins!("exists", exists);
    ins!("is_dir", is_dir);
    ins!("is_file", is_file);
    ins!("list_dir", list_dir);
    ins!("remove", remove);
    ins!("remove_dir", remove_dir);
    ins!("mkdir", mkdir);
    ins!("mkdir_p", mkdir_p);
    ins!("rename", rename);
    ins!("copy", copy);
    ins!("read_lines", read_lines);
    ins!("metadata", metadata);
    env.insert("fs".into(), J2Value::Map(std::sync::Arc::new(std::sync::Mutex::new(m))));
}
