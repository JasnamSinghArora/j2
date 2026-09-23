// TrafficLightClass.py port; -1 remaining_time means None

pub const DEFAULT_GREEN: u32 = 7;
pub const DEFAULT_YELLOW: u32 = 2;
pub const MINIMUM_GREEN_TIME: u32 = 5;

#[derive(Debug, Clone)]
pub struct TrafficLight {
    pub id: String,
    pub state: String,
    pub remaining_time: i32,
    pub saved_time: i32,
}

impl TrafficLight {
    pub fn new(id: &str) -> Self {
        TrafficLight {
            id: id.to_string(),
            state: String::new(),
            remaining_time: -1,
            saved_time: -1,
        }
    }

    pub fn set_green(&mut self, t: u32) {
        if self.state != "green" {
            self.state = "green".to_string();
        }
        self.remaining_time = t as i32;
    }

    pub fn set_yellow(&mut self) {
        if self.state != "yellow" {
            self.state = "yellow".to_string();
        }
        self.remaining_time = DEFAULT_YELLOW as i32;
    }

    pub fn set_red(&mut self) {
        if self.state != "red" {
            self.state = "red".to_string();
        }
        self.remaining_time = 0;
    }
}
