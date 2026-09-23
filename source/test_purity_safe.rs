// Structurally pure body; transform should fire

use std::time::Instant;

const N: u64 = 200_000_000;

#[inline(never)]
pub fn body(i: u64) -> u64 {
    let mut x = i.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
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
    let mut sink = [0u64; 1];
    unsafe { std::ptr::write_volatile(&mut sink[0], acc); }
    println!("RESULT={:#018x}", acc);
    println!("MS={:.3}", dt);
}
