// Post-processing chain: bright-pass, bloom downsample/upsample, and the
// final tonemap composite — docs/PRESENCE_VISUAL_ENTITY.md §3.1 ("optional
// very light glow"), §3.2 ("controlled hotspots"), §7.4 ("optional very
// light bloom, use sparingly", "near-black clear color, brand --ink").
//
// The point pass renders into an Rgba16Float target so accumulated additive
// energy can exceed 1.0 instead of clipping in the swapchain. That is what
// makes dense cores roll off to near-white through a filmic curve rather
// than flattening into solid white, and it is what gives the bloom
// bright-pass something meaningful to threshold against.

struct PostUniform {
    /// xy = texel size of the source texture, zw = unused.
    texel: vec4<f32>,
    /// Pass-specific. See each entry point.
    params: vec4<f32>,
};

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> post: PostUniform;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

/// Fullscreen triangle. Cheaper than a quad and needs no vertex buffer.
@vertex
fn vs_fullscreen(@builtin(vertex_index) index: u32) -> VertexOutput {
    var out: VertexOutput;
    let x = f32((index << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(index & 2u) * 2.0 - 1.0;
    out.clip_position = vec4<f32>(x, -y, 0.0, 1.0);
    out.uv = vec2<f32>((x + 1.0) * 0.5, (y + 1.0) * 0.5);
    return out;
}

/// 13-tap downsample (the filter Call of Duty's bloom presentation
/// popularised). Wider support than a box filter, which matters because a
/// naive 2x box on sparse bright points produces visible pulsing as points
/// drift between source texels.
fn downsample_13(uv: vec2<f32>, texel: vec2<f32>) -> vec3<f32> {
    let a = textureSample(source, source_sampler, uv + texel * vec2<f32>(-2.0, 2.0)).rgb;
    let b = textureSample(source, source_sampler, uv + texel * vec2<f32>(0.0, 2.0)).rgb;
    let c = textureSample(source, source_sampler, uv + texel * vec2<f32>(2.0, 2.0)).rgb;
    let d = textureSample(source, source_sampler, uv + texel * vec2<f32>(-2.0, 0.0)).rgb;
    let e = textureSample(source, source_sampler, uv).rgb;
    let f = textureSample(source, source_sampler, uv + texel * vec2<f32>(2.0, 0.0)).rgb;
    let g = textureSample(source, source_sampler, uv + texel * vec2<f32>(-2.0, -2.0)).rgb;
    let h = textureSample(source, source_sampler, uv + texel * vec2<f32>(0.0, -2.0)).rgb;
    let i = textureSample(source, source_sampler, uv + texel * vec2<f32>(2.0, -2.0)).rgb;
    let j = textureSample(source, source_sampler, uv + texel * vec2<f32>(-1.0, 1.0)).rgb;
    let k = textureSample(source, source_sampler, uv + texel * vec2<f32>(1.0, 1.0)).rgb;
    let l = textureSample(source, source_sampler, uv + texel * vec2<f32>(-1.0, -1.0)).rgb;
    let m = textureSample(source, source_sampler, uv + texel * vec2<f32>(1.0, -1.0)).rgb;

    var result = e * 0.125;
    result = result + (a + c + g + i) * 0.03125;
    result = result + (b + d + f + h) * 0.0625;
    result = result + (j + k + l + m) * 0.125;
    return result;
}

/// Bright-pass with a soft knee, then downsample. `params.x` is the
/// threshold, `params.y` the knee width — a hard threshold makes points pop
/// in and out of the bloom as they brighten, which reads as flickering.
@fragment
fn fs_bright(in: VertexOutput) -> @location(0) vec4<f32> {
    let color = downsample_13(in.uv, post.texel.xy);
    let threshold = post.params.x;
    let knee = max(post.params.y, 1e-4);

    let brightness = max(color.r, max(color.g, color.b));
    let soft = clamp((brightness - threshold + knee) / (2.0 * knee), 0.0, 1.0);
    let weight = max(soft * soft * (brightness - threshold + knee) * 0.5, brightness - threshold);
    let contribution = max(weight, 0.0) / max(brightness, 1e-4);
    return vec4<f32>(color * contribution, 1.0);
}

@fragment
fn fs_down(in: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(downsample_13(in.uv, post.texel.xy), 1.0);
}

/// 9-tap tent upsample. `params.x` scales the sample radius. Output is
/// blended additively into the next level up, which is what accumulates the
/// mip chain into one wide, smooth halo.
@fragment
fn fs_up(in: VertexOutput) -> @location(0) vec4<f32> {
    let r = post.texel.xy * post.params.x;
    var result = textureSample(source, source_sampler, in.uv).rgb * 4.0;
    result = result + textureSample(source, source_sampler, in.uv + vec2<f32>(-r.x, 0.0)).rgb * 2.0;
    result = result + textureSample(source, source_sampler, in.uv + vec2<f32>(r.x, 0.0)).rgb * 2.0;
    result = result + textureSample(source, source_sampler, in.uv + vec2<f32>(0.0, -r.y)).rgb * 2.0;
    result = result + textureSample(source, source_sampler, in.uv + vec2<f32>(0.0, r.y)).rgb * 2.0;
    result = result + textureSample(source, source_sampler, in.uv + vec2<f32>(-r.x, -r.y)).rgb;
    result = result + textureSample(source, source_sampler, in.uv + vec2<f32>(r.x, -r.y)).rgb;
    result = result + textureSample(source, source_sampler, in.uv + vec2<f32>(-r.x, r.y)).rgb;
    result = result + textureSample(source, source_sampler, in.uv + vec2<f32>(r.x, r.y)).rgb;
    return vec4<f32>(result / 16.0, 1.0);
}

// ---------------------------------------------------------------------------
// Composite
// ---------------------------------------------------------------------------

struct CompositeUniform {
    /// x = exposure, y = bloom intensity, z = vignette strength,
    /// w = vignette inner radius.
    params: vec4<f32>,
    /// rgb = the near-black field colour (brand `--ink`, linear), a = unused.
    field: vec4<f32>,
    /// x = highlight desaturation amount, y = HDR luminance at which it
    /// begins, z = luminance at which it is fully applied.
    highlight: vec4<f32>,
};

@group(0) @binding(0) var scene_tex: texture_2d<f32>;
@group(0) @binding(1) var bloom_tex: texture_2d<f32>;
@group(0) @binding(2) var composite_sampler: sampler;
@group(0) @binding(3) var<uniform> composite: CompositeUniform;

/// Narkowicz's ACES approximation. The shoulder is the entire point: it
/// compresses the brightest accumulated cores toward white smoothly, so
/// §3.2's "controlled hotspots" emerge from density instead of from every
/// dense region clipping to a flat white blob.
fn tonemap_aces(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs_composite(in: VertexOutput) -> @location(0) vec4<f32> {
    let scene = textureSample(scene_tex, composite_sampler, in.uv).rgb;
    let bloom = textureSample(bloom_tex, composite_sampler, in.uv).rgb;

    let exposure = composite.params.x;
    let bloom_intensity = composite.params.y;

    var hdr = (scene + bloom * bloom_intensity) * exposure;

    // Highlight desaturation, applied to the accumulated HDR value and before
    // the tonemap. This is what realises §3.1's "occasional near-white
    // hotspots at the densest points" and §3.2's "controlled hotspots": the
    // points themselves are always teal, and only where enough of them
    // overlap does the accumulated energy push through to white. Skipping it
    // leaves dense cores as vivid clipped green, because a tonemap compresses
    // each channel independently and a saturated teal simply pins its green
    // channel while red stays near zero.
    let luma = dot(hdr, vec3<f32>(0.2126, 0.7152, 0.0722));
    let whiten = smoothstep(composite.highlight.y, composite.highlight.z, luma)
        * composite.highlight.x;
    hdr = mix(hdr, vec3<f32>(luma), whiten);

    var color = tonemap_aces(hdr);

    // The field is added after tonemapping so the brand ink stays exactly
    // the intended near-black instead of being crushed by the ACES toe.
    color = color + composite.field.rgb;

    let vignette_strength = composite.params.z;
    let vignette_inner = composite.params.w;
    let centered = in.uv - vec2<f32>(0.5, 0.5);
    let radius = length(centered) * 1.41421356;
    let vignette = 1.0 - smoothstep(vignette_inner, 1.0, radius) * vignette_strength;

    return vec4<f32>(color * vignette, 1.0);
}
