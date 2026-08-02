//! HDR scene target + bloom chain + tonemap composite.
//!
//! Implements the render-path half of `docs/PRESENCE_VISUAL_ENTITY.md` §3.1
//! (soft points with "optional very light glow"), §3.2 ("controlled
//! hotspots"), and §7.4 ("optional very light bloom, use sparingly" and
//! "near-black clear color, brand `--ink`, not pure `#000`").
//!
//! Shape of the chain, per frame:
//!
//! ```text
//! point pass ──▶ hdr_scene (Rgba16Float)
//!                    │
//!                    ├── bright-pass + downsample ──▶ bloom[0]
//!                    │   downsample ──▶ bloom[1..n]
//!                    │   upsample (additive) ──▶ bloom[n-1..0]
//!                    │
//!                    └── composite: ACES tonemap + ink field + vignette ──▶ swapchain
//! ```
//!
//! There is deliberately **no MSAA** and **no depth buffer**:
//!
//! - The points are soft alpha discs, so their visible edge is an alpha
//!   gradient rather than a geometric one. MSAA only anti-aliases geometry
//!   edges, so it would multiply the cost of the heaviest pass while changing
//!   almost nothing. Sub-pixel shimmer on distant points is instead solved
//!   where it actually originates, by clamping minimum screen-space point
//!   size and compensating brightness — see `shader.wgsl`.
//! - Additive blending is order-independent and there is no opaque geometry
//!   to occlude against, so a depth buffer would be written and never read.
//!   The depth *cue* the design calls for is a distance-driven dim in the
//!   point shader, which is what actually makes the volume read as deep.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::palette::PresencePalette;

/// Levels in the bloom mip chain. Five gives a halo wide enough to read as
/// atmosphere on a 1080-tall window without the lowest level collapsing to a
/// handful of texels.
const BLOOM_LEVELS: usize = 5;
/// Smallest allowed dimension for a bloom level; the chain stops early on
/// small windows rather than creating degenerate 1px textures.
const MIN_BLOOM_DIMENSION: u32 = 8;

pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Tunables for the glow and the final grade. Exposed here rather than as
/// shader literals per §9's "color ranges and transition durations should be
/// configurable, not raw hex literals scattered through shader code".
#[derive(Clone, Copy, Debug)]
pub struct PostSettings {
    /// HDR level above which a pixel contributes to the glow.
    pub bloom_threshold: f32,
    /// Soft-knee width around the threshold, so points don't pop into the
    /// glow as they brighten.
    pub bloom_knee: f32,
    /// How much of the accumulated glow is mixed back in. §7.4 says use bloom
    /// sparingly, so this stays well under 1.
    pub bloom_intensity: f32,
    /// Sample radius scale for the tent upsample. Larger spreads the halo.
    pub bloom_radius: f32,
    /// Overall exposure applied before the tonemap curve.
    pub exposure: f32,
    /// How dark the corners get.
    pub vignette_strength: f32,
    /// Normalised radius at which the vignette starts.
    pub vignette_inner: f32,
    /// How far accumulated highlights are pushed toward neutral white. This is
    /// the only thing that turns density into §3.1's near-white hotspots.
    pub highlight_desaturation: f32,
    /// HDR luminance at which whitening begins.
    pub highlight_start: f32,
    /// HDR luminance at which whitening is fully applied.
    pub highlight_end: f32,
}

impl Default for PostSettings {
    fn default() -> Self {
        Self {
            // High enough that only accumulated cluster centres glow, per
            // §7.4's "use bloom sparingly" — a low threshold makes the whole
            // cloud hazy and kills the density read.
            bloom_threshold: 0.8,
            bloom_knee: 0.4,
            bloom_intensity: 0.42,
            bloom_radius: 1.15,
            exposure: 2.1,
            vignette_strength: 0.55,
            vignette_inner: 0.32,
            highlight_desaturation: 0.85,
            // Tuned against measured accumulation, and re-tuned when the point
            // budget moved from 12,000 through a volume to tens of thousands
            // across a surface. That change lowered per-point energy roughly
            // threefold while raising how many points land in a pixel, so the
            // old onset near a single point's luminance meant any two
            // overlapping points whitened — which drained the hue out of the
            // entire entity and made it read as a grey scatter. The onset now
            // sits well above a lone point, so hue survives on the skin and
            // white marks real density, which is the whole claim of §3.2.
            highlight_start: 0.9,
            highlight_end: 3.0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PostUniform {
    texel: [f32; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CompositeUniform {
    params: [f32; 4],
    field: [f32; 4],
    highlight: [f32; 4],
}

struct BloomLevel {
    view: wgpu::TextureView,
    /// Samples the level above, to downsample into this one.
    down_bind_group: wgpu::BindGroup,
    /// Samples this level, to upsample into the level above.
    up_bind_group: wgpu::BindGroup,
    /// Backing uniform for `up_bind_group`, rewritten each frame so the
    /// radius tunable is live rather than baked in at resize.
    up_buffer: wgpu::Buffer,
    /// Texel size of this level, needed to rebuild that uniform.
    texel: [f32; 2],
}

/// Owns every offscreen target and the passes that consume them. Textures and
/// bind groups are rebuilt on resize; pipelines depend only on formats and so
/// are built once.
pub struct PostChain {
    pub settings: PostSettings,

    bright_pipeline: wgpu::RenderPipeline,
    down_pipeline: wgpu::RenderPipeline,
    up_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,

    single_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,

    targets: Targets,
}

impl PostChain {
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("post shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("post.wgsl").into()),
        });

        let texture_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let uniform_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };

        let single_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post single-source layout"),
            entries: &[texture_entry(0), sampler_entry(1), uniform_entry(2)],
        });
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post composite layout"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                sampler_entry(2),
                uniform_entry(3),
            ],
        });

        let make_pipeline = |label: &str,
                             layout: &wgpu::BindGroupLayout,
                             entry: &'static str,
                             format: wgpu::TextureFormat,
                             blend: Option<wgpu::BlendState>| {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[layout],
                push_constant_ranges: &[],
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: "vs_fullscreen",
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: entry,
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };

        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::REPLACE,
        };

        let bright_pipeline = make_pipeline(
            "bloom bright pass",
            &single_layout,
            "fs_bright",
            HDR_FORMAT,
            None,
        );
        let down_pipeline = make_pipeline(
            "bloom downsample",
            &single_layout,
            "fs_down",
            HDR_FORMAT,
            None,
        );
        let up_pipeline = make_pipeline(
            "bloom upsample",
            &single_layout,
            "fs_up",
            HDR_FORMAT,
            Some(additive),
        );
        let composite_pipeline = make_pipeline(
            "post composite",
            &composite_layout,
            "fs_composite",
            surface_format,
            None,
        );

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let settings = PostSettings::default();
        let targets = Targets::build(
            device,
            &single_layout,
            &composite_layout,
            &sampler,
            &settings,
            width,
            height,
        );

        Self {
            settings,
            bright_pipeline,
            down_pipeline,
            up_pipeline,
            composite_pipeline,
            single_layout,
            composite_layout,
            sampler,
            targets,
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.targets = Targets::build(
            device,
            &self.single_layout,
            &self.composite_layout,
            &self.sampler,
            &self.settings,
            width,
            height,
        );
    }

    /// The HDR target the point pass renders into.
    pub fn scene_view(&self) -> &wgpu::TextureView {
        &self.targets.hdr_view
    }

    /// Records the bloom chain and the final composite into `encoder`.
    /// `target` is the swapchain view; the composite writes every pixel of it.
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        target: &wgpu::TextureView,
        palette: &PresencePalette,
    ) {
        // Every tunable is uploaded per frame so the debug panel's sliders are
        // live; these are a handful of 32-byte writes.
        queue.write_buffer(
            &self.targets.composite_buffer,
            0,
            bytemuck::cast_slice(&[self.composite_uniform(palette)]),
        );
        queue.write_buffer(
            &self.targets.bright_buffer,
            0,
            bytemuck::cast_slice(&[PostUniform {
                texel: [
                    self.targets.scene_texel[0],
                    self.targets.scene_texel[1],
                    0.0,
                    0.0,
                ],
                params: [
                    self.settings.bloom_threshold,
                    self.settings.bloom_knee,
                    0.0,
                    0.0,
                ],
            }]),
        );
        for level in &self.targets.levels {
            queue.write_buffer(
                &level.up_buffer,
                0,
                bytemuck::cast_slice(&[PostUniform {
                    texel: [level.texel[0], level.texel[1], 0.0, 0.0],
                    params: [self.settings.bloom_radius, 0.0, 0.0, 0.0],
                }]),
            );
        }

        let levels = &self.targets.levels;
        if let Some(first) = levels.first() {
            fullscreen_pass(
                encoder,
                "bloom bright pass",
                &self.bright_pipeline,
                &self.targets.bright_bind_group,
                &first.view,
                wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            );

            for level in levels.iter().skip(1) {
                fullscreen_pass(
                    encoder,
                    "bloom downsample",
                    &self.down_pipeline,
                    &level.down_bind_group,
                    &level.view,
                    wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                );
            }

            // Walk back up, adding each level into the one above it.
            for i in (1..levels.len()).rev() {
                fullscreen_pass(
                    encoder,
                    "bloom upsample",
                    &self.up_pipeline,
                    &levels[i].up_bind_group,
                    &levels[i - 1].view,
                    wgpu::LoadOp::Load,
                );
            }
        }

        fullscreen_pass(
            encoder,
            "post composite",
            &self.composite_pipeline,
            &self.targets.composite_bind_group,
            target,
            wgpu::LoadOp::Clear(wgpu::Color::BLACK),
        );
    }

    fn composite_uniform(&self, palette: &PresencePalette) -> CompositeUniform {
        let ink = palette.ink;
        CompositeUniform {
            params: [
                self.settings.exposure,
                self.settings.bloom_intensity,
                self.settings.vignette_strength,
                self.settings.vignette_inner,
            ],
            field: [ink[0], ink[1], ink[2], 1.0],
            highlight: [
                self.settings.highlight_desaturation,
                self.settings.highlight_start,
                self.settings.highlight_end,
                0.0,
            ],
        }
    }
}

fn fullscreen_pass(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
    target: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.draw(0..3, 0..1);
}

/// Everything that has to be rebuilt when the window size changes.
struct Targets {
    hdr_view: wgpu::TextureView,
    bright_bind_group: wgpu::BindGroup,
    bright_buffer: wgpu::Buffer,
    /// Texel size of the full-resolution HDR scene.
    scene_texel: [f32; 2],
    levels: Vec<BloomLevel>,
    composite_bind_group: wgpu::BindGroup,
    composite_buffer: wgpu::Buffer,
    /// Held only to keep the downsample uniform buffers alive for as long as
    /// the bind groups that reference them. Those carry no tunables, so unlike
    /// the bright and upsample uniforms they are never rewritten.
    _down_buffers: Vec<wgpu::Buffer>,
}

impl Targets {
    fn build(
        device: &wgpu::Device,
        single_layout: &wgpu::BindGroupLayout,
        composite_layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        settings: &PostSettings,
        width: u32,
        height: u32,
    ) -> Self {
        let width = width.max(1);
        let height = height.max(1);

        let hdr = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hdr scene"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let hdr_view = hdr.create_view(&wgpu::TextureViewDescriptor::default());

        // Bloom level dimensions, halving until they'd become degenerate.
        let mut dimensions: Vec<(u32, u32)> = Vec::new();
        let (mut w, mut h) = (width, height);
        for _ in 0..BLOOM_LEVELS {
            w = (w / 2).max(1);
            h = (h / 2).max(1);
            if w < MIN_BLOOM_DIMENSION || h < MIN_BLOOM_DIMENSION {
                break;
            }
            dimensions.push((w, h));
        }

        let views: Vec<wgpu::TextureView> = dimensions
            .iter()
            .map(|(lw, lh)| {
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("bloom level"),
                    size: wgpu::Extent3d {
                        width: *lw,
                        height: *lh,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: HDR_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                texture.create_view(&wgpu::TextureViewDescriptor::default())
            })
            .collect();

        let make_uniform = |label: &str, source_w: u32, source_h: u32, params: [f32; 4]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(&[PostUniform {
                    texel: [
                        1.0 / source_w.max(1) as f32,
                        1.0 / source_h.max(1) as f32,
                        0.0,
                        0.0,
                    ],
                    params,
                }]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            })
        };

        let make_single_bind_group =
            |label: &str, view: &wgpu::TextureView, buffer: &wgpu::Buffer| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(label),
                    layout: single_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: buffer.as_entire_binding(),
                        },
                    ],
                })
            };

        // The bright pass samples the full-resolution HDR scene.
        let bright_buffer = make_uniform(
            "bloom bright uniform",
            width,
            height,
            [settings.bloom_threshold, settings.bloom_knee, 0.0, 0.0],
        );
        let bright_bind_group =
            make_single_bind_group("bloom bright bind group", &hdr_view, &bright_buffer);

        let mut down_bind_groups: Vec<wgpu::BindGroup> = Vec::with_capacity(views.len());
        let mut down_buffers: Vec<wgpu::Buffer> = Vec::with_capacity(views.len());
        let mut up_bind_groups: Vec<wgpu::BindGroup> = Vec::with_capacity(views.len());
        let mut up_buffers: Vec<wgpu::Buffer> = Vec::with_capacity(views.len());
        for (i, (lw, lh)) in dimensions.iter().enumerate() {
            // Downsampling into level i samples level i-1. Level 0's source is
            // the HDR scene, which the bright pass covers instead, so its
            // down bind group is never used — built only to keep indices
            // aligned with the level list.
            let (source_view, source_w, source_h) = if i == 0 {
                (&hdr_view, width, height)
            } else {
                let (sw, sh) = dimensions[i - 1];
                (&views[i - 1], sw, sh)
            };
            let down_buffer = make_uniform("bloom down uniform", source_w, source_h, [0.0; 4]);
            down_bind_groups.push(make_single_bind_group(
                "bloom down bind group",
                source_view,
                &down_buffer,
            ));
            down_buffers.push(down_buffer);

            // Upsampling from level i samples level i at its own resolution.
            let up_buffer = make_uniform(
                "bloom up uniform",
                *lw,
                *lh,
                [settings.bloom_radius, 0.0, 0.0, 0.0],
            );
            up_bind_groups.push(make_single_bind_group(
                "bloom up bind group",
                &views[i],
                &up_buffer,
            ));
            up_buffers.push(up_buffer);
        }

        let composite_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("post composite uniform"),
            size: std::mem::size_of::<CompositeUniform>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // With no bloom levels (a very small window) the composite still needs
        // a second texture to sample; the HDR scene stands in, scaled down by
        // the bloom intensity to a harmless amount.
        let bloom_source = views.first().unwrap_or(&hdr_view);
        let composite_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post composite bind group"),
            layout: composite_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&hdr_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(bloom_source),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: composite_buffer.as_entire_binding(),
                },
            ],
        });

        let levels = views
            .into_iter()
            .zip(down_bind_groups)
            .zip(up_bind_groups)
            .zip(up_buffers)
            .zip(dimensions.iter())
            .map(
                |((((view, down_bind_group), up_bind_group), up_buffer), (lw, lh))| BloomLevel {
                    view,
                    down_bind_group,
                    up_bind_group,
                    up_buffer,
                    texel: [1.0 / *lw as f32, 1.0 / *lh as f32],
                },
            )
            .collect();

        Self {
            hdr_view,
            bright_bind_group,
            bright_buffer,
            scene_texel: [1.0 / width as f32, 1.0 / height as f32],
            levels,
            composite_bind_group,
            composite_buffer,
            _down_buffers: down_buffers,
        }
    }
}
