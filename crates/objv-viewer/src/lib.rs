//! OBJV viewer: decode an `.objv` mesh and render it in the browser with wgpu.
//!
//! Web-only by design — the surface is created straight from a `<canvas>`, with
//! no winit layer in between. Everything below is gated to `wasm32`; on native
//! the crate compiles to an empty library so the workspace stays buildable.

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use glam::{Mat4, Vec3, Vec4};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::HtmlCanvasElement;
use wgpu::util::DeviceExt;

/// GPU uniform block. `repr(C)` + Pod so it maps 1:1 to the WGSL `Uniforms`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    mvp: [[f32; 4]; 4],
    light_dir: [f32; 4],
    z_min: f32,
    z_max: f32,
    mode: u32,
    _pad: f32,
}

/// Orbit camera around the mesh centre. Z is up (UTM elevation).
struct Camera {
    target: Vec3,
    radius: f32, // half the model's largest extent; sets framing & clip planes
    distance: f32,
    yaw: f32,
    pitch: f32,
}

impl Camera {
    fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.target + self.distance * Vec3::new(cp * cy, cp * sy, sp)
    }

    fn mvp(&self, aspect: f32) -> Mat4 {
        let near = (self.distance - self.radius).max(self.radius * 0.01);
        let far = self.distance + self.radius * 2.0 + 1.0;
        let proj = Mat4::perspective_rh(45f32.to_radians(), aspect, near, far);
        let view = Mat4::look_at_rh(self.eye(), self.target, Vec3::Z);
        proj * view
    }
}

/// Active interaction tool.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Navigate,
    Measure,
    Section,
    Dip,
}

impl Tool {
    fn label(self) -> &'static str {
        match self {
            Tool::Navigate => "navigate",
            Tool::Measure => "measure",
            Tool::Section => "cross-section",
            Tool::Dip => "strike / dip",
        }
    }
}

/// One overlay line vertex: position + RGB color.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct OverlayVertex {
    pos: [f32; 3],
    color: [f32; 3],
}

/// Everything needed to draw a frame.
struct State {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    depth_view: wgpu::TextureView,
    pipeline: wgpu::RenderPipeline,
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    index_count: u32,
    canvas: HtmlCanvasElement,

    camera: Camera,
    z_min: f32,
    z_max: f32,
    mode: u32,
    light_dir: [f32; 4],

    // CPU geometry, retained for ray-cast picking and plane slicing
    cpu_positions: Vec<f32>,
    cpu_indices: Vec<u32>,

    // overlay (measurement lines / section / dip disc)
    overlay_pipeline: wgpu::RenderPipeline,
    overlay_buf: Option<wgpu::Buffer>,
    overlay_count: u32,

    // tools
    tool: Tool,
    points: Vec<[f32; 3]>,
    section_segs: Vec<[[f32; 3]; 2]>,

    // input
    dragging: bool,
    last_x: f32,
    last_y: f32,
    press_x: f32,
    press_y: f32,
    moved: f32,
}

impl State {
    async fn new(canvas: HtmlCanvasElement, data: &[u8]) -> Result<State, String> {
        let mesh = decode_objv(data)?;
        log::info!(
            "decoded mesh: {} verts, {} tris, bbox z[{:.1},{:.1}]",
            mesh.vertex_count(),
            mesh.triangle_count(),
            mesh.bbox_min[2],
            mesh.bbox_max[2]
        );
        set_text("m-verts", &group_thousands(mesh.vertex_count()));
        set_text("m-tris", &group_thousands(mesh.triangle_count()));
        // Absolute elevation range (local Z + origin Z) for the legend.
        set_text(
            "leg-zmax",
            &format!("{:.0} m", mesh.origin[2] + mesh.bbox_max[2] as f64),
        );
        set_text(
            "leg-zmin",
            &format!("{:.0} m", mesh.origin[2] + mesh.bbox_min[2] as f64),
        );

        let width = canvas.width().max(1);
        let height = canvas.height().max(1);

        // Probe for real WebGPU support and fall back to WebGL2 otherwise — the
        // robust path for browsers where `navigator.gpu` exists but can't make
        // an adapter (and for headless Chromium during verification).
        let mut idesc = wgpu::InstanceDescriptor::new_without_display_handle();
        idesc.backends = wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL;
        let instance = wgpu::util::new_instance_with_webgpu_detection(idesc).await;

        // wgpu 29's safe `create_surface(Canvas)` passes a null display handle,
        // and wgpu-core's WebGL2 path rejects that (`MissingDisplayHandle`) —
        // only the WebGPU path tolerates it. So we build the raw handles
        // ourselves with an explicit Web display handle; this works for both
        // WebGPU and the WebGL2 fallback.
        let surface = {
            use wgpu::rwh::{RawDisplayHandle, RawWindowHandle, WebCanvasWindowHandle, WebDisplayHandle};
            let value: &wasm_bindgen::JsValue = canvas.as_ref();
            let obj = std::ptr::NonNull::from(value).cast();
            let raw_window_handle: RawWindowHandle = WebCanvasWindowHandle::new(obj).into();
            let raw_display_handle: RawDisplayHandle = WebDisplayHandle::new().into();
            unsafe {
                instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: Some(raw_display_handle),
                    raw_window_handle,
                })
            }
            .map_err(|e| format!("create_surface: {e}"))?
        };

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
            })
            .await
            .map_err(|e| format!("no GPU adapter: {e}"))?;
        log::info!("adapter: {:?}", adapter.get_info());

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("objv-device"),
                required_features: wgpu::Features::empty(),
                // WebGL2-safe limits, scaled up to whatever the adapter allows
                // (the index buffer is ~120 MB, so we need generous buffer size).
                required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                    .using_resolution(adapter.limits()),
                experimental_features: wgpu::ExperimentalFeatures::default(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|e| format!("request_device: {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        let depth_view = make_depth(&device, width, height);

        // Geometry buffers — positions upload directly as packed f32x3.
        let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertices"),
            contents: bytemuck::cast_slice(&mesh.positions),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let index_count = mesh.indices.len() as u32;

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("bgl"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bg"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("objv-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pl"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 12,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x3,
                        offset: 0,
                        shader_location: 0,
                    }],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // Overlay pipeline: colored lines, drawn on top (depth test disabled).
        let overlay_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("overlay.wgsl").into()),
        });
        let overlay_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &overlay_shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 24,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 12,
                            shader_location: 1,
                        },
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &overlay_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let center = Vec3::new(
            0.5 * (mesh.bbox_min[0] + mesh.bbox_max[0]),
            0.5 * (mesh.bbox_min[1] + mesh.bbox_max[1]),
            0.5 * (mesh.bbox_min[2] + mesh.bbox_max[2]),
        );
        let extent = Vec3::new(
            mesh.bbox_max[0] - mesh.bbox_min[0],
            mesh.bbox_max[1] - mesh.bbox_min[1],
            mesh.bbox_max[2] - mesh.bbox_min[2],
        );
        // Frame on the horizontal footprint so long, thin escarpments fill the
        // view rather than being sized by their (small) vertical extent.
        let footprint = 0.5 * extent.truncate().length();
        let radius = footprint.max(0.5 * extent.max_element());
        let camera = Camera {
            target: center,
            radius,
            distance: radius * 1.8,
            yaw: 0.6,
            pitch: 0.32,
        };

        Ok(State {
            surface,
            device,
            queue,
            config,
            depth_view,
            pipeline,
            vertex_buf,
            index_buf,
            uniform_buf,
            bind_group,
            index_count,
            canvas,
            camera,
            z_min: mesh.bbox_min[2],
            z_max: mesh.bbox_max[2],
            mode: 1, // start on the elevation colormap
            light_dir: normalize4([-0.6, 0.5, 0.7]),
            cpu_positions: mesh.positions,
            cpu_indices: mesh.indices,
            overlay_pipeline,
            overlay_buf: None,
            overlay_count: 0,
            tool: Tool::Navigate,
            points: Vec::new(),
            section_segs: Vec::new(),
            dragging: false,
            last_x: 0.0,
            last_y: 0.0,
            press_x: 0.0,
            press_y: 0.0,
            moved: 0.0,
        })
    }

    /// Reconfigure surface + depth buffer if the canvas backing size changed.
    fn sync_size(&mut self) {
        let w = self.canvas.width().max(1);
        let h = self.canvas.height().max(1);
        if w != self.config.width || h != self.config.height {
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(&self.device, &self.config);
            self.depth_view = make_depth(&self.device, w, h);
        }
    }

    fn set_tool(&mut self, tool: Tool) {
        if self.tool != tool {
            self.tool = tool;
            self.points.clear();
            self.section_segs.clear();
            self.rebuild_overlay();
        }
        set_text("m-tool", tool.label());
        self.recompute_readout();
    }

    fn clear_tool(&mut self) {
        self.points.clear();
        self.section_segs.clear();
        self.rebuild_overlay();
        self.recompute_readout();
    }

    fn undo_point(&mut self) {
        self.points.pop();
        if self.tool == Tool::Section {
            self.section_segs.clear();
            if self.points.len() == 2 {
                self.compute_section();
            }
        }
        self.rebuild_overlay();
        self.recompute_readout();
    }

    /// Build a world-space ray through a pixel of the canvas.
    fn ray_from_pixel(&self, px: f32, py: f32, rect_w: f32, rect_h: f32) -> objv_geom::Ray {
        let ndc_x = (px / rect_w) * 2.0 - 1.0;
        let ndc_y = 1.0 - (py / rect_h) * 2.0;
        let aspect = self.config.width as f32 / self.config.height as f32;
        let inv = self.camera.mvp(aspect).inverse();
        let near = inv * Vec4::new(ndc_x, ndc_y, 0.0, 1.0);
        let far = inv * Vec4::new(ndc_x, ndc_y, 1.0, 1.0);
        let near = near.truncate() / near.w;
        let far = far.truncate() / far.w;
        let dir = far - near;
        objv_geom::Ray {
            origin: [near.x, near.y, near.z],
            dir: [dir.x, dir.y, dir.z],
        }
    }

    /// Ray-cast a click and feed the hit point to the active tool.
    fn click(&mut self, px: f32, py: f32, rect_w: f32, rect_h: f32) {
        if self.tool == Tool::Navigate {
            return;
        }
        let ray = self.ray_from_pixel(px, py, rect_w, rect_h);
        let Some(hit) = objv_geom::raycast(&self.cpu_positions, &self.cpu_indices, &ray) else {
            return;
        };
        if self.tool == Tool::Section && self.points.len() >= 2 {
            self.points.clear();
            self.section_segs.clear();
        }
        self.points.push(hit.point);
        if self.tool == Tool::Section && self.points.len() == 2 {
            self.compute_section();
        }
        self.rebuild_overlay();
        self.recompute_readout();
    }

    fn compute_section(&mut self) {
        if self.points.len() < 2 {
            return;
        }
        let (a, b) = (self.points[0], self.points[1]);
        // Vertical plane containing the line AB (normal is horizontal).
        let normal = objv_geom::normalize([b[1] - a[1], -(b[0] - a[0]), 0.0]);
        let plane = objv_geom::Plane { point: a, normal };
        self.section_segs = objv_geom::slice_plane(&self.cpu_positions, &self.cpu_indices, &plane);
    }

    fn recompute_readout(&self) {
        let s = match self.tool {
            Tool::Navigate => "navigate · pick a tool to measure".to_string(),
            Tool::Measure => self.measure_text(),
            Tool::Section => self.section_text(),
            Tool::Dip => self.dip_text(),
        };
        set_text("results", &s);
    }

    fn measure_text(&self) -> String {
        use objv_geom::{length, polygon_area, polyline_length, sub};
        let n = self.points.len();
        if n == 0 {
            return "measure · click points on the surface".into();
        }
        let mut s = format!("measure · {n} pts\npath {:.2} m", polyline_length(&self.points));
        if n >= 2 {
            let d = sub(self.points[n - 1], self.points[0]);
            let horiz = (d[0] * d[0] + d[1] * d[1]).sqrt();
            s += &format!(
                "\nstraight {:.2} m\nΔhoriz {:.2} m   Δz {:+.2} m",
                length(d),
                horiz,
                d[2]
            );
        }
        if n >= 3 {
            s += &format!("\narea {:.1} m²", polygon_area(&self.points));
        }
        s
    }

    fn section_text(&self) -> String {
        if self.points.len() < 2 {
            return format!(
                "cross-section · {} / 2 pts\nclick two points",
                self.points.len()
            );
        }
        let mut zmin = f32::INFINITY;
        let mut zmax = f32::NEG_INFINITY;
        let mut total = 0.0f32;
        for s in &self.section_segs {
            total += objv_geom::length(objv_geom::sub(s[1], s[0]));
            for p in s {
                zmin = zmin.min(p[2]);
                zmax = zmax.max(p[2]);
            }
        }
        let (a, b) = (self.points[0], self.points[1]);
        let span = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        format!(
            "cross-section\nline {span:.1} m\nprofile {:.0} m\nrelief {:.1} m  (z {:.1}–{:.1})",
            total,
            (zmax - zmin).max(0.0),
            zmin,
            zmax
        )
    }

    fn dip_text(&self) -> String {
        let n = self.points.len();
        if n < 3 {
            return format!("strike / dip · {n} / 3 pts\nclick ≥3 points on a surface");
        }
        match objv_geom::fit_plane(&self.points) {
            Some(plane) => {
                let o = objv_geom::orientation(plane.normal);
                format!(
                    "strike / dip · {n} pts\ndip {:.1}°\ndip dir {:.0}°\nstrike {:.0}°",
                    o.dip, o.dip_direction, o.strike
                )
            }
            None => "strike / dip · degenerate".into(),
        }
    }

    fn rebuild_overlay(&mut self) {
        let mut v: Vec<OverlayVertex> = Vec::new();
        let m = self.camera.radius * 0.008;
        let pts_col = [1.0, 0.85, 0.2];
        match self.tool {
            Tool::Measure => {
                for w in self.points.windows(2) {
                    push_line(&mut v, w[0], w[1], [0.2, 0.9, 1.0]);
                }
                if self.points.len() >= 3 {
                    push_line(
                        &mut v,
                        *self.points.last().unwrap(),
                        self.points[0],
                        [0.2, 0.55, 0.85],
                    );
                }
                for p in &self.points {
                    push_marker(&mut v, *p, pts_col, m);
                }
            }
            Tool::Section => {
                for s in &self.section_segs {
                    push_line(&mut v, s[0], s[1], [1.0, 0.55, 0.15]);
                }
                for p in &self.points {
                    push_marker(&mut v, *p, pts_col, m);
                }
            }
            Tool::Dip => {
                for p in &self.points {
                    push_marker(&mut v, *p, pts_col, m);
                }
                if self.points.len() >= 3 {
                    if let Some(plane) = objv_geom::fit_plane(&self.points) {
                        self.push_dip_disc(&mut v, plane);
                    }
                }
            }
            Tool::Navigate => {}
        }
        self.overlay_count = v.len() as u32;
        self.overlay_buf = if v.is_empty() {
            None
        } else {
            Some(
                self.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("overlay"),
                        contents: bytemuck::cast_slice(&v),
                        usage: wgpu::BufferUsages::VERTEX,
                    }),
            )
        };
    }

    /// Append a dip disc (plane outline + normal pole + strike line) to `v`.
    fn push_dip_disc(&self, v: &mut Vec<OverlayVertex>, plane: objv_geom::Plane) {
        use objv_geom::{add, cross, length, normalize, scale, sub};
        let mut r = self.camera.radius * 0.03;
        for p in &self.points {
            r = r.max(length(sub(*p, plane.point)) * 0.9);
        }
        let n = plane.normal;
        let seed = if n[2].abs() < 0.9 {
            [0.0, 0.0, 1.0]
        } else {
            [1.0, 0.0, 0.0]
        };
        let u = normalize(cross(seed, n));
        let w = normalize(cross(n, u));
        let segs = 48;
        let mut prev = add(plane.point, scale(u, r));
        for i in 1..=segs {
            let t = i as f32 / segs as f32 * std::f32::consts::TAU;
            let cur = add(
                plane.point,
                add(scale(u, r * t.cos()), scale(w, r * t.sin())),
            );
            push_line(v, prev, cur, [0.9, 0.3, 0.9]);
            prev = cur;
        }
        push_line(v, plane.point, add(plane.point, scale(n, r)), [0.4, 1.0, 0.4]);
        let strike = normalize([-n[1], n[0], 0.0]);
        push_line(
            v,
            sub(plane.point, scale(strike, r)),
            add(plane.point, scale(strike, r)),
            [1.0, 1.0, 0.3],
        );
    }

    fn render(&mut self) {
        self.sync_size();
        let aspect = self.config.width as f32 / self.config.height as f32;
        let uniforms = Uniforms {
            mvp: self.camera.mvp(aspect).to_cols_array_2d(),
            light_dir: self.light_dir,
            z_min: self.z_min,
            z_max: self.z_max,
            mode: self.mode,
            _pad: 0.0,
        };
        self.queue
            .write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                t
            }
            _ => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.07,
                            g: 0.08,
                            b: 0.10,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_pipeline(&self.pipeline);
            rp.set_bind_group(0, &self.bind_group, &[]);
            rp.set_vertex_buffer(0, self.vertex_buf.slice(..));
            rp.set_index_buffer(self.index_buf.slice(..), wgpu::IndexFormat::Uint32);
            rp.draw_indexed(0..self.index_count, 0, 0..1);

            if self.overlay_count > 0 {
                if let Some(buf) = &self.overlay_buf {
                    rp.set_pipeline(&self.overlay_pipeline);
                    rp.set_bind_group(0, &self.bind_group, &[]);
                    rp.set_vertex_buffer(0, buf.slice(..));
                    rp.draw(0..self.overlay_count, 0..1);
                }
            }
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
    }
}

fn push_line(v: &mut Vec<OverlayVertex>, a: [f32; 3], b: [f32; 3], color: [f32; 3]) {
    v.push(OverlayVertex { pos: a, color });
    v.push(OverlayVertex { pos: b, color });
}

/// A small 3-axis cross marking a picked point (size `m` in world units).
fn push_marker(v: &mut Vec<OverlayVertex>, p: [f32; 3], color: [f32; 3], m: f32) {
    push_line(v, [p[0] - m, p[1], p[2]], [p[0] + m, p[1], p[2]], color);
    push_line(v, [p[0], p[1] - m, p[2]], [p[0], p[1] + m, p[2]], color);
    push_line(v, [p[0], p[1], p[2] - m], [p[0], p[1], p[2] + m], color);
}

fn make_depth(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth"),
        size: wgpu::Extent3d {
            width: w.max(1),
            height: h.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

/// Set the text content of an element by id (best-effort; ignores if absent).
fn set_text(id: &str, value: &str) {
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
    {
        el.set_text_content(Some(value));
    }
}

/// Format an integer with thousands separators, e.g. 4994411 -> "4,994,411".
fn group_thousands(n: usize) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(b as char);
    }
    out
}

fn normalize4(v: [f32; 3]) -> [f32; 4] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    [v[0] / l, v[1] / l, v[2] / l, 0.0]
}

/// Decode an `.objv` container (header + codec) into a `Mesh`.
fn decode_objv(data: &[u8]) -> Result<objv_format::Mesh, String> {
    let header = objv_format::read_header(data).map_err(|e| e.to_string())?;
    let body = &data[header.body_offset..];
    let payload: Vec<u8> = match header.codec {
        objv_format::Codec::None => body.to_vec(),
        objv_format::Codec::Zstd => {
            use std::io::Read;
            let mut dec = ruzstd::decoding::StreamingDecoder::new(body)
                .map_err(|e| format!("zstd init: {e}"))?;
            let mut out = Vec::with_capacity(header.ulen);
            dec.read_to_end(&mut out)
                .map_err(|e| format!("zstd read: {e}"))?;
            out
        }
        objv_format::Codec::Deflate => miniz_oxide::inflate::decompress_to_vec(body)
            .map_err(|e| format!("deflate decode: {e:?}"))?,
    };
    objv_format::Mesh::from_payload(&payload).map_err(|e| e.to_string())
}

/// Convert raw OBJ bytes to a compact `.objv` (deflate-compressed) in-browser.
/// Returned bytes are ready to download *and* to feed back into [`start`].
#[wasm_bindgen]
pub fn convert_obj(obj: Vec<u8>, quantize: bool) -> Result<Vec<u8>, JsValue> {
    let text = std::str::from_utf8(&obj).map_err(|_| JsValue::from_str("OBJ is not UTF-8/ASCII"))?;
    let parsed = objv_obj::obj_to_mesh(text);
    log::info!(
        "converted OBJ: {} verts, {} tris",
        parsed.mesh.vertex_count(),
        parsed.mesh.triangle_count()
    );
    let payload = parsed.mesh.to_payload(objv_format::EncodeOptions {
        quantize_positions: quantize,
        store_normals: false,
    });
    let body = miniz_oxide::deflate::compress_to_vec(&payload, 7);
    let mut out = Vec::with_capacity(objv_format::FILE_HEADER_LEN + body.len());
    objv_format::write_header(&mut out, payload.len() as u64, objv_format::Codec::Deflate);
    out.extend_from_slice(&body);
    Ok(out)
}

// --- JS entry point + event wiring -----------------------------------------

/// Called from JS: take over `canvas` and render the given `.objv` bytes.
#[wasm_bindgen]
pub fn start(canvas: HtmlCanvasElement, data: Vec<u8>) {
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);
    wasm_bindgen_futures::spawn_local(async move {
        match State::new(canvas, &data).await {
            Ok(state) => run_loop(state),
            Err(e) => {
                web_sys::console::error_1(&format!("objv-viewer init failed: {e}").into());
            }
        }
    });
}

/// Install input listeners and start the requestAnimationFrame loop.
fn run_loop(state: State) {
    let canvas = state.canvas.clone();
    let state = Rc::new(RefCell::new(state));
    let window = web_sys::window().unwrap();

    // Orbit: drag to rotate.
    {
        let s = state.clone();
        let cb = Closure::<dyn FnMut(web_sys::PointerEvent)>::new(move |e: web_sys::PointerEvent| {
            let mut st = s.borrow_mut();
            st.dragging = true;
            st.last_x = e.client_x() as f32;
            st.last_y = e.client_y() as f32;
            st.press_x = st.last_x;
            st.press_y = st.last_y;
            st.moved = 0.0;
        });
        canvas
            .add_event_listener_with_callback("pointerdown", cb.as_ref().unchecked_ref())
            .unwrap();
        cb.forget();
    }
    {
        let s = state.clone();
        let cb = Closure::<dyn FnMut(web_sys::PointerEvent)>::new(move |e: web_sys::PointerEvent| {
            let mut st = s.borrow_mut();
            if !st.dragging {
                return;
            }
            let x = e.client_x() as f32;
            let y = e.client_y() as f32;
            let dx = x - st.last_x;
            let dy = y - st.last_y;
            st.last_x = x;
            st.last_y = y;
            st.moved += dx.abs() + dy.abs();
            st.camera.yaw -= dx * 0.006;
            st.camera.pitch = (st.camera.pitch + dy * 0.006).clamp(-1.5, 1.5);
        });
        window
            .add_event_listener_with_callback("pointermove", cb.as_ref().unchecked_ref())
            .unwrap();
        cb.forget();
    }
    {
        let s = state.clone();
        let cb = Closure::<dyn FnMut(web_sys::PointerEvent)>::new(move |e: web_sys::PointerEvent| {
            let mut st = s.borrow_mut();
            let was_dragging = st.dragging;
            st.dragging = false;
            // A near-stationary press with an active tool is a pick, not a drag.
            if was_dragging && st.tool != Tool::Navigate && st.moved < 6.0 {
                let rect = st.canvas.get_bounding_client_rect();
                let px = e.client_x() as f32 - rect.left() as f32;
                let py = e.client_y() as f32 - rect.top() as f32;
                let (rw, rh) = (rect.width() as f32, rect.height() as f32);
                st.click(px, py, rw, rh);
            }
        });
        window
            .add_event_listener_with_callback("pointerup", cb.as_ref().unchecked_ref())
            .unwrap();
        cb.forget();
    }
    // Zoom.
    {
        let s = state.clone();
        let cb = Closure::<dyn FnMut(web_sys::WheelEvent)>::new(move |e: web_sys::WheelEvent| {
            e.prevent_default();
            let mut st = s.borrow_mut();
            let factor = (1.0 + (e.delta_y() as f32) * 0.001).clamp(0.5, 1.5);
            let min = st.camera.radius * 0.05;
            let max = st.camera.radius * 12.0;
            st.camera.distance = (st.camera.distance * factor).clamp(min, max);
        });
        canvas
            .add_event_listener_with_callback("wheel", cb.as_ref().unchecked_ref())
            .unwrap();
        cb.forget();
    }
    // Colormap mode: keys 1=shaded 2=elevation 3=slope 4=aspect.
    {
        let s = state.clone();
        let cb = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
            move |e: web_sys::KeyboardEvent| {
                let mut st = s.borrow_mut();
                match e.key().as_str() {
                    "1" => st.mode = 0,
                    "2" => st.mode = 1,
                    "3" => st.mode = 2,
                    "4" => st.mode = 3,
                    "n" | "N" => st.set_tool(Tool::Navigate),
                    "m" | "M" => st.set_tool(Tool::Measure),
                    "s" | "S" => st.set_tool(Tool::Section),
                    "d" | "D" => st.set_tool(Tool::Dip),
                    "u" | "U" => st.undo_point(),
                    "x" | "X" | "Escape" => st.clear_tool(),
                    _ => {}
                }
            },
        );
        window
            .add_event_listener_with_callback("keydown", cb.as_ref().unchecked_ref())
            .unwrap();
        cb.forget();
    }

    // RAF loop.
    let raf: Rc<RefCell<Option<Closure<dyn FnMut()>>>> = Rc::new(RefCell::new(None));
    let raf2 = raf.clone();
    let s = state.clone();
    *raf.borrow_mut() = Some(Closure::new(move || {
        s.borrow_mut().render();
        request_animation_frame(raf2.borrow().as_ref().unwrap());
    }));
    request_animation_frame(raf.borrow().as_ref().unwrap());
}

fn request_animation_frame(f: &Closure<dyn FnMut()>) {
    web_sys::window()
        .unwrap()
        .request_animation_frame(f.as_ref().unchecked_ref())
        .unwrap();
}
