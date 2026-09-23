// parallel_invoke_2 vs serial vs thread spawn

#![feature(parallel_runtime_internal)]

use std::hint::black_box;
use std::parallel_runtime as pr;
use std::time::Instant;

/// One unit of "real work"
#[inline(never)]
fn work(seed: u64, n: u64) -> u64 {
    let mut x = seed;
    let k = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut i = 0u64;
    while i < n {
        x = x.wrapping_mul(0xC2B2_AE35_3A11_05F1).wrapping_add(k);
        x ^= x.rotate_right(33);
        x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
        x ^= x.rotate_right(27);
        i = i.wrapping_add(1);
    }
    x
}

fn bench_serial(label: &str, work_n: u64, iters: u64) {
    let t = Instant::now();
    let mut sink = 0u64;
    for k in 0..iters {
        let a = work(black_box(k), black_box(work_n));
        let b = work(black_box(k.wrapping_add(1)), black_box(work_n));
        sink ^= a ^ b;
    }
    let elapsed = t.elapsed().as_nanos() as u64;
    let per_iter = elapsed as f64 / iters as f64;
    println!(
        "{:<20} mode=serial   ns/iter={:>10.1}  sink={:#x}",
        label, per_iter, sink
    );
}

fn bench_invoke(label: &str, work_n: u64, iters: u64) {
    // Warm pool.
    for _ in 0..256 {
        let _ = pr::parallel_invoke_2(|| work(0, 1), || work(0, 1));
    }
    let t = Instant::now();
    let mut sink = 0u64;
    for k in 0..iters {
        let kw = black_box(work_n);
        let (a, b) = pr::parallel_invoke_2(
            move || work(black_box(k), kw),
            move || work(black_box(k.wrapping_add(1)), kw),
        );
        sink ^= a ^ b;
    }
    let elapsed = t.elapsed().as_nanos() as u64;
    let per_iter = elapsed as f64 / iters as f64;
    println!(
        "{:<20} mode=invoke   ns/iter={:>10.1}  sink={:#x}",
        label, per_iter, sink
    );
}

fn bench_spawn(label: &str, work_n: u64, iters: u64) {
    let t = Instant::now();
    let mut sink = 0u64;
    for k in 0..iters {
        let h = std::thread::spawn(move || work(black_box(k), black_box(work_n)));
        let b = work(black_box(k.wrapping_add(1)), black_box(work_n));
        let a = h.join().unwrap();
        sink ^= a ^ b;
    }
    let elapsed = t.elapsed().as_nanos() as u64;
    let per_iter = elapsed as f64 / iters as f64;
    println!(
        "{:<20} mode=spawn    ns/iter={:>10.1}  sink={:#x}",
        label, per_iter, sink
    );
}

fn sweep(label: &str, work_n: u64, iters: u64, run_spawn: bool) {
    println!();
    bench_serial(label, work_n, iters);
    bench_invoke(label, work_n, iters);
    if run_spawn {
        bench_spawn(label, work_n, iters);
    }
}

fn main() {
    println!("=== parallel_invoke_2 wall-clock sweep ===");
    println!("hardware parallelism: {}",
             std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1));

    // calibrate work(_, n) nanoseconds per n
    println!("\n-- calibration --");
    for &n in &[1u64, 4, 16, 64, 256, 1024, 4096, 16384] {
        let t = Instant::now();
        let mut sink = 0u64;
        for k in 0..1000u64 {
            sink ^= work(black_box(k), black_box(n));
        }
        let per_call = t.elapsed().as_nanos() as f64 / 1000.0;
        println!("work_n={:<6} ns/call={:>9.1}  sink={:#x}", n, per_call, sink);
    }

    println!("\n-- TINY: per-task work below dispatch floor --");
    sweep("tiny-1",   1,    500_000, false);
    sweep("tiny-4",   4,    500_000, false);
    sweep("tiny-16",  16,   500_000, false);

    println!("\n-- SMALL: per-task ~100ns-1us --");
    sweep("small-64",   64,   200_000, false);
    sweep("small-256",  256,  100_000, false);

    println!("\n-- MEDIUM: per-task ~1-10us (should win) --");
    sweep("med-1024",  1024,  50_000, false);
    sweep("med-4096",  4096,  20_000, false);

    println!("\n-- LARGE: per-task ~10-100us (definitely wins) --");
    sweep("large-16k",  16_384, 5_000, true);
    sweep("large-64k",  65_536, 2_000, true);
}
