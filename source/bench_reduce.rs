// bench parallel_reduce_* against serial, manual chunked

#![feature(parallel_runtime_internal)]

use std::parallel_runtime as pr;
use std::time::Instant;

/// LLVM-opaque loop body, 5 dependent wrapping ops
#[inline(never)]  // suppress inlining, keep body opaque
fn lane_compute(i: u64) -> u64 {
    let mut x = i.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x = x ^ (x >> 31);
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = x ^ (x >> 27);
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn time_run<F: FnOnce() -> u64>(label: &str, f: F) -> (u64, f64) {
    let t0 = Instant::now();
    let result = f();
    let dt_ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("{:<32} result={:#018x}  time={:>8.3} ms", label, result, dt_ms);
    (result, dt_ms)
}

fn serial(start: u64, end: u64) -> u64 {
    let mut acc = 0u64;
    let mut i = start;
    while i < end {
        acc = acc.wrapping_add(lane_compute(i));
        i = i.wrapping_add(1);
    }
    // Defeat const-folding of the result.
    let mut sink = [0u64; 1];
    unsafe { std::ptr::write_volatile(&mut sink[0], acc); }
    acc
}

fn manual_chunked(start: u64, end: u64) -> u64 {
    use std::thread;
    const N: u64 = 8;
    let total = end - start;
    let chunk = total / N;
    let remainder = total % N;

    let mut handles = Vec::with_capacity((N - 1) as usize);
    let mut cur = start;
    for w in 0..(N - 1) {
        let size = chunk + if w < remainder { 1 } else { 0 };
        let s = cur;
        let e = cur + size;
        cur = e;
        let h = thread::spawn(move || {
            let mut acc = 0u64;
            let mut i = s;
            while i < e {
                acc = acc.wrapping_add(lane_compute(i));
                i = i.wrapping_add(1);
            }
            acc
        });
        handles.push(h);
    }
    // caller does last chunk alongside workers
    let mut acc = 0u64;
    let mut i = cur;
    while i < end {
        acc = acc.wrapping_add(lane_compute(i));
        i = i.wrapping_add(1);
    }
    for h in handles {
        acc = acc.wrapping_add(h.join().unwrap());
    }
    let mut sink = [0u64; 1];
    unsafe { std::ptr::write_volatile(&mut sink[0], acc); }
    acc
}

fn runtime_reduce(start: u64, end: u64) -> u64 {
    let acc = pr::parallel_reduce_add_u64(start, end, lane_compute);
    let mut sink = [0u64; 1];
    unsafe { std::ptr::write_volatile(&mut sink[0], acc); }
    acc
}

fn run_size(label: &str, n: u64) {
    println!("\n=== n = {} ===", n);
    let mut results = Vec::new();
    for run in 0..3 {
        let suffix = if run == 0 { " (warm-up)" } else { "" };
        let (r1, t1) = time_run(&format!("{label} serial{suffix}"), || serial(0, n));
        let (r2, t2) = time_run(&format!("{label} manual_chunked{suffix}"), || manual_chunked(0, n));
        let (r3, t3) = time_run(&format!("{label} runtime_reduce{suffix}"), || runtime_reduce(0, n));
        if r1 != r2 || r2 != r3 {
            println!("CORRECTNESS FAIL: serial={:#018x} manual={:#018x} runtime={:#018x}", r1, r2, r3);
            std::process::exit(1);
        }
        if run > 0 {
            results.push((t1, t2, t3));
        }
    }
    let avg_serial = results.iter().map(|t| t.0).sum::<f64>() / results.len() as f64;
    let avg_manual = results.iter().map(|t| t.1).sum::<f64>() / results.len() as f64;
    let avg_runtime = results.iter().map(|t| t.2).sum::<f64>() / results.len() as f64;
    println!(
        "  averages: serial={:>7.3} ms  manual={:>7.3} ms (×{:.2})  runtime={:>7.3} ms (×{:.2})",
        avg_serial,
        avg_manual,
        avg_serial / avg_manual,
        avg_runtime,
        avg_serial / avg_runtime
    );
}

fn main() {
    println!(
        "hardware parallelism: {}",
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
    );
    println!("3 runs each (first is warm-up; averages over runs 2-3)\n");

    run_size("small", 100_000);
    run_size("medium", 1_000_000);
    run_size("large", 10_000_000);
}
