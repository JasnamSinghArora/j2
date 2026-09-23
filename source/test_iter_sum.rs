// Phase 4, iter().sum() rewritten to parallel_reduce_sum_slice_u64

use std::hint::black_box;
use std::time::Instant;

#[inline(never)]
pub fn run(v: &[u64]) -> u64 {
    v.iter().sum()
}

fn main() {
    let n: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(100_000_000);
    let n = black_box(n);

    let mut v: Vec<u64> = Vec::with_capacity(n);
    for i in 0..n { v.push((i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)); }

    // warm up the parallel pool
    let _ = black_box(run(&v[..1024]));

    let t = Instant::now();
    let r = run(black_box(&v[..]));
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    println!("RESULT={:#018x}  MS={:.3}", black_box(r), ms);
}
