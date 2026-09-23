// Reduction benchmark, serial vs parallel_reduce, results match

#![feature(parallel_runtime_internal)]

use std::parallel_runtime as pr;
use std::time::Instant;

const N: u64 = 50_000_000;

/// Opaque body; inline(never) blocks LLVM vectorisation
#[inline(never)]
fn body(i: u64) -> u64 {
    let mut x = i.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn run_serial(n: u64) -> u64 {
    let mut acc = 0u64;
    let mut i = 0u64;
    while i < n {
        acc = acc.wrapping_add(body(i));
        i = i.wrapping_add(1);
    }
    acc
}

fn run_parallel(n: u64) -> u64 {
    pr::parallel_reduce_add_u64(0, n, body)
}

fn time<F: FnOnce() -> u64>(label: &str, f: F) -> (u64, f64) {
    let t0 = Instant::now();
    let r = f();
    let dt_ms = t0.elapsed().as_secs_f64() * 1000.0;
    eprintln!("{:<10} = {:#018x}  ({:>8.3} ms)", label, r, dt_ms);
    (r, dt_ms)
}

fn main() {
    eprintln!("hardware parallelism: {}",
              std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1));
    eprintln!("N = {}", N);

    // Warmup keeps pool init out of timings
    let _ = pr::parallel_reduce_add_u64(0, 1024, body);

    let (serial_acc, _serial_ms) = time("serial",   || run_serial(N));
    let (parallel_acc, _parallel_ms) = time("parallel", || run_parallel(N));

    if serial_acc != parallel_acc {
        eprintln!("CORRECTNESS FAIL: serial != parallel");
        std::process::exit(1);
    }

    // Keep acc from constant-folding
    let mut sink = [0u64; 1];
    unsafe { std::ptr::write_volatile(&mut sink[0], serial_acc); }

    let fp = serial_acc
        ^ serial_acc.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ serial_acc.rotate_left(13)
        ^ serial_acc.rotate_left(37);
    std::process::exit((fp & 0x7F) as i32);
}
