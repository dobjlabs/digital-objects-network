use std::sync::Arc;

use driver::Driver;

use crate::events::EventTx;
use crate::mcp::McpRuntime;
use crate::runs::RunRegistry;

#[derive(Clone)]
pub struct AppState {
    pub driver: Arc<Driver>,
    pub events: EventTx,
    pub runs: RunRegistry,
    pub mcp: Arc<McpRuntime>,
}

impl AppState {
    pub fn new(
        driver: Arc<Driver>,
        events: EventTx,
        runs: RunRegistry,
        mcp: Arc<McpRuntime>,
    ) -> Self {
        Self {
            driver,
            events,
            runs,
            mcp,
        }
    }
}
