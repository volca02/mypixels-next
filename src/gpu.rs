//! GPU state: wgpu device/surface, rect pipeline (backgrounds, cursor,
//! tab strip), glyphon text rendering.

use std::sync::Arc;
use std::time::Instant;

use glyphon::{Buffer, Cache, ColorMode, FontSystem, Metrics, Resolution, SwashCache, TextAtlas, TextArea, TextRenderer, Viewport};
use winit::window::Window;

use crate::MAX_RUNS;
use crate::{damage::Damage, TerminalStyle};

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RectVertex {
    pub pos: [f32; 2],
    pub color: [f32; 4],
    /// Offset from this rect's top-left in pixels. The fragment shader needs a
    /// local coordinate to evaluate a corner radius.
    pub local: [f32; 2],
    /// (width, height, corner radius, unused) in pixels.
    pub size_radius: [f32; 4],
}

/// One axis-aligned rect as six vertices -- two triangles, since the pipeline
/// is TriangleList. Coordinates are pixels, converted to clip space against a
/// `wf` x `hf` window. `radius` rounds the corners with an antialiased distance
/// field; 0.0 gives a plain rect.
///
/// Vertex order is (TL, TR, BR) + (TL, BR, BL). An earlier version emitted
/// (TL, TR, BL, BR, BL, BR), whose second triangle is degenerate, so only half
/// of every rect filled -- invisible on small swatches, obvious on big ones.
pub fn rect_vertices(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 4],
    radius: f32,
    wf: f32,
    hf: f32,
) -> [RectVertex; 6] {
    let clip = |px: f32, py: f32| -> [f32; 2] {
        [(px / wf) * 2.0 - 1.0, 1.0 - (py / hf) * 2.0]
    };
    let sr = [w, h, radius.min(w.min(h) * 0.5), 0.0];
    let v = |px: f32, py: f32| RectVertex {
        pos: clip(px, py),
        color,
        local: [px - x, py - y],
        size_radius: sr,
    };
    let (x1, y1) = (x + w, y + h);
    [
        v(x, y),
        v(x1, y),
        v(x1, y1),
        v(x, y),
        v(x1, y1),
        v(x, y1),
    ]
}

const RECT_VS_WGSL: &str = r#"
struct VSOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local: vec2<f32>,
    @location(2) size_radius: vec4<f32>,
};
@vertex
fn vs(@location(0) pos: vec2<f32>,
      @location(1) color: vec4<f32>,
      @location(2) local: vec2<f32>,
      @location(3) size_radius: vec4<f32>) -> VSOut {
    return VSOut(vec4<f32>(pos, 0.0, 1.0), color, local, size_radius);
}
"#;

const RECT_FS_WGSL: &str = r#"
@fragment
fn fs(@location(0) color: vec4<f32>,
      @location(1) local: vec2<f32>,
      @location(2) size_radius: vec4<f32>) -> @location(0) vec4<f32> {
    let r = size_radius.z;
    if (r <= 0.0) {
        return color;
    }
    // Signed distance to a rounded box; negative is inside.
    let half = size_radius.xy * 0.5;
    let q = abs(local - half) - half + vec2<f32>(r);
    let d = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
    return vec4<f32>(color.rgb, color.a * smoothstep(0.5, -0.5, d));
}
"#;

/// Stage `rects` into `buffer` (growing it if needed) and draw them through
/// `pipeline`. Each call site owns its buffer: the base and the switcher
/// phases must never share one, because both writes land at offset 0 and the
/// draws only execute at the frame's single submit -- a shared buffer would
/// make both draws read the last write.
fn draw_rects(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pass: &mut wgpu::RenderPass<'_>,
    buffer: &mut wgpu::Buffer,
    pipeline: &wgpu::RenderPipeline,
    rects: &[RectVertex],
) {
    if rects.is_empty() {
        return;
    }
    // Stride must come from the type, not a literal: `RectVertex` grew
    // when rounded corners landed, and a stale 24 here let 1024 vertices
    // be written into a buffer holding 512.
    let need = rects.len() * std::mem::size_of::<RectVertex>();
    if need > buffer.size() as usize {
        // 2x headroom, rounded up, so a busy overview does not
        // reallocate on every frame.
        *buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rects"),
            size: ((need * 2) as u64).next_multiple_of(4096),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
    }
    queue.write_buffer(buffer, 0, bytemuck::cast_slice(rects));
    pass.set_pipeline(pipeline);
    pass.set_vertex_buffer(0, buffer.slice(..));
    pass.draw(0..rects.len() as u32, 0..1);
}

/// One offscreen terminal render, sampled into an overview tile.
struct PreviewSlot {
    _tex: wgpu::Texture,
    view: wgpu::TextureView,
    bind: wgpu::BindGroup,
}

/// Decorations are application state, outside Alacritty's damage tracker.
#[derive(Default)]
pub struct TerminalOverlays {
    pub selection: Vec<(usize, usize, usize)>,
    pub cursor: Option<crate::cursor::Cursor>,
    pub underline: Option<(f32, f32, f32)>,
    pub notice: Option<String>,
    pub images: Vec<ImageQuad>,
}

/// An offscreen frame to render this frame: which tab's target to draw into,
/// the rects and terminal runs for it.
pub struct PreviewJob {
    pub idx: usize,
    pub scissors: Vec<[u32; 4]>,
    pub images: Vec<ImageQuad>,
    pub bounds: glyphon::TextBounds,
    pub text: Vec<PreviewText>,
    pub rects: Vec<RectVertex>,
}

/// Placement and resolved color of a run in an offscreen terminal frame.
pub struct PreviewText {
    pub left: f32,
    pub top: f32,
    pub color: [f32; 3],
}

/// Both the window and offscreen frames use the same glyph placement and color.
pub fn terminal_text_area<'a>(
    buffer: &'a Buffer,
    left: f32,
    top: f32,
    color: [f32; 3],
    bounds: glyphon::TextBounds,
) -> TextArea<'a> {
    TextArea {
        buffer,
        left: left.round(),
        top: top.round(),
        scale: 1.0,
        bounds,
        default_color: glyphon::Color::rgb(
            (color[0] * 255.0) as u8,
            (color[1] * 255.0) as u8,
            (color[2] * 255.0) as u8,
        ),
        custom_glyphs: &[],
    }
}

/// A tile that samples a real rendered terminal frame. `half` is the box filter
/// half-width in UV: one destination pixel covers `1/tile_w` of the texture, so
/// a pixel averages exactly the source area it stands for.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PrevVertex {
    pos: [f32; 2],
    uv: [f32; 2],
    half: [f32; 2],
    /// Thumbnail backdrop; alpha zero selects an exact live-frame copy.
    backdrop: [f32; 4],
}

const PREV_VS_WGSL: &str = r#"
struct VSOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) half: vec2<f32>,
    @location(2) backdrop: vec4<f32>,
};
@vertex
fn vs(@location(0) pos: vec2<f32>,
      @location(1) uv: vec2<f32>,
      @location(2) half: vec2<f32>,
      @location(3) backdrop: vec4<f32>) -> VSOut {
    return VSOut(vec4<f32>(pos, 0.0, 1.0), uv, half, backdrop);
}
"#;

const PREV_FS_WGSL: &str = r#"
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
struct VSIn {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) half: vec2<f32>,
    @location(2) backdrop: vec4<f32>,
};
// 4x4 box filter. Bilinear alone samples one texel in eight at this ratio and
// aliases worse than the colour blocks it replaces, so every destination pixel
// averages the whole source region it covers.
@fragment
fn fs(i: VSIn) -> @location(0) vec4<f32> {
    if (all(i.half == vec2<f32>(0.0))) {
        return textureSampleLevel(tex, samp, i.uv, 0.0);
    }
    var acc = vec4<f32>(0.0);
    for (var y: i32 = 0; y < 4; y = y + 1) {
        for (var x: i32 = 0; x < 4; x = x + 1) {
            let f = (vec2<f32>(f32(x), f32(y)) + vec2<f32>(0.5)) * vec2<f32>(0.25, 0.25) - vec2<f32>(0.5);
            acc = acc + textureSampleLevel(tex, samp, i.uv + f * i.half * vec2<f32>(2.0, 2.0), 0.0);
        }
    }
    let color = acc / 16.0;
    // Tab textures retain their window alpha; thumbnails are opaque pictures.
    return vec4<f32>(color.rgb + (1.0 - color.a) * i.backdrop.rgb, 1.0);
}
"#;

/// One Kitty image to paint this frame: a destination rect in window pixels,
/// the UV sub-rect it shows (the visible part after viewport clipping), and
/// the texture cache key (tab, image id, generation). The caller builds
/// these from its graphics state; nothing below `Gpu` knows the protocol.
#[derive(Clone, PartialEq)]
pub struct ImageQuad {
    pub key: (u32, u32, u32),
    pub dst: [f32; 4],
    pub uv: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ImgVertex {
    pos: [f32; 2],
    uv: [f32; 2],
}

const IMG_VS_WGSL: &str = r#"
struct VSOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex
fn vs(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>) -> VSOut {
    return VSOut(vec4<f32>(pos, 0.0, 1.0), uv);
}
"#;

const IMG_FS_WGSL: &str = r#"
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
struct VSIn {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
// Plain bilinear at LOD 0 over the Rgba8Unorm upload - the surface is
// non-sRGB and display-referred, so texel bytes reach the screen unchanged,
// exactly like the rect pipeline's colors.
@fragment
fn fs(i: VSIn) -> @location(0) vec4<f32> {
    return textureSampleLevel(tex, samp, i.uv, 0.0);
}
"#;

/// An uploaded image texture and its bind group, keyed by (tab, id, gen).
struct ImgSlot {
    _tex: wgpu::Texture,
    _view: wgpu::TextureView,
    bind: wgpu::BindGroup,
}

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
    /// UI generation last presented; terminal changes use damage instead.
    present_generation: Option<u64>,
    present_terminal: Option<(usize, u64)>,
    clear_pipeline: wgpu::RenderPipeline,
    clear_buffer: wgpu::Buffer,
    composite_quad: wgpu::Buffer,
    /// True when the compositor will blend our alpha with whatever is behind the
    /// window, which is the only situation where a translucent terminal
    /// background is possible at all.
    pub alpha_ok: bool,
    pub rect_pipeline: wgpu::RenderPipeline,
    pub rect_buffer: wgpu::Buffer,
    pub viewport: Viewport,
    pub atlas: TextAtlas,
    pub swash_cache: SwashCache,
    pub text_renderer: TextRenderer,
    pub rects: Vec<RectVertex>,
    /// Pooled cosmic_text buffers for terminal runs (stable addresses).
    pub pool: Vec<Buffer>,
    /// Single buffer for terminal notices (terminal font metrics).
    pub prompt_buf: Buffer,
    // --- Retained terminal targets, shared by live display and previews ----
    /// Scratch buffers for background damage batches. The active terminal
    /// uses the window pool; both rasterize into these same per-tab targets.
    pub prev_bufs: Vec<Vec<Buffer>>,
    /// Damage queues survive throttled background preparation.
    pub prev_damage: Vec<Damage>,
    pub prev_revision: Vec<u64>,
    pub prev_styles: Vec<Option<TerminalStyle>>,
    pub overlays: Vec<TerminalOverlays>,
    pub prev_last: Vec<Instant>,
    tab_ids: Vec<usize>,
    next_revision: u64,
    prev_sampler: wgpu::Sampler,
    prev_bgl: wgpu::BindGroupLayout,
    prev_pipeline: wgpu::RenderPipeline,
    previews: Vec<Option<PreviewSlot>>,
    /// Reused box-filter quad for GPU-resident thumbnail updates.
    prev_stage_quad: Option<wgpu::Buffer>,
    prev_size: (u32, u32),
    // --- Kitty images ------------------------------------------------------
    img_pipeline: wgpu::RenderPipeline,
    img_buffer: wgpu::Buffer,
    img_cache: std::collections::HashMap<(u32, u32, u32), ImgSlot>,

}

pub(crate) fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn raw_waker() -> RawWaker {
        fn noop(_: *const ()) {}
        fn clone(_: *const ()) -> RawWaker {
            raw_waker()
        }
        RawWaker::new(std::ptr::null(), &RawWakerVTable::new(clone, noop, noop, noop))
    }
    let waker = unsafe { Waker::from_raw(raw_waker()) };
    let mut cx = Context::from_waker(&waker);
    let mut fut = Box::pin(fut);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

impl Gpu {
    pub fn init(
        event_loop: &winit::event_loop::ActiveEventLoop,
        window: &Arc<Window>,
        font_system: &mut FontSystem,
        font_px: f32,
        font_lh: f32,
    ) -> Gpu {
        let instance = wgpu::Instance::new(
            wgpu::InstanceDescriptor::new_with_display_handle(Box::new(
                event_loop.owned_display_handle(),
            )),
        );
        let surface = instance.create_surface(window.clone()).expect("surface");
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            ..Default::default()
        }))
        .expect("no adapter");
        log::info!("adapter: {}", adapter.get_info().name);
        // Pick a present mode the surface actually supports. Prefer Mailbox
        // (low latency), but some software/nested surfaces only offer Fifo.
        let caps = surface.get_capabilities(&adapter);
        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else if let Some(&first) = caps.present_modes.first() {
            first
        } else {
            wgpu::PresentMode::Fifo
        };
        let (device, queue) =
            block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("no device");

        // Non-sRGB format: our colors are display-referred (sRGB) and are
        // written as-is. An sRGB target would re-encode them (double
        // conversion, washing colors out).
        let format = wgpu::TextureFormat::Bgra8Unorm;
        let (width, height) =
            (window.inner_size().width.max(1), window.inner_size().height.max(1));
        // A translucent background needs the compositor to blend our alpha with
        // what is behind the window, so ask for premultiplied compositing when
        // the surface offers it. Nothing else has to change to produce it: both
        // the rect and the glyph pipelines blend with SRC_ALPHA/ONE_MINUS_SRC_ALPHA,
        // which writes a correctly premultiplied framebuffer, and the pass has
        // always cleared to TRANSPARENT -- the alpha channel was simply ignored
        // while the surface was Opaque.
        // Whether the compositor will honour our alpha at all. Do NOT gate this
        // on PreMultiplied alone: on Wayland the Vulkan swapchain typically
        // advertises only Opaque|Inherit, because INHERIT means the compositor
        // uses the buffer's own format semantics -- and an ARGB buffer's alpha is
        // honoured, premultiplied as wl_shm requires. Gating on PreMultiplied
        // silently disabled transparency on a compositor that supports it.
        let modes = &caps.alpha_modes;
        let want = [
            wgpu::CompositeAlphaMode::PreMultiplied,
            wgpu::CompositeAlphaMode::Inherit,
        ];
        let alpha_ok = modes.iter().any(|m| want.contains(m));
        let alpha_mode = if modes.contains(&want[0]) {
            want[0]
        } else if modes.contains(&want[1]) {
            want[1]
        } else {
            wgpu::CompositeAlphaMode::Auto
        };
        log::info!(
            "alpha: surface offers {:?}, using {:?} (translucent background: {})",
            modes,
            alpha_mode,
            alpha_ok
        );
        if !alpha_ok {
            log::warn!(
                "surface offers no usable alpha ({:?}); terminal background stays opaque",
                modes
            );
        }
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);

        // Overview previews: a real offscreen terminal frame, sampled into a tile.
        let prev_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("preview"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let prev_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("preview"),
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
        let prev_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("preview"),
            bind_group_layouts: &[Some(&prev_bgl)],
            immediate_size: 0,
        });
        let prev_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("preview"),
            layout: Some(&prev_layout),
            vertex: wgpu::VertexState {
                module: &device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("preview.vs"),
                    source: wgpu::ShaderSource::Wgsl(PREV_VS_WGSL.into()),
                }),
                compilation_options: Default::default(),
                entry_point: Some("vs"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<PrevVertex>() as u64,
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
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 24,
                            shader_location: 3,
                            format: wgpu::VertexFormat::Float32x4,
                        },
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("preview.fs"),
                    source: wgpu::ShaderSource::Wgsl(PREV_FS_WGSL.into()),
                }),
                compilation_options: Default::default(),
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // Tiles are opaque by construction, so no blending: a
                    // translucent thumbnail is just mud over the backdrop.
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let cache = Cache::new(&device);
        // ColorMode::Web passes the text color through as-is. Accurate would
        // apply an sRGB->linear conversion in the shader, which would mismatch
        // our as-is non-sRGB surface and our display-referred sRGB colors.
        let mut atlas =
            TextAtlas::with_color_mode(&device, &queue, &cache, format, ColorMode::Web);
        let swash_cache = SwashCache::new();
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);
        let mut viewport = Viewport::new(&device, &cache);
        viewport.update(&queue, Resolution { width, height });

        // One layout/shader, two blend states. The base rects use the plain
        // SRC_ALPHA/ONE_MINUS_SRC_ALPHA blending the app was tuned against; the
        // overlay rects are pushed premultiplied and need "src over" in
        // premultiplied space (see below).
        let make_rect_pipeline = |label: &str, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(
                    &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor::default()),
                ),
                vertex: wgpu::VertexState {
                    module: &device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("rect.vs"),
                        source: wgpu::ShaderSource::Wgsl(RECT_VS_WGSL.into()),
                    }),
                    compilation_options: Default::default(),
                    entry_point: Some("vs"),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<RectVertex>() as u64,
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
                                format: wgpu::VertexFormat::Float32x4,
                            },
                            wgpu::VertexAttribute {
                                offset: 24,
                                shader_location: 2,
                                format: wgpu::VertexFormat::Float32x2,
                            },
                            wgpu::VertexAttribute {
                                offset: 32,
                                shader_location: 3,
                                format: wgpu::VertexFormat::Float32x4,
                            },
                        ],
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("rect.fs"),
                        source: wgpu::ShaderSource::Wgsl(RECT_FS_WGSL.into()),
                    }),
                    compilation_options: Default::default(),
                    entry_point: Some("fs"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let rect_pipeline = make_rect_pipeline("rects", Some(wgpu::BlendState::ALPHA_BLENDING));
        // (The hand-drawn switcher overlay once needed a second rect
        // pipeline here with "src over" alpha, so its dim could darken the
        // terminal without draining the window's own alpha; egui composites
        // its own layers, so one pipeline is enough again.)
        let make_rect_buffer = |device: &wgpu::Device, label: &str| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (1024 * std::mem::size_of::<RectVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let rect_buffer = make_rect_buffer(&device, "rects");
        // Replacement blending is essential: transparent clears must erase the
        // old glyphs and alpha rather than blend repeatedly into retained pixels.
        let clear_pipeline = make_rect_pipeline("damage clear", None);
        let clear_buffer = make_rect_buffer(&device, "damage clear");
        queue.write_buffer(&clear_buffer, 0, bytemuck::cast_slice(&rect_vertices(
            0.0, 0.0, 1.0, 1.0, [0.0; 4], 0.0, 1.0, 1.0,
        )));
        let composite_quad = make_rect_buffer(&device, "terminal composite");
        let vertex = |pos, uv| PrevVertex { pos, uv, half: [0.0; 2], backdrop: [0.0; 4] };
        let quad = [
            vertex([-1.0, 1.0], [0.0, 0.0]), vertex([1.0, 1.0], [1.0, 0.0]),
            vertex([1.0, -1.0], [1.0, 1.0]), vertex([-1.0, 1.0], [0.0, 0.0]),
            vertex([1.0, -1.0], [1.0, 1.0]), vertex([-1.0, -1.0], [0.0, 1.0]),
        ];
        queue.write_buffer(&composite_quad, 0, bytemuck::cast_slice(&quad));

        let pool: Vec<Buffer> = (0..MAX_RUNS)
            .map(|_| Buffer::new(font_system, Metrics::new(font_px, font_lh)))
            .collect();
        let prompt_buf = Buffer::new(font_system, Metrics::new(font_px, font_lh));

        // Kitty image pipeline. The preview bind layout and sampler fit it
        // exactly (one filtered texture + a linear sampler), so there is no
        // second of either.
        let img_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("images"),
            bind_group_layouts: &[Some(&prev_bgl)],
            immediate_size: 0,
        });
        let img_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("images"),
            layout: Some(&img_layout),
            vertex: wgpu::VertexState {
                module: &device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("images.vs"),
                    source: wgpu::ShaderSource::Wgsl(IMG_VS_WGSL.into()),
                }),
                compilation_options: Default::default(),
                entry_point: Some("vs"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<ImgVertex>() as u64,
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
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("images.fs"),
                    source: wgpu::ShaderSource::Wgsl(IMG_FS_WGSL.into()),
                }),
                compilation_options: Default::default(),
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // PNGs carry alpha; images composite over the background
                    // rects and under the glyphs, so ordinary source-alpha.
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let img_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("images"),
            size: 4096,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Gpu {
            device,
            queue,
            surface,
            config,
            present_generation: None,
            present_terminal: None,
            clear_pipeline, clear_buffer, composite_quad,
            rect_pipeline,
            rect_buffer,
            viewport,
            atlas,
            swash_cache,
            text_renderer,
            rects: Vec::with_capacity(1024),
            pool,
            alpha_ok,
            prev_bufs: Vec::new(),
            prev_damage: Vec::new(),
            prev_revision: Vec::new(),
            prev_styles: Vec::new(),
            overlays: Vec::new(),
            tab_ids: Vec::new(),
            next_revision: 0,
            prev_stage_quad: None,
            prev_last: Vec::new(),
            prev_sampler,
            prev_bgl,
            prev_pipeline,
            previews: Vec::new(),
            prev_size: (0, 0),
            img_pipeline,
            img_buffer,
            img_cache: std::collections::HashMap::new(),
            prompt_buf,
        }
    }

    /// Upload and draw one batch of rect vertices.
    fn draw_rects(&mut self, pass: &mut wgpu::RenderPass<'_>, rects: &[RectVertex]) {
        let pipeline = self.rect_pipeline.clone();
        draw_rects(
            &self.device,
            &self.queue,
            pass,
            &mut self.rect_buffer,
            &pipeline,
            rects,
        );
    }

    /// True when this exact image generation is already on the GPU.
    pub fn has_image(&self, key: (u32, u32, u32)) -> bool {
        self.img_cache.contains_key(&key)
    }

    /// Upload an RGBA image (packed rows, w*h*4 bytes) and cache its
    /// texture under `key`. Re-uploading the same id with a new generation
    /// retires the older one: exactly one live texture per (tab, id).
    pub fn upload_image(&mut self, key: (u32, u32, u32), rgba: &[u8], w: u32, h: u32) {
        if self.img_cache.contains_key(&key) {
            return;
        }
        let (tab, id, _) = key;
        self.img_cache.retain(|k, _| !(k.0 == tab && k.1 == id));
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("kitty image"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let bpr = 4 * w;
        let rows = |src: &[u8]| -> Vec<u8> {
            let padded = bpr.next_multiple_of(256);
            let mut out = Vec::with_capacity(padded as usize * h as usize);
            for y in 0..h as usize {
                out.extend_from_slice(&src[y * bpr as usize..][..bpr as usize]);
                out.resize(out.len() + padded as usize - bpr as usize, 0);
            }
            out
        };
        let data: &[u8] = if bpr % 256 == 0 { rgba } else { &rows(rgba) };
        let bpr = bpr.next_multiple_of(256);
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y: 0, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bpr),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("kitty image"),
            layout: &self.prev_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.prev_sampler),
                },
            ],
        });
        self.img_cache.insert(key, ImgSlot { _tex: tex, _view: view, bind });
    }

    /// Drop every texture belonging to a tab (tab close).
    pub fn forget_images(&mut self, tab: u32) {
        self.img_cache.retain(|k, _| k.0 != tab);
    }

    /// Paint this frame's image quads: after the rects (images belong over
    /// the background) and before the glyphs (text stays readable on top).
    /// Bind groups are cloned (each is an Arc) so the pass borrow and the
    /// cache read do not fight over `self`.
    fn draw_images(&mut self, pass: &mut wgpu::RenderPass<'_>, images: &[ImageQuad], wf: f32, hf: f32) {
        if images.is_empty() {
            return;
        }
        let clip = |px: f32, py: f32| -> [f32; 2] {
            [(px / wf) * 2.0 - 1.0, 1.0 - (py / hf) * 2.0]
        };
        let mut verts: Vec<ImgVertex> = Vec::with_capacity(images.len() * 6);
        for q in images {
            let [x, y, w, h] = q.dst;
            let (u0, v0, u1, v1) = (q.uv[0], q.uv[1], q.uv[2], q.uv[3]);
            let v = |px: f32, py: f32, u: f32, vv: f32| ImgVertex {
                pos: clip(px, py),
                uv: [u, vv],
            };
            let (x1, y1) = (x + w, y + h);
            verts.extend_from_slice(&[
                v(x, y, u0, v0),
                v(x1, y, u1, v0),
                v(x1, y1, u1, v1),
                v(x, y, u0, v0),
                v(x1, y1, u1, v1),
                v(x, y1, u0, v1),
            ]);
        }
        let need = verts.len() * std::mem::size_of::<ImgVertex>();
        if need > self.img_buffer.size() as usize {
            self.img_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("images"),
                size: ((need * 2) as u64).next_multiple_of(4096),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        self.queue.write_buffer(&self.img_buffer, 0, bytemuck::cast_slice(&verts));
        let binds: Vec<Option<wgpu::BindGroup>> = images.iter()
            .map(|q| self.img_cache.get(&q.key).map(|s| s.bind.clone()))
            .collect();
        pass.set_pipeline(&self.img_pipeline.clone());
        pass.set_vertex_buffer(0, self.img_buffer.slice(..));
        for (i, bind) in binds.into_iter().enumerate() {
            // A quad whose texture vanished (deleted between building the
            // list and drawing it - one frame's grace at worst) is skipped,
            // never drawn with a stranger's bind group.
            if let Some(bind) = bind {
                pass.set_bind_group(0, &bind, &[]);
                pass.draw((i * 6) as u32..(i * 6 + 6) as u32, 0..1);
            }
        }
    }

    pub fn invalidate_terminals(&mut self) {
        self.prev_damage.fill(Damage::full());
        self.prev_styles.fill(None);
    }

    /// Receiver for the terminal snapshot's damage callback. Rendering the
    /// active tab and preparing a delayed background preview share this queue.
    pub fn damage_terminal(&mut self, idx: usize, damage: &Damage) {
        self.prev_damage[idx].merge(damage);
    }

    pub fn update_overlays(&mut self, idx: usize, next: TerminalOverlays, rows: usize, cell_h: f32, strip_h: f32) {
        let old = &self.overlays[idx];
        let damage = &mut self.prev_damage[idx];
        if next.selection != old.selection {
            for &(row, _, _) in next.selection.iter().chain(&old.selection) { damage.add_line(row); }
        }
        if next.cursor != old.cursor {
            for cursor in next.cursor.iter().chain(old.cursor.iter()) { damage.add_line(cursor.row as usize); }
        }
        if next.underline != old.underline {
            for &(_, y, _) in next.underline.iter().chain(old.underline.iter()) {
                damage.add_line(((y - strip_h) / cell_h).max(0.0) as usize);
            }
        }
        if next.notice != old.notice { damage.add_line(rows.saturating_sub(1)); }
        if next.images != old.images {
            for image in next.images.iter().chain(&old.images) {
                let start = ((image.dst[1] - strip_h) / cell_h).floor().max(0.0) as usize;
                let end = ((image.dst[1] + image.dst[3] - strip_h) / cell_h).ceil().max(0.0) as usize;
                for row in start..end.min(rows) { damage.add_line(row); }
            }
        }
        self.overlays[idx] = next;
    }

    /// Downsample directly into a persistent GPU thumbnail. The box filter
    /// preserves terminal glyphs without a blocking CPU readback/upload cycle.
    pub fn thumbnail(
        &mut self, idx: usize, top: u32, thumb: (u32, u32), existing: Option<&wgpu::Texture>,
    ) -> Option<wgpu::Texture> {
        let (w, h) = (self.config.width, self.config.height);
        let (tw, th) = thumb;
        if w == 0 || h <= top || tw == 0 || th == 0 { return None; }
        let slot = self.previews.get(idx)?.as_ref()?;
        let target = existing.filter(|t| t.width() == tw && t.height() == th).cloned().unwrap_or_else(|| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("terminal thumbnail"),
                size: wgpu::Extent3d { width: tw, height: th, depth_or_array_layers: 1 },
                dimension: wgpu::TextureDimension::D2,
                format: self.config.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
                mip_level_count: 1, sample_count: 1, view_formats: &[],
            })
        });
        // The quad samples the terminal area of the texture (below `top`; the
        // tab strip would be in the frame otherwise): the UV range is
        // offset to start there. Clip space is y-up, texture space y-down.
        let top_frac = top as f32 / h as f32;
        // One destination pixel covers 1/tw (x) and (1-top_frac)/th (y) of
        // the texture in UV: the box filter's half-width.
        let half = [0.5 / tw as f32, 0.5 * (1.0 - top_frac) / th as f32];
        let bg = self.prev_styles.get(idx)?.as_ref()?.bg;
        let backdrop = [bg[0], bg[1], bg[2], 1.0];
        let quad: [PrevVertex; 6] = [
            PrevVertex {
                pos: [-1.0, 1.0],
                uv: [0.0, top_frac],
                half,
                backdrop,
            },
            PrevVertex {
                pos: [1.0, 1.0],
                uv: [1.0, top_frac],
                half,
                backdrop,
            },
            PrevVertex {
                pos: [1.0, -1.0],
                uv: [1.0, 1.0],
                half,
                backdrop,
            },
            PrevVertex {
                pos: [-1.0, 1.0],
                uv: [0.0, top_frac],
                half,
                backdrop,
            },
            PrevVertex {
                pos: [1.0, -1.0],
                uv: [1.0, 1.0],
                half,
                backdrop,
            },
            PrevVertex {
                pos: [-1.0, -1.0],
                uv: [0.0, 1.0],
                half,
                backdrop,
            },
        ];
        if self.prev_stage_quad.is_none() {
            self.prev_stage_quad = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("preview readback quad"),
                size: ((quad.len() * std::mem::size_of::<PrevVertex>()) as u64)
                    .next_multiple_of(4096),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let quad_buf = self.prev_stage_quad.as_ref().unwrap();
        let sview = target.create_view(&wgpu::TextureViewDescriptor::default());
        {
            let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("preview readback"),
            });
            self.queue
                .write_buffer(quad_buf, 0, bytemuck::cast_slice(&quad));
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("preview readback"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &sview,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                // The tile pipeline samples exactly the source region a
                // destination pixel covers (the bind group carries the tab's
                // own preview texture and the shared sampler).
                pass.set_pipeline(&self.prev_pipeline);
                pass.set_bind_group(0, &slot.bind, &[]);
                pass.set_vertex_buffer(0, quad_buf.slice(..));
                pass.draw(0..6, 0..1);
            }
            self.queue.submit([enc.finish()]);
        }
        Some(target)
    }

    /// Explicit copy-as-image capture. Only this user-requested export waits
    /// for a CPU transfer; interactive previews remain entirely on the GPU.
    pub fn readback_preview(&mut self, idx: usize, top: u32, thumb: (u32, u32)) -> Option<Vec<u8>> {
        let stage = self.thumbnail(idx, top, thumb, None)?;
        let (tw, th) = thumb;
        let bpp = self.config.format.block_copy_size(None).unwrap_or(4) as u64;
        let row_bytes = (tw as u64 * bpp) as usize;
        let bytes_per_row = (tw as u64 * bpp).next_multiple_of(256);
        let sbuf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("image export readback"), size: bytes_per_row * th as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &stage, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &sbuf, layout: wgpu::TexelCopyBufferLayout { offset: 0,
                bytes_per_row: Some(bytes_per_row as u32), rows_per_image: Some(th) } },
            wgpu::Extent3d { width: tw, height: th, depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        let map_ok = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        sbuf.slice(..).map_async(wgpu::MapMode::Read, {
            let map_ok = std::sync::Arc::clone(&map_ok);
            move |res| {
                if res.is_err() {
                    map_ok.store(false, std::sync::atomic::Ordering::SeqCst);
                }
            }
        });
        self.device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        if !map_ok.load(std::sync::atomic::Ordering::SeqCst) {
            sbuf.unmap();
            return None;
        }
        let Ok(mapped) = sbuf.slice(..).get_mapped_range() else {
            sbuf.unmap();
            return None;
        };
        let mut rgba = vec![0u8; (tw * th * 4) as usize];
        let bgra = matches!(
            self.config.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        for y in 0..th as usize {
            let row = &mapped[y * bytes_per_row as usize..][..row_bytes];
            if bgra {
                for (i, px) in row.chunks_exact(bpp as usize).enumerate() {
                    rgba
                        [(y * tw as usize + i) * 4..(y * tw as usize + i) * 4 + 4]
                        .copy_from_slice(&[px[2], px[1], px[0], px[3]]);
                }
            } else {
                rgba[y * row_bytes..(y + 1) * row_bytes].copy_from_slice(row);
            }
        }
        drop(mapped);
        sbuf.unmap();
        Some(rgba)
    }

    /// Persistent tab targets follow stable ids through insertion and reordering.
    /// The active terminal, thumbnails, and explicit exports all use these.
    pub fn ensure_previews(&mut self, ids: &[usize], size: (u32, u32)) {
        let (w, h) = size;
        if w == 0 || h == 0 { return; }
        for (i, id) in ids.iter().enumerate() {
            if let Some(old) = self.tab_ids.iter().position(|old| old == id) {
                self.tab_ids.swap(i, old);
                self.previews.swap(i, old);
                self.prev_bufs.swap(i, old);
                self.prev_damage.swap(i, old);
                self.prev_revision.swap(i, old);
                self.prev_styles.swap(i, old);
                self.overlays.swap(i, old);
                self.prev_last.swap(i, old);
            } else {
                self.tab_ids.insert(i, *id);
                self.previews.insert(i, None);
                self.prev_bufs.insert(i, Vec::new());
                self.prev_damage.insert(i, Damage::full());
                self.prev_revision.insert(i, 0);
                self.prev_styles.insert(i, None);
                self.overlays.insert(i, TerminalOverlays::default());
                self.prev_last.insert(i, Instant::now());
            }
        }
        let n = ids.len();
        self.tab_ids.truncate(n);
        self.previews.truncate(n);
        self.prev_bufs.truncate(n);
        self.prev_damage.truncate(n);
        self.prev_revision.truncate(n);
        self.prev_styles.truncate(n);
        self.overlays.truncate(n);
        self.prev_last.truncate(n);
        if self.prev_size != size {
            self.previews.iter_mut().for_each(|slot| *slot = None);
            self.prev_revision.fill(0);
            self.invalidate_terminals();
            self.prev_size = size;
        }
        for i in 0..n {
            if self.previews[i].is_some() { continue; }
            let tex = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("preview"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                dimension: wgpu::TextureDimension::D2,
                format: self.config.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                mip_level_count: 1,
                sample_count: 1,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("preview"),
                layout: &self.prev_bgl,                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.prev_sampler),
                    },
                ],
            });
            log::debug!("preview target {i}: {w}x{h}");
            self.previews[i] = Some(PreviewSlot {
                _tex: tex,
                view,
                bind,
            });
        }
    }

    /// Rasterize only damaged bands into a retained tab texture. Live and
    /// background preparation use the same pass, atlas, and clipping rules.
    fn render_terminal(
        &mut self,
        font_system: &mut FontSystem,
        idx: usize,
        areas: Vec<TextArea<'_>>,
        rects: &[RectVertex],
        scissors: &[[u32; 4]],
        images: &[ImageQuad],
    ) {
        if scissors.is_empty() { return; }
        let view = self.previews[idx].as_ref().expect("tab target").view.clone();
        let viewport = &self.viewport;
        let has_text = !areas.is_empty();
        if has_text {
            self.text_renderer.prepare(
                &self.device, &self.queue, font_system, &mut self.atlas,
                viewport, areas, &mut self.swash_cache,
            ).expect("prepare terminal damage");
        }
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("terminal damage"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terminal damage"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view, depth_slice: None, resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None, timestamp_writes: None,
                occlusion_query_set: None, multiview_mask: None,
            });
            for &[x, y, w, h] in scissors {
                pass.set_scissor_rect(x, y, w, h);
                pass.set_pipeline(&self.clear_pipeline);
                pass.set_vertex_buffer(0, self.clear_buffer.slice(..));
                pass.draw(0..6, 0..1);
                self.draw_rects(&mut pass, rects);
                self.draw_images(&mut pass, images, self.config.width as f32, self.config.height as f32);
                if has_text {
                    self.text_renderer.render(&self.atlas, &self.viewport, &mut pass)
                        .expect("render terminal damage");
                }
            }
        }
        // Submit before another target reuses the text/rect buffers.
        self.queue.submit([encoder.finish()]);
        self.next_revision += 1;
        self.prev_revision[idx] = self.next_revision;
        self.prev_last[idx] = Instant::now();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_frame(
        &mut self,
        font_system: &mut FontSystem,
        areas: Vec<glyphon::TextArea>,
        images: Vec<ImageQuad>,
        previews: Vec<PreviewJob>,
        window: &Window,
        mut egui: Option<crate::egui_ui::EguiFrame<'_>>,
        active: usize,
        live_scissors: Vec<[u32; 4]>,
        generation: u64,
        force_present: bool,
    ) {
        let (w, h) = (self.config.width, self.config.height);
        self.viewport.update(&self.queue, Resolution { width: w, height: h });
        for job in previews {
            // Take ownership while preparing; no references into a vector that
            // can be mutated or reallocated by the shared target renderer.
            let buffers = std::mem::take(&mut self.prev_bufs[job.idx]);
            let areas = buffers.iter().zip(&job.text).map(|(buffer, run)| {
                terminal_text_area(buffer, run.left, run.top, run.color, job.bounds)
            }).collect();
            self.render_terminal(font_system, job.idx, areas, &job.rects, &job.scissors, &job.images);
            self.prev_bufs[job.idx] = buffers;
        }
        let rects = std::mem::take(&mut self.rects);
        self.render_terminal(font_system, active, areas, &rects, &live_scissors, &images);
        self.rects = rects;

        // A compositor redraw with no damage or UI change must not generate a
        // fresh commit. Still apply/free egui texture deltas on skipped frames.
        let terminal_revision = (self.tab_ids[active], self.prev_revision[active]);
        if self.present_terminal == Some(terminal_revision)
            && self.present_generation == Some(generation) && !force_present {
            if let Some(e) = egui.as_mut() {
                crate::egui_ui::apply_textures(&self.device, &self.queue, e.renderer, e.full);
            }
            return;
        }
        let frame_tex = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            _ => { window.request_redraw(); return; },
        };
        let view = frame_tex.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        let egui_updates = egui.as_mut().map(|e| {
            let cbs = e.renderer.update_buffers(&self.device, &self.queue, &mut encoder, &e.paint_jobs, &e.screen);
            crate::egui_ui::apply_textures(&self.device, &self.queue, e.renderer, e.full);
            cbs
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view, depth_slice: None, resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None, timestamp_writes: None,
                occlusion_query_set: None, multiview_mask: None,
            });
            // Full-resolution sampling (zero filter footprint) copies the
            // retained tab texture exactly, including its premultiplied alpha.
            pass.set_pipeline(&self.prev_pipeline);
            pass.set_bind_group(0, &self.previews[active].as_ref().unwrap().bind, &[]);
            pass.set_vertex_buffer(0, self.composite_quad.slice(..));
            pass.draw(0..6, 0..1);
            if let Some(e) = egui.as_ref() {
                e.renderer.render(&mut pass.forget_lifetime(), &e.paint_jobs, &e.screen);
            }
        }
        for cb in egui_updates.into_iter().flatten() { self.queue.submit([cb]); }
        window.pre_present_notify();
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame_tex);
        self.present_generation = Some(generation);
        self.present_terminal = Some(terminal_revision);
    }
}
