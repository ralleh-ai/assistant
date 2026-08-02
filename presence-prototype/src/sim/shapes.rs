//! Parametric surfaces — points *on* a skin rather than through a volume.
//!
//! This sits alongside the generator/behavior split in
//! `docs/PRESENCE_SCENES.md` §5 rather than replacing it: a `SurfaceShape`
//! answers "where is the skin right now", `SurfaceGenerator` seeds a
//! population across it, and `SurfaceBehavior` is an ordinary `PointBehavior`
//! that springs particles toward it.
//!
//! ## Why surfaces at all
//!
//! `ClusterGenerator` distributes points through sphere volumes, and no amount
//! of tuning makes that read as scanned: a LiDAR return only ever comes from a
//! surface. Three consequences follow from the volume model directly, and each
//! one is a thing the reference concepts have and a volume fill structurally
//! cannot:
//!
//! - **No silhouette.** A volume is brightest at its centre; a surface is
//!   brightest at its grazing rim, because that is where the skin's depth along
//!   the view ray is greatest. That rim is what makes a point cloud read as
//!   solid.
//! - **No creases.** Fold filaments are surface curvature. A volume has no
//!   surface, so it has no folds to brighten.
//! - **Wrong point budget and size.** Covering a surface takes far more, far
//!   smaller points than filling a volume at the same apparent detail.
//!
//! ## Why parametric rather than projecting onto an implicit surface
//!
//! Projecting each particle onto an SDF every frame needs a field gradient —
//! roughly four field evaluations plus Newton steps per particle, per step. At
//! the point counts this needs, that is far outside a 2-core budget. A
//! parametric shell evaluates displacement along a fixed radial seed in about
//! three evaluations with no iteration, and hands back an exact normal for
//! free.

use glam::{Mat3, Vec3};
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

use super::behaviors::PointBehavior;
use super::generators::PointGenerator;
use super::noise::NoiseField;
use super::types::{hash01, EntityParams, Layer, Particle, PresenceSignals, ShellDrive};

/// One point on a deformed surface, in world space.
pub struct SurfaceSample {
    pub position: Vec3,
    /// Outward unit normal. Drives the grazing/silhouette term in the point
    /// shader and the per-layer off-skin offset here.
    pub normal: Vec3,
}

/// The expensive, slowly-varying part of a shape at one seed.
#[derive(Clone, Copy, Debug)]
pub struct SurfaceDeform {
    /// The surface point in the shape's own local space, before this frame's
    /// rigid motion and breathing are applied.
    pub local: Vec3,
    /// `0..1` fold intensity — how much this spot sits on a crease.
    pub crease: f32,
}

/// A deformable skin that particles live on.
///
/// The trait is split three ways because a shape's work divides cleanly by how
/// often it actually needs doing, and the per-particle loop runs tens of
/// thousands of times per step:
///
/// - `frame` — once per step. Rotation matrices, breathing scale, mode indices.
/// - `deform` — the noise. Expensive (this is essentially the whole simulation
///   cost) but slowly varying, so `SurfaceBehavior` refreshes it for a rotating
///   fraction of the population each step instead of all of it.
/// - `place` — every step for every particle. Rigid motion and breathing only,
///   which must stay smooth and is nearly free.
///
/// Collapsing these into one `sample` call is the obvious design and it caps
/// the point budget at roughly a quarter of what the references need, because
/// it forces the noise to be re-evaluated at the frame rate for motion that
/// takes seconds to develop.
pub trait SurfaceShape {
    /// Values shared by every particle this frame.
    type Frame;

    /// The parameter space this shape's seeds are drawn from.
    fn domain(&self) -> SurfaceDomain;

    fn frame(&self, params: &EntityParams) -> Self::Frame;

    fn deform(&self, seed: Vec3, frame: &Self::Frame) -> SurfaceDeform;

    fn place(
        &self,
        seed: Vec3,
        local: Vec3,
        frame: &Self::Frame,
        params: &EntityParams,
    ) -> SurfaceSample;
}

/// Share of points tagged `Core`, then `Body`; the remainder is `Halo`.
///
/// On a surface, `Core` means *on the skin*, so the skin is the main population
/// — the inverse of the volumetric split, where the core was a minority at the
/// centre. `Body` is a thin scatter just off the skin and `Halo` is the sparse
/// drift that gives §3.3's atmosphere without blurring the silhouette the skin
/// exists to draw.
///
/// This is where §9's `core_density_bias` lands. Its volumetric meaning —
/// how radially centre-weighted the fill is — has no analogue on a surface,
/// since there is no interior to weight; the knob instead sets how much of the
/// population sits exactly on the skin. Low values read hazy and soft-edged,
/// high values read as a hard scanned shell with almost no atmosphere. The
/// halo keeps a floor at either extreme, because an empty layer silently turns
/// its material gradient into dead code.
fn layer_quantiles(bias: f32) -> (f32, f32) {
    let bias = bias.clamp(0.0, 1.0);
    let core = 0.42 + 0.40 * bias;
    let halo_share = 0.04 + 0.08 * (1.0 - bias);
    (core, 1.0 - halo_share)
}

/// How far off the skin each layer may sit, as a fraction of entity scale.
///
/// `Core` is not exactly zero: a mathematically perfect shell renders as a
/// suspiciously clean surface, and real scan returns have depth noise. This is
/// small enough to stay a surface and large enough to look measured.
const CORE_SHELL: f32 = 0.006;
const BODY_SHELL: f32 = 0.035;
const HALO_SHELL_NEAR: f32 = 0.05;
const HALO_SHELL_FAR: f32 = 0.20;

/// How far in from the rim the sheet's density starts thinning.
const SHEET_RIM: f32 = 0.74;

/// The parameter space a shape's seeds are drawn from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceDomain {
    /// Unit directions, for shells.
    Sphere,
    /// Unit-square coordinates in XY with a `[-1, 1]` thickness seed in Z,
    /// for plates. Square rather than round because the Chladni figures a
    /// plate is recognisable by are square-plate figures; a circular plate's
    /// modes are Bessel patterns, which are a different (and much more
    /// expensive) shape.
    Sheet,
}

impl SurfaceDomain {
    fn seed(self, rng: &mut SmallRng) -> Vec3 {
        match self {
            SurfaceDomain::Sphere => random_unit_vector(rng),
            SurfaceDomain::Sheet => loop {
                let x = rng.gen_range(-1.0..1.0_f32);
                let y = rng.gen_range(-1.0..1.0_f32);
                // Thin the outer margin to nothing so the sheet dissolves at
                // its rim instead of ending on four crisp straight edges —
                // `PRESENCE_VISUAL_ENTITY.md` §10 rules out "hard geometric
                // shapes that never dissolve", and a literal square outline is
                // exactly that. The figure's nodal lines simply fade out as
                // they reach the edge.
                let edge = x.abs().max(y.abs());
                if rng.gen::<f32>() < 1.0 - smoothstep(SHEET_RIM, 1.0, edge) {
                    break Vec3::new(x, y, rng.gen_range(-1.0..1.0));
                }
            },
        }
    }

    /// Where a freshly-seeded particle starts, before any deformation. Close
    /// enough to the real skin that the opening frames are a settle rather than
    /// an implosion from the origin.
    fn rest_position(self, seed: Vec3) -> Vec3 {
        match self {
            SurfaceDomain::Sphere => seed,
            SurfaceDomain::Sheet => Vec3::new(seed.x, seed.y, 0.0),
        }
    }
}

/// Seeds a population across a `SurfaceShape`'s domain.
///
/// Deliberately does *not* hold the shape, only its domain. Seeding depends
/// only on the domain's geometry, and keeping the generator shape-free means
/// the shape's noise state has exactly one owner — the behavior — so the skin
/// a particle is sprung toward can never disagree with the skin it was seeded
/// on.
pub struct SurfaceGenerator {
    domain: SurfaceDomain,
}

impl SurfaceGenerator {
    pub fn new(domain: SurfaceDomain) -> Self {
        Self { domain }
    }
}

impl PointGenerator for SurfaceGenerator {
    fn generate(&self, count: usize, params: &EntityParams) -> Vec<Particle> {
        let mut rng = SmallRng::seed_from_u64(0x5C4E_D5C1_5EED ^ self.domain as u64);
        let mut particles = Vec::with_capacity(count);
        let (core_quantile, body_quantile) = layer_quantiles(params.core_density_bias);

        for _ in 0..count {
            let seed = self.domain.seed(&mut rng);
            let quantile = rng.gen::<f32>();
            let layer = if quantile < core_quantile {
                Layer::Core
            } else if quantile < body_quantile {
                Layer::Body
            } else {
                Layer::Halo
            };

            // Sizes are tight and near-uniform. The volumetric generator gave
            // the halo the largest points so it would read as haze; a surface
            // wants the opposite, because a large soft point on a skin blurs
            // the very silhouette the skin is there to draw.
            let size = match layer {
                Layer::Core => rng.gen_range(0.45..0.75),
                Layer::Body => rng.gen_range(0.5..0.9),
                Layer::Halo => rng.gen_range(0.6..1.1),
            };

            let rest = self.domain.rest_position(seed);
            particles.push(Particle {
                position: params.center + rest * params.scale,
                base_offset: seed,
                normal: rest.normalize_or_zero(),
                // Seeded at the un-deformed surface so the first steps are a
                // settle rather than an implosion, since the staggered refresh
                // only reaches every particle after a few steps.
                local: rest,
                shell_offset: shell_offset(layer, seed),
                layer,
                size,
                ..Default::default()
            });
        }
        particles
    }
}

/// Signed distance off the skin for a particle. Static per particle: it
/// depends only on the layer and the seed, both fixed at generation.
fn shell_offset(layer: Layer, seed: Vec3) -> f32 {
    let h = hash01(seed * 7.7);
    match layer {
        Layer::Core => (h - 0.5) * 2.0 * CORE_SHELL,
        Layer::Body => (h - 0.5) * 2.0 * BODY_SHELL,
        // Halo sits outside only. Inside the skin it would be occluded by the
        // surface it is meant to be the atmosphere around.
        Layer::Halo => HALO_SHELL_NEAR + h * (HALO_SHELL_FAR - HALO_SHELL_NEAR),
    }
}

/// Default steps a full refresh of every particle's cached deform takes.
///
/// Set against how fast the deformation it caches actually moves. `PresenceShell`
/// reshapes its folds over tens of seconds, so refreshing a given particle at
/// 15 Hz rather than 60 Hz is far above what the motion can resolve — it is
/// invisible, and it is the difference between a budget of ~15k points and one
/// of ~60k.
///
/// Refreshing in strided slices rather than contiguous blocks matters: a block
/// would update one spatial region at a time, since generation walks the
/// surface in seed order, and a region snapping while its neighbours lag is
/// visible in a way that a scatter of individual particles is not.
///
/// Made a runtime field on `SurfaceBehavior` rather than a compile-time
/// constant so the quality-tier system can raise it on slower hardware —
/// each doubling roughly halves the deform cost. This is the value used
/// unless the tier system asks for something else.
pub const DEFAULT_DEFORM_STRIDE: usize = 4;

/// Springs particles onto whatever skin its `SurfaceShape` describes.
///
/// Keeps the same critically-damped spring the volumetric behaviors use, so
/// the surface deforming never reads as points teleporting — motion stays as
/// soft as `docs/PRESENCE_VISUAL_ENTITY.md` §2.3 asks for even though the
/// target is now a hard surface. The spring also quietly absorbs the staggered
/// refresh: a particle whose cached deform has just jumped is pulled to the new
/// target over several frames rather than snapping to it.
///
/// Note there is no per-particle wander noise here, unlike the volumetric
/// behavior. A volume needs it because its points have nothing else to do; a
/// surface is already breathing, turning, and reshaping its folds under them,
/// and adding noise on top only blurs the silhouette while costing as much as
/// the entire shape evaluation.
pub struct SurfaceBehavior<S: SurfaceShape> {
    pub shape: S,
    pub spring_k: f32,
    pub damping: f32,
    /// Which stride class refreshes its cached deform this step.
    refresh_phase: usize,
    /// How many steps between refreshes of a given particle's cached deform.
    /// See `DEFAULT_DEFORM_STRIDE`; higher is cheaper and less responsive.
    pub deform_stride: usize,
}

impl<S: SurfaceShape> SurfaceBehavior<S> {
    pub fn new(shape: S) -> Self {
        Self {
            shape,
            spring_k: 14.0,
            damping: 7.5,
            refresh_phase: 0,
            deform_stride: DEFAULT_DEFORM_STRIDE,
        }
    }

    /// Per-layer energy contribution.
    ///
    /// Far lower than the volumetric behavior's, and necessarily so: these are
    /// additive contributions into an HDR target, and moving from 12,000 points
    /// through a volume to tens of thousands across a surface multiplies the
    /// overlap at any given pixel. Keeping single-point energy low is what lets
    /// §3.1's near-white hotspots stay a property of genuine density instead of
    /// becoming the whole entity.
    ///
    /// `voice` is the syllable-rate speech channel. It arrives here, and not
    /// in the geometry, because the spring that carries every other kind of
    /// motion cannot pass a 4-7 Hz signal — see `PulseTerm`. Brightness is
    /// assigned outright rather than integrated, so it lands within one step.
    fn layer_brightness(layer: Layer, energy: f32, voice: f32) -> f32 {
        let lift = 0.5 * energy.min(1.0);
        match layer {
            Layer::Core => 0.105 + lift * 0.12 + voice * 0.085,
            Layer::Body => 0.060 + lift * 0.07 + voice * 0.045,
            // Barely lit by voice. The halo's job is atmosphere, and driving it
            // at syllable rate makes the whole entity flicker rather than
            // making its skin articulate.
            Layer::Halo => 0.022 + lift * 0.03 + voice * 0.012,
        }
    }
}

impl<S: SurfaceShape> PointBehavior for SurfaceBehavior<S> {
    fn set_deform_stride(&mut self, stride: usize) {
        self.deform_stride = stride.max(1);
    }

    fn update(
        &mut self,
        particles: &mut [Particle],
        dt: f32,
        params: &EntityParams,
        signals: &PresenceSignals,
    ) {
        // The live signal override is folded into a local copy so shapes see a
        // single resolved `intensity` and don't each have to remember to add
        // the two sources together (§5.2).
        let mut params = *params;
        params.intensity = (params.intensity + signals.intensity).clamp(0.0, 2.0);
        params.progress = (params.progress + signals.progress).clamp(0.0, 1.0);
        let energy = params.intensity;
        // Gated by the speaking weight so the raw level only lights the shell
        // while it is actually talking. Read straight off `signals` rather than
        // off the phrase envelope in `params` — the whole point of this channel
        // is that it is the unsmoothed one.
        let voice = params.drive.pulse * signals.audio_level.clamp(0.0, 1.0);

        let frame = self.shape.frame(&params);
        let stride = self.deform_stride.max(1);
        let phase = self.refresh_phase % stride;
        self.refresh_phase = (self.refresh_phase + 1) % stride;

        for (i, p) in particles.iter_mut().enumerate() {
            if i % stride == phase {
                let deform = self.shape.deform(p.base_offset, &frame);
                p.local = deform.local;
                // Only the skin carries creases. Off-skin layers reporting them
                // would smear the filaments into a glow and lose the structure
                // they exist to draw. Folded in here rather than at render time
                // because the layer never changes.
                p.crease = match p.layer {
                    Layer::Core => deform.crease,
                    Layer::Body => deform.crease * 0.4,
                    Layer::Halo => 0.0,
                };
            }

            let sample = self.shape.place(p.base_offset, p.local, &frame, &params);
            let target = sample.position + sample.normal * p.shell_offset * params.scale;

            let accel = (target - p.position) * self.spring_k * p.layer.spring_scale();
            p.velocity = (p.velocity + accel * dt) * (1.0 - (self.damping * dt).min(0.95));
            p.position += p.velocity * dt;

            p.normal = sample.normal;
            p.brightness = Self::layer_brightness(p.layer, energy, voice) * params.presence;
            p.color_bias = params.cool.clamp(0.0, 1.0);
        }
    }
}

/// The assistant's presence: one shell whose radius is a weighted sum of
/// independent terms — `docs/adr/adr-012-additive-mode-composition.md`.
///
/// ```text
/// r(seed, t) = 1 + Σ wᵢ · termᵢ(seed, t)
/// ```
///
/// A mode does not pick a shape here; it raises a weight in
/// [`ShellDrive`](crate::sim::ShellDrive). Two consequences follow, and both
/// are the reason for the model:
///
/// - **Concurrency is free.** An assistant narrating a tool call while still
///   thinking is three raised weights, not a three-way blend between exclusive
///   shapes that would have to be written out pairwise.
/// - **Transitions are weight lerps.** Nothing cross-fades and no population is
///   swapped, so the particle set is untouched by a state change and there is
///   no moment where the entity is two half-drawn things at once.
///
/// Every term past the first is gated on its weight and skipped outright below
/// [`ShellDrive::GATE`]. Idle therefore costs exactly what the single-purpose
/// fold shell cost before the other terms existed, which is what makes it
/// affordable to keep adding them.
///
/// `fold` is the resting identity and is the one term that never gates off. It
/// yields depth to the others under load (see the director's `FOLD_YIELD`) but
/// a mode that erased it would stop reading as the same living thing
/// (`docs/PRESENCE_VISUAL_ENTITY.md` §3.1).
pub struct PresenceShell {
    pub noise: NoiseField,
    pub base_radius: f32,
    pub fold: FoldTerm,
    pub lobes: LobeTerm,
    pub pulse: PulseTerm,
    pub neck: NeckTerm,
    pub breath_amplitude: f32,
    pub breath_speed: f32,
    /// Revolutions per second of the shell's slow turn.
    pub spin_speed: f32,
}

/// The resting signature: a sphere displaced radially by ridged noise, so it
/// reads as a folded shell or a closed rose — `docs/PRESENCE_SCENES.md` §4.1.
///
/// The whole effect rests on one thing: displace by **ridged** noise, then
/// reuse *the same ridge value* as the crease brightness. Creases then land
/// exactly on folds by construction and at zero extra cost. Computing crease
/// intensity separately is both more expensive and guaranteed to drift out of
/// alignment with the geometry it is supposed to describe.
pub struct FoldTerm {
    /// Peak-to-trough fold depth as a fraction of `base_radius`.
    pub depth: f32,
    /// Spatial frequency of the folds — effectively the petal count.
    pub scale: f32,
    pub octaves: u32,
    /// How fast the fold pattern itself reshapes, independent of the spin.
    pub evolution: f32,
    /// Ridge value at which a crease starts to register.
    pub crease_threshold: f32,
}

/// Thinking, as bulges that rise through the skin — `docs/PRESENCE_SCENES.md`
/// §4.3. The lava-lamp reference: a mass gathers, swells, travels, thins, and
/// is reabsorbed, and another is already starting somewhere else.
///
/// This is what `PRESENCE_VISUAL_ENTITY.md` §6 called a "strong curl swirl" for
/// thinking, revised. Curl noise displaces points *through* a volume; these
/// points live on a skin, so a curl field moves them off it and the only
/// visible result is that the shell goes fuzzy. On a surface the same idea —
/// internal churn made visible — has to be expressed as the surface itself
/// deforming, which is a bulge.
///
/// Each lobe's centre, width, and amplitude resolve once per frame in `frame`.
/// Per particle the term is a dot product and an exponential per live lobe and
/// evaluates no noise at all, which is why thinking can afford to run
/// concurrently with the fold rather than replacing it.
pub struct LobeTerm {
    /// Peak outward displacement of one lobe, as a fraction of `base_radius`.
    pub depth: f32,
    /// Angular extent, as the `1 - cos θ` at which a lobe falls to `1/e`.
    /// Small is a blister, large is a whole hemisphere swelling.
    pub width: f32,
    /// Seconds for one lobe to gather, rise, thin, and vanish.
    pub period: f32,
    /// How far a lobe's centre travels between birth and reabsorption, as a
    /// fraction of the pole-to-pole span.
    pub travel: f32,
}

/// Ceiling on concurrent lobes. Past four the bulges overlap into a single
/// lumpy inflation and the shell stops reading as *separate* things rising —
/// which is the whole content of the state, since one continuous swell is
/// indistinguishable from breathing.
const MAX_LOBES: usize = 4;

/// One bulge, resolved for this frame.
#[derive(Clone, Copy, Debug, Default)]
struct Lobe {
    /// Unit direction of the bulge's centre.
    dir: Vec3,
    /// Outward displacement at the centre, already in radius units.
    amp: f32,
    /// `0..1` life envelope, for the shoulder crease.
    strength: f32,
    /// Reciprocal width, precomputed out of the per-particle loop.
    falloff: f32,
}

/// Speaking, as a wave travelling across the skin.
///
/// The only term that lives in `place` rather than `deform`, and that is a
/// consequence of what it has to express rather than a preference: `deform` is
/// staggered to roughly 15 Hz per particle, which would alias a wave into a
/// visible crawl. Being in `place` means it must stay to a dot product and a
/// sine — `place` runs for every particle every step, four times as often as
/// `deform`.
///
/// ## Why the geometry is slow and the brightness is not
///
/// Speech's legible rhythm is the syllable, at roughly 4-7 Hz. Geometry cannot
/// carry that. `SurfaceBehavior` springs particles toward the skin at
/// `spring_k = 14`, putting its natural frequency near 0.7 Hz, and a
/// second-order system passes about two percent of a signal ten times its
/// corner. A shell driven at syllable rate would sit visibly still while the
/// panel insisted it was speaking.
///
/// So the state is split across two channels by what each can actually carry:
///
/// - **Geometry** follows a *phrase* envelope (`EntityParams::audio_envelope`),
///   slow enough for the spring to pass — this term.
/// - **Brightness** follows the raw level, assigned directly in
///   `SurfaceBehavior::update` and never sprung, so it responds within a single
///   step.
///
/// Raising `spring_k` to chase syllables instead would take roughly seventy
/// times the stiffness, and the spring is shared by every mode and both shapes
/// — it would trade §2.3's softness everywhere for one state's responsiveness.
pub struct PulseTerm {
    /// Peak radial modulation, as a fraction of the current radius.
    pub depth: f32,
    /// Rings across the shell, in radians per unit of axial position.
    pub wavenumber: f32,
    /// Travelling frequency, in Hz. Held under the spring's corner on purpose;
    /// `the_pulse_stays_inside_what_the_spring_passes` guards it.
    pub speed: f32,
    /// Direction the wave travels along.
    pub axis: Vec3,
    /// Modulation present whenever speech is engaged, before the envelope adds
    /// the rest. Speech is still happening between loud syllables, and a shell
    /// that goes glassy in every gap reads as having stopped talking.
    pub floor: f32,
}

/// A tool call, as a pendant the shell extends and then draws back in —
/// `docs/PRESENCE_SCENES.md` §4.3, revised. A localized outward reach with a
/// pinched waist behind it, so it reads as something the shell has *pushed
/// out* rather than as a bump.
///
/// ## Why it retracts instead of detaching
///
/// §4.3 originally described tool use as shedding a pendant that detaches and
/// travels away. A `SurfaceShape` is star-shaped about its centre — one radius
/// per direction — so a detached droplet would be two surfaces on the same ray
/// and is structurally inexpressible here, not merely awkward.
///
/// The revision is a better fit anyway. A call is a request *and* a response,
/// and reaching out and pulling back shows both, where shedding shows only the
/// outbound half and leaves completion invisible. Detachment stays available
/// later as a separate `DataStream` entity, which is where a thing that leaves
/// the presence belongs.
///
/// ## The weight is the extension
///
/// Unlike the other terms, `ShellDrive::neck` is not a uniform scale on this
/// one — it is distributed across the live pendants, so they reach out in
/// sequence as the mode engages and withdraw in reverse as it releases. Two
/// calls starting together should not look like one thicker pendant.
pub struct NeckTerm {
    /// How far a pendant's tip reaches past the skin, as a fraction of
    /// `base_radius`.
    pub reach: f32,
    /// Angular tightness of the tip: the `1 - cos θ` at which it falls to
    /// `1/e`. Much tighter than a lobe's — a broad pendant is a lobe.
    pub tip_width: f32,
    /// How deeply the waist cuts in behind the tip.
    pub pinch: f32,
    /// Where the waist sits, as the cosine of its angle from the pendant's
    /// axis.
    pub waist_at: f32,
    pub waist_width: f32,
}

/// Ceiling on concurrent pendants. Three overlapping reaches is already at the
/// limit of being countable, and a presence that cannot be counted is showing
/// activity rather than status.
const MAX_NECKS: usize = 3;

/// One pendant, resolved for this frame.
#[derive(Clone, Copy, Debug, Default)]
struct Neck {
    dir: Vec3,
    /// `0..1` how far this pendant has reached out.
    extension: f32,
    tip_falloff: f32,
    waist_falloff: f32,
}

/// Band the summed radius is held inside, as a fraction of `base_radius`.
///
/// A safety net for the one failure additive composition introduces that no
/// single term can cause: several terms displacing outward at the same spot at
/// once — a pendant emerging through the summit of a lobe, say. The director's
/// fold yield is what keeps the sum inside this in the ordinary case, so the
/// clamp should bind only at genuine coincidences; if it binds often, the term
/// depths are wrong rather than the clamp being what saves the frame.
///
/// Below the floor a shell's folds pass through its own centre.
///
/// The ceiling is not a taste judgement: it is the viewport's *vertical*
/// half-extent divided by the shell's scale, since vertical is the tighter of
/// the two axes at the camera's framing. A term reaching past it crops against
/// the top of the window, and a presence that crops reads as a texture behind
/// the window rather than as an object inside it. This is also why pendants aim
/// near the equator, where the frame is wider and a reach has somewhere to go.
const RADIUS_MIN: f32 = 0.45;
const RADIUS_MAX: f32 = 1.62;

pub struct ShellFrame {
    rotation: Mat3,
    radius: f32,
    drive: ShellDrive,
    fold_time: f32,
    /// Multiplier on the crease term only, in `[fold_rest_floor, 1.0]`. A very
    /// slow rise and fall of *how brightly the folds draw*, without changing
    /// the geometry — so from the corner of the eye the shell looks like it
    /// is subtly resting between features rather than continuously churning.
    /// This is the material half of the idle-calm pass; slowing evolution and
    /// breath was the geometric half.
    fold_rest: f32,
    lobes: [Lobe; MAX_LOBES],
    live_lobes: usize,
    pulse_amp: f32,
    pulse_phase: f32,
    necks: [Neck; MAX_NECKS],
    live_necks: usize,
}

impl PresenceShell {
    pub fn new(seed: u32) -> Self {
        Self {
            noise: NoiseField::new(seed),
            base_radius: 1.0,
            fold: FoldTerm {
                // Deep. A shallow displacement is indistinguishable from a
                // sphere once the points are small, and then none of the
                // surface machinery shows: the silhouette is a circle and the
                // creases have nothing to trace.
                depth: 0.78,
                // Low enough that folds read as a handful of large petals
                // rather than as surface roughness. Past roughly 3 the
                // displacement stops being structure and becomes noise, and the
                // shell loses the legible silhouette that is the reason for a
                // surface at all.
                scale: 1.05,
                // Three octaves: one gives smooth lobes with no fine crease
                // detail, four is not visibly different from three but costs a
                // third more in the hottest loop in the program.
                octaves: 3,
                // Halved from 0.045 during the idle-calm pass. At the previous
                // rate the peripheral eye caught the folds visibly reshaping
                // once every fifteen seconds or so, which is fine to look
                // *at* and wrong to look *past* — the guide's peripheral test
                // is what this tuning is for. The silhouette still reshapes,
                // just slowly enough that noticing it means noticing on
                // purpose.
                evolution: 0.022,
                // Low enough that creases cover a real fraction of the skin.
                // Set high, the filaments technically exist but sit almost
                // entirely on the limb, where the grazing term is already
                // saturated — so the face of the shell stays empty and the
                // whole form reads as a hollow bubble instead of a folded one.
                crease_threshold: 0.48,
            },
            lobes: LobeTerm {
                // Enough to be unmistakably a bulge and not enough for two
                // overlapping ones to break the shell's silhouette open. The
                // fold yields depth as this rises, so the pair stays inside the
                // radius band without either being timid on its own.
                depth: 0.28,
                // Wide enough that a bulge is unmistakably larger than a fold —
                // a narrow one on a shell carrying folds this deep just reads
                // as another fold — and no wider. Past roughly this, four lobes
                // overlap far enough that their tails sum to a constant
                // everywhere, and the shell uniformly inflates instead of
                // growing bulges. That failure is easy to misread as the lobes
                // being too deep, since inflation is what it looks like.
                width: 0.32,
                // Slow enough to watch a single lobe through its whole life —
                // the state is "internal complexity", and complexity that
                // resolves in under a second reads as a glitch instead.
                period: 6.5,
                travel: 0.8,
            },
            pulse: PulseTerm {
                // Small. This is a ripple across an already-folded skin, not a
                // deformation of it, and the fold deliberately does not yield
                // any depth to it — a shell that visibly deflates the moment it
                // starts speaking reads as losing composure.
                depth: 0.055,
                // A couple of wavelengths from pole to pole. More and the rings
                // are finer than the fold detail they cross and read as
                // shimmer; fewer and the whole shell simply swells, which the
                // breathing term already does.
                wavenumber: 7.0,
                // Just under the spring's ~0.7 Hz corner, so most of the
                // motion actually reaches the skin.
                speed: 0.62,
                // Tilted off vertical for the same reason the spin axis is:
                // an exactly horizontal set of rings reads as a fixed grid
                // rather than as something moving over a form.
                axis: Vec3::new(0.18, 1.0, -0.12).normalize(),
                floor: 0.35,
            },
            neck: NeckTerm {
                // Far enough past the skin to read as reaching rather than as
                // bulging, and no further: the shell already fills most of the
                // frame's height, so a longer pendant would crop.
                reach: 0.34,
                tip_width: 0.10,
                // Shallow. A deep waist on a shell whose folds are this deep
                // reads as the pendant having been severed, which is the story
                // this shape is explicitly not telling.
                pinch: 0.14,
                // Roughly 45 degrees off the pendant's axis — far enough back
                // that the pinch reads as the pendant's root rather than as a
                // groove in its side.
                waist_at: 0.72,
                waist_width: 0.13,
            },
            // Breathing is the one motion always present at rest, so its
            // cadence sets what "at rest" feels like. The previous ~12-second
            // period pulled the eye every twelve seconds; ~18 seconds crosses
            // the threshold where it stops registering as motion at all and
            // starts registering as the thing being alive. Amplitude drops
            // with speed on purpose: a slower breath at the old amplitude
            // reads as *heavier* breathing rather than calmer, which is the
            // opposite of what this pass is for.
            breath_amplitude: 0.016,
            breath_speed: 0.055,
            spin_speed: 0.017,
        }
    }

    /// Resolves this frame's live lobes.
    ///
    /// Lobes are evenly staggered across one period rather than started
    /// independently, so there is always one mid-life: independent random
    /// starts leave gaps where every lobe happens to be near zero at once, and
    /// the shell falls still in the middle of a state that is supposed to mean
    /// continuous activity.
    fn lobe_frame(&self, time: f32, intensity: f32) -> ([Lobe; MAX_LOBES], usize) {
        // Two bulges is the floor at which they read as separate events; load
        // adds more rather than making each bigger, so busier thinking looks
        // like more going on rather than one larger swell.
        let live = (2.0 + intensity.clamp(0.0, 1.0) * (MAX_LOBES - 2) as f32).round() as usize;
        let live = live.clamp(1, MAX_LOBES);

        let mut lobes = [Lobe::default(); MAX_LOBES];
        for (k, lobe) in lobes.iter_mut().take(live).enumerate() {
            let phase = (time / self.lobes.period.max(0.1) + k as f32 / live as f32).fract();

            // Rises over the first quarter, then thins over the last half.
            // Asymmetric on purpose: a symmetric envelope reads as something
            // inflating and deflating in place, where the asymmetry reads as
            // something surfacing and then being drawn back under.
            let strength = smoothstep(0.0, 0.22, phase) * (1.0 - smoothstep(0.5, 1.0, phase));

            // Each lobe keeps its own longitude for life, and the longitudes
            // themselves drift slowly, so a viewer never learns where the next
            // one will appear. A fixed set of longitudes turns into a visible
            // repeating rotation within about a minute.
            let azimuth =
                std::f32::consts::TAU * (hash01(Vec3::splat(k as f32 * 1.7 + 0.31)) + time * 0.011);
            // Bottom to top. Sinking instead would read as the shell shedding
            // rather than as something being worked through and released.
            let rise = self.lobes.travel * (phase - 0.5);
            let horizontal = (1.0 - rise * rise).max(0.0).sqrt();

            *lobe = Lobe {
                dir: Vec3::new(azimuth.cos() * horizontal, rise, azimuth.sin() * horizontal),
                amp: strength * self.lobes.depth,
                strength,
                falloff: 1.0 / self.lobes.width.max(1e-3),
            };
        }
        (lobes, live)
    }

    /// Resolves this frame's pendants, distributing the mode's weight across
    /// them so they extend one after another rather than in unison.
    fn neck_frame(&self, weight: f32, time: f32, intensity: f32) -> ([Neck; MAX_NECKS], usize) {
        let live = (1.0 + intensity.clamp(0.0, 1.0) * (MAX_NECKS - 1) as f32).round() as usize;
        let live = live.clamp(1, MAX_NECKS);
        let span = 1.0 / live as f32;

        let mut necks = [Neck::default(); MAX_NECKS];
        for (k, neck) in necks.iter_mut().take(live).enumerate() {
            let extension = smoothstep(k as f32 * span, (k + 1) as f32 * span, weight);

            // An evenly-spaced slot plus a hashed jitter, rather than a purely
            // hashed direction: hashing alone puts two pendants within twenty
            // degrees of each other often enough to matter, and two pendants
            // that close are visually one thick one — which defeats the entire
            // reason for having more than one.
            //
            // Slotted against `MAX_NECKS` and not `live`, so a pendant keeps
            // its direction if load changes while it is extended. Spacing them
            // over the live count instead would swing every existing pendant
            // sideways the moment another call started.
            //
            // Fixed for the pendant's life either way. A wandering pendant
            // reads as a tentacle feeling around, which says searching rather
            // than working; a call goes to one place.
            let jitter = hash01(Vec3::splat(k as f32 * 2.3 + 0.71)) * 0.12;
            let azimuth = std::f32::consts::TAU * (k as f32 / MAX_NECKS as f32 + jitter);
            // Below the equator, but only just. Straight down reads as
            // dripping — which the reference image is, but a drip is something
            // falling away from the presence, and this is something it is
            // holding on to.
            let rise = -0.34 * hash01(Vec3::splat(k as f32 * 4.1 + 0.19));
            let horizontal = (1.0 - rise * rise).max(0.0).sqrt();

            // An extended pendant that is perfectly still reads as a fixture on
            // the shell rather than as work in progress. Small enough not to
            // become a second animation.
            let working = 1.0 + 0.05 * (time * 0.9 + k as f32 * 2.1).sin();

            *neck = Neck {
                dir: Vec3::new(azimuth.cos() * horizontal, rise, azimuth.sin() * horizontal),
                extension: extension * working,
                tip_falloff: 1.0 / self.neck.tip_width.max(1e-3),
                waist_falloff: 1.0 / self.neck.waist_width.max(1e-3),
            };
        }
        (necks, live)
    }

    /// A pendant's radial displacement and crease at one seed direction.
    fn sample_necks(&self, seed: Vec3, necks: &[Neck]) -> (f32, f32) {
        let mut displacement = 0.0;
        let mut crease = 0.0_f32;
        for neck in necks {
            let along = seed.dot(neck.dir);
            let tip = ((along - 1.0) * neck.tip_falloff).exp();
            let offset = (along - self.neck.waist_at) * neck.waist_falloff;
            let waist = (-offset * offset).exp();

            displacement += neck.extension * (self.neck.reach * tip - self.neck.pinch * waist);
            // The waist *is* a crease — it is a fold in the surface, which is
            // exactly what the crease channel draws. Nothing has to be
            // invented to light it, and the light lands on the geometry by
            // construction rather than by tuning.
            crease = crease.max(neck.extension * waist);
        }
        (displacement, crease)
    }
}

/// A lobe's outward displacement and shoulder crease at one seed direction.
fn sample_lobes(seed: Vec3, lobes: &[Lobe]) -> (f32, f32) {
    let mut bulge = 0.0;
    let mut shoulder = 0.0_f32;
    for lobe in lobes {
        // Peaked at the lobe's centre and falling off with angle. Cheaper than
        // a true angular gaussian and indistinguishable at these widths, since
        // `1 - cos θ` and `θ²/2` agree closely over the range a lobe spans.
        let falloff = ((seed.dot(lobe.dir) - 1.0) * lobe.falloff).exp();
        bulge += lobe.amp * falloff;
        // The rim, not the summit. A bulge's summit is a smooth cap with no
        // structure to draw; what makes it read as a *thing* under the skin is
        // the ring where the surface turns, which is where the falloff is
        // halfway down. Squared to keep that ring tight — unsquared it covers
        // most of the shell faintly and reads as a haze rather than an edge.
        let rim = 4.0 * falloff * (1.0 - falloff);
        shoulder = shoulder.max(lobe.strength * rim * rim);
    }
    (bulge, shoulder)
}

impl SurfaceShape for PresenceShell {
    type Frame = ShellFrame;

    fn domain(&self) -> SurfaceDomain {
        SurfaceDomain::Sphere
    }

    fn frame(&self, params: &EntityParams) -> ShellFrame {
        let breath = 1.0
            + self.breath_amplitude
                * (params.time * self.breath_speed * std::f32::consts::TAU).sin();
        // Gated here as well as in `deform`: resolving lobes for a term nobody
        // will read is only a few dozen operations, but it is also the place a
        // future term with real per-frame setup cost would want the gate.
        let (lobes, live_lobes) = if params.drive.lobes > ShellDrive::GATE {
            self.lobe_frame(params.time, params.intensity)
        } else {
            ([Lobe::default(); MAX_LOBES], 0)
        };
        let (necks, live_necks) = if params.drive.neck > ShellDrive::GATE {
            self.neck_frame(params.drive.neck, params.time, params.intensity)
        } else {
            ([Neck::default(); MAX_NECKS], 0)
        };
        ShellFrame {
            // Rotation is applied to the finished surface point, not to the
            // noise input. Rotating the noise instead makes folds travel across
            // a stationary point set, which reads as a shimmer rather than as
            // an object turning.
            //
            // Tilted off the vertical so the axis is not a fixed feature of the
            // silhouette — an exactly vertical spin has a visible unmoving
            // pole, which reads as mechanical.
            rotation: Mat3::from_axis_angle(
                Vec3::new(0.12, 1.0, 0.06).normalize(),
                params.time * self.spin_speed * std::f32::consts::TAU,
            ),
            radius: self.base_radius * breath * (1.0 + params.expand * 0.16),
            drive: params.drive,
            fold_time: params.time * self.fold.evolution,
            // ~35-second period, floor 0.62. Deliberately *not* commensurate
            // with the breath period, so the two never phase-lock into a
            // single visible rhythm — the compound motion is what makes idle
            // stop reading as a machine. Held above zero because folds that
            // *actually* stop drawing look like a rendering bug rather than
            // like calm.
            fold_rest: 0.62 + 0.38 * 0.5
                * (1.0 + (params.time * (std::f32::consts::TAU / 35.0) - 0.7).sin()),
            lobes,
            live_lobes,
            pulse_amp: self.pulse.depth
                * (self.pulse.floor
                    + (1.0 - self.pulse.floor) * params.audio_envelope.clamp(0.0, 1.0)),
            pulse_phase: params.time * self.pulse.speed * std::f32::consts::TAU,
            necks,
            live_necks,
        }
    }

    fn deform(&self, seed: Vec3, frame: &ShellFrame) -> SurfaceDeform {
        let mut radius = 1.0;
        // Creases accumulate rather than replace. A spot can be on a fold and
        // on a lobe shoulder at the same time, and picking one would make
        // structure blink out wherever two terms overlap.
        let mut crease = 0.0;

        if frame.drive.fold > ShellDrive::GATE {
            let ridge =
                self.noise
                    .ridged(seed * self.fold.scale, frame.fold_time, self.fold.octaves);
            // Centred on the mean, so depth deepens the folds without also
            // inflating the shell. Breathing is deliberately *not* applied
            // here — it belongs to `place`, since a cached breath would stutter
            // at the refresh rate.
            radius += frame.drive.fold * self.fold.depth * (ridge - 0.5);
            // Rest applies to crease only, not to the geometric term above.
            // If it modulated the displacement the silhouette would visibly
            // swell and shrink on the rest period, which is exactly the
            // large-motion signal this pass is trying to remove.
            crease += frame.drive.fold
                * frame.fold_rest
                * smoothstep(self.fold.crease_threshold, 1.0, ridge);
        }

        if frame.drive.lobes > ShellDrive::GATE {
            let (bulge, shoulder) = sample_lobes(seed, &frame.lobes[..frame.live_lobes]);
            radius += frame.drive.lobes * bulge;
            crease += frame.drive.lobes * shoulder;
        }

        if frame.drive.neck > ShellDrive::GATE {
            // Not scaled by the weight here: `neck_frame` already spent it as
            // each pendant's extension, and applying it twice would make a
            // half-engaged pendant reach half as far *and* be half as solid.
            let (reach, waist) = self.sample_necks(seed, &frame.necks[..frame.live_necks]);
            radius += reach;
            crease += waist;
        }

        SurfaceDeform {
            local: seed * radius.clamp(RADIUS_MIN, RADIUS_MAX),
            crease: crease.min(1.0),
        }
    }

    fn place(
        &self,
        seed: Vec3,
        local: Vec3,
        frame: &ShellFrame,
        params: &EntityParams,
    ) -> SurfaceSample {
        let mut radius = frame.radius;
        if frame.drive.pulse > ShellDrive::GATE {
            // Modulates the radius rather than adding to it, so the wave rides
            // the folds instead of flattening them — the ripple is something
            // passing *through* the shell's existing form.
            let along = seed.dot(self.pulse.axis);
            let wave = (self.pulse.wavenumber * along - frame.pulse_phase).sin();
            radius *= 1.0 + frame.drive.pulse * frame.pulse_amp * wave;
        }

        SurfaceSample {
            position: params.center + frame.rotation * (local * radius) * params.scale,
            // The seed direction, not a finite-differenced gradient — which
            // would cost roughly six extra noise evaluations per particle. For
            // a star-shaped radial surface this is exact at the limb, which is
            // precisely where the grazing term is read; fold-local normal error
            // is carried by the crease term instead.
            normal: frame.rotation * seed,
        }
    }
}

/// Sand on a driven Chladni plate — `docs/PRESENCE_SCENES.md` §4.2. This is
/// the Loading signature.
///
/// **The plate itself does not move.** It is a sheet with sand on it. The
/// animation is the *driving frequency stepping between resonances*: hold a
/// mode and the grains sit still on its nodal lines, step to the next and the
/// grains slide into an entirely different figure over about a second. That is
/// what a real Chladni plate does when you turn the frequency knob, and it is
/// why there is no rotation here — a plate spinning on its own axis reads as a
/// loading spinner, which is decoration standing in for status rather than
/// status itself (`PRESENCE_VISUAL_ENTITY.md` §2.6).
///
/// Four properties were arrived at by getting them wrong first, and all four
/// are structural rather than matters of tuning:
///
/// - **The figure is a superposition, not a single product.** This used
///   `v = cos(m·x)·cos(n·y)`, which only ever draws a grid: changing the mode
///   numbers changes the grid's spacing and nothing else, so stepping the
///   frequency produced no new *shapes* — which is the entire point of the
///   scene. A free square plate's standing wave is a mode superposed with its
///   transpose, and that is what gives the crosses, stars, and lattices a
///   Chladni plate is recognised by.
/// - **Mode numbers are integers held for seconds, not floats drifting
///   continuously.** A continuous drift means the plate is never actually at a
///   resonance, so the sand never settles and the figure never resolves — it
///   just churns. Real plates jump between discrete resonances, and the
///   stillness *between* jumps is what makes each figure legible.
/// - **The plate faces the viewer; it is not horizontal.** A real Chladni plate
///   lies flat, and that is how this was built first. A horizontal plane reads
///   as exactly the ground plane `PRESENCE_VISUAL_ENTITY.md` §2.2 rules out,
///   and since the camera sits nearly level with the origin it is seen edge-on
///   and collapses to a line — hiding the modal pattern that is the entire
///   content of the scene.
/// - **Nodal drift is a bounded displacement of the rest position, never a
///   force.** The first version added a force proportional to the gradient of
///   `v²`. That is unbounded in principle, not merely badly tuned: its
///   magnitude scales with the mode index and is computed from the fixed rest
///   position, so it does not weaken as a grain strays. Grains whose force
///   outran the restoring spring left the plate permanently and drifted toward
///   the camera as blown-out foreground blobs.
///
/// Nodal proximity is reported as the shape's `crease`, which is the same
/// channel `PresenceShell` uses for fold filaments. That is not a convenient
/// reuse of a field — a nodal line and a fold crease are the same thing to a
/// viewer, structure emerging on a surface, so they should render identically.
pub struct ResonancePlate {
    pub noise: NoiseField,
    /// Seconds each resonance is held before the drive steps to the next.
    pub dwell: f32,
    /// Ceiling on how far a grain may be displaced toward a nodal line, as a
    /// fraction of the plate half-width. The step is self-limiting (see
    /// `deform`), so this only catches the pathological case near a critical
    /// point of the field, where the gradient vanishes.
    pub nodal_migration: f32,
    /// Half-depth of the plate, as a fraction of its half-width.
    pub thickness: f32,
    /// Half-width of the ridge of sand that piles along a nodal line, as a
    /// fraction of the plate half-width.
    pub pile_width: f32,
}

/// Resonant `(m, n)` mode pairs, in ascending order of complexity.
///
/// Integers, because a Chladni figure only closes at an integer mode pair —
/// between them the nodal lines don't meet the plate's edges consistently and
/// the figure reads as a smear. Only `m < n` is listed: swapping them negates
/// `v`, which leaves the nodal set (and therefore the figure) identical.
const PLATE_MODES: [(f32, f32); 8] = [
    (1.0, 2.0),
    (2.0, 3.0),
    (1.0, 4.0),
    (3.0, 4.0),
    (2.0, 5.0),
    (4.0, 5.0),
    (3.0, 6.0),
    (5.0, 6.0),
];

pub struct PlateFrame {
    /// Mode numbers for the resonance currently being held, pre-multiplied by
    /// π so `deform` doesn't repeat it per particle.
    m: f32,
    n: f32,
    time: f32,
}

impl ResonancePlate {
    pub fn new(seed: u32) -> Self {
        Self {
            noise: NoiseField::new(seed),
            // Long enough to read the figure as a held, settled state and not
            // as motion; short enough that a viewer waiting on a loading
            // indicator sees it doing something. The grains take roughly a
            // second of that to slide, since the behavior's spring sits near
            // 0.6 Hz, so this is about three-quarters hold and a quarter
            // rearrangement.
            dwell: 3.6,
            nodal_migration: 0.55,
            thickness: 0.05,
            pile_width: 0.022,
        }
    }

    /// The resonance being driven at `time`, and how far up the mode table
    /// this load level is allowed to reach.
    ///
    /// Drive selects *range*, not position: heavier load ranges further into
    /// the fine-grained figures while still cycling through the simple ones.
    /// Mapping drive directly to a single mode index would freeze the plate
    /// whenever the signal was steady, which is the one thing a loading
    /// indicator must never do.
    fn mode_at(&self, time: f32, drive: f32) -> (f32, f32) {
        let reach = 2 + ((PLATE_MODES.len() - 2) as f32 * drive.clamp(0.0, 1.0)).round() as usize;
        let step = (time.max(0.0) / self.dwell.max(0.1)) as usize;
        PLATE_MODES[step % reach]
    }
}

impl SurfaceShape for ResonancePlate {
    type Frame = PlateFrame;

    fn domain(&self) -> SurfaceDomain {
        SurfaceDomain::Sheet
    }

    fn frame(&self, params: &EntityParams) -> PlateFrame {
        let drive = (params.intensity * 0.4 + params.progress * 0.6).clamp(0.0, 1.0);
        let (m, n) = self.mode_at(params.time, drive);
        PlateFrame {
            m: m * std::f32::consts::PI,
            n: n * std::f32::consts::PI,
            time: params.time,
        }
    }

    fn deform(&self, seed: Vec3, frame: &PlateFrame) -> SurfaceDeform {
        let (x, y) = (seed.x, seed.y);
        let (smx, cmx) = (frame.m * x).sin_cos();
        let (snx, cnx) = (frame.n * x).sin_cos();
        let (smy, cmy) = (frame.m * y).sin_cos();
        let (sny, cny) = (frame.n * y).sin_cos();

        // The standing wave on a free square plate: a mode superposed with its
        // transpose. The subtraction is what produces a *figure* rather than a
        // grid — it puts a nodal line along every diagonal where the two terms
        // cancel, and those diagonals are what the classic crosses and stars
        // are made of.
        let v = cnx * cmy - cmx * cny;
        let grad = Vec3::new(
            -frame.n * snx * cmy + frame.m * smx * cny,
            -frame.m * cnx * smy + frame.n * cmx * sny,
            0.0,
        );

        // How far this grain's home sits from the nearest nodal line. For a
        // smooth field the distance to its zero set is `|v| / |grad v|` to
        // first order, which is both the step that lands a grain *on* a line
        // and the measure of whether it can reach one at all.
        //
        // A fixed-size step scaled by |v| — what this did before — overshoots
        // whenever the line spacing is smaller than the step, which is most of
        // the mode table. Grains then sail past one line toward the next and
        // the anti-nodal regions never empty out, so the figure stays a
        // suggestion instead of resolving.
        let distance = v.abs() / grad.length().max(1e-4);
        let across = grad.normalize_or_zero() * v.signum();

        // Sand piles have width. Real grains stack against each other rather
        // than balancing on an infinitely thin curve, and landing every grain
        // exactly on the zero set renders the figure as a one-pixel wireframe —
        // a diagram of a Chladni plate rather than sand on one. The spread is
        // hashed from the seed, so a grain keeps its place in the ridge instead
        // of shimmering across it.
        let spread = (hash01(seed * 3.1) - 0.5) * 2.0 * self.pile_width;
        let settled = Vec3::new(x, y, 0.0) - across * (distance.min(self.nodal_migration) - spread);

        // Clamped to the sheet, so the field cannot grow past the extent it
        // was generated at.
        let jitter = self.noise.sample(seed * 1.4, frame.time * 0.3) * 0.5;
        SurfaceDeform {
            local: Vec3::new(
                settled.x.clamp(-1.0, 1.0),
                settled.y.clamp(-1.0, 1.0),
                (seed.z + jitter) * self.thickness,
            ),
            // Whether this grain actually reached a line, *not* how large `v`
            // is where it started. Every grain within reach ends up on the
            // ridge and must be lit as such; measuring the field at the seed
            // instead would leave most of the sand in the figure unlit, because
            // a grain that migrated in from an anti-node still has a large `v`
            // at the position it came from.
            crease: 1.0 - smoothstep(self.nodal_migration * 0.6, self.nodal_migration, distance),
        }
    }

    fn place(
        &self,
        _seed: Vec3,
        local: Vec3,
        _frame: &PlateFrame,
        params: &EntityParams,
    ) -> SurfaceSample {
        SurfaceSample {
            // No rotation term, and no time dependence at all: the sheet is
            // stationary and everything visible comes from the sand moving on
            // it. `no_time_dependence_in_placement` guards this.
            position: params.center + local * params.scale * (1.0 + params.expand * 0.2),
            // Constant across the plate, so the grazing term contributes
            // nothing here — correct, since a sheet seen face-on has no limb to
            // brighten. Its structure comes entirely from the nodal crease.
            normal: Vec3::Z,
        }
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn random_in_unit_sphere(rng: &mut SmallRng) -> Vec3 {
    loop {
        let p = Vec3::new(
            rng.gen_range(-1.0..1.0),
            rng.gen_range(-1.0..1.0),
            rng.gen_range(-1.0..1.0),
        );
        if p.length_squared() <= 1.0 {
            return p;
        }
    }
}

/// Uniformly-distributed direction. Rejection-sampled rather than built from
/// two angles, which would cluster samples at the poles.
fn random_unit_vector(rng: &mut SmallRng) -> Vec3 {
    loop {
        let p = random_in_unit_sphere(rng);
        let len_sq = p.length_squared();
        if len_sq > 1e-6 {
            return p / len_sq.sqrt();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell_params() -> EntityParams {
        EntityParams::new(Vec3::ZERO, 1.0)
    }

    #[test]
    fn generator_produces_exact_count_on_the_unit_sphere() {
        let params = shell_params();
        let particles = SurfaceGenerator::new(SurfaceDomain::Sphere).generate(400, &params);
        assert_eq!(particles.len(), 400);
        for p in &particles {
            let r = p.base_offset.length();
            assert!((r - 1.0).abs() < 1e-4, "seed is not a unit direction: {r}");
        }
    }

    #[test]
    fn sheet_domain_seeds_stay_inside_the_sheet_and_thin_at_the_rim() {
        let params = shell_params();
        let particles = SurfaceGenerator::new(SurfaceDomain::Sheet).generate(4_000, &params);
        assert_eq!(particles.len(), 4_000, "rejection sampling lost particles");

        let mut rim = 0;
        for p in &particles {
            let seed = p.base_offset;
            assert!(
                seed.x.abs() <= 1.0 && seed.y.abs() <= 1.0,
                "sheet seed escaped: {seed:?}"
            );
            assert!(seed.z.abs() <= 1.0);
            if seed.x.abs().max(seed.y.abs()) > SHEET_RIM {
                rim += 1;
            }
        }

        // The rim band is ~45% of a unit square's area, so a uniform fill would
        // put roughly that share of the population in it. The taper has to cut
        // that down substantially or the sheet still ends on a visible straight
        // edge, which is the whole reason for the rejection step.
        let share = rim as f32 / particles.len() as f32;
        assert!(
            share < 0.28,
            "sheet rim was not thinned: {share} of points in the margin"
        );
        assert!(
            share > 0.02,
            "sheet rim was thinned to nothing, leaving a hard cut"
        );
    }

    #[test]
    fn every_layer_is_populated_at_any_density_bias() {
        for bias in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let mut params = shell_params();
            params.core_density_bias = bias;
            let particles = SurfaceGenerator::new(SurfaceDomain::Sphere).generate(4_000, &params);
            for layer in [Layer::Core, Layer::Body, Layer::Halo] {
                assert!(
                    particles.iter().any(|p| p.layer == layer),
                    "{layer:?} layer is empty at bias {bias}, so its material gradient is dead code"
                );
            }
        }
    }

    #[test]
    fn density_bias_moves_points_onto_the_skin_without_emptying_the_halo() {
        let share = |bias: f32| {
            let mut params = shell_params();
            params.core_density_bias = bias;
            let particles = SurfaceGenerator::new(SurfaceDomain::Sphere).generate(4_000, &params);
            let count = |layer: Layer| particles.iter().filter(|p| p.layer == layer).count();
            (count(Layer::Core), count(Layer::Halo), particles.len())
        };

        let (soft_core, soft_halo, total) = share(0.1);
        let (hard_core, hard_halo, _) = share(0.9);
        assert!(hard_core > soft_core, "bias did not concentrate the skin");
        assert!(hard_halo < soft_halo, "bias did not thin the atmosphere");
        // At the default the skin must still be the majority, or the silhouette
        // the surface model exists to produce is being drawn by the scatter
        // around it rather than by the skin.
        let (default_core, _, _) = share(0.5);
        assert!(default_core * 2 > total, "core is not the main population");
    }

    /// The displaced radius must stay in a band around the nominal radius. A
    /// shell that can reach the origin has self-intersecting folds, and one
    /// that can grow without bound leaves the viewport.
    #[test]
    fn resting_shell_radius_is_bounded_and_actually_folds() {
        let shell = PresenceShell::new(9);
        let mut params = shell_params();
        let mut rng = SmallRng::seed_from_u64(4);
        let seeds: Vec<Vec3> = (0..300).map(|_| shell.domain().seed(&mut rng)).collect();

        let mut min_r = f32::MAX;
        let mut max_r = 0.0_f32;
        for step in 0..40 {
            params.time = step as f32 * 0.5;
            let frame = shell.frame(&params);
            for seed in &seeds {
                let deform = shell.deform(*seed, &frame);
                let sample = shell.place(*seed, deform.local, &frame, &params);
                let r = sample.position.length();
                min_r = min_r.min(r);
                max_r = max_r.max(r);
                assert!((0.0..=1.0).contains(&deform.crease));
                assert!((sample.normal.length() - 1.0).abs() < 1e-3);
            }
        }

        assert!(min_r > 0.5, "shell folded in on itself: min radius {min_r}");
        assert!(
            max_r < 1.5,
            "shell inflated past its budget: max radius {max_r}"
        );
        // If the band is flat there is no fold structure, only a sphere.
        assert!(
            max_r - min_r > 0.1,
            "shell is effectively a plain sphere: range {}",
            max_r - min_r
        );
    }

    /// The weight has to both scale the term continuously and switch it off
    /// entirely. Scaling without gating leaves every term evaluated forever
    /// once any mode has been engaged, which is the cost the additive model
    /// only avoids by skipping.
    #[test]
    fn the_fold_weight_scales_the_term_and_gates_it_off() {
        let shell = PresenceShell::new(11);
        let mut params = shell_params();
        let mut rng = SmallRng::seed_from_u64(19);
        let seeds: Vec<Vec3> = (0..200).map(|_| shell.domain().seed(&mut rng)).collect();

        let spread = |weight: f32, params: &mut EntityParams| {
            params.drive = ShellDrive {
                fold: weight,
                ..ShellDrive::IDLE
            };
            let frame = shell.frame(params);
            let radii: Vec<f32> = seeds
                .iter()
                .map(|s| shell.deform(*s, &frame).local.length())
                .collect();
            let max = radii.iter().cloned().fold(0.0_f32, f32::max);
            let min = radii.iter().cloned().fold(f32::MAX, f32::min);
            max - min
        };

        let full = spread(1.0, &mut params);
        let half = spread(0.5, &mut params);
        let off = spread(0.0, &mut params);

        assert!(full > 0.1, "the fold is not producing structure at all");
        assert!(
            (half - full * 0.5).abs() < full * 0.1,
            "the weight does not scale the term linearly: {half} vs {full}"
        );
        assert!(
            off < 1e-5,
            "a gated-off fold still displaced the shell: {off}"
        );
    }

    fn thinking(intensity: f32) -> EntityParams {
        let mut params = shell_params();
        params.intensity = intensity;
        params.drive = ShellDrive {
            lobes: 1.0,
            ..ShellDrive::IDLE
        };
        params
    }

    /// A lobe has to be an *event* — it arrives, moves, and is gone. A bulge
    /// that merely pulses in place is a second breathing term, and the state
    /// would read as the idle shell doing the same thing slightly harder.
    #[test]
    fn a_lobe_migrates_across_the_shell_and_is_reabsorbed() {
        let shell = PresenceShell::new(13);
        let mut params = thinking(0.0);

        // Inclusive of the end of the period, which is where a lobe is fully
        // reabsorbed and its successor takes over.
        let mut path: Vec<(Vec3, f32)> = Vec::new();
        for step in 0..=40 {
            params.time = step as f32 * shell.lobes.period / 40.0;
            let frame = shell.frame(&params);
            path.push((frame.lobes[0].dir, frame.lobes[0].strength));
        }

        let travelled = path
            .iter()
            .zip(path.iter().skip(1))
            .map(|((a, _), (b, _))| (*b - *a).length())
            .sum::<f32>();
        assert!(
            travelled > 0.5,
            "the lobe stayed put: travelled {travelled}"
        );

        let peak = path.iter().fold(0.0_f32, |m, (_, s)| m.max(*s));
        assert!(peak > 0.9, "the lobe never fully swelled: peak {peak}");
        assert_eq!(path[0].1, 0.0, "the lobe was born already swollen");
        assert_eq!(path.last().unwrap().1, 0.0, "the lobe was never reabsorbed");
    }

    /// Staggering exists so the shell is never momentarily still while
    /// thinking. Independently-phased lobes leave gaps where all of them
    /// happen to be near zero, and the state silently stops expressing itself.
    #[test]
    fn some_lobe_is_always_mid_life() {
        let shell = PresenceShell::new(17);
        let mut params = thinking(0.0);

        for step in 0..200 {
            params.time = step as f32 * shell.lobes.period / 60.0;
            let frame = shell.frame(&params);
            let strongest = frame.lobes[..frame.live_lobes]
                .iter()
                .fold(0.0_f32, |m, l| m.max(l.strength));
            assert!(
                strongest > 0.2,
                "the shell fell still at t={}: strongest lobe {strongest}",
                params.time
            );
        }
    }

    #[test]
    fn load_adds_lobes_rather_than_enlarging_one() {
        let shell = PresenceShell::new(23);
        let calm = shell.frame(&thinking(0.0)).live_lobes;
        let busy = shell.frame(&thinking(1.0)).live_lobes;
        assert!(calm >= 2, "fewer than two lobes reads as a single swell");
        assert!(busy > calm, "load did not add lobes: {calm} vs {busy}");
        assert!(busy <= MAX_LOBES);
        assert_eq!(
            shell.lobes.depth,
            PresenceShell::new(23).lobes.depth,
            "load must not change a lobe's size"
        );
    }

    /// The crease marks the bulge's rim, not its summit — the rim is the edge
    /// that makes a lobe read as a discrete mass rather than as the shell
    /// having grown.
    #[test]
    fn the_lobe_crease_lands_on_the_shoulder_not_the_summit() {
        let shell = PresenceShell::new(29);
        let mut params = thinking(0.0);
        // Quarter of the way in, so the first lobe is near full strength.
        params.time = shell.lobes.period * 0.25;
        let frame = shell.frame(&params);
        let lobe = frame.lobes[0];

        // A basis to swing away from the lobe's centre in.
        let across = lobe.dir.cross(Vec3::Y).normalize_or(Vec3::X);
        let at = |theta: f32| {
            let dir = (lobe.dir * theta.cos() + across * theta.sin()).normalize();
            sample_lobes(dir, &frame.lobes[..frame.live_lobes])
        };

        let (summit_bulge, summit_crease) = at(0.0);
        let (rim_bulge, rim_crease) = at(0.85);
        let (far_bulge, far_crease) = at(2.6);

        assert!(
            summit_bulge > rim_bulge && rim_bulge > far_bulge,
            "the bulge is not peaked"
        );
        assert!(
            rim_crease > summit_crease,
            "crease is on the summit: {rim_crease} vs {summit_crease}"
        );
        assert!(
            rim_crease > far_crease,
            "crease bleeds across the whole shell"
        );
    }

    /// The gate is what keeps idle at the cost it had before this term
    /// existed, so a zeroed weight must reproduce the fold shell bit for bit
    /// rather than merely closely.
    #[test]
    fn a_zero_lobe_weight_reproduces_the_fold_shell_exactly() {
        let shell = PresenceShell::new(31);
        let mut rng = SmallRng::seed_from_u64(41);
        let seeds: Vec<Vec3> = (0..200).map(|_| shell.domain().seed(&mut rng)).collect();

        let mut resting = shell_params();
        resting.time = 12.0;
        let mut silenced = resting;
        silenced.drive.lobes = 0.0;
        silenced.intensity = 1.0;

        let a = shell.frame(&resting);
        let b = shell.frame(&silenced);
        assert_eq!(b.live_lobes, 0, "lobes were resolved for a gated-off term");
        for seed in &seeds {
            assert_eq!(shell.deform(*seed, &a).local, shell.deform(*seed, &b).local);
            assert_eq!(
                shell.deform(*seed, &a).crease,
                shell.deform(*seed, &b).crease
            );
        }
    }

    /// Gain of `SurfaceBehavior`'s spring at a driving frequency, as a
    /// standard second-order low-pass. This is the constraint the speaking
    /// state is designed around, so it is computed from the spring's actual
    /// constants rather than asserted as a remembered number — if the spring
    /// is ever retuned, the tests below move with it.
    fn spring_gain(hz: f32, spring_k: f32, damping: f32, layer_scale: f32) -> f32 {
        let natural = (spring_k * layer_scale).sqrt();
        let zeta = damping / (2.0 * natural);
        let ratio = hz * std::f32::consts::TAU / natural;
        1.0 / ((1.0 - ratio * ratio).powi(2) + (2.0 * zeta * ratio).powi(2)).sqrt()
    }

    /// The finding the whole speaking design rests on: geometry cannot carry a
    /// syllable rate, so the pulse must stay slow and the syllables must go
    /// somewhere that isn't sprung. If the pulse ever drifts up toward speech's
    /// real rhythm it will simply stop being visible, which looks like a
    /// tuning problem and is not one.
    #[test]
    fn the_pulse_stays_inside_what_the_spring_passes() {
        let shell = PresenceShell::new(53);
        let behavior = SurfaceBehavior::new(PresenceShell::new(53));
        let gain = |hz: f32| {
            spring_gain(
                hz,
                behavior.spring_k,
                behavior.damping,
                Layer::Core.spring_scale(),
            )
        };

        let passed = gain(shell.pulse.speed);
        assert!(
            passed > 0.4,
            "the pulse is above the spring's corner and would barely move the skin: \
             {passed} of it reaches the surface"
        );

        // The rate the pulse is deliberately *not* running at.
        let syllables = gain(5.0);
        assert!(
            syllables < 0.1,
            "the spring passes syllable rate after all ({syllables}), which would \
             make the brightness split unnecessary"
        );
    }

    /// Speech has to reach the skin within a step. Routing it through the
    /// spring instead would make the shell respond a beat behind every word.
    #[test]
    fn audio_level_changes_brightness_within_a_single_step() {
        let mut params = shell_params();
        params.drive.pulse = 1.0;
        let mut particles = SurfaceGenerator::new(SurfaceDomain::Sphere).generate(200, &params);
        let mut behavior = SurfaceBehavior::new(PresenceShell::new(59));

        let quiet = PresenceSignals {
            audio_level: 0.0,
            ..Default::default()
        };
        let loud = PresenceSignals {
            audio_level: 1.0,
            ..Default::default()
        };

        behavior.update(&mut particles, 1.0 / 60.0, &params, &quiet);
        let before: Vec<f32> = particles.iter().map(|p| p.brightness).collect();
        behavior.update(&mut particles, 1.0 / 60.0, &params, &loud);

        for (p, was) in particles.iter().zip(&before) {
            assert!(
                p.brightness > was + 0.005,
                "brightness lagged the audio level: {was} -> {}",
                p.brightness
            );
        }
    }

    /// The channel is speech's, not a general brightness knob. Left ungated it
    /// would light the shell whenever anything happened to be making noise,
    /// including while the presence is idle.
    #[test]
    fn audio_does_not_light_the_shell_when_speech_is_not_engaged() {
        let params = shell_params();
        let mut particles = SurfaceGenerator::new(SurfaceDomain::Sphere).generate(80, &params);
        let mut behavior = SurfaceBehavior::new(PresenceShell::new(61));

        let loud = PresenceSignals {
            audio_level: 1.0,
            ..Default::default()
        };
        behavior.update(&mut particles, 1.0 / 60.0, &params, &loud);
        let with_audio: Vec<f32> = particles.iter().map(|p| p.brightness).collect();

        let quiet = PresenceSignals::default();
        behavior.update(&mut particles, 1.0 / 60.0, &params, &quiet);

        for (p, was) in particles.iter().zip(&with_audio) {
            assert_eq!(
                p.brightness, *was,
                "idle brightness followed the audio level"
            );
        }
    }

    /// The wave has to travel, not stand. A standing ripple is a second
    /// breathing term with more rings, and reads as texture rather than as
    /// something moving across the form.
    #[test]
    fn the_pulse_travels_across_the_shell() {
        let shell = PresenceShell::new(67);
        let mut params = shell_params();
        params.drive.pulse = 1.0;
        params.audio_envelope = 1.0;

        // Pick the probe by *rotating the pulse axis* rather than by picking
        // a plausible-looking seed by eye: the previous version happened to
        // land close to a zero of `sin(k·along)`, where a travelling and a
        // standing wave produce indistinguishable small displacements and
        // any unrelated per-frame motion (breath, evolution) is enough to
        // flip the sign of the residual. Sampling one radian off the axis
        // puts the probe well away from every zero and node.
        let probe = Mat3::from_axis_angle(Vec3::new(0.0, 0.0, 1.0), 1.0) * shell.pulse.axis;
        let probe = probe.normalize();
        let radius_at = |t: f32, params: &mut EntityParams| {
            params.time = t;
            let frame = shell.frame(params);
            shell.place(probe, probe, &frame, params).position.length()
        };

        let quarter = 1.0 / (4.0 * shell.pulse.speed);
        let a = radius_at(0.0, &mut params);
        let b = radius_at(quarter, &mut params);
        let c = radius_at(2.0 * quarter, &mut params);

        assert!((a - b).abs() > 1e-3, "the pulse did not move");
        assert!((b - c).abs() > 1e-3);
        assert!(
            (a - c).abs() > 1e-3,
            "the pulse returned to its starting displacement half a period on, \
             so it is standing rather than travelling"
        );
    }

    /// Loudness has to change the depth, but silence between phrases must not
    /// switch the shell off — speech is still happening in the gaps.
    #[test]
    fn the_phrase_envelope_scales_the_pulse_without_ever_stilling_it() {
        let shell = PresenceShell::new(71);
        let mut params = shell_params();
        params.drive.pulse = 1.0;

        params.audio_envelope = 0.0;
        let quiet = shell.frame(&params).pulse_amp;
        params.audio_envelope = 1.0;
        let loud = shell.frame(&params).pulse_amp;

        assert!(quiet > 0.0, "the shell goes glassy between phrases");
        assert!(loud > quiet * 1.5, "loudness barely changed the pulse");
        assert!(loud <= shell.pulse.depth + 1e-6);
    }

    fn tool_use(weight: f32, intensity: f32) -> EntityParams {
        let mut params = shell_params();
        params.intensity = intensity;
        params.drive = ShellDrive {
            neck: weight,
            ..ShellDrive::IDLE
        };
        params
    }

    /// A pendant has to be *localized*. A reach spread over the whole shell is
    /// inflation, and inflation is what the breathing term already does.
    #[test]
    fn a_pendant_reaches_out_over_a_small_part_of_the_shell() {
        let shell = PresenceShell::new(73);
        let params = tool_use(1.0, 0.0);
        let frame = shell.frame(&params);
        assert_eq!(frame.live_necks, 1);
        let dir = frame.necks[0].dir;

        let (tip, _) = shell.sample_necks(dir, &frame.necks[..1]);
        assert!(
            tip > shell.neck.reach * 0.9,
            "the pendant barely extended: {tip}"
        );

        // Directly opposite: the far side of the shell must be untouched.
        let (far, far_crease) = shell.sample_necks(-dir, &frame.necks[..1]);
        assert!(
            far.abs() < 0.01,
            "the pendant displaced the far side too: {far}"
        );
        assert_eq!(far_crease, 0.0);

        let mut rng = SmallRng::seed_from_u64(79);
        let touched = (0..600)
            .filter(|_| {
                let seed = shell.domain().seed(&mut rng);
                shell.sample_necks(seed, &frame.necks[..1]).0.abs() > 0.02
            })
            .count();
        assert!(
            touched * 4 < 600,
            "the pendant covers most of the shell ({touched} of 600 points), \
             so it reads as inflation rather than as a reach"
        );
    }

    /// The pinch is what makes a reach read as a pendant rather than a bump,
    /// and its crease is what draws it.
    #[test]
    fn the_waist_pinches_in_behind_the_tip_and_carries_the_crease() {
        let shell = PresenceShell::new(83);
        let params = tool_use(1.0, 0.0);
        let frame = shell.frame(&params);
        let neck = frame.necks[0];

        let across = neck.dir.cross(Vec3::Y).normalize_or(Vec3::X);
        let at = |theta: f32| {
            let dir = (neck.dir * theta.cos() + across * theta.sin()).normalize();
            shell.sample_necks(dir, &frame.necks[..1])
        };

        let (tip, tip_crease) = at(0.0);
        let (waist, waist_crease) = at(shell.neck.waist_at.acos());
        let (away, _) = at(2.0);

        assert!(tip > 0.0, "the tip does not reach outward");
        assert!(waist < 0.0, "there is no waist behind the tip: {waist}");
        assert!(
            away.abs() < waist.abs(),
            "the pinch is not localized either"
        );
        assert!(
            waist_crease > tip_crease,
            "the crease is not on the waist: {waist_crease} vs {tip_crease}"
        );
    }

    /// Extension and retraction are the mode's weight, so a pendant must
    /// follow it all the way back to nothing. A pendant that lingers after a
    /// call finishes is showing stale status, which is worse than showing none.
    #[test]
    fn a_pendant_extends_with_the_weight_and_retracts_to_nothing() {
        let shell = PresenceShell::new(89);
        let mut params = tool_use(1.0, 0.0);
        let dir = shell.frame(&params).necks[0].dir;

        let mut reach = |weight: f32| {
            params.drive.neck = weight;
            let frame = shell.frame(&params);
            shell.sample_necks(dir, &frame.necks[..frame.live_necks]).0
        };

        let full = reach(1.0);
        let part = reach(0.5);
        assert!(
            full > part && part > 0.0,
            "the pendant did not extend gradually"
        );

        // Below the gate the frame resolves no pendants at all, so this is the
        // real retracted state rather than a small residual reach.
        params.drive.neck = 0.0;
        let frame = shell.frame(&params);
        assert_eq!(
            frame.live_necks, 0,
            "pendants were resolved for a gated-off term"
        );
        assert_eq!(
            shell.deform(dir, &frame).local,
            shell.deform(dir, &shell.frame(&shell_params())).local
        );
    }

    #[test]
    fn concurrent_calls_add_pendants_rather_than_thickening_one() {
        let shell = PresenceShell::new(97);
        let one = shell.frame(&tool_use(1.0, 0.0));
        let many = shell.frame(&tool_use(1.0, 1.0));
        assert_eq!(one.live_necks, 1);
        assert!(many.live_necks > 1, "load did not add pendants");
        assert!(many.live_necks <= MAX_NECKS);

        // Distinct directions, or they are visually one pendant regardless of
        // how many the frame thinks it resolved.
        for i in 0..many.live_necks {
            for j in (i + 1)..many.live_necks {
                let apart = many.necks[i].dir.dot(many.necks[j].dir);
                assert!(apart < 0.9, "two pendants point the same way: {apart}");
            }
        }
    }

    /// The failure the additive model introduces that no single term can
    /// cause: four terms displacing the same spot outward at once. Guarded at
    /// full weight on everything, which is stronger than any state the director
    /// actually produces.
    #[test]
    fn every_term_at_full_weight_keeps_the_shell_inside_its_band() {
        let shell = PresenceShell::new(101);
        let mut params = shell_params();
        params.intensity = 1.0;
        params.audio_envelope = 1.0;
        params.drive = ShellDrive {
            fold: 1.0,
            lobes: 1.0,
            pulse: 1.0,
            neck: 1.0,
        };

        let mut rng = SmallRng::seed_from_u64(103);
        let seeds: Vec<Vec3> = (0..500).map(|_| shell.domain().seed(&mut rng)).collect();

        // The placed radius may exceed the deform band by the pulse's ripple
        // and the breath, both of which scale the finished radius rather than
        // adding to it. Derived rather than written down, so retuning either
        // moves the bound with it.
        let ceiling = RADIUS_MAX * (1.0 + shell.pulse.depth) * (1.0 + shell.breath_amplitude);
        let floor = RADIUS_MIN * (1.0 - shell.pulse.depth) * (1.0 - shell.breath_amplitude);

        for step in 0..60 {
            params.time = step as f32 * 0.37;
            let frame = shell.frame(&params);
            for seed in &seeds {
                let deform = shell.deform(*seed, &frame);
                let radius = shell
                    .place(*seed, deform.local, &frame, &params)
                    .position
                    .length();
                assert!(
                    (floor..=ceiling).contains(&radius),
                    "the summed shell escaped its band at t={}: radius {radius}",
                    params.time
                );
                assert!((0.0..=1.0).contains(&deform.crease));
            }
        }
    }

    /// Composition is the model's central claim, so check it produces a shell
    /// distinguishable from either mode alone rather than one mode quietly
    /// dominating.
    #[test]
    fn thinking_and_tool_use_together_differ_from_either_alone() {
        let shell = PresenceShell::new(107);
        let mut rng = SmallRng::seed_from_u64(109);
        let seeds: Vec<Vec3> = (0..600).map(|_| shell.domain().seed(&mut rng)).collect();

        let shape = |drive: ShellDrive| {
            let mut params = shell_params();
            params.intensity = 0.6;
            params.time = shell.lobes.period * 0.3;
            params.drive = drive;
            let frame = shell.frame(&params);
            seeds
                .iter()
                .map(|s| shell.deform(*s, &frame).local)
                .collect::<Vec<_>>()
        };

        let thinking = shape(ShellDrive {
            fold: 0.58,
            lobes: 1.0,
            ..ShellDrive::IDLE
        });
        let calling = shape(ShellDrive {
            fold: 0.58,
            neck: 1.0,
            ..ShellDrive::IDLE
        });
        let both = shape(ShellDrive {
            fold: 0.58,
            lobes: 1.0,
            neck: 1.0,
            ..ShellDrive::IDLE
        });

        let differing = |a: &[Vec3], b: &[Vec3]| {
            a.iter()
                .zip(b)
                .filter(|(x, y)| (**x - **y).length() > 0.02)
                .count()
        };
        assert!(
            differing(&both, &thinking) > 20,
            "adding a tool call changed nothing about the thinking shell"
        );
        assert!(
            differing(&both, &calling) * 4 > seeds.len(),
            "the tool call swallowed the thinking shell instead of adding to it"
        );
    }

    #[test]
    fn thinking_produces_a_shell_distinguishable_from_idle() {
        let shell = PresenceShell::new(37);
        let mut rng = SmallRng::seed_from_u64(43);
        let seeds: Vec<Vec3> = (0..400).map(|_| shell.domain().seed(&mut rng)).collect();

        let mut idle = shell_params();
        idle.time = shell.lobes.period * 0.25;
        let mut busy = thinking(0.5);
        busy.time = idle.time;
        // What the director would do: the fold yields as the lobes rise.
        busy.drive.fold = 0.58;

        let idle_frame = shell.frame(&idle);
        let busy_frame = shell.frame(&busy);
        let moved = seeds
            .iter()
            .filter(|s| {
                let a = shell.deform(**s, &idle_frame).local;
                let b = shell.deform(**s, &busy_frame).local;
                (a - b).length() > 0.05
            })
            .count();
        assert!(
            moved * 4 > seeds.len(),
            "thinking is not visibly different from idle: {moved} of {} points moved",
            seeds.len()
        );
    }

    #[test]
    fn creases_land_on_the_deepest_displacement() {
        let shell = PresenceShell::new(21);
        let params = shell_params();
        let frame = shell.frame(&params);
        let mut rng = SmallRng::seed_from_u64(7);

        // The crease value is the ridge value, and the ridge value is what
        // drives displacement outward — so a creased sample must never be a
        // trough. This is the alignment guarantee that reusing one value buys,
        // and a separately-computed crease would break it silently.
        let mut creased = Vec::new();
        let mut plain = Vec::new();
        for _ in 0..600 {
            let seed = shell.domain().seed(&mut rng);
            let deform = shell.deform(seed, &frame);
            let radius = shell
                .place(seed, deform.local, &frame, &params)
                .position
                .length();
            if deform.crease > 0.5 {
                creased.push(radius);
            } else if deform.crease == 0.0 {
                plain.push(radius);
            }
        }
        assert!(!creased.is_empty() && !plain.is_empty());
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
        assert!(
            mean(&creased) > mean(&plain),
            "creases are not on the ridges: {} vs {}",
            mean(&creased),
            mean(&plain)
        );
    }

    /// The stagger must reach every particle within one full cycle. If it ever
    /// misses a stride class, those particles freeze at their seeded placeholder
    /// and the surface renders with permanent holes in it — a failure that is
    /// easy to mistake for a generation bug.
    #[test]
    fn staggered_refresh_reaches_every_particle_within_one_cycle() {
        let mut params = shell_params();
        let mut particles = SurfaceGenerator::new(SurfaceDomain::Sphere).generate(97, &params);
        let seeded: Vec<Vec3> = particles.iter().map(|p| p.local).collect();
        let mut behavior = SurfaceBehavior::new(PresenceShell::new(5));
        let signals = PresenceSignals::default();

        for step in 0..DEFAULT_DEFORM_STRIDE {
            params.time = step as f32 / 60.0;
            behavior.update(&mut particles, 1.0 / 60.0, &params, &signals);
        }

        for (p, was) in particles.iter().zip(&seeded) {
            assert!(
                p.local != *was,
                "particle never had its deform refreshed: still at {was:?}"
            );
        }
    }

    /// The nodal migration was originally an unbounded force term, which let
    /// grains leave the plate entirely and drift toward the camera as blown-out
    /// blobs. Guard the bound rather than the tuning, since the failure was
    /// structural.
    #[test]
    fn resonance_plate_keeps_grains_on_the_plate() {
        let scale = 1.85;
        let mut params = EntityParams::new(Vec3::ZERO, scale);
        params.intensity = 0.7;
        params.expand = 0.1;

        let mut particles = SurfaceGenerator::new(SurfaceDomain::Sheet).generate(600, &params);
        let mut behavior = SurfaceBehavior::new(ResonancePlate::new(0x400D));
        let signals = PresenceSignals::default();

        // Long enough to cross several dwell periods, so the mode steps and
        // every grain is yanked to a new figure repeatedly.
        for step in 0..1_800 {
            params.time = step as f32 / 60.0;
            behavior.update(&mut particles, 1.0 / 60.0, &params, &signals);
        }

        // The sheet is a unit square scaled by `scale` and expanded by
        // `expand`, so its corners sit at `scale * sqrt(2)`; this bound is that
        // geometry plus slack for the spring's overshoot, the thickness jitter,
        // and the halo layer's offset.
        let bound = scale * std::f32::consts::SQRT_2 * (1.0 + params.expand * 0.2) + 0.5;
        for p in &particles {
            assert!(
                p.position.length() < bound,
                "grain left the plate: {:?} (bound {bound})",
                p.position
            );
        }
    }

    /// The plate must stay shallow relative to its width, or it stops being
    /// §4.2's "mostly planar" field.
    #[test]
    fn resonance_plate_stays_shallow() {
        let plate = ResonancePlate::new(2);
        let mut params = EntityParams::new(Vec3::ZERO, 1.0);
        params.time = 0.0;
        let frame = plate.frame(&params);
        let mut rng = SmallRng::seed_from_u64(12);
        for _ in 0..400 {
            let seed = plate.domain().seed(&mut rng);
            let deform = plate.deform(seed, &frame);
            assert!(
                deform.local.z.abs() <= plate.thickness * 1.6,
                "plate grain is not shallow: z={}",
                deform.local.z
            );
            assert!(deform.local.x.abs() <= 1.0 + 1e-4 && deform.local.y.abs() <= 1.0 + 1e-4);
        }
    }

    /// The sheet is stationary — sand moves on it, it does not turn. A spin
    /// term is the easiest thing in the world to reintroduce while "adding a
    /// bit of life", and it turns the scene straight back into a loading
    /// spinner, so guard placement's time-independence directly.
    #[test]
    fn no_time_dependence_in_placement() {
        let plate = ResonancePlate::new(4);
        let mut params = EntityParams::new(Vec3::ZERO, 1.6);
        let local = Vec3::new(0.4, -0.7, 0.02);
        let seed = Vec3::new(0.4, -0.7, 0.3);

        params.time = 0.0;
        let first = plate.place(seed, local, &plate.frame(&params), &params);
        // Deliberately not a multiple of the dwell: if placement depended on
        // time at all, a partial period would expose it.
        params.time = 17.3;
        let later = plate.place(seed, local, &plate.frame(&params), &params);

        assert_eq!(first.position, later.position, "the sheet moved");
        assert_eq!(first.normal, later.normal, "the sheet turned");
    }

    /// Stepping the drive frequency has to produce a genuinely *different*
    /// figure, not the same one rescaled. The previous single-product field
    /// failed exactly here: its mode numbers changed the grid spacing and
    /// nothing else, so the scene had no content beyond "something is
    /// happening".
    #[test]
    fn stepping_the_frequency_redraws_the_figure() {
        let plate = ResonancePlate::new(7);
        let mut params = EntityParams::new(Vec3::ZERO, 1.0);
        params.progress = 1.0;

        let mut rng = SmallRng::seed_from_u64(3);
        let seeds: Vec<Vec3> = (0..800).map(|_| plate.domain().seed(&mut rng)).collect();

        // Where the sand ends up, for each of the first few resonances the
        // drive steps through. Settled position is the probe rather than the
        // crease flag, because position is what actually draws the figure —
        // nearly every grain within reach ends up on some line in every mode,
        // so the flag is true almost everywhere and distinguishes nothing.
        let mut figure = |time: f32| {
            params.time = time;
            let frame = plate.frame(&params);
            seeds
                .iter()
                .map(|s| plate.deform(*s, &frame).local.truncate())
                .collect::<Vec<_>>()
        };

        let mut figures = Vec::new();
        for step in 0..4 {
            let settled = figure((step as f32 + 0.5) * plate.dwell);
            let collected = settled
                .iter()
                .zip(&seeds)
                .filter(|(at, seed)| (**at - seed.truncate()).length() > 0.02)
                .count();
            assert!(
                collected * 3 > settled.len(),
                "step {step} left the sand where it lay instead of collecting it"
            );
            figures.push(settled);
        }

        for (i, a) in figures.iter().enumerate() {
            for (j, b) in figures.iter().enumerate().skip(i + 1) {
                let moved = a
                    .iter()
                    .zip(b)
                    .filter(|(x, y)| (**x - **y).length() > 0.05)
                    .count();
                let share = moved as f32 / a.len() as f32;
                assert!(
                    share > 0.5,
                    "modes {i} and {j} draw effectively the same figure \
                     (only {share} of grains relocated)"
                );
            }
        }
    }

    /// Within one dwell the figure must hold perfectly still, or the sand
    /// never appears to settle and each resonance stops being legible.
    #[test]
    fn the_figure_holds_still_between_steps() {
        let plate = ResonancePlate::new(8);
        let mut params = EntityParams::new(Vec3::ZERO, 1.0);
        let seed = Vec3::new(-0.3, 0.55, 0.1);

        params.time = 0.1 * plate.dwell;
        let early = plate.deform(seed, &plate.frame(&params));
        params.time = 0.9 * plate.dwell;
        let late = plate.deform(seed, &plate.frame(&params));

        // Only the shallow out-of-plane jitter may differ; the in-plane
        // settling position is what draws the figure and must be identical.
        assert_eq!(early.local.truncate(), late.local.truncate());
        assert_eq!(early.crease, late.crease);
    }

    /// Load must widen the range of figures visited without ever parking the
    /// plate on one. A steady signal freezing the indicator is the specific
    /// failure a "map drive straight to a mode index" implementation has.
    #[test]
    fn drive_widens_the_mode_range_and_never_stalls() {
        let plate = ResonancePlate::new(9);
        let visited = |drive: f32| {
            let mut modes: Vec<(u32, u32)> = (0..24)
                .map(|step| {
                    let (m, n) = plate.mode_at((step as f32 + 0.5) * plate.dwell, drive);
                    (m as u32, n as u32)
                })
                .collect();
            modes.sort_unstable();
            modes.dedup();
            modes
        };

        let calm = visited(0.0);
        let busy = visited(1.0);
        assert!(calm.len() >= 2, "the plate stalls on one figure when idle");
        assert!(
            busy.len() > calm.len(),
            "load did not reach further into the mode table: {} vs {}",
            busy.len(),
            calm.len()
        );
        assert_eq!(
            busy.len(),
            PLATE_MODES.len(),
            "full load should reach every mode"
        );
    }

    /// The abstraction's payoff: one behavior drives both shapes. If either
    /// stops satisfying `SurfaceShape`, this fails to compile rather than
    /// failing subtly at runtime.
    #[test]
    fn one_behavior_drives_both_shapes() {
        let mut params = EntityParams::new(Vec3::ZERO, 1.2);
        let signals = PresenceSignals::default();

        let mut shell_pts = SurfaceGenerator::new(SurfaceDomain::Sphere).generate(80, &params);
        let mut plate_pts = SurfaceGenerator::new(SurfaceDomain::Sheet).generate(80, &params);
        let mut shell = SurfaceBehavior::new(PresenceShell::new(1));
        let mut plate = SurfaceBehavior::new(ResonancePlate::new(1));

        for step in 0..120 {
            params.time = step as f32 / 60.0;
            shell.update(&mut shell_pts, 1.0 / 60.0, &params, &signals);
            plate.update(&mut plate_pts, 1.0 / 60.0, &params, &signals);
        }

        for p in shell_pts.iter().chain(plate_pts.iter()) {
            assert!(p.position.is_finite());
            assert!((0.0..=1.0).contains(&p.crease));
        }
    }

    #[test]
    fn surface_behavior_settles_particles_onto_the_skin() {
        let mut params = shell_params();
        params.scale = 1.4;
        let mut particles = SurfaceGenerator::new(SurfaceDomain::Sphere).generate(300, &params);
        let mut behavior = SurfaceBehavior::new(PresenceShell::new(3));
        let signals = PresenceSignals::default();

        for step in 0..300 {
            params.time = step as f32 / 60.0;
            behavior.update(&mut particles, 1.0 / 60.0, &params, &signals);
        }

        let frame = behavior.shape.frame(&params);
        for p in &particles {
            assert!(p.position.is_finite());
            let target = behavior
                .shape
                .place(p.base_offset, p.local, &frame, &params);
            let off_skin = (p.position - target.position).length();
            // Generous, since the halo deliberately floats off the skin and
            // the spring is soft — this catches particles that never converged
            // at all, which is the failure that matters.
            assert!(
                off_skin < 0.5 * params.scale,
                "particle never reached the skin: off by {off_skin}"
            );
        }
    }
}
