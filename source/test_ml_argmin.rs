#![feature(parallel_runtime_internal)]
use std::hint::black_box;
use std::time::Instant;

const N: usize = 8_000_000;
const ITERS: usize = 30;

#[inline(never)]
fn argmin_serial(slice: &[f64]) -> usize {
    let mut best_v = slice[0]; let mut best_i = 0;
    for i in 1..slice.len() { if slice[i] < best_v { best_v = slice[i]; best_i = i; } }
    best_i
}

fn main() {
    let mut a = vec![0f64; N];
    for i in 0..N { a[i] = ((i as f64) * 1e-7).sin(); }

    let t0 = Instant::now();
    let mut acc_ser = 0usize;
    for _ in 0..ITERS { acc_ser ^= argmin_serial(black_box(&a)); }
    let ser_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let t0 = Instant::now();
    let mut acc_par = 0usize;
    for _ in 0..ITERS { acc_par ^= std::parallel_runtime::parallel_argmin_f64(black_box(&a)); }
    let par_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let speedup = ser_ms / par_ms;
    println!("ARGMIN N={} ITERS={} ser_ms={:.2} par_ms={:.2} speedup={:.2}x acc_ser=0x{:x} acc_par=0x{:x}",
        N, ITERS, ser_ms, par_ms, speedup, acc_ser, acc_par);
}
