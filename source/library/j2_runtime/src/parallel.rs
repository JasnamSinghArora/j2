//! Ordered parallel collection operations and checked integer reductions.
//!
//! The compiler calls the native helpers only after proving a callback has no
//! observable side effects. `Send + Sync` alone does not prove purity. Small
//! inputs, nested operations, and `J2_PARALLEL=0` use the serial path.

use std::cell::Cell;
use std::thread;

use crate::error::{J2Err, J2Result};
use crate::value::J2Value;

/// Threshold matching `library/std/src/parallel_runtime.rs:PARALLEL_REDUCE_THRESHOLD`.
const PAR_THRESHOLD: usize = 32_768;
const N_WORKERS: usize = 8;

thread_local! {
    static IN_WORKER: Cell<bool> = const { Cell::new(false) };
}

/// Restore the previous state even when a callback panics.
struct WorkerGuard(bool);

impl WorkerGuard {
    fn enter() -> Self {
        Self(IN_WORKER.with(|active| active.replace(true)))
    }
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        IN_WORKER.with(|active| active.set(self.0));
    }
}

fn worker_count(len: usize) -> usize {
    if len < PAR_THRESHOLD
        || IN_WORKER.with(Cell::get)
        || std::env::var_os("J2_PARALLEL").is_some_and(|value| value == "0")
    {
        return 1;
    }
    thread::available_parallelism().map_or(1, |n| n.get().min(N_WORKERS))
}

/// Borrow the input without copying it and join *every* worker before returning.
/// Keep outcomes in input-chunk order so callers can choose the first error or
/// original panic deterministically, without leaving background work running.
fn scoped_chunks<T, U, F>(items: &[T], workers: usize, f: &F) -> Vec<thread::Result<U>>
where
    T: Sync,
    U: Send,
    F: Fn(&[T]) -> U + Sync,
{
    debug_assert!(!items.is_empty() && workers > 0);
    let chunk_size = items.len().div_ceil(workers);
    thread::scope(|scope| {
        let handles: Vec<_> = items
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    let _guard = WorkerGuard::enter();
                    f(chunk)
                })
            })
            .collect();
        handles.into_iter().map(|handle| handle.join()).collect()
    })
}

/// Apply a compiler-proven pure callback to native elements, preserving order.
/// Captured immutable references may borrow the caller's stack; all workers are
/// joined before this function returns or propagates a callback's panic.
pub fn map_native<T, U, F>(items: &[T], f: F) -> Vec<U>
where
    T: Sync,
    U: Send,
    F: Fn(&T) -> U + Sync,
{
    let workers = worker_count(items.len());
    if workers == 1 {
        return items.iter().map(f).collect();
    }
    let chunks =
        scoped_chunks(items, workers, &|chunk: &[T]| chunk.iter().map(&f).collect::<Vec<U>>());
    let mut out = Vec::with_capacity(items.len());
    for chunk in chunks {
        out.extend(chunk.unwrap_or_else(|panic| std::panic::resume_unwind(panic)));
    }
    out
}

/// Keep elements selected by a compiler-proven pure predicate in input order.
pub fn filter_native<T, F>(items: &[T], pred: F) -> Vec<T>
where
    T: Clone + Send + Sync,
    F: Fn(&T) -> bool + Sync,
{
    let workers = worker_count(items.len());
    if workers == 1 {
        return items.iter().filter(|item| pred(item)).cloned().collect();
    }
    let chunks = scoped_chunks(items, workers, &|chunk: &[T]| {
        chunk.iter().filter(|item| pred(item)).cloned().collect::<Vec<T>>()
    });
    let mut out = Vec::new();
    for chunk in chunks {
        out.extend(chunk.unwrap_or_else(|panic| std::panic::resume_unwind(panic)));
    }
    out
}

/// Update independent indices using a compiler-proven pure callback.
///
/// Equivalent to `for i in start..end { dst[i] = f(i, dst[i]); }`: each
/// callback receives the index and its original value. It must not observe or
/// mutate any other destination element. Read-only captures may borrow the
/// caller's stack. A filtered-out iteration can return its original value.
///
/// Workers stage successful results and their first panic instead of mutating
/// `dst` directly. After every worker joins, results commit in index order up
/// to the earliest failing iteration, then its original panic is resumed. This
/// preserves partial writes as well as error order. Later speculative results
/// have no observable effects because the compiler has established purity.
/// The serial path avoids staging, and empty/reversed ranges do not index `dst`.
///
/// Reading the old destination value precedes `f`, including for out-of-bounds
/// indices. A compiler lowering an assignment that does not read its old value
/// must prove the destination bounds or retain its original serial error path.
pub fn update_native<T, F>(dst: &mut [T], start: usize, end: usize, f: F)
where
    T: Copy + Send + Sync,
    F: Fn(usize, T) -> T + Sync,
{
    if start >= end {
        return;
    }
    let valid_end = end.min(dst.len());
    let workers = worker_count(valid_end.saturating_sub(start));
    if workers == 1 {
        for index in start..end {
            let old = dst[index];
            dst[index] = f(index, old);
        }
        return;
    }

    // Borrow the original values while evaluating callbacks. No destination
    // element changes until all workers have finished reading this snapshot.
    let originals = &dst[start..valid_end];
    let chunk_size = originals.len().div_ceil(workers);
    let results = thread::scope(|scope| {
        let callback = &f;
        let handles: Vec<_> = originals
            .chunks(chunk_size)
            .enumerate()
            .map(|(chunk_index, chunk)| {
                scope.spawn(move || {
                    let _guard = WorkerGuard::enter();
                    let mut updates = Vec::with_capacity(chunk.len());
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let first = start + chunk_index * chunk_size;
                        for (offset, old) in chunk.iter().copied().enumerate() {
                            updates.push(callback(first + offset, old));
                        }
                    }));
                    (updates, outcome)
                })
            })
            .collect();
        handles.into_iter().map(|handle| handle.join()).collect::<Vec<_>>()
    });

    let mut index = start;
    for result in results {
        let (updates, outcome) = result.unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        for update in updates {
            dst[index] = update;
            index += 1;
        }
        if let Err(panic) = outcome {
            std::panic::resume_unwind(panic);
        }
    }
    if valid_end < end {
        // A sequential loop reaches this bounds failure only after committing
        // the valid prefix, and only if no earlier callback has already failed.
        let _ = dst[valid_end];
    }
}

/// Sum a nonempty numeric sequence. Integer parallelism preserves the exact
/// checked-add semantics of the serial path, including intermediate overflow.
/// Floating-point addition retains source order because it is not associative.
pub fn sum_seq(items: &[J2Value]) -> J2Result<J2Value> {
    if items.is_empty() {
        return Err(J2Err::value("sum requires a non-empty seq"));
    }
    if items.iter().all(|v| matches!(v, J2Value::Int(_))) {
        let workers = worker_count(items.len());
        if workers > 1 {
            return par_sum_i64(items, workers);
        }
        let mut sum: i64 = 0;
        for v in items {
            if let J2Value::Int(n) = v {
                sum = sum.checked_add(*n).ok_or_else(|| J2Err::overflow("sum overflow"))?;
            }
        }
        return Ok(J2Value::Int(sum));
    }
    if items.iter().all(|v| matches!(v, J2Value::Int(_) | J2Value::Float(_) | J2Value::Bool(_))) {
        let mut sum = 0.0;
        for v in items {
            sum += v.as_num_f64()?;
        }
        return Ok(J2Value::Float(sum));
    }
    Err(J2Err::type_err("sum requires a seq of numbers"))
}

fn par_sum_i64(items: &[J2Value], workers: usize) -> J2Result<J2Value> {
    let chunks = scoped_chunks(items, workers, &|chunk: &[J2Value]| {
        // i128 holds any prefix sum of an addressable slice of i64 values.
        // Checking chunk sums alone would miss overflow followed by cancellation
        // and could falsely reject a chunk whose incoming accumulator offsets it.
        let (mut sum, mut low, mut high) = (0i128, 0i128, 0i128);
        for value in chunk {
            if let J2Value::Int(n) = value {
                sum += i128::from(*n);
                low = low.min(sum);
                high = high.max(sum);
            }
        }
        (sum, low, high)
    });
    let mut total = 0i128;
    for result in chunks {
        let (sum, low, high) = result.map_err(|_| J2Err::runtime("worker panicked"))?;
        if total + low < i128::from(i64::MIN) || total + high > i128::from(i64::MAX) {
            return Err(J2Err::overflow("sum overflow"));
        }
        total += sum;
    }
    Ok(J2Value::Int(total as i64))
}

/// Apply a pure fallible callback, preserving result/error order. The caller
/// must establish purity; `Send + Sync` only establishes thread safety.
pub fn map_seq<F>(items: Vec<J2Value>, f: F) -> J2Result<Vec<J2Value>>
where
    F: Fn(&J2Value) -> J2Result<J2Value> + Send + Sync + 'static,
{
    let workers = worker_count(items.len());
    if workers == 1 {
        return items.iter().map(f).collect();
    }
    let chunks = scoped_chunks(&items, workers, &|chunk: &[J2Value]| {
        chunk.iter().map(&f).collect::<J2Result<Vec<_>>>()
    });
    let mut out = Vec::with_capacity(items.len());
    for result in chunks {
        out.extend(result.map_err(|_| J2Err::runtime("worker panicked"))??);
    }
    Ok(out)
}

/// Count elements selected by a pure fallible predicate. All workers complete
/// before the earliest chunk's error is returned.
pub fn filter_count<F>(items: &[J2Value], pred: F) -> J2Result<i64>
where
    F: Fn(&J2Value) -> J2Result<bool> + Send + Sync + 'static,
{
    let count_chunk = |chunk: &[J2Value]| -> J2Result<i64> {
        let mut count = 0;
        for value in chunk {
            if pred(value)? {
                count += 1;
            }
        }
        Ok(count)
    };
    let workers = worker_count(items.len());
    if workers == 1 {
        return count_chunk(items);
    }
    let chunks = scoped_chunks(items, workers, &count_chunk);
    let mut total = 0;
    for result in chunks {
        total += result.map_err(|_| J2Err::runtime("worker panicked"))??;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::J2ErrKind;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, Mutex};

    #[test]
    fn native_transforms_preserve_order_and_borrow_captures() {
        let values: Vec<_> = (0..PAR_THRESHOLD + 17).collect();
        let weights = [2, 3, 4];
        let mapped = map_native(&values, |n| n * weights[n % weights.len()]);
        assert_eq!(
            mapped,
            values.iter().map(|n| n * weights[n % weights.len()]).collect::<Vec<_>>()
        );
        assert_eq!(
            filter_native(&values, |n| n % 7 == 0),
            values.iter().copied().filter(|n| n % 7 == 0).collect::<Vec<_>>()
        );
        assert!(map_native::<i64, _, _>(&[], |n| n * 2).is_empty());
        assert!(filter_native::<i64, _>(&[], |_| true).is_empty());
    }

    #[test]
    fn native_transforms_use_workers_only_for_large_inputs() {
        let caller = thread::current().id();
        assert_eq!(map_native(&[1, 2], |_| thread::current().id()), vec![caller; 2]);
        let values = vec![0; PAR_THRESHOLD];
        let ids: HashSet<_> = map_native(&values, |_| thread::current().id()).into_iter().collect();
        if worker_count(values.len()) > 1 {
            assert!(ids.len() > 1);
            assert!(!ids.contains(&caller));
        } else {
            assert_eq!(ids, HashSet::from([caller]));
        }
    }

    #[test]
    fn nested_work_stays_on_its_existing_worker() {
        let nested = vec![1; PAR_THRESHOLD];
        let results = scoped_chunks(&[0, 1], 2, &|_: &[i32]| {
            let outer = thread::current().id();
            assert_eq!(worker_count(nested.len()), 1);
            assert!(map_native(&nested, |_| thread::current().id()).iter().all(|id| *id == outer));
            assert_eq!(
                filter_native(&nested, |_| thread::current().id() == outer).len(),
                nested.len()
            );
        });
        assert!(results.iter().all(Result::is_ok));
        assert!(!IN_WORKER.with(Cell::get));
    }

    #[test]
    fn all_workers_join_and_panics_remain_in_input_order() {
        let finished = AtomicUsize::new(0);
        let barrier = Barrier::new(3);
        let results = scoped_chunks(&[0, 1, 2], 3, &|chunk: &[i32]| {
            barrier.wait();
            finished.fetch_add(1, Ordering::SeqCst);
            match chunk[0] {
                0 => std::panic::panic_any("first"),
                1 => std::panic::panic_any("second"),
                _ => 42,
            }
        });
        assert_eq!(finished.load(Ordering::SeqCst), 3);
        assert_eq!(results[0].as_ref().unwrap_err().downcast_ref::<&str>(), Some(&"first"));
        assert_eq!(results[1].as_ref().unwrap_err().downcast_ref::<&str>(), Some(&"second"));
        assert_eq!(*results[2].as_ref().unwrap(), 42);
    }

    #[test]
    fn native_map_propagates_original_panic_after_joining_workers() {
        let values: Vec<_> = (0..PAR_THRESHOLD).collect();
        let workers = worker_count(values.len());
        let completed = AtomicUsize::new(0);
        let panic = std::panic::catch_unwind(|| {
            map_native(&values, |n| {
                if *n == 0 {
                    std::panic::panic_any("original callback panic");
                }
                completed.fetch_add(1, Ordering::SeqCst);
                *n
            })
        })
        .unwrap_err();
        assert_eq!(panic.downcast_ref::<&str>(), Some(&"original callback panic"));
        let expected = if workers == 1 { 0 } else { values.len() - values.len().div_ceil(workers) };
        assert_eq!(completed.load(Ordering::SeqCst), expected);
    }

    #[test]
    fn indexed_updates_preserve_indices_old_values_and_read_only_captures() {
        let mut values: Vec<_> = (0..PAR_THRESHOLD + 19).map(|n| n as i64).collect();
        let original = values.clone();
        let weights = [3, 5, 7];
        let end = values.len() - 4;
        update_native(&mut values, 5, end, |index, old| {
            if index % 2 == 0 { old * weights[index % weights.len()] + index as i64 } else { old }
        });
        let expected: Vec<_> = original.iter().enumerate().map(|(index, old)| {
            if (5..end).contains(&index) && index % 2 == 0 {
                old * weights[index % weights.len()] + index as i64
            } else {
                *old
            }
        }).collect();
        assert_eq!(values, expected);
    }

    #[test]
    fn indexed_updates_use_workers_only_for_large_inputs() {
        let caller = thread::current().id();
        let mut small = vec![caller; 2];
        update_native(&mut small, 0, 2, |_, _| thread::current().id());
        assert_eq!(small, vec![caller; 2]);
        let mut large = vec![caller; PAR_THRESHOLD];
        update_native(&mut large, 0, PAR_THRESHOLD, |_, _| thread::current().id());
        let ids: HashSet<_> = large.into_iter().collect();
        if worker_count(PAR_THRESHOLD) > 1 {
            assert!(ids.len() > 1);
            assert!(!ids.contains(&caller));
        } else {
            assert_eq!(ids, HashSet::from([caller]));
        }
    }

    #[test]
    fn indexed_updates_join_all_workers_and_commit_only_the_successful_prefix() {
        let mut values: Vec<_> = (0..PAR_THRESHOLD).collect();
        let workers = worker_count(values.len());
        let chunk_size = values.len().div_ceil(workers);
        let visited = AtomicUsize::new(0);
        let barrier = Barrier::new(workers);
        let failures = [17, PAR_THRESHOLD / 2 + 3];
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            update_native(&mut values, 0, PAR_THRESHOLD, |index, old| {
                if index % chunk_size == 0 { barrier.wait(); }
                visited.fetch_add(1, Ordering::SeqCst);
                if failures.contains(&index) { std::panic::panic_any(index); }
                old + 100
            });
        })).unwrap_err();
        assert_eq!(panic.downcast_ref::<usize>(), Some(&17));
        assert_eq!(&values[..17], &(100..117).collect::<Vec<_>>());
        assert_eq!(&values[17..], &(17..PAR_THRESHOLD).collect::<Vec<_>>());

        // Later chunks have finished (or reached their own panic) before the
        // caller receives the earliest panic. None can keep running detached.
        let expected: usize = (0..PAR_THRESHOLD).step_by(chunk_size).map(|first| {
            let last = (first + chunk_size).min(PAR_THRESHOLD);
            failures.iter().copied().filter(|n| (first..last).contains(n)).min()
                .map_or(last - first, |failure| failure - first + 1)
        }).sum();
        assert_eq!(visited.load(Ordering::SeqCst), expected);
    }

    #[test]
    fn indexed_updates_preserve_bounds_failures_and_empty_ranges() {
        let mut small = vec![10, 20, 30];
        update_native(&mut small, 9, 9, |_, _| panic!("empty range called callback"));
        update_native(&mut small, 9, 8, |_, _| panic!("reversed range called callback"));
        assert_eq!(small, vec![10, 20, 30]);
        let called = AtomicUsize::new(0);
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            update_native(&mut small, 7, 8, |_, old| {
                called.fetch_add(1, Ordering::SeqCst);
                old
            });
        })).is_err());
        assert_eq!(called.load(Ordering::SeqCst), 0);
        assert_eq!(small, vec![10, 20, 30]);

        let mut values = vec![3; PAR_THRESHOLD + 1];
        let end = values.len() + 1;
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            update_native(&mut values, 0, end, |index, old| old + index);
        })).is_err());
        assert_eq!(values, (3..PAR_THRESHOLD + 4).collect::<Vec<_>>());

        let mut values = vec![3; PAR_THRESHOLD + 1];
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            update_native(&mut values, 0, end, |index, old| {
                if index == 11 { std::panic::panic_any("callback before bounds"); }
                old + index
            });
        })).unwrap_err();
        assert_eq!(panic.downcast_ref::<&str>(), Some(&"callback before bounds"));
        assert_eq!(&values[..11], &(3..14).collect::<Vec<_>>());
        assert!(values[11..].iter().all(|value| *value == 3));
    }

    #[test]
    fn indexed_updates_do_not_spawn_nested_worker_groups() {
        let results = scoped_chunks(&[0, 1], 2, &|_: &[i32]| {
            let outer = thread::current().id();
            let mut nested = vec![outer; PAR_THRESHOLD];
            update_native(&mut nested, 0, PAR_THRESHOLD, |_, _| thread::current().id());
            assert!(nested.iter().all(|id| *id == outer));
        });
        assert!(results.iter().all(Result::is_ok));
        assert!(!IN_WORKER.with(Cell::get));
    }

    #[test]
    fn checked_sum_detects_intermediate_and_cross_chunk_overflow() {
        for values in [
            vec![J2Value::Int(i64::MAX), J2Value::Int(1), J2Value::Int(-1)],
            vec![J2Value::Int(i64::MIN), J2Value::Int(-1), J2Value::Int(1)],
        ] {
            assert_eq!(sum_seq(&values).unwrap_err().kind, J2ErrKind::OverflowError);
            let mut large = values;
            large.resize(PAR_THRESHOLD, J2Value::Int(0));
            assert_eq!(sum_seq(&large).unwrap_err().kind, J2ErrKind::OverflowError);
            assert_eq!(par_sum_i64(&large, 2).unwrap_err().kind, J2ErrKind::OverflowError);
        }
        let mut values = vec![J2Value::Int(0); PAR_THRESHOLD];
        values[0] = J2Value::Int(i64::MAX);
        values[PAR_THRESHOLD / 2] = J2Value::Int(1);
        assert_eq!(par_sum_i64(&values, 2).unwrap_err().kind, J2ErrKind::OverflowError);
    }

    #[test]
    fn checked_sum_accepts_chunks_offset_by_incoming_accumulator() {
        let mut values = vec![J2Value::Int(0); PAR_THRESHOLD];
        values[0] = J2Value::Int(-i64::MAX);
        values[PAR_THRESHOLD / 2] = J2Value::Int(i64::MAX);
        values[PAR_THRESHOLD / 2 + 1] = J2Value::Int(i64::MAX);
        assert!(matches!(par_sum_i64(&values, 2), Ok(J2Value::Int(i64::MAX))));
        assert!(matches!(sum_seq(&values), Ok(J2Value::Int(i64::MAX))));
    }

    #[test]
    fn float_sum_keeps_sequential_rounding_at_any_size() {
        let mut values = vec![J2Value::Float(0.0); PAR_THRESHOLD];
        values[0] = J2Value::Float(1e16);
        values[PAR_THRESHOLD / 2] = J2Value::Float(-1e16);
        values[PAR_THRESHOLD / 2 + 1] = J2Value::Float(1.0);
        assert!(matches!(sum_seq(&values), Ok(J2Value::Float(1.0))));
    }

    #[test]
    fn fallible_map_joins_workers_before_returning_first_error() {
        let values: Vec<_> = (0..PAR_THRESHOLD).map(|n| J2Value::Int(n as i64)).collect();
        let completed = Arc::new(AtomicUsize::new(0));
        let done = completed.clone();
        let result = map_seq(values, move |value| {
            done.fetch_add(1, Ordering::SeqCst);
            match value {
                J2Value::Int(0) => Err(J2Err::value("first error")),
                J2Value::Int(1) => panic!("a later panic must not replace an earlier error"),
                _ => Ok(value.clone()),
            }
        });
        assert_eq!(result.unwrap_err().msg, "first error");
        let workers = worker_count(PAR_THRESHOLD);
        let expected = if workers > 1 {
            PAR_THRESHOLD - PAR_THRESHOLD.div_ceil(workers) + 1
        } else {
            1
        };
        assert_eq!(completed.load(Ordering::SeqCst), expected);
    }

    #[test]
    fn filter_count_preserves_errors_and_counts() {
        let values: Vec<_> = (0..PAR_THRESHOLD).map(|n| J2Value::Int(n as i64)).collect();
        assert_eq!(
            filter_count(&values, |value| Ok(value.as_num_i64()? % 2 == 0)).unwrap(),
            (PAR_THRESHOLD / 2) as i64
        );
        let seen = Arc::new(Mutex::new(HashSet::new()));
        let captured = seen.clone();
        let result = filter_count(&values, move |value| {
            let n = value.as_num_i64()?;
            captured.lock().unwrap().insert(thread::current().id());
            if n == 0 { Err(J2Err::value("first error")) } else { Ok(true) }
        });
        assert_eq!(result.unwrap_err().msg, "first error");
        if worker_count(values.len()) > 1 {
            assert!(seen.lock().unwrap().len() > 1);
        }
    }
}
