//! Per-entity compositing mode — overlay vs replace (`PRESENCE_ADAPTIVE_SCENES` §3.0).

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Disposition {
    /// Composite alongside the cloud; the director subdues the shell via the
    /// existing attention-hierarchy path.
    #[default]
    Overlay,
    /// Crossfade: cloud `presence → 0` while this scene fades in; cloud
    /// returns on dismiss / TTL.
    Replace,
}
