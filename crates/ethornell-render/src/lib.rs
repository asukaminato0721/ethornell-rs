use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use ethornell_core::{classify_native_blend, EthornellError, NativeBlendPath, Result};
use ethornell_image::DecodedImage;
use std::collections::{BTreeMap, BTreeSet};
use winit::window::Window;

pub type TextureId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextStyleSpan {
    pub start_char: usize,
    pub end_char: usize,
    pub packed_rgb: Option<u32>,
    pub bold: bool,
    pub italic: bool,
}

#[derive(Debug, Clone)]
pub enum RenderCommand {
    Clear {
        color: [f32; 4],
    },
    DrawTexture {
        texture: TextureId,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        src_x: f32,
        src_y: f32,
        src_width: f32,
        src_height: f32,
        opacity: f32,
        /// Ignore sampled texture alpha. Native bitmap format 1 is XRGB; its
        /// fourth byte is not source coverage, while object transparency is
        /// still supplied separately through `opacity`.
        ignore_source_alpha: bool,
        /// Native CDspObj blend selector. Common recovered modes are routed
        /// to dedicated pipelines; unknown values retain ordinary alpha.
        blend_mode: i32,
        rotation_degrees: f32,
        /// Optional affine destination footprint in virtual/game coordinates,
        /// ordered TL, BL, BR, TR. Native mode 5 supplies this directly from
        /// the inverse of sub_416750 rather than rotating a raster bbox.
        destination_quad: Option<[[f32; 2]; 4]>,
        /// Texture interpolation selector. Native mode 5 recovers this from
        /// CDspObjSprite+0x280 (0 nearest, nonzero bilinear).
        linear_sampling: bool,
        clip: Option<[f32; 4]>,
        z: i32,
    },
    DrawText {
        text: String,
        x: f32,
        y: f32,
        color: [f32; 4],
        size: f32,
        styles: Vec<TextStyleSpan>,
        clip: Option<[f32; 4]>,
        z: i32,
    },
}

#[derive(Debug, Clone)]
pub struct TextureHandle {
    pub id: TextureId,
    pub width: u32,
    pub height: u32,
}

struct TextureRecord {
    texture: wgpu::Texture,
    linear_bind_group: wgpu::BindGroup,
    nearest_bind_group: wgpu::BindGroup,
    width: u32,
    height: u32,
    opaque: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct TextCacheKey {
    text: String,
    size_bits: u32,
    color_bits: [u32; 4],
    styles: Vec<TextStyleSpan>,
}

impl TextCacheKey {
    fn new(text: &str, size: f32, color: [f32; 4], styles: &[TextStyleSpan]) -> Self {
        Self {
            text: text.to_string(),
            size_bits: size.to_bits(),
            color_bits: color.map(f32::to_bits),
            styles: styles.to_vec(),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    texcoord: [f32; 2],
    opacity: f32,
    ignore_source_alpha: f32,
}

impl Vertex {
    fn layout<'a>() -> wgpu::VertexBufferLayout<'a> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 8,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 16,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32,
                },
                wgpu::VertexAttribute {
                    offset: 20,
                    shader_location: 3,
                    format: wgpu::VertexFormat::Float32,
                },
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativePipelineKind {
    Alpha,
    Replace,
    Additive,
    Subtractive,
    ConstantInterpolation,
}

fn native_pipeline_kind(blend_mode: i32, opacity: f32, texture_opaque: bool) -> NativePipelineKind {
    match classify_native_blend(blend_mode) {
        // CDspObj's default copy selector may bypass destination blending only
        // when the source is provably opaque.  RGBA input remains source-over.
        NativeBlendPath::DefaultCopy if opacity >= 0.999 && texture_opaque => {
            NativePipelineKind::Replace
        }
        NativeBlendPath::Additive => NativePipelineKind::Additive,
        NativeBlendPath::Subtractive => NativePipelineKind::Subtractive,
        NativeBlendPath::ConstantInterpolation => NativePipelineKind::ConstantInterpolation,
        NativeBlendPath::DefaultCopy | NativeBlendPath::Unrecovered(_) => NativePipelineKind::Alpha,
    }
}

pub struct Renderer<'w> {
    surface: wgpu::Surface<'w>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    clear_color: wgpu::Color,
    alpha_pipeline: wgpu::RenderPipeline,
    replace_pipeline: wgpu::RenderPipeline,
    additive_pipeline: wgpu::RenderPipeline,
    subtractive_pipeline: wgpu::RenderPipeline,
    constant_interpolation_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    linear_sampler: wgpu::Sampler,
    nearest_sampler: wgpu::Sampler,
    textures: BTreeMap<TextureId, TextureRecord>,
    text_cache: BTreeMap<TextCacheKey, TextureHandle>,
    next_texture_id: TextureId,
    commands: Vec<RenderCommand>,
    font: FontArc,
    virtual_width: f32,
    virtual_height: f32,
}

impl<'w> Renderer<'w> {
    pub async fn new(window: &'w Window) -> Result<Self> {
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window)
            .map_err(|err| EthornellError::Other(format!("create surface failed: {err}")))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .ok_or_else(|| EthornellError::Other("no compatible wgpu adapter".into()))?;
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("ethornell-device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await
            .map_err(|err| EthornellError::Other(format!("request device failed: {err}")))?;
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|format| format.is_srgb())
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: caps.present_modes[0],
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ethornell-texture-layout"),
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
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ethornell-2d-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ethornell-2d-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let create_pipeline = |label: &'static str, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: "vs_main",
                    buffers: &[Vertex::layout()],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: "fs_main",
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
            })
        };
        let alpha_pipeline = create_pipeline(
            "ethornell-2d-alpha-pipeline",
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let replace_pipeline = create_pipeline("ethornell-2d-replace-pipeline", None);
        let additive_pipeline = create_pipeline(
            "ethornell-2d-additive-pipeline",
            Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
        );
        let subtractive_pipeline = create_pipeline(
            "ethornell-2d-subtractive-pipeline",
            Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::ReverseSubtract,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
        );
        let constant_interpolation_pipeline = create_pipeline(
            "ethornell-2d-constant-interpolation-pipeline",
            Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Constant,
                    dst_factor: wgpu::BlendFactor::OneMinusConstant,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Constant,
                    dst_factor: wgpu::BlendFactor::OneMinusConstant,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
        );
        let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ethornell-linear-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let nearest_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ethornell-nearest-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        Ok(Self {
            surface,
            device,
            queue,
            config,
            clear_color: wgpu::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
            alpha_pipeline,
            replace_pipeline,
            additive_pipeline,
            subtractive_pipeline,
            constant_interpolation_pipeline,
            bind_group_layout,
            linear_sampler,
            nearest_sampler,
            textures: BTreeMap::new(),
            text_cache: BTreeMap::new(),
            next_texture_id: 1,
            commands: Vec::new(),
            font: load_system_cjk_font()?,
            virtual_width: 1280.0,
            virtual_height: 720.0,
        })
    }

    pub fn set_virtual_size(&mut self, width: f32, height: f32) {
        self.virtual_width = width.max(1.0);
        self.virtual_height = height.max(1.0);
    }

    pub fn surface_size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Target renderer offset +0x58 is used as a maximum pixel-capacity field
    /// when large bitmaps are split into strips. The closest wgpu equivalent
    /// is the square of max_texture_dimension_2d.
    pub fn graphics_memory_metric(&self) -> i32 {
        let side = u64::from(self.device.limits().max_texture_dimension_2d);
        side.saturating_mul(side).min(i32::MAX as u64) as i32
    }

    pub fn game_to_surface_rect(
        &self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> (f32, f32, f32, f32) {
        let (scale, offset_x, offset_y) = viewport_transform(
            self.config.width as f32,
            self.config.height as f32,
            self.virtual_width,
            self.virtual_height,
        );
        (
            offset_x + x * scale,
            offset_y + y * scale,
            width * scale,
            height * scale,
        )
    }

    pub fn surface_to_game_point(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        surface_to_virtual_point(
            x,
            y,
            self.config.width as f32,
            self.config.height as f32,
            self.virtual_width,
            self.virtual_height,
        )
    }

    pub fn insert_rgba(&mut self, image: &DecodedImage) -> Result<TextureHandle> {
        validate_rgba(image)?;
        let id = self.next_texture_id;
        self.next_texture_id += 1;
        let record = self.create_texture_record(image);
        self.textures.insert(id, record);
        Ok(TextureHandle {
            id,
            width: image.width,
            height: image.height,
        })
    }

    pub fn update_rgba(
        &mut self,
        handle: &TextureHandle,
        image: &DecodedImage,
    ) -> Result<TextureHandle> {
        validate_rgba(image)?;
        let same_dimensions = self
            .textures
            .get(&handle.id)
            .is_some_and(|record| record.width == image.width && record.height == image.height);
        if same_dimensions {
            let record = self
                .textures
                .get(&handle.id)
                .ok_or_else(|| EthornellError::Parse("unknown texture handle".into()))?;
            self.write_texture(&record.texture, image);
            if let Some(record) = self.textures.get_mut(&handle.id) {
                record.opaque = image.rgba.chunks_exact(4).all(|pixel| pixel[3] == 255);
            }
        } else {
            let record = self.create_texture_record(image);
            self.textures.insert(handle.id, record);
        }
        Ok(TextureHandle {
            id: handle.id,
            width: image.width,
            height: image.height,
        })
    }

    pub fn remove_texture(&mut self, texture: TextureId) {
        self.textures.remove(&texture);
        self.text_cache.retain(|_, handle| handle.id != texture);
    }

    fn create_texture_record(&self, image: &DecodedImage) -> TextureRecord {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ethornell-rgba-texture"),
            size: wgpu::Extent3d {
                width: image.width,
                height: image.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.write_texture(&texture, image);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let make_bind_group = |label: &'static str, sampler: &wgpu::Sampler| {
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            })
        };
        let linear_bind_group =
            make_bind_group("ethornell-texture-linear-bind-group", &self.linear_sampler);
        let nearest_bind_group = make_bind_group(
            "ethornell-texture-nearest-bind-group",
            &self.nearest_sampler,
        );
        TextureRecord {
            texture,
            linear_bind_group,
            nearest_bind_group,
            width: image.width,
            height: image.height,
            opaque: image.rgba.chunks_exact(4).all(|pixel| pixel[3] == 255),
        }
    }

    fn write_texture(&self, texture: &wgpu::Texture, image: &DecodedImage) {
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &image.rgba,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(image.width * 4),
                rows_per_image: Some(image.height),
            },
            wgpu::Extent3d {
                width: image.width,
                height: image.height,
                depth_or_array_layers: 1,
            },
        );
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn submit(&mut self, commands: &[RenderCommand]) {
        self.commands.clear();
        for command in commands {
            if let RenderCommand::Clear { color } = command {
                self.clear_color = wgpu::Color {
                    r: color[0] as f64,
                    g: color[1] as f64,
                    b: color[2] as f64,
                    a: color[3] as f64,
                };
            }
            self.commands.push(command.clone());
        }
    }

    pub fn render(&mut self) -> Result<()> {
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            Err(wgpu::SurfaceError::Timeout) => return Ok(()),
            Err(err) => return Err(EthornellError::Other(format!("surface error: {err}"))),
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut draw_items = Vec::new();
        let mut frame_vertices = Vec::<Vertex>::new();
        let mut active_text_keys = BTreeSet::new();
        let mut draw_commands: Vec<_> = self
            .commands
            .iter()
            .filter_map(|command| match command {
                RenderCommand::DrawTexture { z, .. } => Some((*z, command.clone())),
                RenderCommand::DrawText { z, .. } => Some((*z, command.clone())),
                RenderCommand::Clear { .. } => None,
            })
            .collect();
        draw_commands.sort_by_key(|(z, _)| *z);
        for (_, command) in draw_commands {
            let (
                texture,
                x,
                y,
                width,
                height,
                src_x,
                src_y,
                src_width,
                src_height,
                opacity,
                ignore_source_alpha,
                blend_mode,
                rotation_degrees,
                destination_quad,
                linear_sampling,
                clip,
            ) = match command {
                RenderCommand::DrawTexture {
                    texture,
                    x,
                    y,
                    width,
                    height,
                    src_x,
                    src_y,
                    src_width,
                    src_height,
                    opacity,
                    ignore_source_alpha,
                    blend_mode,
                    rotation_degrees,
                    destination_quad,
                    linear_sampling,
                    clip,
                    ..
                } => {
                    if !self.textures.contains_key(&texture) {
                        continue;
                    }
                    (
                        texture,
                        x,
                        y,
                        width,
                        height,
                        src_x,
                        src_y,
                        src_width,
                        src_height,
                        opacity,
                        ignore_source_alpha,
                        blend_mode,
                        rotation_degrees,
                        destination_quad,
                        linear_sampling,
                        clip,
                    )
                }
                RenderCommand::DrawText {
                    text,
                    x,
                    y,
                    color,
                    size,
                    styles,
                    clip,
                    ..
                } => {
                    let key = TextCacheKey::new(&text, size, color, &styles);
                    active_text_keys.insert(key.clone());
                    let handle = if let Some(handle) = self.text_cache.get(&key).cloned() {
                        handle
                    } else {
                        let image = rasterize_text(&self.font, &text, size, color, &styles);
                        let handle = self.insert_rgba(&image)?;
                        self.text_cache.insert(key, handle.clone());
                        handle
                    };
                    (
                        handle.id,
                        x,
                        y,
                        handle.width as f32,
                        handle.height as f32,
                        0.0,
                        0.0,
                        handle.width as f32,
                        handle.height as f32,
                        1.0,
                        false,
                        1,
                        0.0,
                        None,
                        true,
                        clip,
                    )
                }
                RenderCommand::Clear { .. } => continue,
            };
            let surface_destination_quad = destination_quad.map(|quad| {
                let (scale, offset_x, offset_y) = viewport_transform(
                    self.config.width as f32,
                    self.config.height as f32,
                    self.virtual_width,
                    self.virtual_height,
                );
                quad.map(|[game_x, game_y]| [offset_x + game_x * scale, offset_y + game_y * scale])
            });
            let (x, y, width, height) = self.game_to_surface_rect(x, y, width, height);
            let scissor = match clip {
                Some(rect) => match self.game_clip_to_scissor(rect) {
                    Some(scissor) => Some(scissor),
                    None => continue,
                },
                None => None,
            };
            let texture_opaque = self
                .textures
                .get(&texture)
                .map(|record| record.opaque)
                .unwrap_or(false);
            let pipeline_kind =
                native_pipeline_kind(blend_mode, opacity, texture_opaque || ignore_source_alpha);
            let shader_opacity = if pipeline_kind == NativePipelineKind::ConstantInterpolation {
                1.0
            } else {
                opacity
            };
            let vertices = if let Some(points) = surface_destination_quad {
                quad_vertices_from_points(
                    points,
                    src_x,
                    src_y,
                    src_width,
                    src_height,
                    shader_opacity,
                    ignore_source_alpha,
                    self.config.width as f32,
                    self.config.height as f32,
                )
            } else {
                quad_vertices(
                    x,
                    y,
                    width,
                    height,
                    src_x,
                    src_y,
                    src_width,
                    src_height,
                    shader_opacity,
                    ignore_source_alpha,
                    rotation_degrees,
                    self.config.width as f32,
                    self.config.height as f32,
                )
            };
            let vertex_start = frame_vertices.len() as u32;
            frame_vertices.extend_from_slice(&vertices);
            let vertex_range = vertex_start..vertex_start + vertices.len() as u32;
            draw_items.push((
                texture,
                vertex_range,
                scissor,
                pipeline_kind,
                opacity,
                linear_sampling,
            ));
        }

        let stale_text_keys = self
            .text_cache
            .keys()
            .filter(|key| !active_text_keys.contains(*key))
            .cloned()
            .collect::<Vec<_>>();
        for key in stale_text_keys {
            if let Some(handle) = self.text_cache.remove(&key) {
                self.textures.remove(&handle.id);
            }
        }

        // The target renderer preserves the submitted composition order, but it
        // does not require one host GPU allocation per sprite.  Keep all quads
        // in one frame-local vertex buffer so a scene with many display objects
        // does not create hundreds of tiny Metal/D3D/Vulkan buffers every frame.
        let frame_vertex_buffer = (!frame_vertices.is_empty()).then(|| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("ethornell-frame-vertices"),
                    contents: bytemuck::cast_slice(&frame_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                })
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ethornell-render"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ethornell-render-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.clear_color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if let Some(vertex_buffer) = frame_vertex_buffer.as_ref() {
                pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            }
            for (texture, vertex_range, scissor, pipeline_kind, opacity, linear_sampling) in
                &draw_items
            {
                let pipeline = match pipeline_kind {
                    NativePipelineKind::Replace => &self.replace_pipeline,
                    NativePipelineKind::Additive => &self.additive_pipeline,
                    NativePipelineKind::Subtractive => &self.subtractive_pipeline,
                    NativePipelineKind::Alpha => &self.alpha_pipeline,
                    NativePipelineKind::ConstantInterpolation => {
                        &self.constant_interpolation_pipeline
                    }
                };
                pass.set_pipeline(pipeline);
                if *pipeline_kind == NativePipelineKind::ConstantInterpolation {
                    let factor = f64::from(opacity.clamp(0.0, 1.0));
                    pass.set_blend_constant(wgpu::Color {
                        r: factor,
                        g: factor,
                        b: factor,
                        a: factor,
                    });
                }
                if let Some((x, y, width, height)) = scissor {
                    pass.set_scissor_rect(*x, *y, *width, *height);
                } else {
                    pass.set_scissor_rect(0, 0, self.config.width, self.config.height);
                }
                if let Some(record) = self.textures.get(texture) {
                    let bind_group = if *linear_sampling {
                        &record.linear_bind_group
                    } else {
                        &record.nearest_bind_group
                    };
                    pass.set_bind_group(0, bind_group, &[]);
                    pass.draw(vertex_range.clone(), 0..1);
                }
            }
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(())
    }

    fn game_clip_to_scissor(&self, clip: [f32; 4]) -> Option<(u32, u32, u32, u32)> {
        let (x, y, width, height) = self.game_to_surface_rect(clip[0], clip[1], clip[2], clip[3]);
        let x0 = x.floor().max(0.0).min(self.config.width as f32) as u32;
        let y0 = y.floor().max(0.0).min(self.config.height as f32) as u32;
        let x1 = (x + width).ceil().max(0.0).min(self.config.width as f32) as u32;
        let y1 = (y + height).ceil().max(0.0).min(self.config.height as f32) as u32;
        (x1 > x0 && y1 > y0).then_some((x0, y0, x1 - x0, y1 - y0))
    }
}

fn viewport_transform(
    surface_width: f32,
    surface_height: f32,
    virtual_width: f32,
    virtual_height: f32,
) -> (f32, f32, f32) {
    let scale = (surface_width / virtual_width)
        .min(surface_height / virtual_height)
        .max(0.01);
    let offset_x = (surface_width - virtual_width * scale) * 0.5;
    let offset_y = (surface_height - virtual_height * scale) * 0.5;
    (scale, offset_x, offset_y)
}

fn surface_to_virtual_point(
    x: f32,
    y: f32,
    surface_width: f32,
    surface_height: f32,
    virtual_width: f32,
    virtual_height: f32,
) -> Option<(f32, f32)> {
    let (scale, offset_x, offset_y) =
        viewport_transform(surface_width, surface_height, virtual_width, virtual_height);
    let virtual_x = (x - offset_x) / scale;
    let virtual_y = (y - offset_y) / scale;
    if (0.0..=virtual_width).contains(&virtual_x) && (0.0..=virtual_height).contains(&virtual_y) {
        Some((virtual_x, virtual_y))
    } else {
        None
    }
}

#[cfg(test)]
mod native_blend_tests {
    use super::{native_pipeline_kind, NativePipelineKind};

    #[test]
    fn opaque_copy_uses_replace_pipeline() {
        assert_eq!(
            native_pipeline_kind(128, 1.0, true),
            NativePipelineKind::Replace
        );
    }

    #[test]
    fn transparent_copy_keeps_alpha_composition() {
        assert_eq!(
            native_pipeline_kind(128, 1.0, false),
            NativePipelineKind::Alpha
        );
        assert_eq!(
            native_pipeline_kind(128, 0.5, true),
            NativePipelineKind::Alpha
        );
    }

    #[test]
    fn additive_and_subtractive_modes_have_dedicated_pipelines() {
        assert_eq!(
            native_pipeline_kind(2, 1.0, false),
            NativePipelineKind::Additive
        );
        assert_eq!(
            native_pipeline_kind(3, 1.0, false),
            NativePipelineKind::Subtractive
        );
    }

    #[test]
    fn backb_constant_interpolation_has_a_dedicated_pipeline() {
        assert_eq!(
            native_pipeline_kind(0xf0, 0.5, false),
            NativePipelineKind::ConstantInterpolation
        );
    }
}

#[cfg(test)]
mod viewport_tests {
    use super::{surface_to_virtual_point, viewport_transform};

    #[test]
    fn hdpi_surface_uses_the_physical_device_scale() {
        assert_eq!(
            viewport_transform(2560.0, 1440.0, 1280.0, 720.0),
            (2.0, 0.0, 0.0)
        );
    }

    #[test]
    fn letterbox_offset_is_shared_by_rendering_and_input() {
        assert_eq!(
            viewport_transform(1600.0, 1200.0, 1280.0, 720.0),
            (1.25, 0.0, 150.0)
        );
    }

    #[test]
    fn hdpi_input_round_trips_to_virtual_game_coordinates() {
        assert_eq!(
            surface_to_virtual_point(1086.0, 314.0, 2560.0, 1440.0, 1280.0, 720.0),
            Some((543.0, 157.0))
        );
    }

    #[test]
    fn letterbox_input_rejects_bars_and_maps_the_viewport() {
        assert_eq!(
            surface_to_virtual_point(800.0, 150.0, 1600.0, 1200.0, 1280.0, 720.0),
            Some((640.0, 0.0))
        );
        assert_eq!(
            surface_to_virtual_point(800.0, 149.0, 1600.0, 1200.0, 1280.0, 720.0),
            None
        );
    }
}

pub fn load_system_cjk_font() -> Result<FontArc> {
    for path in SYSTEM_CJK_FONT_CANDIDATES {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if let Ok(font) = FontArc::try_from_vec(bytes) {
            tracing::info!(path, "loaded text font");
            return Ok(font);
        }
    }
    Err(EthornellError::Other(
        "no usable system font found for text rendering".into(),
    ))
}

fn rasterize_text(
    font: &FontArc,
    text: &str,
    px_height: f32,
    color: [f32; 4],
    styles: &[TextStyleSpan],
) -> DecodedImage {
    let lines: Vec<&str> = text.split('\n').collect();
    let scale = PxScale::from(px_height.max(1.0));
    let scaled = font.as_scaled(scale);
    let line_height = (scaled.ascent() - scaled.descent() + scaled.line_gap())
        .ceil()
        .max(px_height.ceil()) as u32;
    let widths: Vec<u32> = lines
        .iter()
        .map(|line| measure_line(font, *line, px_height))
        .collect();
    let style_overhang = styles
        .iter()
        .any(|style| style.bold || style.italic)
        .then_some((px_height * 0.25).ceil() as u32 + 1)
        .unwrap_or_default();
    let width = widths
        .iter()
        .copied()
        .max()
        .unwrap_or(1)
        .saturating_add(style_overhang)
        .max(1);
    let height = (line_height * lines.len().max(1) as u32).max(1);
    let mut rgba = vec![0u8; width as usize * height as usize * 4];

    let mut char_index = 0usize;
    for (line_index, line) in lines.iter().enumerate() {
        let baseline_y = line_index as f32 * line_height as f32 + scaled.ascent();
        let mut cursor_x = 0.0f32;
        for ch in line.chars() {
            let style = text_style_at(styles, char_index);
            let glyph_color = style
                .and_then(|style| style.packed_rgb)
                .map(|packed| {
                    [
                        ((packed >> 16) & 0xff) as f32 / 255.0,
                        ((packed >> 8) & 0xff) as f32 / 255.0,
                        (packed & 0xff) as f32 / 255.0,
                        color[3],
                    ]
                })
                .unwrap_or(color);
            let rgba_color = [
                (glyph_color[0].clamp(0.0, 1.0) * 255.0) as u8,
                (glyph_color[1].clamp(0.0, 1.0) * 255.0) as u8,
                (glyph_color[2].clamp(0.0, 1.0) * 255.0) as u8,
                (glyph_color[3].clamp(0.0, 1.0) * 255.0) as u8,
            ];
            let glyph_id = font.glyph_id(ch);
            let glyph =
                glyph_id.with_scale_and_position(scale, ab_glyph::point(cursor_x, baseline_y));
            cursor_x += scaled.h_advance(glyph_id);
            if let Some(outlined) = font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                outlined.draw(|gx, gy, coverage| {
                    let italic_dx = style
                        .filter(|style| style.italic)
                        .map(|_| ((bounds.height() - gy as f32).max(0.0) * 0.20).round() as i32)
                        .unwrap_or_default();
                    let px = bounds.min.x as i32 + gx as i32 + italic_dx;
                    let py = bounds.min.y as i32 + gy as i32;
                    let bold_width = usize::from(style.is_some_and(|style| style.bold));
                    for bold_dx in 0..=bold_width {
                        let px = px + bold_dx as i32;
                        if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                            continue;
                        }
                        let idx = (py as usize * width as usize + px as usize) * 4;
                        let alpha = (coverage * rgba_color[3] as f32).round() as u8;
                        if alpha >= rgba[idx + 3] {
                            rgba[idx] = rgba_color[0];
                            rgba[idx + 1] = rgba_color[1];
                            rgba[idx + 2] = rgba_color[2];
                        }
                        rgba[idx + 3] = rgba[idx + 3].max(alpha);
                    }
                });
            }
            char_index += 1;
        }
        char_index += usize::from(line_index + 1 < lines.len());
    }

    DecodedImage {
        width,
        height,
        rgba,
    }
}

fn text_style_at(styles: &[TextStyleSpan], char_index: usize) -> Option<TextStyleSpan> {
    styles
        .iter()
        .copied()
        .find(|style| style.start_char <= char_index && char_index < style.end_char)
}

fn measure_line(font: &FontArc, text: &str, px_height: f32) -> u32 {
    let scale = PxScale::from(px_height.max(1.0));
    let scaled = font.as_scaled(scale);
    text.chars()
        .map(|c| scaled.h_advance(font.glyph_id(c)))
        .sum::<f32>()
        .ceil()
        .max(1.0) as u32
}

const SYSTEM_CJK_FONT_CANDIDATES: &[&str] = &[
    "/System/Library/Fonts/ヒラギノ角ゴシック W8.ttc",
    "/System/Library/Fonts/AppleSDGothicNeo.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/STHeiti Medium.ttc",
    "/System/Library/Fonts/Supplemental/AppleGothic.ttf",
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    "/Library/Fonts/Arial Unicode.ttf",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "C:/Windows/Fonts/msgothic.ttc",
    "C:/Windows/Fonts/msyh.ttc",
];

#[cfg(test)]
mod font_tests {
    use super::*;

    #[test]
    fn selected_system_font_contains_japanese_glyphs() {
        let font = load_system_cjk_font().expect("system CJK font");
        assert_ne!(font.glyph_id('\u{65e5}').0, 0);
    }

    #[test]
    fn styled_text_rasterizer_applies_per_character_color_and_shape() {
        let font = load_system_cjk_font().expect("system CJK font");
        let image = rasterize_text(
            &font,
            "AB",
            32.0,
            [1.0; 4],
            &[TextStyleSpan {
                start_char: 1,
                end_char: 2,
                packed_rgb: Some(0xff0000),
                bold: true,
                italic: true,
            }],
        );
        assert!(image
            .rgba
            .chunks_exact(4)
            .any(|pixel| pixel[0] > 200 && pixel[1] > 200 && pixel[2] > 200 && pixel[3] > 0));
        assert!(image
            .rgba
            .chunks_exact(4)
            .any(|pixel| pixel[0] > 200 && pixel[1] < 20 && pixel[2] < 20 && pixel[3] > 0));
    }
}

fn validate_rgba(image: &DecodedImage) -> Result<()> {
    if image.width == 0 || image.height == 0 {
        return Err(EthornellError::Parse("zero-sized texture".into()));
    }
    if image.rgba.len() != image.width as usize * image.height as usize * 4 {
        return Err(EthornellError::Parse("texture RGBA size mismatch".into()));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn quad_vertices(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    src_x: f32,
    src_y: f32,
    src_w: f32,
    src_h: f32,
    opacity: f32,
    ignore_source_alpha: bool,
    rotation_degrees: f32,
    sw: f32,
    sh: f32,
) -> [Vertex; 6] {
    let center_x = x + w * 0.5;
    let center_y = y + h * 0.5;
    let radians = rotation_degrees.to_radians();
    let sin = radians.sin();
    let cos = radians.cos();
    let rotate = |px: f32, py: f32| {
        let dx = px - center_x;
        let dy = py - center_y;
        [
            center_x + dx * cos - dy * sin,
            center_y + dx * sin + dy * cos,
        ]
    };
    quad_vertices_from_points(
        [
            rotate(x, y),
            rotate(x, y + h),
            rotate(x + w, y + h),
            rotate(x + w, y),
        ],
        src_x,
        src_y,
        src_w,
        src_h,
        opacity,
        ignore_source_alpha,
        sw,
        sh,
    )
}

fn quad_vertices_from_points(
    points: [[f32; 2]; 4],
    src_x: f32,
    src_y: f32,
    src_w: f32,
    src_h: f32,
    opacity: f32,
    ignore_source_alpha: bool,
    sw: f32,
    sh: f32,
) -> [Vertex; 6] {
    let to_ndc = |[px, py]: [f32; 2]| [px / sw * 2.0 - 1.0, 1.0 - py / sh * 2.0];
    let [top_left, bottom_left, bottom_right, top_right] = points.map(to_ndc);
    let u0 = src_x.clamp(0.0, 1.0);
    let v0 = src_y.clamp(0.0, 1.0);
    let u1 = (src_x + src_w).clamp(0.0, 1.0);
    let v1 = (src_y + src_h).clamp(0.0, 1.0);
    let ignore_source_alpha = if ignore_source_alpha { 1.0 } else { 0.0 };
    [
        Vertex {
            position: top_left,
            texcoord: [u0, v0],
            opacity,
            ignore_source_alpha,
        },
        Vertex {
            position: bottom_left,
            texcoord: [u0, v1],
            opacity,
            ignore_source_alpha,
        },
        Vertex {
            position: bottom_right,
            texcoord: [u1, v1],
            opacity,
            ignore_source_alpha,
        },
        Vertex {
            position: top_left,
            texcoord: [u0, v0],
            opacity,
            ignore_source_alpha,
        },
        Vertex {
            position: bottom_right,
            texcoord: [u1, v1],
            opacity,
            ignore_source_alpha,
        },
        Vertex {
            position: top_right,
            texcoord: [u1, v0],
            opacity,
            ignore_source_alpha,
        },
    ]
}

const SHADER: &str = r#"
struct VertexIn {
  @location(0) position: vec2<f32>,
  @location(1) texcoord: vec2<f32>,
  @location(2) opacity: f32,
  @location(3) ignore_source_alpha: f32,
};

struct VertexOut {
  @builtin(position) position: vec4<f32>,
  @location(0) texcoord: vec2<f32>,
  @location(1) opacity: f32,
  @location(2) ignore_source_alpha: f32,
};

@vertex
fn vs_main(v: VertexIn) -> VertexOut {
  var out: VertexOut;
  out.position = vec4<f32>(v.position, 0.0, 1.0);
  out.texcoord = v.texcoord;
  out.opacity = v.opacity;
  out.ignore_source_alpha = v.ignore_source_alpha;
  return out;
}

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@fragment
fn fs_main(v: VertexOut) -> @location(0) vec4<f32> {
  let c = textureSample(tex, samp, v.texcoord);
  let source_alpha = select(c.a, 1.0, v.ignore_source_alpha > 0.5);
  return vec4<f32>(c.rgb, source_alpha * v.opacity);
}
"#;

use wgpu::util::DeviceExt;
