// lazy stream; infinite-flow policy enforced in builtins.rs

use crate::value::J2Value;
use crate::error::{J2Err, J2Result};

pub enum J2Flow {
    /// `start..end`, ascending or descending
    Range { start: i64, end: i64, step: i64, current: i64, done: bool },
    /// `start..` - open-ended, monotonically increasing
    OpenRange { current: i64 },
    /// Iterator-backed flow; must be Send for J2Value
    Iter(Box<dyn Iterator<Item = J2Result<J2Value>> + Send>),
}

impl J2Flow {
    pub fn range(start: i64, end: i64) -> Self {
        let step = if start <= end { 1 } else { -1 };
        Self::Range { start, end, step, current: start, done: false }
    }
    pub fn open_range(start: i64) -> Self {
        Self::OpenRange { current: start }
    }
    pub fn is_infinite(&self) -> bool {
        matches!(self, Self::OpenRange { .. })
    }
    /// Try to draw the next value.
    pub fn next(&mut self) -> J2Result<Option<J2Value>> {
        match self {
            Self::Range { end, step, current, done, .. } => {
                if *done { return Ok(None); }
                let v = *current;
                // inclusive; emit current, advance, stop after `end`
                let at_end = if *step > 0 { v > *end } else { v < *end };
                if at_end {
                    *done = true;
                    return Ok(None);
                }
                *current += *step;
                Ok(Some(J2Value::Int(v)))
            }
            Self::OpenRange { current } => {
                let v = *current;
                *current = current.checked_add(1).ok_or_else(|| J2Err::overflow("open flow overflow"))?;
                Ok(Some(J2Value::Int(v)))
            }
            Self::Iter(it) => match it.next() {
                Some(r) => r.map(Some),
                None => Ok(None),
            },
        }
    }
    /// Drain into a Vec
    pub fn collect_all(mut self) -> J2Result<Vec<J2Value>> {
        let mut out = Vec::new();
        while let Some(v) = self.next()? {
            out.push(v);
        }
        Ok(out)
    }
    /// Drain through first element satisfying `pred`, inclusive
    pub fn collect_until<F: Fn(&J2Value) -> J2Result<bool>>(mut self, pred: F) -> J2Result<Vec<J2Value>> {
        let mut out = Vec::new();
        loop {
            match self.next()? {
                Some(v) => {
                    let stop = pred(&v)?;
                    out.push(v);
                    if stop { return Ok(out); }
                }
                None => return Ok(out),
            }
        }
    }
}
