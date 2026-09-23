// helper.py port; sleep polling becomes 2-tick simulation

use std::collections::HashMap;

use crate::controller::Controller;
use crate::detect::check_ambulance;
use crate::traffic_light::{DEFAULT_GREEN, MINIMUM_GREEN_TIME};

pub fn can_change_light(controller: &Controller) -> bool {
    let l = &controller.lights[&controller.current_green_id];
    (DEFAULT_GREEN as i32 - l.remaining_time) > MINIMUM_GREEN_TIME as i32
        || (l.state == "yellow" && l.remaining_time == 0)
}

pub fn cannot_change_light(controller: &Controller) -> bool {
    let l = &controller.lights[&controller.current_green_id];
    l.remaining_time != -1
        && l.remaining_time >= (DEFAULT_GREEN as i32 - MINIMUM_GREEN_TIME as i32)
}

pub fn ambulance_light_green(
    controller: &mut Controller,
    id: &str,
    plate_numbers: &HashMap<String, Vec<String>>,
) {
    controller.prioritize_ambulance(id);
    // 2-tick sim, plates unshrunk so parallelism measurable
    for t in 1..=2u32 {
        let _ = check_ambulance(plate_numbers.get(id).map(|v| v.as_slice()).unwrap_or(&[]));
        controller.logln(format!("ambulance | {} | green | {}", id, t));
    }
    controller.logln("ambulance passed".to_string());
    if controller.ambulance_queue.is_empty() {
        controller.saved_order = Controller::reorder(controller.saved_order.clone());
        controller.restore(DEFAULT_GREEN);
    }
}

pub fn ambulance_light_red(
    controller: &mut Controller,
    id: &str,
    plate_numbers: &HashMap<String, Vec<String>>,
) {
    controller.prioritize_ambulance(id);
    for t in 1..=2u32 {
        let _ = check_ambulance(plate_numbers.get(id).map(|v| v.as_slice()).unwrap_or(&[]));
        controller.logln(format!("ambulance | {} | green | {}", id, t));
    }
    controller.logln("ambulance passed".to_string());
    if let Some(l) = controller.lights.get_mut(id) {
        l.set_yellow();
    }
    controller.logln("yellow".to_string());
    if let Some(l) = controller.lights.get_mut(id) {
        l.set_red();
    }
    controller.restore(DEFAULT_GREEN);
    controller.logln("restored".to_string());
}
