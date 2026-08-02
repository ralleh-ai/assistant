//! Scene registration — `docs/PRESENCE_SCENES.md` §5.4.
//!
//! Phase 1's `SceneDirector` hardcodes exactly two entities (see
//! `director.rs`) rather than instantiating from this registry
//! dynamically — that wiring is a fair chunk of extra machinery for a
//! two-scene prototype and would be speculative right now. This registry
//! exists anyway as the "clear registration point" the design calls for
//! (`docs/PRESENCE_SCENES.md` §9 item 5): it's where a future scene's
//! *description* is declared, and the debug overlay reads from it, so
//! adding a third scene starts here even before the director loop is
//! generalized to consume it directly.

use std::collections::HashMap;

pub type SceneId = &'static str;

#[derive(Clone, Copy, Debug)]
pub struct SceneDescriptor {
    pub id: SceneId,
    pub label: &'static str,
    pub summary: &'static str,
}

pub struct SceneRegistry {
    scenes: HashMap<SceneId, SceneDescriptor>,
}

impl SceneRegistry {
    pub fn with_builtin_scenes() -> Self {
        let mut registry = Self {
            scenes: HashMap::new(),
        };
        registry.register(SceneDescriptor {
            id: "idle",
            label: "Idle — Viscous Cloud",
            summary: "Always active. Slow rise/fall clusters, low energy.",
        });
        registry.register(SceneDescriptor {
            id: "loading",
            label: "Loading — Resonance Field",
            summary: "Secondary entity. Chladni-style standing wave, toggled on/off.",
        });
        registry
    }

    pub fn register(&mut self, descriptor: SceneDescriptor) {
        self.scenes.insert(descriptor.id, descriptor);
    }

    /// Not yet called by this Phase 1 binary (the debug overlay iterates
    /// `all()` instead) — kept as the registry's primary lookup API for
    /// when the `SceneDirector` consumes this dynamically (see the module
    /// doc comment).
    #[allow(dead_code)]
    pub fn get(&self, id: SceneId) -> Option<&SceneDescriptor> {
        self.scenes.get(id)
    }

    pub fn all(&self) -> impl Iterator<Item = &SceneDescriptor> {
        self.scenes.values()
    }
}
