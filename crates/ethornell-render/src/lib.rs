use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use ethornell_core::{EthornellError, Result};
use ethornell_image::DecodedImage;
use std::collections::BTreeMap;
use winit::window::Window;

pub type TextureId = u64;

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
        z: i32,
    },
    DrawText {
        text: String,
        x: f32,
        y: f32,
        color: [f32; 4],
        size: f32,
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
    bind_group: wgpu::BindGroup,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    texcoord: [f32; 2],
    opacity: f32,
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
            ],
        }
    }
}

pub struct Renderer<'w> {
    surface: wgpu::Surface<'w>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    clear_color: wgpu::Color,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: BTreeMap<TextureId, TextureRecord>,
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
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ethornell-2d-pipeline"),
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
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ethornell-linear-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
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
            pipeline,
            bind_group_layout,
            sampler,
            textures: BTreeMap::new(),
            next_texture_id: 1,
            commands: Vec::new(),
            font: load_default_font()?,
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

    pub fn game_to_surface_rect(
        &self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> (f32, f32, f32, f32) {
        let scale = (self.config.width as f32 / self.virtual_width)
            .min(self.config.height as f32 / self.virtual_height)
            .max(0.01);
        let offset_x = (self.config.width as f32 - self.virtual_width * scale) * 0.5;
        let offset_y = (self.config.height as f32 - self.virtual_height * scale) * 0.5;
        (
            offset_x + x * scale,
            offset_y + y * scale,
            width * scale,
            height * scale,
        )
    }

    pub fn surface_to_game_point(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let scale = (self.config.width as f32 / self.virtual_width)
            .min(self.config.height as f32 / self.virtual_height)
            .max(0.01);
        let offset_x = (self.config.width as f32 - self.virtual_width * scale) * 0.5;
        let offset_y = (self.config.height as f32 - self.virtual_height * scale) * 0.5;
        let game_x = (x - offset_x) / scale;
        let game_y = (y - offset_y) / scale;
        if (0.0..=self.virtual_width).contains(&game_x)
            && (0.0..=self.virtual_height).contains(&game_y)
        {
            Some((game_x, game_y))
        } else {
            None
        }
    }

    pub fn insert_rgba(&mut self, image: &DecodedImage) -> Result<TextureHandle> {
        if image.rgba.len() != image.width as usize * image.height as usize * 4 {
            return Err(EthornellError::Parse("texture RGBA size mismatch".into()));
        }
        let id = self.next_texture_id;
        self.next_texture_id += 1;
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
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
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
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ethornell-texture-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let handle = TextureHandle {
            id,
            width: image.width,
            height: image.height,
        };
        self.textures.insert(id, TextureRecord { bind_group });
        Ok(handle)
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
            let (texture, x, y, width, height, src_x, src_y, src_width, src_height, opacity) =
                match command {
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
                        ..
                    } => {
                        if !self.textures.contains_key(&texture) {
                            continue;
                        }
                        (
                            texture, x, y, width, height, src_x, src_y, src_width, src_height,
                            opacity,
                        )
                    }
                    RenderCommand::DrawText {
                        text,
                        x,
                        y,
                        color,
                        size,
                        ..
                    } => {
                        let image = rasterize_text(&self.font, &text, size, color);
                        let handle = self.insert_rgba(&image)?;
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
                            color[3],
                        )
                    }
                    RenderCommand::Clear { .. } => continue,
                };
            let (x, y, width, height) = self.game_to_surface_rect(x, y, width, height);
            let vertices = quad_vertices(
                x,
                y,
                width,
                height,
                src_x,
                src_y,
                src_width,
                src_height,
                opacity,
                self.config.width as f32,
                self.config.height as f32,
            );
            let vertex_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("ethornell-quad-vertices"),
                    contents: bytemuck::cast_slice(&vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            draw_items.push((texture, vertex_buffer));
        }
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
            pass.set_pipeline(&self.pipeline);
            for (texture, vertex_buffer) in &draw_items {
                if let Some(record) = self.textures.get(texture) {
                    pass.set_bind_group(0, &record.bind_group, &[]);
                    pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                    pass.draw(0..6, 0..1);
                }
            }
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(())
    }
}

fn load_default_font() -> Result<FontArc> {
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

fn rasterize_text(font: &FontArc, text: &str, px_height: f32, color: [f32; 4]) -> DecodedImage {
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
    let width = widths.iter().copied().max().unwrap_or(1).max(1);
    let height = (line_height * lines.len().max(1) as u32).max(1);
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    let rgba_color = [
        (color[0].clamp(0.0, 1.0) * 255.0) as u8,
        (color[1].clamp(0.0, 1.0) * 255.0) as u8,
        (color[2].clamp(0.0, 1.0) * 255.0) as u8,
        (color[3].clamp(0.0, 1.0) * 255.0) as u8,
    ];

    for (line_index, line) in lines.iter().enumerate() {
        let baseline_y = line_index as f32 * line_height as f32 + scaled.ascent();
        let mut cursor_x = 0.0f32;
        for ch in line.chars() {
            let glyph_id = font.glyph_id(ch);
            let glyph =
                glyph_id.with_scale_and_position(scale, ab_glyph::point(cursor_x, baseline_y));
            cursor_x += scaled.h_advance(glyph_id);
            if let Some(outlined) = font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                outlined.draw(|gx, gy, coverage| {
                    let px = bounds.min.x as i32 + gx as i32;
                    let py = bounds.min.y as i32 + gy as i32;
                    if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                        return;
                    }
                    let idx = (py as usize * width as usize + px as usize) * 4;
                    let alpha = (coverage * rgba_color[3] as f32) as u8;
                    let inv = 255u16.saturating_sub(alpha as u16);
                    rgba[idx] = ((rgba_color[0] as u16 * alpha as u16 + rgba[idx] as u16 * inv)
                        / 255) as u8;
                    rgba[idx + 1] = ((rgba_color[1] as u16 * alpha as u16
                        + rgba[idx + 1] as u16 * inv)
                        / 255) as u8;
                    rgba[idx + 2] = ((rgba_color[2] as u16 * alpha as u16
                        + rgba[idx + 2] as u16 * inv)
                        / 255) as u8;
                    rgba[idx + 3] = rgba[idx + 3].saturating_add(alpha);
                });
            }
        }
    }

    DecodedImage {
        width,
        height,
        rgba,
    }
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
    "C:/Windows/Fonts/msgothic.ttc",
    "C:/Windows/Fonts/msyh.ttc",
];

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
    sw: f32,
    sh: f32,
) -> [Vertex; 6] {
    let left = x / sw * 2.0 - 1.0;
    let right = (x + w) / sw * 2.0 - 1.0;
    let top = 1.0 - y / sh * 2.0;
    let bottom = 1.0 - (y + h) / sh * 2.0;
    let u0 = src_x.clamp(0.0, 1.0);
    let v0 = src_y.clamp(0.0, 1.0);
    let u1 = (src_x + src_w).clamp(0.0, 1.0);
    let v1 = (src_y + src_h).clamp(0.0, 1.0);
    [
        Vertex {
            position: [left, top],
            texcoord: [u0, v0],
            opacity,
        },
        Vertex {
            position: [left, bottom],
            texcoord: [u0, v1],
            opacity,
        },
        Vertex {
            position: [right, bottom],
            texcoord: [u1, v1],
            opacity,
        },
        Vertex {
            position: [left, top],
            texcoord: [u0, v0],
            opacity,
        },
        Vertex {
            position: [right, bottom],
            texcoord: [u1, v1],
            opacity,
        },
        Vertex {
            position: [right, top],
            texcoord: [u1, v0],
            opacity,
        },
    ]
}

const SHADER: &str = r#"
struct VertexIn {
  @location(0) position: vec2<f32>,
  @location(1) texcoord: vec2<f32>,
  @location(2) opacity: f32,
};

struct VertexOut {
  @builtin(position) position: vec4<f32>,
  @location(0) texcoord: vec2<f32>,
  @location(1) opacity: f32,
};

@vertex
fn vs_main(v: VertexIn) -> VertexOut {
  var out: VertexOut;
  out.position = vec4<f32>(v.position, 0.0, 1.0);
  out.texcoord = v.texcoord;
  out.opacity = v.opacity;
  return out;
}

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@fragment
fn fs_main(v: VertexOut) -> @location(0) vec4<f32> {
  let c = textureSample(tex, samp, v.texcoord);
  return vec4<f32>(c.rgb, c.a * v.opacity);
}
"#;

use wgpu::util::DeviceExt;
