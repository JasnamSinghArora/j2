// Panic-propagation test for parallel_invoke_5

#![feature(parallel_runtime_internal)]

use std::any::Any;
use std::hint::black_box;
use std::panic;
use std::parallel_runtime as pr;
use std::sync::atomic::{AtomicUsize, Ordering};

static PASSES: AtomicUsize = AtomicUsize::new(0);
static FAILS: AtomicUsize = AtomicUsize::new(0);

#[inline(never)]
fn ok_work(i: u64) -> u64 {
    let mut x = i.wrapping_add(1);
    let mut k: u64 = 0;
    while k < 2_000 {
        x = x.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(k);
        x ^= x.rotate_right(17);
        k = k.wrapping_add(1);
    }
    black_box(x)
}

#[inline(never)]
fn boom(slot: u32) -> u64 {
    panic!("PANIC_TAG_slot{}", slot);
}

type CaughtPayload = Box<dyn Any + Send + 'static>;

fn check_caught<R>(invoke_label: &str, n: u32, slot: u32, r: Result<R, CaughtPayload>) {
    match r {
        Err(payload) => {
            let s = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "<non-string payload>".to_string());
            let want = format!("PANIC_TAG_slot{}", slot);
            if s.contains(&want) {
                PASSES.fetch_add(1, Ordering::Relaxed);
                println!("OK   {invoke_label} N={n} slot={slot}: caught \"{s}\"");
            } else {
                FAILS.fetch_add(1, Ordering::Relaxed);
                println!(
                    "FAIL {invoke_label} N={n} slot={slot}: wrong payload \"{s}\" (want substring \"{want}\")"
                );
            }
        }
        Ok(_) => {
            FAILS.fetch_add(1, Ordering::Relaxed);
            println!(
                "FAIL {invoke_label} N={n} slot={slot}: panic NOT propagated (closure returned)"
            );
        }
    }
}

fn check_clean<R>(invoke_label: &str, n: u32, r: Result<R, CaughtPayload>) {
    match r {
        Ok(_) => {
            PASSES.fetch_add(1, Ordering::Relaxed);
            println!("OK   {invoke_label} N={n}: clean run");
        }
        Err(p) => {
            let s = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "<unknown>".to_string());
            FAILS.fetch_add(1, Ordering::Relaxed);
            println!("FAIL {invoke_label} N={n}: spurious panic on clean: {s}");
        }
    }
}

// One closure factory per slot index
fn mk(i: u32, panic_at: i32) -> impl FnOnce() -> u64 + Send {
    move || {
        if panic_at == i as i32 {
            boom(i)
        } else {
            ok_work(i as u64)
        }
    }
}

fn run5(panic_at: i32) {
    let r = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        pr::parallel_invoke_5(mk(0, panic_at), mk(1, panic_at), mk(2, panic_at), mk(3, panic_at), mk(4, panic_at))
    }));
    if panic_at < 0 { check_clean("parallel_invoke_5", 5, r); } else { check_caught("parallel_invoke_5", 5, panic_at as u32, r); }
}
fn run6(panic_at: i32) {
    let r = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        pr::parallel_invoke_6(mk(0, panic_at), mk(1, panic_at), mk(2, panic_at), mk(3, panic_at), mk(4, panic_at), mk(5, panic_at))
    }));
    if panic_at < 0 { check_clean("parallel_invoke_6", 6, r); } else { check_caught("parallel_invoke_6", 6, panic_at as u32, r); }
}
fn run7(panic_at: i32) {
    let r = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        pr::parallel_invoke_7(mk(0, panic_at), mk(1, panic_at), mk(2, panic_at), mk(3, panic_at), mk(4, panic_at), mk(5, panic_at), mk(6, panic_at))
    }));
    if panic_at < 0 { check_clean("parallel_invoke_7", 7, r); } else { check_caught("parallel_invoke_7", 7, panic_at as u32, r); }
}
fn run8(panic_at: i32) {
    let r = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        pr::parallel_invoke_8(mk(0, panic_at), mk(1, panic_at), mk(2, panic_at), mk(3, panic_at), mk(4, panic_at), mk(5, panic_at), mk(6, panic_at), mk(7, panic_at))
    }));
    if panic_at < 0 { check_clean("parallel_invoke_8", 8, r); } else { check_caught("parallel_invoke_8", 8, panic_at as u32, r); }
}
fn run9(panic_at: i32) {
    let r = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        pr::parallel_invoke_9(mk(0, panic_at), mk(1, panic_at), mk(2, panic_at), mk(3, panic_at), mk(4, panic_at), mk(5, panic_at), mk(6, panic_at), mk(7, panic_at), mk(8, panic_at))
    }));
    if panic_at < 0 { check_clean("parallel_invoke_9", 9, r); } else { check_caught("parallel_invoke_9", 9, panic_at as u32, r); }
}
fn run10(panic_at: i32) {
    let r = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        pr::parallel_invoke_10(mk(0, panic_at), mk(1, panic_at), mk(2, panic_at), mk(3, panic_at), mk(4, panic_at), mk(5, panic_at), mk(6, panic_at), mk(7, panic_at), mk(8, panic_at), mk(9, panic_at))
    }));
    if panic_at < 0 { check_clean("parallel_invoke_10", 10, r); } else { check_caught("parallel_invoke_10", 10, panic_at as u32, r); }
}
fn run11(panic_at: i32) {
    let r = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        pr::parallel_invoke_11(mk(0, panic_at), mk(1, panic_at), mk(2, panic_at), mk(3, panic_at), mk(4, panic_at), mk(5, panic_at), mk(6, panic_at), mk(7, panic_at), mk(8, panic_at), mk(9, panic_at), mk(10, panic_at))
    }));
    if panic_at < 0 { check_clean("parallel_invoke_11", 11, r); } else { check_caught("parallel_invoke_11", 11, panic_at as u32, r); }
}
fn run12(panic_at: i32) {
    let r = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        pr::parallel_invoke_12(mk(0, panic_at), mk(1, panic_at), mk(2, panic_at), mk(3, panic_at), mk(4, panic_at), mk(5, panic_at), mk(6, panic_at), mk(7, panic_at), mk(8, panic_at), mk(9, panic_at), mk(10, panic_at), mk(11, panic_at))
    }));
    if panic_at < 0 { check_clean("parallel_invoke_12", 12, r); } else { check_caught("parallel_invoke_12", 12, panic_at as u32, r); }
}
fn run13(panic_at: i32) {
    let r = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        pr::parallel_invoke_13(mk(0, panic_at), mk(1, panic_at), mk(2, panic_at), mk(3, panic_at), mk(4, panic_at), mk(5, panic_at), mk(6, panic_at), mk(7, panic_at), mk(8, panic_at), mk(9, panic_at), mk(10, panic_at), mk(11, panic_at), mk(12, panic_at))
    }));
    if panic_at < 0 { check_clean("parallel_invoke_13", 13, r); } else { check_caught("parallel_invoke_13", 13, panic_at as u32, r); }
}
fn run14(panic_at: i32) {
    let r = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        pr::parallel_invoke_14(mk(0, panic_at), mk(1, panic_at), mk(2, panic_at), mk(3, panic_at), mk(4, panic_at), mk(5, panic_at), mk(6, panic_at), mk(7, panic_at), mk(8, panic_at), mk(9, panic_at), mk(10, panic_at), mk(11, panic_at), mk(12, panic_at), mk(13, panic_at))
    }));
    if panic_at < 0 { check_clean("parallel_invoke_14", 14, r); } else { check_caught("parallel_invoke_14", 14, panic_at as u32, r); }
}
fn run15(panic_at: i32) {
    let r = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        pr::parallel_invoke_15(mk(0, panic_at), mk(1, panic_at), mk(2, panic_at), mk(3, panic_at), mk(4, panic_at), mk(5, panic_at), mk(6, panic_at), mk(7, panic_at), mk(8, panic_at), mk(9, panic_at), mk(10, panic_at), mk(11, panic_at), mk(12, panic_at), mk(13, panic_at), mk(14, panic_at))
    }));
    if panic_at < 0 { check_clean("parallel_invoke_15", 15, r); } else { check_caught("parallel_invoke_15", 15, panic_at as u32, r); }
}
fn run16(panic_at: i32) {
    let r = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        pr::parallel_invoke_16(mk(0, panic_at), mk(1, panic_at), mk(2, panic_at), mk(3, panic_at), mk(4, panic_at), mk(5, panic_at), mk(6, panic_at), mk(7, panic_at), mk(8, panic_at), mk(9, panic_at), mk(10, panic_at), mk(11, panic_at), mk(12, panic_at), mk(13, panic_at), mk(14, panic_at), mk(15, panic_at))
    }));
    if panic_at < 0 { check_clean("parallel_invoke_16", 16, r); } else { check_caught("parallel_invoke_16", 16, panic_at as u32, r); }
}

fn main() {
    panic::set_hook(Box::new(|_| {}));

    let _ = black_box(pr::parallel_invoke_4(
        || ok_work(0), || ok_work(1), || ok_work(2), || ok_work(3),
    ));

    // Each N, panic at none/head/middle/tail
    run5(-1);  run5(0);  run5(2);  run5(4);
    run6(-1);  run6(0);  run6(3);  run6(5);
    run7(-1);  run7(0);  run7(3);  run7(6);
    run8(-1);  run8(0);  run8(4);  run8(7);
    run9(-1);  run9(0);  run9(4);  run9(8);
    run10(-1); run10(0); run10(5); run10(9);
    run11(-1); run11(0); run11(5); run11(10);
    run12(-1); run12(0); run12(6); run12(11);
    run13(-1); run13(0); run13(6); run13(12);
    run14(-1); run14(0); run14(7); run14(13);
    run15(-1); run15(0); run15(7); run15(14);
    run16(-1); run16(0); run16(8); run16(15);

    let p = PASSES.load(Ordering::Relaxed);
    let f = FAILS.load(Ordering::Relaxed);
    println!("---");
    println!("PASS={p} FAIL={f}");
    if f == 0 {
        println!("OVERALL=PASS");
    } else {
        println!("OVERALL=FAIL");
        std::process::exit(1);
    }
}
