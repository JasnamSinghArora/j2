// Sanity test generic-T runtime helpers on non-primitives
#![feature(parallel_runtime_internal)]

use std::parallel_runtime as pr;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn main() {
    // Test 1: for_each over &[String]
    let strings: Vec<String> = (0..100_000).map(|i| format!("item-{i}")).collect();
    COUNTER.store(0, Ordering::Relaxed);
    pr::parallel_for_each_slice_dyn_generic(&strings, &|s: &String| {
        COUNTER.fetch_add(s.len() as u64, Ordering::Relaxed);
    });
    let total_len = COUNTER.load(Ordering::Relaxed);
    let ref_total: u64 = strings.iter().map(|s| s.len() as u64).sum();
    assert_eq!(total_len, ref_total, "for_each_slice_dyn_generic correctness");

    // Test 2: filter+count over &[String]
    let count = pr::parallel_filter_count_slice_dyn_generic(&strings, &|s: &&String| {
        s.starts_with("item-1")
    });
    let ref_count = strings.iter().filter(|s| s.starts_with("item-1")).count();
    assert_eq!(count, ref_count, "filter_count_slice_dyn_generic correctness");

    // Test 3, filter+count over &[u64] primitives
    let nums: Vec<u64> = (0..50_000).collect();
    let even_count = pr::parallel_filter_count_slice_dyn_generic(&nums, &|x: &&u64| {
        **x % 2 == 0
    });
    assert_eq!(even_count, 25_000, "filter_count u64 correctness");

    println!("All generic-T runtime helpers work for String, u64, etc.");
    println!("PASS");
}
