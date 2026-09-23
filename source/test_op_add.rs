// Does auto-reduction fire on operator-based BinaryOp reduction

#[inline(never)]
pub fn body(i: u64) -> u64 {
    let mut x = i.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 31;
    x.wrapping_mul(0xBF58_476D_1CE4_E5B9)
}

fn run(n: u64) -> u64 {
    let mut acc: u64 = 0;
    let mut i: u64 = 0;
    while i < n {
        // operator form, not method form
        acc = acc + body(i);
        i = i + 1;
    }
    acc
}

fn main() {
    let acc = run(200_000_000);
    let mut sink = [0u64; 1];
    unsafe { std::ptr::write_volatile(&mut sink[0], acc); }
    println!("RESULT={:#018x}", acc);
}
