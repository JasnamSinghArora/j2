// detect.py port; force_fn keeps closure kind Fn

use crate::add_data::ambulance_plate_db;

// Static reference into the ambulance database
fn db_static() -> &'static [&'static str] {
    use std::sync::OnceLock;
    static DB: OnceLock<Vec<&'static str>> = OnceLock::new();
    DB.get_or_init(ambulance_plate_db)
}

#[inline(always)]
fn force_fn<F: Fn(&&String) -> bool + Sync>(f: F) -> F { f }

#[inline(never)]
pub fn check_ambulance(plates: &[String]) -> usize {
    let db = db_static();
    let f = force_fn(|p: &&String| db.contains(&p.as_str()));
    plates.iter().filter(f).count()
}
