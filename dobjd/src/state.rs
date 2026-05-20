use std::sync::Arc;

use driver::Driver;

use crate::events::EventTx;

#[derive(Clone)]
pub struct AppState {
    pub driver: Arc<Driver>,
    pub events: EventTx,
}

impl AppState {
    pub fn new(driver: Arc<Driver>, events: EventTx) -> Self {
        Self { driver, events }
    }
}
