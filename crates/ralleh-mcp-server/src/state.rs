use std::sync::Arc;

use ralleh_ai_router::AiRouter;
use ralleh_tool_gateway::ToolGateway;

/// Shared application state for the Axum router. Wraps the `ToolGateway`
/// and `AiRouter` behind `Arc`s so they can be cloned cheaply into every
/// request handler without duplicating the registry, policy engine, or
/// backend.
#[derive(Clone)]
pub struct AppState {
    pub gateway: Arc<ToolGateway>,
    pub ai_router: Arc<AiRouter>,
}

impl AppState {
    pub fn new(gateway: ToolGateway, ai_router: AiRouter) -> Self {
        Self {
            gateway: Arc::new(gateway),
            ai_router: Arc::new(ai_router),
        }
    }
}
