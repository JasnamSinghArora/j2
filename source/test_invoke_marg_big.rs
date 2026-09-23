// 3-way invoke, 8-arg callees, expect parallel_invoke_3_fn8

use std::hint::black_box;
use std::time::Instant;

#[inline(never)]
fn mix8_a(
    a: u64, b: u64, c: u64, d: u64,
    e: u64, f: u64, g: u64, h: u64,
) -> u64 {
    let mut x = a;
    let mut i: u64 = 0;
    while i < 200_000 {
        x = x.wrapping_mul(0x100000001b3).wrapping_add(b ^ i);
        x ^= x.rotate_right(33);
        x = x.wrapping_mul(c).wrapping_add(d);
        x ^= x.rotate_right(27);
        x = x.wrapping_mul(e).wrapping_add(f);
        x ^= x.rotate_right(31);
        x = x.wrapping_mul(g).wrapping_add(h);
        i = i.wrapping_add(1);
    }
    x
}

#[inline(never)]
fn mix8_b(
    a: u64, b: u64, c: u64, d: u64,
    e: u64, f: u64, g: u64, h: u64,
) -> u64 {
    let mut x = a.wrapping_mul(7);
    let mut i: u64 = 0;
    while i < 200_000 {
        x = x.wrapping_mul(0xc2b2ae3d27d4eb4f).wrapping_add(b ^ i);
        x ^= x.rotate_right(31);
        x = x.wrapping_mul(c).wrapping_add(d);
        x ^= x.rotate_right(29);
        x = x.wrapping_mul(e).wrapping_add(f);
        x ^= x.rotate_right(25);
        x = x.wrapping_mul(g).wrapping_add(h);
        i = i.wrapping_add(1);
    }
    x
}

#[inline(never)]
fn mix8_c(
    a: u64, b: u64, c: u64, d: u64,
    e: u64, f: u64, g: u64, h: u64,
) -> u64 {
    let mut x = a.wrapping_mul(13);
    let mut i: u64 = 0;
    while i < 200_000 {
        x = x.wrapping_mul(0xd6e8feb86659fd93).wrapping_add(b ^ i);
        x ^= x.rotate_right(30);
        x = x.wrapping_mul(c).wrapping_add(d);
        x ^= x.rotate_right(28);
        x = x.wrapping_mul(e).wrapping_add(f);
        x ^= x.rotate_right(24);
        x = x.wrapping_mul(g).wrapping_add(h);
        i = i.wrapping_add(1);
    }
    x
}

#[inline(never)]
fn run_three_8arg(seed: u64) -> (u64, u64, u64) {
    let r1 = mix8_a(seed, 1, 2, 3, 4, 5, 6, 7);
    let r2 = mix8_b(seed, 11, 12, 13, 14, 15, 16, 17);
    let r3 = mix8_c(seed, 21, 22, 23, 24, 25, 26, 27);
    (r1, r2, r3)
}

fn main() {
    const ITERS: u32 = 30;
    let t0 = Instant::now();
    let mut acc: u64 = 0;
    for i in 0..ITERS {
        let (a, b, c) = run_three_8arg(black_box(i as u64));
        acc ^= a ^ b ^ c;
    }
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("INVOKE_MARG_BIG arity=8 ITERS={} ms={:.2} acc=0x{:016x}", ITERS, elapsed_ms, acc);
}
