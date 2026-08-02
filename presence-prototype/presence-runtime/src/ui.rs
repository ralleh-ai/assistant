//! Dev-only debug overlay — `docs/PRESENCE_VISUAL_ENTITY.md` §9's "simple
//! debug overlay" requirement: current mode/entities + raw signal values,
//! plus the scene-select controls Phase 1 needs since there's no real
//! assistant signal to drive this prototype yet.

use egui_wgpu::ScreenDescriptor;
use winit::event::WindowEvent;
use winit::window::Window;

use presence_core::palette::{PaletteId, PresencePalette};
use presence_core::render::camera::PointMaterial;
use presence_core::render::post::PostSettings;
use presence_core::render::Frame;
use presence_core::scene::entity::EntityInstance;
use presence_core::scene::mode::PresenceMode;
use presence_core::scene::{SceneDirector, SceneRegistry};
use presence_core::sim::Layer;

pub struct EguiLayer {
    context: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
}

/// Everything the panel reads or writes, separated from the GPU plumbing
/// `draw` needs. Borrowed rather than owned: the panel edits the live
/// material/post/palette in place, which is the entire point of having it.
pub struct PanelState<'a> {
    pub director: &'a mut SceneDirector,
    pub registry: &'a SceneRegistry,
    pub material: &'a mut PointMaterial,
    pub post: &'a mut PostSettings,
    pub palette: &'a mut PresencePalette,
    pub fps: f32,
}

impl EguiLayer {
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat, window: &Window) -> Self {
        let context = egui::Context::default();
        let viewport_id = context.viewport_id();
        let state = egui_winit::State::new(context.clone(), viewport_id, window, None, None, None);
        let renderer = egui_wgpu::Renderer::new(device, color_format, None, 1, false);
        Self {
            context,
            state,
            renderer,
        }
    }

    /// Forwards a window event to `egui`. Returns `true` if `egui` wants
    /// to consume it (e.g. the pointer is over a panel).
    pub fn handle_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        self.state.on_window_event(window, event).consumed
    }

    /// Builds this frame's UI and records it into `frame.encoder`,
    /// targeting `frame.view` with `LoadOp::Load` so the point cloud drawn
    /// earlier this frame stays visible underneath.
    pub fn draw(
        &mut self,
        window: &Window,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &mut Frame,
        screen_descriptor: &ScreenDescriptor,
        panel: &mut PanelState,
    ) {
        let Frame { view, encoder, .. } = frame;
        let raw_input = self.state.take_egui_input(window);
        let full_output = self.context.run(raw_input, |ctx| {
            build_panel(ctx, panel);
        });

        self.state
            .handle_platform_output(window, full_output.platform_output);

        let clipped_primitives = self
            .context
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        for (id, delta) in &full_output.textures_delta.set {
            self.renderer.update_texture(device, queue, *id, delta);
        }
        self.renderer.update_buffers(
            device,
            queue,
            encoder,
            &clipped_primitives,
            screen_descriptor,
        );

        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("egui pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                })
                .forget_lifetime();
            self.renderer
                .render(&mut pass, &clipped_primitives, screen_descriptor);
        }

        for id in &full_output.textures_delta.free {
            self.renderer.free_texture(id);
        }
    }
}

/// `Core`/`Body`/`Halo` population counts. The layering in §3.3 is easy to
/// get subtly wrong (a generator change can silently empty a layer), and the
/// visual difference between "sparse halo" and "no halo" is hard to eyeball,
/// so the split is shown numerically.
fn layer_counts(entity: &EntityInstance) -> (usize, usize, usize) {
    let mut counts = (0, 0, 0);
    for p in &entity.particles {
        match p.layer {
            Layer::Core => counts.0 += 1,
            Layer::Body => counts.1 += 1,
            Layer::Halo => counts.2 += 1,
        }
    }
    counts
}

fn build_panel(ctx: &egui::Context, panel: &mut PanelState) {
    let PanelState {
        director,
        registry,
        material,
        post,
        palette,
        fps,
    } = panel;
    let fps = *fps;

    // Deliberately compact and collapsible: the panel shares a small window
    // with the thing it is describing, and a debug overlay that covers the
    // entity is useless for judging the entity.
    egui::Window::new("Presence — Phase 1 Prototype")
        .default_pos([12.0, 12.0])
        .default_width(268.0)
        // Collapsed on open. The whole point of this prototype is judging how
        // the entity looks, and a panel that covers a third of a small window
        // makes that impossible; one click expands it.
        .default_open(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label(format!("fps: {fps:.0}"));
            ui.separator();

            egui::CollapsingHeader::new("Scenes")
                .default_open(false)
                .show(ui, |ui| {
                    for descriptor in registry.all() {
                        ui.label(format!("• {} — {}", descriptor.label, descriptor.summary));
                    }
                });
            ui.separator();
            let (core, body, halo) = layer_counts(&director.assistant_cloud);
            ui.label(format!(
                "{} (idle): {} pts, presence {:.2}",
                director.assistant_cloud.kind.label(),
                director.assistant_cloud.particles.len(),
                director.assistant_cloud.presence
            ));
            ui.label(format!(
                "   layers — core {core} · body {body} · halo {halo}"
            ));
            ui.label(format!(
                "{}: {} pts, presence {:.2} ({})",
                director.loading_ring.kind.label(),
                director.loading_ring.particles.len(),
                director.loading_ring.presence,
                if director.ring_wanted {
                    "wanted"
                } else {
                    "fading out"
                }
            ));
            ui.separator();

            let mut ring_on = director.ring_wanted;
            if ui
                .checkbox(&mut ring_on, "Loading (secondary entity) — [L]")
                .changed()
            {
                director.set_ring_wanted(ring_on);
            }
            ui.separator();

            // Checkboxes rather than a radio group, because the modes are
            // additive and the overlaps are the part worth looking at — a
            // single-select control would make the model's central claim
            // untestable by hand.
            ui.heading(format!(
                "Mode: {}{}",
                director.modes.summary(),
                if director.modes.is_settled() {
                    ""
                } else {
                    " (settling)"
                }
            ));
            for mode in PresenceMode::ALL {
                let mut on = director.modes.is_engaged(mode);
                let label = format!("{} — [{}]", mode.label(), mode.key());
                if ui.checkbox(&mut on, label).changed() {
                    director.set_mode(mode, on);
                }
            }
            let drive = director.modes.drive();
            ui.label(format!(
                "   fold {:.2} · lobes {:.2} · pulse {:.2} · neck {:.2}",
                drive.fold, drive.lobes, drive.pulse, drive.neck
            ));
            ui.label(format!(
                "   activity_scale {:.2} (dampen while Loading composites)",
                director.activity_scale()
            ));
            ui.checkbox(&mut director.reduced_motion, "reduced motion — [R]");

            ui.horizontal(|ui| {
                ui.label("quality:");
                let mut selected = director.tier();
                egui::ComboBox::from_id_salt("quality")
                    .selected_text(selected.label())
                    .show_ui(ui, |ui| {
                        for tier in presence_core::scene::QualityTier::ALL {
                            ui.selectable_value(&mut selected, tier, tier.label());
                        }
                    });
                if selected != director.tier() {
                    director.set_quality_tier(selected);
                }
            });
            ui.separator();

            ui.heading("Signals (dev override)");
            ui.add(egui::Slider::new(&mut director.signals.intensity, 0.0..=1.5).text("intensity"));
            ui.add(
                egui::Slider::new(&mut director.signals.audio_level, 0.0..=1.0).text("audio_level"),
            );
            ui.add(egui::Slider::new(&mut director.signals.progress, 0.0..=1.0).text("progress"));
            ui.separator();

            // Stands in for the settings control this becomes in the shell.
            // Switching live is the point: it is the check that nothing in the
            // render path still assumes a compile-time hue.
            ui.horizontal(|ui| {
                ui.label("palette:");
                let mut selected = palette.id;
                egui::ComboBox::from_id_salt("palette")
                    .selected_text(selected.as_str())
                    .show_ui(ui, |ui| {
                        for id in PaletteId::ALL {
                            ui.selectable_value(&mut selected, id, id.as_str());
                        }
                    });
                if selected != palette.id {
                    **palette = selected.palette();
                }
            });
            ui.separator();

            egui::CollapsingHeader::new("Surface & grade")
                .default_open(false)
                .show(ui, |ui| {
                    ui.add(
                        egui::Slider::new(&mut material.calm_undertone, 0.0..=1.0)
                            .text("hue undertone"),
                    );
                    ui.add(
                        egui::Slider::new(&mut material.grazing_boost, 0.0..=4.0)
                            .text("silhouette"),
                    );
                    ui.add(
                        egui::Slider::new(&mut material.crease_boost, 0.0..=5.0).text("creases"),
                    );
                    ui.add(
                        egui::Slider::new(&mut material.point_scale, 0.004..=0.04)
                            .text("point size"),
                    );
                    ui.add(
                        egui::Slider::new(&mut material.tint_energy_scale, 0.05..=3.0)
                            .text("tint energy"),
                    );
                    ui.add(egui::Slider::new(&mut post.exposure, 0.2..=3.0).text("exposure"));
                    ui.add(
                        egui::Slider::new(&mut post.bloom_intensity, 0.0..=1.0)
                            .text("bloom intensity"),
                    );
                    ui.add(
                        egui::Slider::new(&mut post.bloom_threshold, 0.0..=2.0)
                            .text("bloom threshold"),
                    );
                    ui.add(
                        egui::Slider::new(&mut post.bloom_radius, 0.5..=3.0).text("bloom radius"),
                    );
                    ui.add(
                        egui::Slider::new(&mut post.highlight_desaturation, 0.0..=1.0)
                            .text("highlight whiten"),
                    );
                    ui.add(
                        egui::Slider::new(&mut post.highlight_start, 0.0..=4.0)
                            .text("whiten start"),
                    );
                    ui.add(
                        egui::Slider::new(&mut post.highlight_end, 0.1..=8.0).text("whiten end"),
                    );
                    ui.add(
                        egui::Slider::new(&mut post.vignette_strength, 0.0..=1.0).text("vignette"),
                    );
                });
            ui.separator();
            ui.label(
                "Keys: [L] loading · [T] thinking · [S] speaking · [U] tool_use \
                 · [N] listening · [A] attention · [E] error · [R] reduced motion \
                 · [Q] cycle quality · [Esc] quit",
            );
        });
}
