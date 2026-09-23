// Matcher should fire on &[String] slices

use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

static SUM: AtomicU64 = AtomicU64::new(0);

fn force_count<F: Fn(&&String) -> bool + Sync>(f: F) -> F { f }
fn force_unit<F: Fn(&String) + Sync>(f: F) -> F { f }

#[inline(never)]
fn count_long_strings(strings: &[String], threshold: usize) -> usize {
    // Use force_count directly as the predicate
    let f = force_count(move |s: &&String| s.len() > threshold);
    strings.iter().filter(f).count()
}

#[inline(never)]
fn process_each_string(strings: &[String]) {
    let f = force_unit(|s: &String| {
        SUM.fetch_add(s.len() as u64, Ordering::Relaxed);
    });
    strings.iter().for_each(f);
}

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(100_000);

    // Wider length spread so threshold filter matches
    let strings: Vec<String> = (0..n)
        .map(|i| format!("item-{i}-padding-{}", i % 1_000_000))
        .collect();

    // Test 1, &[String] filter+count, ~half match
    let t = Instant::now();
    let count = count_long_strings(black_box(&strings), 20);
    let ms1 = t.elapsed().as_secs_f64() * 1000.0;

    // Reference (serial)
    let ref_count = strings.iter().filter(|s| s.len() > 20).count();
    assert_eq!(count, ref_count, "count_long_strings");

    // Test 2: for_each over &[String]
    SUM.store(0, Ordering::Relaxed);
    let t = Instant::now();
    process_each_string(black_box(&strings));
    let ms2 = t.elapsed().as_secs_f64() * 1000.0;
    let total_len = SUM.load(Ordering::Relaxed);

    let ref_total: u64 = strings.iter().map(|s| s.len() as u64).sum();
    assert_eq!(total_len, ref_total, "process_each_string");

    println!("count_long_strings  N={n} ms={ms1:8.3} count={count}");
    println!("process_each_string N={n} ms={ms2:8.3} total_len={total_len}");
    println!("PASS");
}
