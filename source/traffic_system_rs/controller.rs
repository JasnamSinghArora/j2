// ControllerClass.py port; sleeps dropped for determinism

use std::collections::HashMap;

use crate::manage::detect_lanes;
use crate::traffic_light::{TrafficLight, DEFAULT_GREEN};

pub struct Controller {
    pub lights: HashMap<String, TrafficLight>,
    pub normal_order: Vec<String>,
    pub ambulance_queue: Vec<String>,
    pub current_green_id: String,
    pub mode: String,
    pub saved_order: Vec<String>,
    pub log: Vec<String>,
}

impl Controller {
    pub fn new(
        lights: HashMap<String, TrafficLight>,
        normal_order: Vec<String>,
        current_green_id: &str,
    ) -> Self {
        Controller {
            lights,
            normal_order,
            ambulance_queue: Vec::new(),
            current_green_id: current_green_id.to_string(),
            mode: String::new(),
            saved_order: Vec::new(),
            log: Vec::new(),
        }
    }

    pub fn logln(&mut self, s: String) { self.log.push(s); }

    pub fn start_cycle(&mut self, t: u32) {
        let first = self.normal_order.remove(0);
        self.normal_order.push(first.clone());
        self.current_green_id = first.clone();
        let order_snapshot: Vec<String> = self.normal_order.clone();
        for id in order_snapshot {
            if id == self.current_green_id {
                if let Some(l) = self.lights.get_mut(&id) {
                    l.set_green(t);
                }
            } else if let Some(l) = self.lights.get_mut(&id) {
                l.set_red();
            }
        }
    }

    pub fn detect_ambulance(
        &mut self,
        plate_numbers: &HashMap<String, Vec<String>>,
    ) -> Vec<String> {
        let detected = detect_lanes(plate_numbers);
        self.mode = if detected.is_empty() {
            "normal".to_string()
        } else {
            "ambulance".to_string()
        };
        detected
    }

    pub fn save(&mut self) { self.saved_order = self.normal_order.clone(); }

    pub fn restore(&mut self, t: u32) {
        self.normal_order = self.saved_order.clone();
        self.start_cycle(t);
    }

    pub fn prioritize_ambulance(&mut self, ambulance_light_id: &str) {
        let ids: Vec<String> = self.lights.keys().cloned().collect();
        for id in ids {
            if id == ambulance_light_id {
                let l = self.lights.get_mut(&id).unwrap();
                if l.state == "red" {
                    l.set_green(DEFAULT_GREEN);
                }
            } else {
                let state = self
                    .lights
                    .get(&id)
                    .map(|l| l.state.clone())
                    .unwrap_or_default();
                if state == "green" {
                    if let Some(l) = self.lights.get_mut(&id) {
                        l.set_yellow();
                    }
                    self.logln("yellow".to_string());
                    if let Some(l) = self.lights.get_mut(&id) {
                        l.set_red();
                    }
                }
            }
        }
    }

    pub fn reorder(order: Vec<String>) -> Vec<String> {
        let mut o = order;
        if let Some(last) = o.pop() {
            o.insert(0, last);
        }
        o
    }

    pub fn normal_cycle(&mut self) {
        let cgid = self.current_green_id.clone();
        let (light_state, remaining) = {
            let l = self.lights.get(&cgid).unwrap();
            (l.state.clone(), l.remaining_time)
        };

        if remaining == 0 && light_state == "green" {
            self.lights.get_mut(&cgid).unwrap().set_yellow();
        } else if remaining == 0 && light_state == "yellow" {
            self.lights.get_mut(&cgid).unwrap().set_red();
            if self.normal_order.last() != Some(&cgid) {
                self.normal_order.push(cgid.clone());
            }
            if self.mode == "normal" {
                let next = self.normal_order.remove(0);
                self.current_green_id = next.clone();
                self.lights
                    .get_mut(&next)
                    .unwrap()
                    .set_green(DEFAULT_GREEN);
            }
        } else {
            self.logln(format!("{} | {} | {}", cgid, light_state, remaining));
            if let Some(l) = self.lights.get_mut(&cgid) {
                l.remaining_time -= 1;
            }
        }
    }
}
