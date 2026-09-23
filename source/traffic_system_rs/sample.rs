// sample.py port; scaled up past PARALLEL_REDUCE_THRESHOLD

use std::collections::HashMap;

use crate::add_data::ambulance_plate_db;
use crate::controller::Controller;
use crate::traffic_light::TrafficLight;

/// Plates per lane, sized to amortize dispatch
pub const PLATES_PER_LANE: usize = 2_000_000;

/// Synthetic lane plates; every 1000th is ambulance
fn build_lane_plates(seed: usize, n: usize) -> Vec<String> {
    let db = ambulance_plate_db();
    (0..n)
        .map(|i| {
            if i % 1000 == 0 {
                db[(seed + i / 1000) % db.len()].to_string()
            } else {
                format!("XX{:02}{}{:06}", seed % 100, lane_letter(seed), i)
            }
        })
        .collect()
}

fn lane_letter(seed: usize) -> char {
    match seed % 4 {
        0 => 'U',
        1 => 'D',
        2 => 'L',
        _ => 'R',
    }
}

pub fn build_plate_numbers() -> HashMap<String, Vec<String>> {
    let mut p: HashMap<String, Vec<String>> = HashMap::new();
    p.insert("up".into(), build_lane_plates(0, PLATES_PER_LANE));
    p.insert("down".into(), build_lane_plates(1, PLATES_PER_LANE));
    p.insert("left".into(), build_lane_plates(2, PLATES_PER_LANE));
    p.insert("right".into(), build_lane_plates(3, PLATES_PER_LANE));
    p
}

pub fn build_controller() -> Controller {
    let mut lights: HashMap<String, TrafficLight> = HashMap::new();
    for id in ["up", "down", "left", "right"] {
        lights.insert(id.to_string(), TrafficLight::new(id));
    }
    let order = vec![
        "up".to_string(),
        "left".to_string(),
        "down".to_string(),
        "right".to_string(),
    ];
    Controller::new(lights, order, "up")
}
