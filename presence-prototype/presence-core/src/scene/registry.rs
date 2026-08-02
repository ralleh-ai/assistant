//! Scene registration — `docs/PRESENCE_SCENES.md` §5.4 and §8's
//! "Adding a scene" flow.
//!
//! Phase 1's `SceneDirector` still hardcodes exactly two entities in
//! `director::SceneDirector::new` rather than instantiating from a builder
//! stored here; a real factory-registered director is speculative for a
//! two-scene prototype and would trade a lot of dynamism for no shipping
//! behaviour. The registry's job right now is narrower:
//!
//! - It is the *single* description of what scenes exist, so the debug
//!   overlay and any future integration point read the same names,
//!   priorities, and entity kinds instead of duplicating literals.
//! - The builtin set has to match what the director actually constructs;
//!   `builtins_match_the_scene_director` in `director` asserts this at test
//!   time, so a scene added to one side but not the other is a compilation
//!   *check* rather than a runtime surprise.
//! - New scenes register their descriptor first (`register`), then wire
//!   their factory into `SceneDirector::new` — the sequence documented in
//!   `docs/PRESENCE_SCENES.md` §8.
//!
//! When the director generalises to consume factories from the registry
//! directly, this file's shape is what that generalisation extends; the
//! rest of the code base already reads only from here.

use std::collections::HashMap;

use crate::scene::entity::EntityKind;

pub type SceneId = &'static str;

// These fields form the registry's public contract; they are read by the
// `builtins_match_the_scene_director` test in `director` and by the future
// factory-registered director path described at the top of this file. The
// non-test binary happens not to read them today, and the compiler cannot
// tell the difference between "field never read yet" and "field part of an
// API someone will register descriptors against tomorrow". The `#[allow]`
// documents that difference so it does not gradually become an argument
// for deleting the fields.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub struct SceneDescriptor {
    pub id: SceneId,
    pub label: &'static str,
    pub summary: &'static str,
    /// The entity kind this scene builds. Kept on the descriptor so the
    /// registry itself knows the truth of every scene rather than only its
    /// display text — that is what makes the
    /// `builtins_match_the_scene_director` test possible.
    pub entity_kind: EntityKind,
    /// Compositing order — `0` is background, higher numbers layer over it.
    /// Two scenes with the same priority are drawn in registration order,
    /// which is only stable within the builtin set and is not something
    /// callers should depend on.
    pub priority: u8,
    /// Whether the scene starts active when the presence launches. Only one
    /// builtin is active on launch (the shell); the loading plate is
    /// summoned on demand.
    pub default_active: bool,
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
            label: "Idle — Presence Shell",
            summary: "Always active. Folded surface with the mode terms \
                      resolving to zero; the shell any mode raises weights on.",
            entity_kind: EntityKind::AssistantCloud,
            priority: 0,
            default_active: true,
        });
        registry.register(SceneDescriptor {
            id: "loading",
            label: "Loading — Chladni Plate",
            summary: "Secondary entity. Grains migrating onto the nodal \
                      lines of a driven square plate; toggled on/off.",
            entity_kind: EntityKind::LoadingRing,
            priority: 1,
            default_active: false,
        });
        registry
    }

    /// Registers a scene. Also used by third-party integrators: the future
    /// factory-based path will accept a builder here rather than the plain
    /// descriptor, but the shape of the call — one function, one entry per
    /// scene — is what §8's "adding a scene" flow relies on.
    pub fn register(&mut self, descriptor: SceneDescriptor) {
        self.scenes.insert(descriptor.id, descriptor);
    }

    // See the comment above `SceneDescriptor` — these are the registry's
    // read side, exercised by the crate's tests and by the future
    // consumers described in this module's docs.
    #[allow(dead_code)]
    pub fn get(&self, id: SceneId) -> Option<&SceneDescriptor> {
        self.scenes.get(id)
    }

    pub fn all(&self) -> impl Iterator<Item = &SceneDescriptor> {
        self.scenes.values()
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.scenes.len()
    }

    /// Companion to [`len`] required by clippy for public APIs. Always
    /// `false` in the current build because `with_builtin_scenes` inserts
    /// two entries, but the method is kept so callers can compose their own
    /// registries in the future without a wart.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.scenes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_expose_the_expected_ids_and_kinds() {
        let registry = SceneRegistry::with_builtin_scenes();
        assert_eq!(registry.len(), 2);
        let idle = registry.get("idle").expect("idle scene missing");
        assert_eq!(idle.entity_kind, EntityKind::AssistantCloud);
        assert!(idle.default_active);
        let loading = registry.get("loading").expect("loading scene missing");
        assert_eq!(loading.entity_kind, EntityKind::LoadingRing);
        assert!(!loading.default_active);
        assert!(loading.priority > idle.priority);
    }

    #[test]
    fn register_replaces_an_existing_descriptor_by_id() {
        let mut registry = SceneRegistry::with_builtin_scenes();
        let before = registry.get("idle").unwrap().label;
        registry.register(SceneDescriptor {
            id: "idle",
            label: "Idle — reshaped",
            summary: "replacement",
            entity_kind: EntityKind::AssistantCloud,
            priority: 0,
            default_active: true,
        });
        let after = registry.get("idle").unwrap().label;
        assert_ne!(before, after);
        assert_eq!(after, "Idle — reshaped");
    }
}
