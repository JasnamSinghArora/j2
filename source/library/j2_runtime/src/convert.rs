// dynamic-native boundary; unbox once, rebox &mut seqs

use crate::error::{J2Err, J2Result};
use crate::value::J2Value;

#[inline]
pub fn unbox_f64(v: &J2Value) -> J2Result<f64> {
    v.as_num_f64()
}

#[inline]
pub fn unbox_i64(v: &J2Value) -> J2Result<i64> {
    v.as_num_i64()
}

#[inline]
pub fn unbox_bool(v: &J2Value) -> J2Result<bool> {
    match v {
        J2Value::Bool(b) => Ok(*b),
        other => Ok(other.truthy()),
    }
}

#[inline]
pub fn unbox_text(v: &J2Value) -> J2Result<String> {
    v.as_text()
}

/// Copy a numeric seq into packed `Vec<f64>`
pub fn unbox_seq_f64(v: &J2Value) -> J2Result<Vec<f64>> {
    match v {
        // packed seq clones contiguously, no per-element coercion
        J2Value::SeqF64(s) => Ok(s.lock().unwrap_or_else(|p| p.into_inner()).clone()),
        J2Value::Seq(s) => {
            let s = s.lock().unwrap_or_else(|p| p.into_inner());
            let mut out = Vec::with_capacity(s.items.len());
            for it in &s.items {
                out.push(it.as_num_f64()?);
            }
            Ok(out)
        }
        _ => Err(J2Err::type_err(format!(
            "expected seq<float>, got {}",
            v.type_name()
        ))),
    }
}

/// Copy a `seq<text>` out into a `Vec<String>`.
pub fn unbox_seq_text(v: &J2Value) -> J2Result<Vec<String>> {
    match v {
        J2Value::Seq(s) => {
            let s = s.lock().unwrap_or_else(|p| p.into_inner());
            let mut out = Vec::with_capacity(s.items.len());
            for it in &s.items {
                out.push(it.as_text()?);
            }
            Ok(out)
        }
        _ => Err(J2Err::type_err(format!("expected seq<text>, got {}", v.type_name()))),
    }
}

pub fn rebox_seq_text(orig: &J2Value, data: Vec<String>) -> J2Result<()> {
    match orig {
        J2Value::Seq(s) => {
            let mut s = s.lock().unwrap_or_else(|p| p.into_inner());
            s.items = data.into_iter().map(J2Value::text).collect();
            Ok(())
        }
        _ => Err(J2Err::type_err(format!("cannot write back into non-seq {}", orig.type_name()))),
    }
}

pub fn box_seq_text(v: Vec<String>) -> J2Value {
    J2Value::seq(v.into_iter().map(J2Value::text).collect())
}

/// Copy a `seq<int>` into packed `Vec<i64>`
pub fn unbox_seq_i64(v: &J2Value) -> J2Result<Vec<i64>> {
    match v {
        // packed floats as int via `as_num_i64` rule
        J2Value::SeqF64(s) => {
            let s = s.lock().unwrap_or_else(|p| p.into_inner());
            let mut out = Vec::with_capacity(s.len());
            for &x in s.iter() {
                if x.fract() == 0.0 && x.is_finite() {
                    out.push(x as i64);
                } else {
                    return Err(J2Err::type_err("expected integer, got non-integer float"));
                }
            }
            Ok(out)
        }
        J2Value::Seq(s) => {
            let s = s.lock().unwrap_or_else(|p| p.into_inner());
            let mut out = Vec::with_capacity(s.items.len());
            for it in &s.items {
                out.push(it.as_num_i64()?);
            }
            Ok(out)
        }
        _ => Err(J2Err::type_err(format!(
            "expected seq<int>, got {}",
            v.type_name()
        ))),
    }
}

/// Write packed `Vec<f64>` back, preserving reference semantics
pub fn rebox_seq_f64(orig: &J2Value, data: Vec<f64>) -> J2Result<()> {
    match orig {
        // packed seq, move buffer back in, O(1)
        J2Value::SeqF64(s) => {
            *s.lock().unwrap_or_else(|p| p.into_inner()) = data;
            Ok(())
        }
        J2Value::Seq(s) => {
            let mut s = s.lock().unwrap_or_else(|p| p.into_inner());
            s.items = data.into_iter().map(J2Value::Float).collect();
            Ok(())
        }
        _ => Err(J2Err::type_err(format!(
            "cannot write back into non-seq {}",
            orig.type_name()
        ))),
    }
}

/// Write packed `Vec<i64>` back into original seq
pub fn rebox_seq_i64(orig: &J2Value, data: Vec<i64>) -> J2Result<()> {
    match orig {
        // int writeback into packed seq keeps f64
        J2Value::SeqF64(s) => {
            *s.lock().unwrap_or_else(|p| p.into_inner()) = data.into_iter().map(|x| x as f64).collect();
            Ok(())
        }
        J2Value::Seq(s) => {
            let mut s = s.lock().unwrap_or_else(|p| p.into_inner());
            s.items = data.into_iter().map(J2Value::Int).collect();
            Ok(())
        }
        _ => Err(J2Err::type_err(format!(
            "cannot write back into non-seq {}",
            orig.type_name()
        ))),
    }
}

#[inline]
pub fn box_f64(x: f64) -> J2Value {
    J2Value::Float(x)
}

#[inline]
pub fn box_i64(x: i64) -> J2Value {
    J2Value::Int(x)
}

#[inline]
pub fn box_bool(b: bool) -> J2Value {
    J2Value::Bool(b)
}

#[inline]
pub fn box_text(s: String) -> J2Value {
    J2Value::text(s)
}

pub fn box_seq_f64(v: Vec<f64>) -> J2Value {
    // O(1) wrap as packed seq, no boxing
    J2Value::seq_f64(v)
}

pub fn box_seq_i64(v: Vec<i64>) -> J2Value {
    J2Value::seq(v.into_iter().map(J2Value::Int).collect())
}

// text-keyed maps unbox to HashMap<String, f64|i64>

use std::collections::HashMap as StdHashMap;

pub fn unbox_map_f64(v: &J2Value) -> J2Result<StdHashMap<String, f64>> {
    match v {
        J2Value::Map(m) => {
            let m = m.lock().unwrap_or_else(|p| p.into_inner());
            let mut out = StdHashMap::with_capacity(m.len());
            for (k, val) in m.iter() {
                out.insert(k.clone(), val.as_num_f64()?);
            }
            Ok(out)
        }
        _ => Err(J2Err::type_err(format!("expected map<text,float>, got {}", v.type_name()))),
    }
}

pub fn unbox_map_i64(v: &J2Value) -> J2Result<StdHashMap<String, i64>> {
    match v {
        J2Value::Map(m) => {
            let m = m.lock().unwrap_or_else(|p| p.into_inner());
            let mut out = StdHashMap::with_capacity(m.len());
            for (k, val) in m.iter() {
                out.insert(k.clone(), val.as_num_i64()?);
            }
            Ok(out)
        }
        _ => Err(J2Err::type_err(format!("expected map<text,int>, got {}", v.type_name()))),
    }
}

pub fn rebox_map_f64(orig: &J2Value, data: StdHashMap<String, f64>) -> J2Result<()> {
    match orig {
        J2Value::Map(m) => {
            let mut m = m.lock().unwrap_or_else(|p| p.into_inner());
            *m = data.into_iter().map(|(k, v)| (k, J2Value::Float(v))).collect();
            Ok(())
        }
        _ => Err(J2Err::type_err(format!("cannot write back into non-map {}", orig.type_name()))),
    }
}

pub fn rebox_map_i64(orig: &J2Value, data: StdHashMap<String, i64>) -> J2Result<()> {
    match orig {
        J2Value::Map(m) => {
            let mut m = m.lock().unwrap_or_else(|p| p.into_inner());
            *m = data.into_iter().map(|(k, v)| (k, J2Value::Int(v))).collect();
            Ok(())
        }
        _ => Err(J2Err::type_err(format!("cannot write back into non-map {}", orig.type_name()))),
    }
}

pub fn box_map_f64(m: StdHashMap<String, f64>) -> J2Value {
    J2Value::map(m.into_iter().map(|(k, v)| (k, J2Value::Float(v))).collect())
}

pub fn box_map_i64(m: StdHashMap<String, i64>) -> J2Value {
    J2Value::map(m.into_iter().map(|(k, v)| (k, J2Value::Int(v))).collect())
}
