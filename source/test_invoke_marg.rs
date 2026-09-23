// 3-way invoke with 2-arg callees, expect parallel_invoke_3_fn2

use std::hint::black_box;
use std::time::Instant;

#[inline(never)]
fn hash_pair_a(seed: u64, n: u64) -> u64 {
    let mut h: u64 = seed;
    let mut i: u64 = 0;
    while i < n {
        h = h.wrapping_mul(0x100000001b3).wrapping_add(i);
        h ^= h.rotate_right(33);
        h = h.wrapping_mul(0xff51afd7ed558ccd).wrapping_add(i);
        h ^= h.rotate_right(27);
        h = h.wrapping_mul(0x94d049bb133111eb).wrapping_add(i);
        i = i.wrapping_add(1);
    }
    h
}

#[inline(never)]
fn hash_pair_b(seed: u64, n: u64) -> u64 {
    let mut h: u64 = seed.wrapping_mul(7);
    let mut i: u64 = 0;
    while i < n {
        h = h.wrapping_mul(0xc2b2ae3d27d4eb4f).wrapping_add(i);
        h ^= h.rotate_right(31);
        h = h.wrapping_mul(0x9e3779b97f4a7c15).wrapping_add(i);
        h ^= h.rotate_right(29);
        h = h.wrapping_mul(0xbf58476d1ce4e5b9).wrapping_add(i);
        i = i.wrapping_add(1);
    }
    h
}

#[inline(never)]
fn hash_pair_c(seed: u64, n: u64) -> u64 {
    let mut h: u64 = seed.wrapping_mul(13);
    let mut i: u64 = 0;
    while i < n {
        h = h.wrapping_mul(0xd6e8feb86659fd93).wrapping_add(i);
        h ^= h.rotate_right(30);
        h = h.wrapping_mul(0xc6a4a7935bd1e995).wrapping_add(i);
        h ^= h.rotate_right(28);
        h = h.wrapping_mul(0x517cc1b727220a95).wrapping_add(i);
        i = i.wrapping_add(1);
    }
    h
}

#[inline(never)]
fn run_three(seed: u64, n: u64) -> (u64, u64, u64) {
    let r1 = hash_pair_a(seed, n);
    let r2 = hash_pair_b(seed, n);
    let r3 = hash_pair_c(seed, n);
    (r1, r2, r3)
}

fn main() {
    const N: u64 = 200_000;
    const ITERS: u32 = 30;

    let t0 = Instant::now();
    let mut acc: u64 = 0;
    for i in 0..ITERS {
        let (a, b, c) = run_three(black_box(i as u64), black_box(N));
        acc ^= a ^ b ^ c;
    }
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("INVOKE_MARG N={} ITERS={} ms={:.2} acc=0x{:016x}", N, ITERS, elapsed_ms, acc);
}
