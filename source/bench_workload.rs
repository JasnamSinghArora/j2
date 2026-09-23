// sequential vs pool_seq vs pool_parallel workload sweep

#![feature(parallel_runtime_internal)]

use std::parallel_runtime as pr;
use std::time::Instant;

#[inline(always)]
fn lane_compute(seed: u64, iter: u64, ops: usize) -> u64 {
    let mut x = seed.wrapping_add(iter);
    let k = iter.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    // alternate ADD/XOR, matches pool_parallel shape
    let mut i = 0;
    while i < ops {
        x = x.wrapping_add(k);
        i += 1;
        if i >= ops { break; }
        x ^= k;
        i += 1;
    }
    x
}

fn build_pool_group(
    shape_id: usize,
    tasks: usize,
    ops_per_task: usize,
    max_par: usize,
) -> usize {
    let handle = pr::group_new_cached(shape_id, tasks, max_par);
    for t in 0..tasks {
        let slot_count = 2 + ops_per_task;
        // out is the last op-output slot
        let out_slot = 1 + ops_per_task;
        pr::task_init_outputs4(handle, t, slot_count, 1, out_slot, 0, 0, 0);
        // add then alternating xor/add with in_b
        if ops_per_task >= 1 {
            // out[0] := slots[0] + slots[1]
            pr::task_push_binary::<u64>(handle, t, 2, 0, 1, /*ADD*/ 0);
        }
        for i in 1..ops_per_task {
            let dst = 2 + i;
            let src = 2 + i - 1;
            // alternate ADD (0) and XOR (5)
            let op = if i % 2 == 1 { /*XOR*/ 5 } else { /*ADD*/ 0 };
            pr::task_push_binary::<u64>(handle, t, dst, src, 1, op);
        }
    }
    handle
}

fn run_sequential(label: &str, tasks: usize, ops: usize, iters: u64) {
    let seed: u64 = 0xDEAD_BEEF_CAFE_F00D;
    let mut sink: u64 = 0;
    let start = Instant::now();
    for k in 0..iters {
        for t in 0..tasks {
            sink ^= lane_compute(seed.wrapping_add(t as u64), k, ops);
        }
    }
    let elapsed = start.elapsed();
    let ns_total = elapsed.as_nanos() as u64;
    let ns_per_iter = ns_total as f64 / iters as f64;
    let ns_per_op = ns_per_iter / (tasks * ops).max(1) as f64;
    println!(
        "{:<14} mode=seq        tasks={:>2} ops/task={:>3}  ns/iter={:>9.1}  ns/op={:>5.2}  sink={:#018x}",
        label, tasks, ops, ns_per_iter, ns_per_op, sink
    );
}

fn run_pool(
    label: &str,
    tasks: usize,
    ops: usize,
    iters: u64,
    max_par: usize,
    mode_label: &str,
) {
    let shape_id = (label.as_ptr() as usize)
        ^ (tasks << 16)
        ^ (ops << 4)
        ^ max_par;
    let handle = build_pool_group(shape_id, tasks, ops, max_par);

    let seed: u64 = 0xDEAD_BEEF_CAFE_F00D;
    // Warmup
    for k in 0..256u64 {
        for t in 0..tasks {
            pr::group_set_input_raw(handle, t, 0, seed.wrapping_add(t as u64) as usize);
            pr::group_set_input_raw(
                handle,
                t,
                1,
                k.wrapping_mul(0x9E37_79B9_7F4A_7C15) as usize,
            );
        }
        pr::group_execute(handle);
        for t in 0..tasks {
            let _v: u64 = pr::group_get_output_ready(handle, t, 0);
            std::hint::black_box(_v);
        }
    }

    let mut sink: u64 = 0;
    let start = Instant::now();
    for k in 0..iters {
        let k_u = k as usize;
        for t in 0..tasks {
            pr::group_set_input_raw(handle, t, 0, seed.wrapping_add(t as u64) as usize);
            pr::group_set_input_raw(
                handle,
                t,
                1,
                (k_u.wrapping_mul(0x9E37_79B9_7F4A_7C15usize)) as usize,
            );
        }
        pr::group_execute(handle);
        for t in 0..tasks {
            let v: u64 = pr::group_get_output_ready(handle, t, 0);
            sink ^= v;
        }
    }
    let elapsed = start.elapsed();
    let ns_total = elapsed.as_nanos() as u64;
    let ns_per_iter = ns_total as f64 / iters as f64;
    let ns_per_op = ns_per_iter / (tasks * ops).max(1) as f64;
    println!(
        "{:<14} mode={:<10} tasks={:>2} ops/task={:>3}  ns/iter={:>9.1}  ns/op={:>5.2}  sink={:#018x}",
        label, mode_label, tasks, ops, ns_per_iter, ns_per_op, sink
    );
    pr::group_free(handle);
}

fn sweep(label: &str, tasks: usize, ops: usize, iters: u64, max_par: usize) {
    run_sequential(label, tasks, ops, iters);
    run_pool(label, tasks, ops, iters, 1, "pool_seq");
    run_pool(label, tasks, ops, iters, max_par, "pool_par");
    println!();
}

fn main() {
    let max_par = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
    println!("=== parallel_runtime workload sweep ===");
    println!("hardware parallelism: {}", max_par);
    println!();

    println!("---------- TINY: should reject parallel ----------");
    sweep("tiny-04", 4, 4, 500_000, max_par);
    sweep("tiny-08", 8, 4, 500_000, max_par);

    println!("---------- SMALL: marginal ----------");
    sweep("small-08", 8, 12, 200_000, max_par);
    sweep("small-16", 16, 12, 200_000, max_par);

    println!("---------- MEDIUM: should approach break-even ----------");
    sweep("med-08", 8, 24, 100_000, max_par);
    sweep("med-16", 16, 24, 100_000, max_par);

    println!("---------- LARGE: should beat sequential ----------");
    // long chains amortise dispatch; upper bound
    sweep("large-08", 8, 28, 50_000, max_par);
    sweep("large-16", 16, 28, 50_000, max_par);

    pr::parallel_runtime_stats_dump_final();
}
