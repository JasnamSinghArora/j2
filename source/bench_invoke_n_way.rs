// Direct N-way runtime bench

#![feature(parallel_runtime_internal)]

use std::hint::black_box;
use std::parallel_runtime as pr;
use std::time::Instant;

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

fn bench_serial_n(label: &str, n: u64, iters: u64, ways: usize) {
    let t = Instant::now();
    let mut sink = 0u64;
    for k in 0..iters {
        let mut acc = 0u64;
        for w in 0..ways {
            acc ^= work(black_box(k.wrapping_add(w as u64)), black_box(n));
        }
        sink ^= acc;
    }
    let elapsed = t.elapsed().as_nanos() as u64;
    let per = elapsed as f64 / iters as f64;
    println!("{:<14} mode=serial-{}     ns/iter={:>10.1}  sink={:#x}",
             label, ways, per, sink);
}

fn bench_invoke_3(label: &str, n: u64, iters: u64) {
    // Warm.
    for _ in 0..256 {
        let _ = pr::parallel_invoke_3(|| work(0, 1), || work(0, 1), || work(0, 1));
    }
    let t = Instant::now();
    let mut sink = 0u64;
    for k in 0..iters {
        let kw = black_box(n);
        let (a, b, c) = pr::parallel_invoke_3(
            move || work(black_box(k), kw),
            move || work(black_box(k.wrapping_add(1)), kw),
            move || work(black_box(k.wrapping_add(2)), kw),
        );
        sink ^= a ^ b ^ c;
    }
    let elapsed = t.elapsed().as_nanos() as u64;
    let per = elapsed as f64 / iters as f64;
    println!("{:<14} mode=invoke-3     ns/iter={:>10.1}  sink={:#x}",
             label, per, sink);
}

fn bench_invoke_4(label: &str, n: u64, iters: u64) {
    for _ in 0..256 {
        let _ = pr::parallel_invoke_4(
            || work(0, 1), || work(0, 1), || work(0, 1), || work(0, 1));
    }
    let t = Instant::now();
    let mut sink = 0u64;
    for k in 0..iters {
        let kw = black_box(n);
        let (a, b, c, d) = pr::parallel_invoke_4(
            move || work(black_box(k), kw),
            move || work(black_box(k.wrapping_add(1)), kw),
            move || work(black_box(k.wrapping_add(2)), kw),
            move || work(black_box(k.wrapping_add(3)), kw),
        );
        sink ^= a ^ b ^ c ^ d;
    }
    let elapsed = t.elapsed().as_nanos() as u64;
    let per = elapsed as f64 / iters as f64;
    println!("{:<14} mode=invoke-4     ns/iter={:>10.1}  sink={:#x}",
             label, per, sink);
}

fn sweep_3(label: &str, n: u64, iters: u64) {
    bench_serial_n(label, n, iters, 3);
    bench_invoke_3(label, n, iters);
}

fn sweep_4(label: &str, n: u64, iters: u64) {
    bench_serial_n(label, n, iters, 4);
    bench_invoke_4(label, n, iters);
}

fn main() {
    println!("=== direct N-way invoke wall-clock sweep ===");
    println!("hardware parallelism: {}",
             std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1));

    println!("\n--- 3-way (claims 2 workers + caller) ---");
    sweep_3("med-1024", 1024, 50_000);
    sweep_3("med-4096", 4096, 20_000);
    sweep_3("large-16k", 16_384, 5_000);

    println!("\n--- 4-way (claims 3 workers + caller) ---");
    sweep_4("med-1024", 1024, 50_000);
    sweep_4("med-4096", 4096, 20_000);
    sweep_4("large-16k", 16_384, 5_000);
}
