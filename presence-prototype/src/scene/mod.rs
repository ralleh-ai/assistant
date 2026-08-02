pub mod director;
pub mod entity;
pub mod mode;
pub mod registry;

pub use director::SceneDirector;
#[allow(unused_imports)]
pub use mode::{ModeLayer, PresenceMode};
// Re-exported for future consumers (e.g. a real Scene Director consuming
// `SceneRegistry` dynamically, per `docs/PRESENCE_SCENES.md` §5.4) — not
// all of these are named directly by this Phase 1 binary yet.
#[allow(unused_imports)]
pub use entity::{EntityInstance, EntityKind};
#[allow(unused_imports)]
pub use registry::{SceneDescriptor, SceneId, SceneRegistry};
