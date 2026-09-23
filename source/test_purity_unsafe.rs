// Side-effecting body; purity check must block transform

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

const N: u64 = 200_000_000;
static SIDE_EFFECT_COUNTER: AtomicU64 = AtomicU64::new(0);

#[inline(never)]
pub fn body(i: u64) -> u64 {
    // Side effect; parallel order changes observable history
    let _ = SIDE_EFFECT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut x = i.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 31;
    x.wrapping_mul(0xBF58_476D_1CE4_E5B9)
}

fn run(n: u64) -> u64 {
    let mut acc = 0u64;
    let mut i = 0u64;
    while i < n {
        acc = acc.wrapping_add(body(i));
        i = i.wrapping_add(1);
    }
    acc
}

fn main() {
    let t0 = Instant::now();
    let acc = run(N);
    let dt = t0.elapsed().as_secs_f64() * 1000.0;
    let counter = SIDE_EFFECT_COUNTER.load(Ordering::Relaxed);
    let mut sink = [0u64; 1];
    unsafe { std::ptr::write_volatile(&mut sink[0], acc); }
    println!("RESULT={:#018x}", acc);
    println!("COUNTER={}", counter);
    println!("MS={:.3}", dt);
}
