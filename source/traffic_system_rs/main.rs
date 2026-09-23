// final.py port; loop bounded to TICKS

mod add_data;
mod controller;
mod detect;
mod helper;
mod manage;
mod sample;
mod traffic_light;

use std::time::Instant;

use crate::helper::{
    ambulance_light_green, ambulance_light_red, can_change_light, cannot_change_light,
};
use crate::sample::{build_controller, build_plate_numbers, PLATES_PER_LANE};
use crate::traffic_light::DEFAULT_GREEN;

const TICKS: u32 = 10;

fn main() {
    let plate_numbers = build_plate_numbers();
    let mut controller = build_controller();
    controller.start_cycle(DEFAULT_GREEN);

    // Warm up the parallel runtime's thread pool
    {
        let warmup_plates: Vec<String> =
            (0..crate::sample::PLATES_PER_LANE)
                .map(|i| format!("warmup{:08}", i))
                .collect();
        let _ = crate::detect::check_ambulance(&warmup_plates);
    }

    let t_total = Instant::now();
    let mut t_check_ms = 0.0_f64;
    let mut total_check_calls: u64 = 0;

    for _tick in 0..TICKS {
        // detect_ambulance runs check_ambulance on every lane
        let t = Instant::now();
        let detected = controller.detect_ambulance(&plate_numbers);
        t_check_ms += t.elapsed().as_secs_f64() * 1000.0;
        total_check_calls += plate_numbers.len() as u64;

        controller.ambulance_queue = detected;

        if controller.mode == "normal" {
            controller.normal_cycle();
        } else if controller.mode == "ambulance" {
            controller.save();
            controller.logln("AMBULANCE".to_string());
            while !controller.ambulance_queue.is_empty() {
                let id = controller.ambulance_queue.remove(0);
                if controller.current_green_id == id {
                    let t = Instant::now();
                    ambulance_light_green(&mut controller, &id, &plate_numbers);
                    t_check_ms += t.elapsed().as_secs_f64() * 1000.0;
                    total_check_calls += 2; // helper does 2 checks
                } else if can_change_light(&controller) {
                    let t = Instant::now();
                    ambulance_light_red(&mut controller, &id, &plate_numbers);
                    t_check_ms += t.elapsed().as_secs_f64() * 1000.0;
                    total_check_calls += 2;
                } else if cannot_change_light(&controller) {
                    controller.ambulance_queue.insert(0, id);
                    controller.normal_cycle();
                }
            }
        }
    }
    let total_ms = t_total.elapsed().as_secs_f64() * 1000.0;

    // Log digest exposes serial vs parallel divergence
    let mut sum: u64 = 0;
    for line in &controller.log {
        for b in line.bytes() {
            sum = sum.wrapping_mul(1315423911).wrapping_add(b as u64);
        }
        sum = sum.wrapping_add(0xDEAD_BEEF);
    }

    println!("plates_per_lane={} ticks={} check_calls={}", PLATES_PER_LANE, TICKS, total_check_calls);
    println!("total_ms={:.3} hot_check_ms={:.3} log_lines={}", total_ms, t_check_ms, controller.log.len());
    println!("LOG_DIGEST=0x{:016x}", sum);
    println!("PASS");
}
