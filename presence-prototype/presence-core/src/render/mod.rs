pub mod camera;
pub mod post;

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::palette::{PaletteId, PresencePalette};
use crate::sim::Particle;
use camera::{Camera, PointMaterial};
use post::PostChain;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct QuadVertex {
    corner: [f32; 2],
}

/// Starting (and floor) capacity of the GPU instance buffer, in instances.
/// The buffer grows past this on demand and may shrink back toward it, but
/// never below — a small permanent reservation avoids reallocating on the
/// common small-scene case.
const INITIAL_INSTANCE_CAPACITY: usize = 8_192;

/// Frames of sustained low utilization before the instance buffer is shrunk.
/// ~10 s at 60 FPS: long enough that a brief scene simplification doesn't
/// thrash the allocator, short enough that peak memory isn't pinned forever.
const INSTANCE_SHRINK_AFTER_FRAMES: u32 = 600;

const QUAD_VERTICES: [QuadVertex; 4] = [
    QuadVertex {
        corner: [-1.0, -1.0],
    },
    QuadVertex {
        corner: [1.0, -1.0],
    },
    QuadVertex {
        corner: [-1.0, 1.0],
    },
    QuadVertex { corner: [1.0, 1.0] },
];

/// Per-point instance data. 48 bytes, up from 32 when the surface normal and
/// crease were added — the extra 16 bytes buy the silhouette and fold
/// filaments, which are the two things that separate a scanned surface from a
/// spray of dots.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct InstanceRaw {
    position: [f32; 3],
    size: f32,
    brightness: f32,
    color_bias: f32,
    layer: f32,
    crease: f32,
    /// Zero for volume-based entities, which the shader reads as "no
    /// silhouette term" so both entity families share one pipeline.
    normal: [f32; 3],
    _pad: f32,
}

/// Owns the GPU device/queue/surface plus the point-cloud render pipeline.
/// `egui`'s renderer (see `crate::ui`) shares `device`/`queue`/`surface`
/// with this struct rather than creating its own.
pub struct Renderer {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface_config: wgpu::SurfaceConfiguration,
    pub camera: Camera,
    pub material: PointMaterial,
    pub post: PostChain,
    /// The active colour scheme. A runtime value rather than a constant
    /// because it is a user setting — see `crate::palette`.
    pub palette: PresencePalette,

    pipeline: wgpu::RenderPipeline,
    quad_vertex_buffer: wgpu::Buffer,
    camera_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    /// Reused CPU staging vector for per-frame instance data. Cleared and
    /// refilled each frame instead of allocating a fresh `Vec` — the old
    /// per-frame allocation showed up as GC-like hitches and steady allocator
    /// pressure at high point counts.
    instance_scratch: Vec<InstanceRaw>,
    /// Consecutive frames the instance buffer has been under-utilized, driving
    /// the hysteresis in [`Renderer::ensure_instance_capacity`] so the buffer
    /// can shrink back toward [`INITIAL_INSTANCE_CAPACITY`] after a peak.
    instance_low_frames: u32,
}

/// Construction-time options. Grouping them here rather than as
/// positional args keeps the runtime's `new(...)` call readable and
/// leaves room for the next few knobs (custom point budget, vsync
/// override) without another API break.
#[derive(Debug, Clone, Copy, Default)]
pub struct RendererOptions {
    /// Ask for a per-pixel alpha swapchain and drive the composite in
    /// premultiplied-alpha mode. Falls back to the platform default
    /// (opaque) with a `warn!` log if the adapter reports no
    /// compatible alpha mode — no visual difference on that path,
    /// which is what we want on machines that can't do it.
    pub transparent: bool,
}

impl Renderer {
    pub async fn new(window: Arc<Window>) -> Self {
        Self::new_with_options(window, RendererOptions::default()).await
    }

    pub async fn new_with_options(window: Arc<Window>, options: RendererOptions) -> Self {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let surface = instance
            .create_surface(window.clone())
            .expect("create wgpu surface");

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("no suitable GPU adapter found (see README for troubleshooting)");

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("presence-prototype device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                        .using_resolution(adapter.limits()),
                    memory_hints: wgpu::MemoryHints::default(),
                },
                None,
            )
            .await
            .expect("failed to create wgpu device");

        let surface_caps = surface.get_capabilities(&adapter);
        let format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);

        // Alpha mode selection. When `transparent` is requested, prefer
        // `PreMultiplied` (Windows/DX12 supports it) then `PostMultiplied`;
        // otherwise use the adapter's first preference (usually `Opaque`).
        // Falling back to Opaque with a warn keeps this path safe on
        // adapters that can't do per-pixel alpha — the droplet just shows
        // its brand-ink background there.
        let alpha_mode = if options.transparent {
            let picked = surface_caps.alpha_modes.iter().copied().find(|m| {
                matches!(
                    m,
                    wgpu::CompositeAlphaMode::PreMultiplied
                        | wgpu::CompositeAlphaMode::PostMultiplied
                )
            });
            match picked {
                Some(m) => m,
                None => {
                    log::warn!(
                        "presence-core: transparent surface requested, but no \
                         per-pixel alpha mode is available on this adapter \
                         (falling back to {:?}). Droplet will render opaque.",
                        surface_caps.alpha_modes[0]
                    );
                    surface_caps.alpha_modes[0]
                }
            }
        } else {
            surface_caps.alpha_modes[0]
        };
        // Only claim we're transparent to the composite shader when the
        // surface actually is — otherwise the coverage-alpha output would
        // pre-darken every pixel by its own alpha over an opaque swapchain,
        // which reads as a heavy vignette on adapters that fell back.
        let transparent_effective = options.transparent
            && matches!(
                alpha_mode,
                wgpu::CompositeAlphaMode::PreMultiplied | wgpu::CompositeAlphaMode::PostMultiplied
            );

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        let camera = Camera::new(surface_config.width as f32 / surface_config.height as f32);
        let material = PointMaterial::default();
        // `PRESENCE_PALETTE` stands in for the settings field this reads from
        // once the entity is hosted in `desktop-edge`; the prototype's debug
        // panel switches it live.
        let palette = std::env::var("PRESENCE_PALETTE")
            .map(|name| PaletteId::from_str_or_default(&name))
            .unwrap_or(PaletteId::Teal)
            .palette();

        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera uniform"),
            contents: bytemuck::cast_slice(&[camera.uniform(
                surface_config.height as f32,
                &material,
                &palette,
            )]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("point shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("point pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[wgpu::VertexAttribute {
                offset: 0,
                shader_location: 0,
                format: wgpu::VertexFormat::Float32x2,
            }],
        };

        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<InstanceRaw>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 12,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32,
                },
                wgpu::VertexAttribute {
                    offset: 16,
                    shader_location: 3,
                    format: wgpu::VertexFormat::Float32,
                },
                wgpu::VertexAttribute {
                    offset: 20,
                    shader_location: 4,
                    format: wgpu::VertexFormat::Float32,
                },
                wgpu::VertexAttribute {
                    offset: 24,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Float32,
                },
                wgpu::VertexAttribute {
                    offset: 28,
                    shader_location: 6,
                    format: wgpu::VertexFormat::Float32,
                },
                wgpu::VertexAttribute {
                    offset: 32,
                    shader_location: 7,
                    format: wgpu::VertexFormat::Float32x3,
                },
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("point pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[vertex_layout, instance_layout],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                // Renders into the HDR scene target, not the swapchain — the
                // tonemap composite in `post` is what writes the surface.
                targets: &[Some(wgpu::ColorTargetState {
                    format: post::HDR_FORMAT,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let quad_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("quad vertex buffer"),
            contents: bytemuck::cast_slice(&QUAD_VERTICES),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let initial_capacity = INITIAL_INSTANCE_CAPACITY;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instance buffer"),
            size: (initial_capacity * std::mem::size_of::<InstanceRaw>()) as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut post = PostChain::new(
            &device,
            surface_config.format,
            surface_config.width,
            surface_config.height,
        );
        post.transparent = transparent_effective;

        Self {
            surface,
            device,
            queue,
            surface_config,
            camera,
            material,
            post,
            palette,
            pipeline,
            quad_vertex_buffer,
            camera_buffer,
            bind_group,
            instance_buffer,
            instance_capacity: initial_capacity,
            instance_scratch: Vec::with_capacity(initial_capacity),
            instance_low_frames: 0,
        }
    }

    pub fn animate_camera(&mut self, dt: f32) {
        self.camera.animate(dt);
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        self.camera.set_aspect(width as f32 / height as f32);
        self.post.resize(&self.device, width, height);
    }

    fn ensure_instance_capacity(&mut self, needed: usize) {
        if needed > self.instance_capacity {
            let new_capacity = needed.next_power_of_two();
            self.instance_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("instance buffer (grown)"),
                size: (new_capacity * std::mem::size_of::<InstanceRaw>()) as wgpu::BufferAddress,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_capacity = new_capacity;
            self.instance_low_frames = 0;
            return;
        }

        // Shrink with hysteresis: only after sustained under-use (needed at or
        // below a quarter of capacity for `INSTANCE_SHRINK_AFTER_FRAMES`
        // frames), and never below `INITIAL_INSTANCE_CAPACITY`. This releases
        // the memory a transient peak reserved without reallocating on every
        // small scene change.
        if self.instance_capacity > INITIAL_INSTANCE_CAPACITY
            && needed <= self.instance_capacity / 4
        {
            self.instance_low_frames = self.instance_low_frames.saturating_add(1);
            if self.instance_low_frames >= INSTANCE_SHRINK_AFTER_FRAMES {
                let target = needed.next_power_of_two().max(INITIAL_INSTANCE_CAPACITY);
                if target < self.instance_capacity {
                    self.instance_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("instance buffer (shrunk)"),
                        size: (target * std::mem::size_of::<InstanceRaw>()) as wgpu::BufferAddress,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    self.instance_capacity = target;
                }
                self.instance_low_frames = 0;
            }
        } else {
            self.instance_low_frames = 0;
        }
    }

    /// Uploads the camera uniform and all currently-visible particles
    /// (concatenated across every active entity), then draws the point
    /// pass into a fresh frame and hands the encoder/view back so the
    /// caller (see `crate::app`) can layer the `egui` debug overlay into
    /// the *same* encoder before presenting — see `Frame::finish`.
    ///
    /// `entity_particles` is `(particles, presence_opacity)` per entity so
    /// per-particle brightness can be scaled by the transition fade
    /// without mutating simulation state.
    pub fn begin_frame(
        &mut self,
        entity_particles: &[(&[Particle], f32)],
    ) -> Result<Frame, wgpu::SurfaceError> {
        // Refill the reused staging vector rather than allocating a new one
        // each frame (see `instance_scratch`).
        {
            let scratch = &mut self.instance_scratch;
            scratch.clear();
            for (particles, opacity) in entity_particles {
                for p in particles.iter() {
                    scratch.push(InstanceRaw {
                        position: p.position.to_array(),
                        size: p.size,
                        brightness: p.brightness * opacity,
                        color_bias: p.color_bias,
                        layer: p.layer.as_f32(),
                        crease: p.crease,
                        normal: p.normal.to_array(),
                        _pad: 0.0,
                    });
                }
            }
        }
        let instance_count = self.instance_scratch.len();

        self.ensure_instance_capacity(instance_count.max(1));
        if instance_count > 0 {
            self.queue.write_buffer(
                &self.instance_buffer,
                0,
                bytemuck::cast_slice(&self.instance_scratch),
            );
        }
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&[self.camera.uniform(
                self.surface_config.height as f32,
                &self.material,
                &self.palette,
            )]),
        );

        let output = self.surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("point render encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("point pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    // Cleared to black rather than the brand ink: the field
                    // colour is added *after* tonemapping in the composite, so
                    // that the near-black ink lands at exactly its intended
                    // value instead of being crushed by the ACES toe.
                    view: self.post.scene_view(),
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            if instance_count > 0 {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, self.quad_vertex_buffer.slice(..));
                pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
                pass.draw(0..4, 0..instance_count as u32);
            }
        }

        // Bloom + tonemap + vignette, ending in the swapchain. The `egui`
        // overlay is drawn onto `view` afterwards by the caller, deliberately
        // *outside* the tonemap so debug UI stays at its authored colours.
        self.post
            .render(&mut encoder, &self.queue, &view, &self.palette);

        Ok(Frame {
            output,
            view,
            encoder,
        })
    }
}

/// An in-flight frame: the point pass has already been recorded. The
/// caller may record additional passes (the `egui` overlay) into
/// `encoder`/`view` with `LoadOp::Load` before calling `finish`.
pub struct Frame {
    output: wgpu::SurfaceTexture,
    pub view: wgpu::TextureView,
    pub encoder: wgpu::CommandEncoder,
}

impl Frame {
    pub fn finish(self, queue: &wgpu::Queue) {
        queue.submit(std::iter::once(self.encoder.finish()));
        self.output.present();
    }
}
