#![feature(parallel_runtime_internal)]
use std::hint::black_box;
use std::time::Instant;

const N: usize = 4_000_000;
const ITERS: usize = 30;

#[inline(never)]
fn select_serial(dst: &mut [f64], cond: &[bool], a: &[f64], b: &[f64]) {
    let n = dst.len();
    for i in 0..n { dst[i] = if cond[i] { a[i] } else { b[i] }; }
}

fn main() {
    let a = vec![1.0f64; N];
    let b = vec![2.0f64; N];
    let mut cond = vec![false; N];
    for i in 0..N { cond[i] = i % 2 == 0; }
    let mut dst = vec![0f64; N];

    let t0 = Instant::now();
    for _ in 0..ITERS { select_serial(black_box(&mut dst), black_box(&cond), black_box(&a), black_box(&b)); }
    let ser_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let cs_ser = dst.iter().sum::<f64>();

    let t0 = Instant::now();
    for _ in 0..ITERS { std::parallel_runtime::parallel_select_f64(black_box(&mut dst), black_box(&cond), black_box(&a), black_box(&b)); }
    let par_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let cs_par = dst.iter().sum::<f64>();

    let speedup = ser_ms / par_ms;
    println!("SELECT N={} ITERS={} ser_ms={:.2} par_ms={:.2} speedup={:.2}x cs_ser={:.6e} cs_par={:.6e}",
        N, ITERS, ser_ms, par_ms, speedup, cs_ser, cs_par);
}
