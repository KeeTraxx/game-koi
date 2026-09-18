//! The CRT display pass.
//!
//! A post-process between the scaler and the surface. `pixels` scales the 160x144
//! framebuffer; normally it does that straight into the surface, but with an effect
//! switched on it draws into an intermediate texture instead and this module reads that
//! back, applies the effect, and writes the surface. `ScalingRenderer::render` takes any
//! texture view rather than insisting on the surface, which is what makes the
//! redirection possible without forking `pixels`.
//!
//! # Why a post-process and not the framebuffer
//!
//! The tempting shortcut is to darken alternate rows while expanding shades into
//! `Pixels::frame_mut`. That is wrong, not merely crude: the framebuffer is 160x144 at
//! 1:1, so "alternate rows" means alternate *emulated* scanlines — half the picture at
//! full strength no matter how large the window is. A scanline is a property of the
//! display, so it has to be applied in the display's own pixels, which is what
//! `@builtin(position)` in `crt.wgsl` gives.
//!
//! # What the intermediate texture buys
//!
//! A darkening-only effect could be done as a blended pass straight over the surface,
//! with no intermediate and no sampler. Reading the scaled picture costs one texture but
//! is what any real CRT treatment needs: compensating the brightness the scanlines take
//! out (which this already does), and later barrel distortion, a phosphor mask, or
//! bloom, all of which have to sample neighbouring pixels.
//!
//! When the mode is [`CrtMode::Off`] none of this runs — the scaler writes the surface
//! directly, exactly as it did before this module existed.

use pixels::wgpu;

use game_koi_core::ppu::SCREEN_HEIGHT;

/// How deep the dark rows go, from 0.0 to 1.0.
///
/// Compensated for afterwards (see `crt.wgsl`), so this trades contrast between the lit
/// and unlit rows rather than overall brightness. Half measured out is a visible CRT
/// texture without the picture turning into a venetian blind.
const SCANLINE_STRENGTH: f32 = 0.5;

/// Which display effect the pass applies.
///
/// Cycled with F3 rather than toggled, because this is the first of several: the
/// intermediate texture exists so that a full CRT treatment can be added as another
/// variant without disturbing the plumbing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CrtMode {
    /// Straight to the surface. No intermediate texture, no second pass.
    #[default]
    Off,
    /// Dark rows between the emulated scanlines, brightness-compensated.
    Scanlines,
}

impl CrtMode {
    /// Every mode, in the order F3 steps through them.
    ///
    /// One list rather than a hand-written `match` in `next`, so adding a variant cannot
    /// accidentally leave it unreachable from the key.
    const ALL: [CrtMode; 2] = [CrtMode::Off, CrtMode::Scanlines];

    /// The next mode in the cycle, wrapping round to [`CrtMode::Off`].
    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|&mode| mode == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    /// How to name it in the overlay.
    pub fn label(self) -> &'static str {
        match self {
            CrtMode::Off => "off",
            CrtMode::Scanlines => "scanlines",
        }
    }

    /// Whether this mode needs the post-processing pass to run at all.
    ///
    /// `Off` is not "the effect with its strength at zero": it skips the intermediate
    /// texture and the second pass entirely, so a feature that is switched off costs a
    /// branch rather than a render pass.
    pub fn needs_pass(self) -> bool {
        !matches!(self, CrtMode::Off)
    }
}

/// The intermediate texture and the pipeline that reads it.
pub struct Crt {
    /// The scaled picture before the effect, which the scaler draws into.
    ///
    /// Kept at the surface's size and format: the scaler's own pipeline is built for the
    /// surface format and will not render into anything else, and matching the size
    /// keeps the final pass a 1:1 blit.
    source: wgpu::Texture,
    source_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    /// Kept so the bind group can be rebuilt when a resize replaces the texture.
    layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
    /// The surface format, likewise needed to rebuild the texture on resize.
    format: wgpu::TextureFormat,
    /// The intermediate's current size, so an event that did not actually change it does
    /// not throw the texture away and make a new one.
    size: (u32, u32),
}

impl Crt {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        size: winit::dpi::PhysicalSize<u32>,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("crt_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("crt.wgsl").into()),
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("crt_bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("crt_pipeline_layout"),
            bind_group_layouts: &[Some(&layout)],
            // wgpu 29's name for push constants. Nothing here uses them: the uniform
            // buffer carries the parameters, which avoids depending on a feature that is
            // not in the default set.
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("crt_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                // The full-screen triangle is generated from the vertex index, so there
                // is nothing to bind.
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // Opaque: the pass writes every pixel of the surface, so there is
                    // nothing underneath to blend with.
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                // No culling, so the triangle's winding does not matter.
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("crt_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            // Linear, though at 1:1 it returns exactly the texel a nearest sampler
            // would. It is here for the effects that will sample off centre.
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("crt_uniforms"),
            size: PARAMS_SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let size = (size.width.max(1), size.height.max(1));
        let (source, source_view) = create_source(device, format, size);
        let bind_group = create_bind_group(device, &layout, &source_view, &sampler, &uniforms);

        Crt {
            source,
            source_view,
            sampler,
            uniforms,
            layout,
            bind_group,
            pipeline,
            format,
            size,
        }
    }

    /// The view the scaler should draw into when an effect is on.
    pub fn source_view(&self) -> &wgpu::TextureView {
        &self.source_view
    }

    /// Rebuilds the intermediate texture for a new surface size.
    ///
    /// `image` is irrelevant here — only the surface size matters, because the
    /// intermediate mirrors the surface rather than the picture inside it.
    pub fn resize(&mut self, device: &wgpu::Device, size: winit::dpi::PhysicalSize<u32>) {
        // A minimised window reports a zero dimension on some platforms, and a texture
        // of zero extent is a validation error.
        let size = (size.width.max(1), size.height.max(1));
        if size == self.size {
            return;
        }

        let (source, source_view) = create_source(device, self.format, size);
        self.bind_group = create_bind_group(
            device,
            &self.layout,
            &source_view,
            &self.sampler,
            &self.uniforms,
        );
        self.source = source;
        self.source_view = source_view;
        self.size = size;
    }

    /// Updates the uniforms for this frame.
    ///
    /// `image` is the scaler's clip rect: where the picture sits inside the surface,
    /// which is what the shader needs to leave the letterbox borders alone and to line
    /// the stripes up with the emulated lines.
    pub fn prepare(&self, queue: &wgpu::Queue, image: (u32, u32, u32, u32)) {
        let bytes = params_bytes(image, scanline_period(image.3), SCANLINE_STRENGTH);
        queue.write_buffer(&self.uniforms, 0, &bytes);
    }

    /// Draws the intermediate texture to `target`, with the effect applied.
    pub fn render(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("crt_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    // Clear rather than Load: the triangle covers the whole surface, so
                    // there is nothing worth preserving underneath.
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Makes the intermediate texture and its view.
///
/// `RENDER_ATTACHMENT` because the scaler draws into it, `TEXTURE_BINDING` because the
/// effect pass reads it — the pair is the whole reason it cannot just be the surface.
fn create_source(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    (width, height): (u32, u32),
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("crt_source"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    source_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    uniforms: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("crt_bind_group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(source_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniforms.as_entire_binding(),
            },
        ],
    })
}

/// Size of the uniform block, in bytes.
///
/// Four floats for the image rect, two for the period and strength, two of padding: a
/// uniform struct's size has to be a multiple of 16.
const PARAMS_SIZE: usize = 32;

/// How many physical rows one emulated scanline covers.
///
/// Taken from the picture's height on screen, not the window's. The scaler letterboxes
/// to hold 10:9, so a window of the wrong shape has black borders, and dividing by its
/// full height would put the stripes out of step with the lines they sit between.
fn scanline_period(image_height: u32) -> f32 {
    image_height as f32 / SCREEN_HEIGHT as f32
}

/// Packs the uniform block to match `Params` in `crt.wgsl`.
///
/// By hand because `bytemuck` is not in the tree and this is eight floats; the two
/// crates that would bring it in are host-side and it is not worth a dependency. The
/// layout is the contract with the shader — change one and the other has to follow.
fn params_bytes(image: (u32, u32, u32, u32), period: f32, strength: f32) -> [u8; PARAMS_SIZE] {
    let values = [
        image.0 as f32,
        image.1 as f32,
        image.2 as f32,
        image.3 as f32,
        period,
        strength,
        0.0,
        0.0,
    ];

    let mut bytes = [0u8; PARAMS_SIZE];
    // as_chunks_mut rather than chunks_exact_mut: the chunk size is a constant, so each
    // chunk comes back as a `&mut [u8; 4]` and the write is a plain assignment with no
    // length check to elide. PARAMS_SIZE is a multiple of 4, so the `.1` remainder is
    // empty by construction.
    for (chunk, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(values) {
        *chunk = value.to_ne_bytes();
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f3_cycles_through_every_mode_and_wraps() {
        // Every variant has to be reachable from the key, and the cycle has to close.
        let mut mode = CrtMode::Off;
        let mut seen = vec![mode];
        for _ in 0..CrtMode::ALL.len() - 1 {
            mode = mode.next();
            assert!(!seen.contains(&mode), "{mode:?} came round twice");
            seen.push(mode);
        }
        assert_eq!(seen.len(), CrtMode::ALL.len(), "every mode is reachable");
        assert_eq!(mode.next(), CrtMode::Off, "the cycle wraps");
    }

    #[test]
    fn only_off_skips_the_pass() {
        assert!(!CrtMode::Off.needs_pass());
        for mode in CrtMode::ALL.into_iter().filter(|m| *m != CrtMode::Off) {
            assert!(mode.needs_pass(), "{mode:?} needs the pass");
        }
    }

    #[test]
    fn the_period_comes_from_the_picture_height() {
        // The default window is 144 * 4 tall, so four rows per emulated line.
        assert_eq!(scanline_period(SCREEN_HEIGHT as u32 * 4), 4.0);
        // A letterboxed window: the picture is shorter than the surface, and it is the
        // picture that sets the period.
        assert_eq!(scanline_period(SCREEN_HEIGHT as u32 * 2), 2.0);
    }

    #[test]
    fn a_fractional_period_is_not_rounded_away() {
        // Resizing freely gives periods like this, and the shader's cosine mask is what
        // copes with them — rounding here would make the stripes drift out of step.
        let period = scanline_period(500);
        assert!((period - 3.4722).abs() < 0.001, "got {period}");
    }

    #[test]
    fn the_uniform_block_is_packed_where_the_shader_expects() {
        let bytes = params_bytes((8, 16, 640, 576), 4.0, 0.5);
        assert_eq!(bytes.len() % 16, 0, "a uniform struct must be 16-aligned");

        // Read the floats back out the way the GPU will, to catch a transposed field.
        let at = |index: usize| {
            let offset = index * 4;
            f32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap())
        };
        assert_eq!([at(0), at(1), at(2), at(3)], [8.0, 16.0, 640.0, 576.0]);
        assert_eq!(at(4), 4.0, "period follows the rect");
        assert_eq!(at(5), 0.5, "then strength");
    }
}
