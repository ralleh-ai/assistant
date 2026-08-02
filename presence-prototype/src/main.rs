//! Phase 1 standalone prototype for the Point Cloud Presence / Live System
//! Scanner. See `docs/PRESENCE_VISUAL_ENTITY.md`, `docs/PRESENCE_SCENES.md`,
//! and `docs/PRESENCE_INTEGRATION_PLAN.md` in the main repo for the design
//! and rationale. This crate is intentionally standalone — see this
//! crate's `README.md`.

mod app;
mod palette;
mod render;
mod scene;
mod sim;
mod ui;

use winit::event_loop::{ControlFlow, EventLoop};

fn main() {
    env_logger::init();

    let event_loop = EventLoop::new().expect("failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = app::App::new();
    event_loop.run_app(&mut app).expect("event loop error");
}
