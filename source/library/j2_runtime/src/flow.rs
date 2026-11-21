// J2Flow: lazy stream. Generates values on demand.
//
// Implementations cover the forms the spec describes:
//   - Range: `start..end` (eager-bound but lazy) and `start..` (open-ended)
//   - Mapped:    a flow obtained by `>>` over an underlying flow/seq
//   - Filtered:  a flow obtained by `?` (cond filter) over an underlying flow/seq
//
// Per spec §built-in types, a flow is consumed when:
//   - iterated in a for loop
//   - passed to collect() (with `until` clause)
//   - passed to a reducer: sum, max, min, count
//
// `collect` and any unbounded reducer on an open-ended flow must raise
// InfiniteFlowError - that policy is enforced at the call site (builtins.rs).

use crate::value::J2Value;
use crate::error::{J2Err, J2Result};

pub enum J2Flow {
    /// `start..end`, ascending or descending. Inclusive on both ends per spec.
    Range { start: i64, end: i64, step: i64, current: i64, done: bool },
    /// `start..`: open-ended, monotonically increasing.
    OpenRange { current: i64 },
    /// A flow built from a backend iterator (used by map / filter / etc.).
    /// The iterator must be Send so J2Value stays Send + Sync.
    Iter(Box<dyn Iterator<Item = J2Result<J2Value>> + Send>),
}

impl J2Flow {
    /// An exhausted finite flow, suitable for replacing a consumed stream.
    /// A reversed range is not empty: J2 ranges descend when start > end.
    pub fn empty() -> Self {
        Self::Range { start: 0, end: 0, step: 1, current: 0, done: true }
    }
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
                // Inclusive: emit current, then advance. Stop after emitting `end`.
                let at_end = if *step > 0 { v > *end } else { v < *end };
                if at_end {
                    *done = true;
                    return Ok(None);
                }
                // The endpoint is a value, not a request to advance again.
                // Advancing past MAX/MIN would panic in debug builds and
                // wrap into an effectively unbounded range in release.
                if v == *end {
                    *done = true;
                } else {
                    *current += *step;
                }
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
    /// Drain into a Vec. Caller must ensure the flow is finite (e.g. via `until`).
    pub fn collect_all(mut self) -> J2Result<Vec<J2Value>> {
        let mut out = Vec::new();
        while let Some(v) = self.next()? {
            out.push(v);
        }
        Ok(out)
    }
    /// Drain up to and including the first element satisfying `pred`.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn next_int(flow: &mut J2Flow) -> Option<i64> {
        flow.next().unwrap().map(|value| match value {
            J2Value::Int(n) => n,
            _ => panic!("range must yield integers"),
        })
    }

    #[test]
    fn empty_flow_is_finite_and_stays_exhausted() {
        let mut flow = J2Flow::empty();
        assert!(!flow.is_infinite());
        assert_eq!(next_int(&mut flow), None);
        assert_eq!(next_int(&mut flow), None);
        assert!(flow.collect_all().unwrap().is_empty());
    }

    #[test]
    fn closed_ranges_stop_at_integer_extremes() {
        let cases = [
            (i64::MAX, i64::MAX, vec![i64::MAX]),
            (i64::MAX - 1, i64::MAX, vec![i64::MAX - 1, i64::MAX]),
            (i64::MIN, i64::MIN, vec![i64::MIN]),
            (i64::MIN + 1, i64::MIN, vec![i64::MIN + 1, i64::MIN]),
        ];
        for (start, end, expected) in cases {
            let mut flow = J2Flow::range(start, end);
            for n in expected {
                assert_eq!(next_int(&mut flow), Some(n));
            }
            assert_eq!(next_int(&mut flow), None);
            assert_eq!(next_int(&mut flow), None);
        }
    }

    #[test]
    fn ranges_are_inclusive_in_both_directions() {
        for (start, end, expected) in [
            (-1, 1, vec![-1, 0, 1]),
            (1, -1, vec![1, 0, -1]),
            (0, 0, vec![0]),
        ] {
            let values = J2Flow::range(start, end).collect_all().unwrap();
            let actual: Vec<_> = values.into_iter().map(|v| match v {
                J2Value::Int(n) => n,
                _ => panic!("range must yield integers"),
            }).collect();
            assert_eq!(actual, expected);
        }
    }
}
