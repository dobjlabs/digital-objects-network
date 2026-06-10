use std::sync::Arc;

use driver::Driver;

use crate::events::EventTx;
use crate::runs::RunRegistry;

#[derive(Clone)]
pub struct AppState {
    pub driver: Arc<Driver>,
    pub events: EventTx,
    pub runs: RunRegistry,
}

impl AppState {
    pub fn new(driver: Arc<Driver>, events: EventTx, runs: RunRegistry) -> Self {
        Self {
            driver,
            events,
            runs,
        }
    }
}
