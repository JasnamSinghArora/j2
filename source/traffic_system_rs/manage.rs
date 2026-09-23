// manage.py port; explicit tie-break since HashMap unordered

use std::collections::HashMap;

use crate::detect::check_ambulance;

#[allow(dead_code)]
pub const CAMERAS: [&str; 4] = ["up", "down", "right", "left"];

pub fn detect_lanes(plate_numbers: &HashMap<String, Vec<String>>) -> Vec<String> {
    let mut lane_to_count: Vec<(String, usize)> = Vec::new();
    for (camera_id, plates) in plate_numbers.iter() {
        let cnt = check_ambulance(plates);
        if cnt > 0 {
            lane_to_count.push((camera_id.clone(), cnt));
        }
    }
    // count desc, then id asc
    lane_to_count.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    lane_to_count.into_iter().map(|(k, _)| k).collect()
}
