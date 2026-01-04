pub mod process;
pub mod spawn;

use std::sync::Mutex;

static OVERRIDE_PORT: Mutex<Option<u16>> = Mutex::new(None);

pub fn set_override_port(port: u16) {
    if let Ok(mut p) = OVERRIDE_PORT.lock() {
        *p = Some(port);
    }
}

pub fn get_override_port() -> Option<u16> {
    OVERRIDE_PORT.lock().ok().and_then(|p| *p)
}
