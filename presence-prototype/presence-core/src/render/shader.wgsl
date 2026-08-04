// Instanced billboard point renderer.
//
// wgpu has no portable point-size primitive across backends, so each particle
// is a small view-aligned quad (4-vertex triangle strip) offset by the
// camera's right/up basis vectors in the vertex shader, with a soft circular
// falloff computed in the fragment shader — see
// docs/PRESENCE_VISUAL_ENTITY.md §7.4.
//
// This pass renders into an Rgba16Float target (see post.rs), so accumulated
// additive energy is allowed to exceed 1.0 and is resolved by the tonemap
// composite rather than clipping here.

struct Camera {
    view_proj: mat4x4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    /// xyz = eye position, w = pixels-per-world-unit at unit distance
    /// (viewport_height / (2 * tan(fovy/2))).
    eye: vec4<f32>,
    /// Camera forward direction, for the grazing-angle silhouette term.
    forward: vec4<f32>,
    /// Pure-chroma tints (peak channel 1.0). `calm` and `cool` are the two
    /// ends of the *state* axis, `hot` is the *density* axis's ceiling, and
    /// `accent` is the *structure* axis's — fold creases. Point lightness comes
    /// from the energy term, never from these.
    tint_calm: vec4<f32>,
    tint_cool: vec4<f32>,
    tint_hot: vec4<f32>,
    tint_accent: vec4<f32>,
    /// x = haze start distance, y = haze end distance, z = haze strength,
    /// w = minimum point radius in pixels.
    depth_params: vec4<f32>,
    /// x = world-space size multiplier for every point, y = energy that
    /// reaches the hot end of the tint ramp, z = grazing/silhouette boost,
    /// w = crease boost.
    material: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

struct VertexInput {
    @location(0) corner: vec2<f32>,
};

struct InstanceInput {
    @location(1) position: vec3<f32>,
    @location(2) size: f32,
    @location(3) brightness: f32,
    @location(4) color_bias: f32,
    /// Density ramp 0 = core, 1 = body, 2 = halo (§3.3); effect material
    /// classes past it: 3 = aura, 4 = energy, 5 = sparks, 6 = trails (ADR-014).
    @location(5) layer: f32,
    /// 0..1 fold-crease intensity from the surface shape.
    @location(6) crease: f32,
    /// Outward surface normal, or zero for volume-based entities.
    @location(7) normal: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) brightness: f32,
    @location(2) color_bias: f32,
    @location(3) layer: f32,
    @location(4) crease: f32,
};

@vertex
fn vs_main(vert: VertexInput, inst: InstanceInput) -> VertexOutput {
    let distance_to_eye = max(length(camera.eye.xyz - inst.position), 1e-3);
    let pixels_per_world_unit = camera.eye.w;

    var half_size = inst.size * camera.material.x;
    var energy_scale = 1.0;

    // Size attenuation is already implicit in projecting a world-space quad,
    // but sub-pixel points shimmer as they drift across the pixel grid. Clamp
    // the apparent radius to a floor and scale brightness down by the same
    // area factor, so enlarging a distant point doesn't also brighten it.
    // This is the correct fix for point aliasing — MSAA would not help, since
    // the visible edge here is an alpha gradient rather than geometry.
    let radius_pixels = half_size * pixels_per_world_unit / distance_to_eye;
    let min_radius_pixels = camera.depth_params.w;
    if (radius_pixels < min_radius_pixels) {
        let growth = min_radius_pixels / max(radius_pixels, 1e-4);
        half_size = half_size * growth;
        energy_scale = 1.0 / (growth * growth);
    }

    let offset =
        (camera.right.xyz * vert.corner.x + camera.up.xyz * vert.corner.y) * half_size;
    let world_pos = inst.position + offset;

    // Depth cue: points further into the volume dim toward the field. Without
    // this an additive point cloud has no depth ordering information at all
    // and reads as a flat spray (§2.1's "scanned volume", §3.3's layering).
    let haze_start = camera.depth_params.x;
    let haze_end = camera.depth_params.y;
    let haze_strength = camera.depth_params.z;
    let haze = clamp(
        (distance_to_eye - haze_start) / max(haze_end - haze_start, 1e-3),
        0.0,
        1.0,
    ) * haze_strength;

    // Grazing-angle silhouette. Where the skin turns away from the camera, the
    // view ray passes through more of it, so more returns land in the same
    // pixel — the rim of a scanned surface is genuinely brighter than its face.
    // This is the single term that makes a point surface read as a solid object
    // rather than a flat spray, and it is also why a volume fill can never get
    // there: a volume has no normal to take this angle against.
    //
    // A zero normal means the entity is volume-based, and the term drops out.
    // The length in between is how much skin the point still has: a body part
    // way through a morph is part way onto its surface, and the silhouette has
    // to arrive and leave with it. Renormalizing and ignoring the length would
    // hold the term at full strength all the way down and then switch it off in
    // one frame at the cutoff, which reads as a brightness flick across the
    // whole body rather than as a fade.
    let normal_length = length(inst.normal);
    var surface_gain = 1.0;
    if (normal_length > 1e-4) {
        let to_eye = normalize(camera.eye.xyz - inst.position);
        let facing = abs(dot(inst.normal / normal_length, to_eye));
        // Quadratic rather than linear: the visual interest is concentrated in
        // the last few degrees before the limb, and a linear ramp spreads the
        // lift across the whole face where it just looks like extra exposure.
        let grazing = 1.0 - facing;
        let skin = min(normal_length, 1.0);
        surface_gain = 1.0 + camera.material.z * grazing * grazing * skin;
    }

    var out: VertexOutput;
    out.clip_position = camera.view_proj * vec4<f32>(world_pos, 1.0);
    out.uv = vert.corner;
    out.brightness = inst.brightness * energy_scale * (1.0 - haze) * surface_gain;
    out.color_bias = inst.color_bias;
    out.layer = inst.layer;
    out.crease = inst.crease;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let d = length(in.uv);
    // Soft circular core fading to fully transparent at the quad's inscribed
    // circle. Note this is NOT `smoothstep(1.0, 0.15, d)`: with edge0 > edge1
    // that stays fully opaque out to d == 0.15 and only fades after, making
    // every point a broad, near-opaque disc that washes the cloud out once
    // thousands of them overlap additively.
    let base = 1.0 - smoothstep(0.0, 1.0, d);

    // §3.3's density layers differ in material, not just count: the core is a
    // tight concentrated point, the halo a soft diffuse one. The effect classes
    // (layer >= 3, ADR-014 M6) are their own materials and branch off the ramp
    // so this interpolation is untouched for surface entities.
    let core_to_body = clamp(in.layer, 0.0, 1.0);
    let body_to_halo = clamp(in.layer - 1.0, 0.0, 1.0);
    var sharpness = mix(mix(1.5, 1.0, core_to_body), 0.55, body_to_halo);
    if (in.layer > 2.5) {
        // Nearest effect class. Sharpness alone distinguishes them here; size
        // and brightness are already applied per-point via Layer::material.
        // aura: very soft glow; energy: tight; sparks: pinpoint; trails: soft.
        if (in.layer < 3.5) {
            sharpness = 0.35;      // aura
        } else if (in.layer < 4.5) {
            sharpness = 1.2;       // energy
        } else if (in.layer < 5.5) {
            sharpness = 2.5;       // sparks
        } else {
            sharpness = 0.7;       // trails
        }
    }

    let falloff = pow(base, sharpness);
    if (falloff <= 0.001) {
        discard;
    }

    // Creases lift brightness as well as shifting hue: a fold catches the scan
    // at a sharper angle, so it returns more. Doing only the hue shift makes
    // folds read as a discoloration rather than as structure.
    let crease = clamp(in.crease, 0.0, 1.0);
    let energy = falloff * in.brightness * (1.0 + camera.material.w * crease);

    // State axis first: calm sits near-neutral with a faint hue undertone,
    // heavy compute shifts cooler (§3.1, §3.2).
    let state_tint =
        mix(camera.tint_calm.rgb, camera.tint_cool.rgb, clamp(in.color_bias, 0.0, 1.0));
    // Structure axis: fold filaments pull toward the accent, so the surface's
    // shape is drawn in hue and not only in brightness.
    let structured = mix(state_tint, camera.tint_accent.rgb, crease * 0.75);
    // Then the density axis, which lifts bright fragment centres toward the
    // neutral end regardless of state.
    let t = clamp(energy / max(camera.material.y, 1e-4), 0.0, 1.0);
    let color = mix(structured, camera.tint_hot.rgb, smoothstep(0.45, 1.0, t));

    // Additive blending — see the pipeline's blend state — so overlapping
    // points accumulate into brighter cores with no sorting required. The
    // accumulation is allowed to exceed 1.0; the tonemap resolves it.
    //
    // Note this cannot by itself produce §3.1's near-white hotspots: summing a
    // teal tint only ever yields a more saturated teal that the tonemap clips
    // to vivid green. Whitening has to happen after accumulation, so it lives
    // in the composite's highlight desaturation.
    return vec4<f32>(color * energy, energy);
}
