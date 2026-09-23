// Parallel-only reduction; RESULT must match test_serial

#![feature(parallel_runtime_internal)]

use std::parallel_runtime as pr;
use std::time::Instant;

// MUST match test_serial.rs N
const N: u64 = 1_000_000_000;

/// Byte-identical to test_serial body; ~4 ns/call
#[inline(never)]
pub fn body(i: u64) -> u64 {
    let mut x = i.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    x = x.wrapping_mul(0xD6E8_FEB8_6659_FD93);
    x ^= x >> 32;
    x = x.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    x ^= x >> 33;
    x = x.wrapping_mul(0xCC9E_2D51_1B87_3593);
    x ^= x >> 16;
    x = x.wrapping_mul(0x85EB_CA77_C2B2_AE63);
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2_AE35_27D4_EB2F);
    x ^ (x >> 17)
}

fn main() {
    let t0 = Instant::now();
    let acc = pr::parallel_reduce_add_u64(0, N, body);
    let dt_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let mut sink = [0u64; 1];
    unsafe { std::ptr::write_volatile(&mut sink[0], acc); }

    println!("RESULT={:#018x}", acc);
    println!("CUSTOM_PARALLEL_MS={:.3}", dt_ms);
}
