// Python traffic-system port; expect zero matcher hits

use std::collections::{BTreeMap, HashMap};

const DEFAULT_GREEN: u32 = 7;
const DEFAULT_YELLOW: u32 = 2;
const MINIMUM_GREEN_TIME: u32 = 5;

// ------------------ TrafficLight (TrafficLightClass.py) -------------
#[derive(Debug, Clone)]
struct TrafficLight {
    id: String,
    state: String,            // "red" | "yellow" | "green" | ""
    remaining_time: i32,      // -1 = "None"
}

impl TrafficLight {
    fn new(id: &str) -> Self {
        TrafficLight { id: id.to_string(), state: String::new(), remaining_time: -1 }
    }
    fn set_green(&mut self, t: u32) {
        if self.state != "green" {
            self.state = "green".to_string();
        }
        self.remaining_time = t as i32;
    }
    fn set_yellow(&mut self) {
        if self.state != "yellow" {
            self.state = "yellow".to_string();
        }
        self.remaining_time = DEFAULT_YELLOW as i32;
    }
    fn set_red(&mut self) {
        if self.state != "red" {
            self.state = "red".to_string();
        }
        self.remaining_time = 0;
    }
}

// detect.check_ambulance; hardcoded plate db replaces SQLite
fn ambulance_plate_db() -> Vec<&'static str> {
    vec![
        "PB10AB1234", "DL09CD5678", "MH04XY7890", "RJ14EF3456",
        "KA03GH5678", "TN07IJ6789", "UP16KL7890", "WB20MN8901",
        "CH01OP9012", "GJ05QR0123", "KA19P8488",
    ]
}

// Force Fn kind; FnMut inference blocks matcher
#[inline(always)]
fn force_fn<F: Fn(&&String) -> bool + Sync>(f: F) -> F { f }

#[inline(never)]
fn check_ambulance(plates: &[String], db: &[&'static str]) -> usize {
    // Python count loop; force_fn so matcher fires
    let f = force_fn(|p: &&String| db.contains(&p.as_str()));
    plates.iter().filter(f).count()
}

// ------------------ manage.detect_lanes (manage.py) -----------------
fn detect_lanes(plate_numbers: &HashMap<String, Vec<String>>) -> Vec<String> {
    let db = ambulance_plate_db();
    let mut lane_to_count: BTreeMap<String, usize> = BTreeMap::new();
    for (camera_id, plates) in plate_numbers.iter() {
        let cnt = check_ambulance(plates, &db);
        if cnt > 0 {
            lane_to_count.insert(camera_id.clone(), cnt);
        }
    }
    // Count descending, tie-break camera id ascending
    let mut entries: Vec<(String, usize)> = lane_to_count.into_iter().collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    entries.into_iter().map(|(k, _)| k).collect()
}

// ------------------ Controller (ControllerClass.py) -----------------
struct Controller {
    lights: HashMap<String, TrafficLight>,
    normal_order: Vec<String>,
    ambulance_queue: Vec<String>,
    current_green_id: String,
    mode: String,                // "" | "ambulance" | "normal"
    saved_order: Vec<String>,
    log: Vec<String>,            // capture all logs for output diff
}

impl Controller {
    fn new(lights: HashMap<String, TrafficLight>, normal_order: Vec<String>, current_green_id: &str) -> Self {
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
    fn logln(&mut self, s: String) { self.log.push(s); }

    fn start_cycle(&mut self, t: u32) {
        let first = self.normal_order.remove(0);
        self.normal_order.push(first.clone());
        self.current_green_id = first.clone();
        // Iterate normal_order: turn current_green green, others red.
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

    fn detect_ambulance(&mut self, plate_numbers: &HashMap<String, Vec<String>>) -> Vec<String> {
        let detected = detect_lanes(plate_numbers);
        self.mode = if detected.is_empty() {
            "normal".to_string()
        } else {
            "ambulance".to_string()
        };
        detected
    }

    fn save(&mut self) { self.saved_order = self.normal_order.clone(); }

    fn restore(&mut self, t: u32) {
        self.normal_order = self.saved_order.clone();
        self.start_cycle(t);
    }

    fn prioritize_ambulance(&mut self, ambulance_light_id: &str) {
        // Snapshot keys to avoid double-borrow.
        let ids: Vec<String> = self.lights.keys().cloned().collect();
        for id in ids {
            if id == ambulance_light_id {
                let l = self.lights.get_mut(&id).unwrap();
                if l.state == "red" { l.set_green(DEFAULT_GREEN); }
            } else {
                let state = self.lights.get(&id).map(|l| l.state.clone()).unwrap_or_default();
                if state == "green" {
                    if let Some(l) = self.lights.get_mut(&id) {
                        l.set_yellow();
                    }
                    self.logln("yellow".to_string());
                    // Skip time.sleep - deterministic test
                    if let Some(l) = self.lights.get_mut(&id) {
                        l.set_red();
                    }
                }
            }
        }
    }

    fn reorder(order: Vec<String>) -> Vec<String> {
        let mut o = order;
        if let Some(last) = o.pop() {
            o.insert(0, last);
        }
        o
    }

    fn normal_cycle(&mut self) {
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
                self.lights.get_mut(&next).unwrap().set_green(DEFAULT_GREEN);
            }
        } else {
            self.logln(format!(
                "{} | {} | {}",
                cgid, light_state, remaining
            ));
            if let Some(l) = self.lights.get_mut(&cgid) {
                l.remaining_time -= 1;
            }
        }
    }
}

// ------------------ helper.py functions -----------------------------
fn can_change_light(controller: &Controller) -> bool {
    let l = &controller.lights[&controller.current_green_id];
    (DEFAULT_GREEN as i32 - l.remaining_time) > MINIMUM_GREEN_TIME as i32
        || (l.state == "yellow" && l.remaining_time == 0)
}

fn cannot_change_light(controller: &Controller) -> bool {
    let l = &controller.lights[&controller.current_green_id];
    l.remaining_time != -1
        && l.remaining_time >= (DEFAULT_GREEN as i32 - MINIMUM_GREEN_TIME as i32)
}

// Deterministic 2-tick clear replaces Python's sleep loop
fn ambulance_light_green(controller: &mut Controller, id: &str, plate_numbers: &mut HashMap<String, Vec<String>>) {
    controller.prioritize_ambulance(id);
    controller.logln(format!("ambulance | {} | green | 1", id));
    controller.logln(format!("ambulance | {} | green | 2", id));
    plate_numbers.insert(id.to_string(), vec!["stop".to_string()]);
    controller.logln("ambulance passed".to_string());
    if controller.ambulance_queue.is_empty() {
        controller.saved_order = Controller::reorder(controller.saved_order.clone());
        controller.restore(DEFAULT_GREEN);
    }
}

fn ambulance_light_red(controller: &mut Controller, id: &str, plate_numbers: &mut HashMap<String, Vec<String>>) {
    controller.prioritize_ambulance(id);
    controller.logln(format!("ambulance | {} | green | 1", id));
    controller.logln(format!("ambulance | {} | green | 2", id));
    plate_numbers.insert(id.to_string(), vec!["stop".to_string()]);
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

// ------------------ main pipeline (final.py) ------------------------
fn main() {
    // Sample data - same as sample.py
    let mut plate_numbers: HashMap<String, Vec<String>> = HashMap::new();
    plate_numbers.insert("up".to_string(), vec!["dtfygu".to_string(), "fyguhigubh".to_string()]);
    plate_numbers.insert("down".to_string(), vec!["hfvgubhi".to_string(), "wihbdfn".to_string()]);
    plate_numbers.insert("left".to_string(), vec!["WB20MN8901".to_string(), "yfguh".to_string()]);
    plate_numbers.insert(
        "right".to_string(),
        vec!["fyvgubhi".to_string(), "WB20MN8901".to_string(), "WB20MN8901".to_string()],
    );

    let mut lights: HashMap<String, TrafficLight> = HashMap::new();
    for id in ["up", "down", "left", "right"] {
        lights.insert(id.to_string(), TrafficLight::new(id));
    }
    let order = vec!["up".to_string(), "left".to_string(), "down".to_string(), "right".to_string()];

    let mut controller = Controller::new(lights, order, "up");
    controller.start_cycle(DEFAULT_GREEN);

    // Bounded ticks replace Python's `while True`
    for tick in 0..30 {
        let detected = controller.detect_ambulance(&plate_numbers);
        controller.ambulance_queue = detected;

        if controller.mode == "normal" {
            controller.normal_cycle();
        } else if controller.mode == "ambulance" {
            controller.save();
            controller.logln("AMBULANCE".to_string());
            while !controller.ambulance_queue.is_empty() {
                let id = controller.ambulance_queue.remove(0);
                if controller.current_green_id == id {
                    ambulance_light_green(&mut controller, &id, &mut plate_numbers);
                } else if can_change_light(&controller) {
                    ambulance_light_red(&mut controller, &id, &mut plate_numbers);
                } else if cannot_change_light(&controller) {
                    controller.ambulance_queue.insert(0, id);
                    controller.normal_cycle();
                }
            }
        }
        let _ = tick;
    }

    // Stable output, diffable across parallelize on/off
    println!("=== TRAFFIC SYSTEM LOG ({} entries) ===", controller.log.len());
    for line in &controller.log {
        println!("{}", line);
    }
    println!("=== FINAL STATE ===");
    let mut light_keys: Vec<&String> = controller.lights.keys().collect();
    light_keys.sort();
    for id in light_keys {
        let l = &controller.lights[id];
        println!("{}: state={} remaining={}", id, l.state, l.remaining_time);
    }
}
