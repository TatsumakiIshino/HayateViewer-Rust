mod config;
mod render;
mod image;
mod state;

use crate::config::Settings;
use crate::render::d2d::D2DRenderer;
use crate::image::decoder;
use crate::image::archive::ArchiveLoader;
use crate::state::{AppState, BindingDirection};
use std::sync::Arc;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap1;
use winit::{
    event::{Event, WindowEvent, ElementState, MouseButton, MouseScrollDelta, KeyEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
    keyboard::{PhysicalKey, KeyCode},
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use walkdir::WalkDir;
use winit::platform::windows::WindowBuilderExtWindows;

struct ViewState {
    zoom_level: f32,
    pan_offset: (f32, f32),
    is_dragging: bool,
    last_mouse_pos: (f32, f32),
    cursor_pos: (f32, f32),
}

impl ViewState {
    fn new() -> Self {
        Self {
            zoom_level: 1.0,
            pan_offset: (0.0, 0.0),
            is_dragging: false,
            last_mouse_pos: (0.0, 0.0),
            cursor_pos: (0.0, 0.0),
        }
    }

    fn zoom(&mut self, factor: f32, center: (f32, f32)) {
        let old_zoom = self.zoom_level;
        self.zoom_level *= factor;
        if self.zoom_level < 0.1 { self.zoom_level = 0.1; }
        if self.zoom_level > 50.0 { self.zoom_level = 50.0; }

        let actual_factor = self.zoom_level / old_zoom;
        self.pan_offset.0 = center.0 - (center.0 - self.pan_offset.0) * actual_factor;
        self.pan_offset.1 = center.1 - (center.1 - self.pan_offset.1) * actual_factor;
    }

    fn clamp_pan_offset(&mut self, window_size: (f32, f32), content_size: (f32, f32)) {
        let max_pan_x = (content_size.0 - window_size.0).max(0.0) / 2.0;
        let max_pan_y = (content_size.1 - window_size.1).max(0.0) / 2.0;

        self.pan_offset.0 = self.pan_offset.0.clamp(-max_pan_x, max_pan_x);
        self.pan_offset.1 = self.pan_offset.1.clamp(-max_pan_y, max_pan_y);
    }
}

enum ImageSource {
    Files(Vec<String>),
    Archive(ArchiveLoader),
}

impl ImageSource {
    fn len(&self) -> usize {
        match self {
            Self::Files(f) => f.len(),
            Self::Archive(a) => a.get_file_names().len(),
        }
    }

    fn load_image(&mut self, index: usize) -> Result<decoder::DecodedImage, Box<dyn std::error::Error>> {
        match self {
            Self::Files(f) => {
                let decoded = decoder::decode_image(&f[index])?;
                Ok(decoded)
            }
            Self::Archive(a) => {
                a.load_image(index)
            }
        }
    }
}

fn get_image_source(path: &str) -> Option<ImageSource> {
    let path_buf = std::path::Path::new(path);
    if path_buf.is_dir() {
        let mut files: Vec<String> = Vec::new();
        let supported = ["jpg", "jpeg", "png", "webp", "bmp", "jp2"];
        for entry in WalkDir::new(path).max_depth(1).into_iter().filter_map(|e| e.ok()) {
            if let Some(ext) = entry.path().extension().and_then(|s| s.to_str()) {
                if supported.contains(&ext.to_lowercase().as_str()) {
                    files.push(entry.path().to_string_lossy().to_string());
                }
            }
        }
        files.sort_by(|a, b| natord::compare(a, b));
        return Some(ImageSource::Files(files));
    } else if let Some(ext) = path_buf.extension().and_then(|s| s.to_str()) {
        let ext_lower = ext.to_lowercase();
        if ext_lower == "zip" || ext_lower == "7z" {
            if let Ok(loader) = ArchiveLoader::open(path) {
                return Some(ImageSource::Archive(loader));
            }
        } else {
            // 単一ファイル
            return Some(ImageSource::Files(vec![path.to_string()]));
        }
    }
    None
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config_path = "config.json";
    let settings = Settings::load_or_default(config_path);
    if !std::path::Path::new(config_path).exists() { let _ = settings.save(config_path); }

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let window = Arc::new(WindowBuilder::new()
        .with_title("HayateViewer Rust")
        .with_inner_size(winit::dpi::LogicalSize::new(settings.window_size.0, settings.window_size.1))
        .with_drag_and_drop(true)
        .build(&event_loop)?);

    let hwnd = match window.window_handle()?.as_raw() {
        RawWindowHandle::Win32(handle) => HWND(handle.hwnd.get() as _),
        _ => return Err("Unsupported window handle".into()),
    };

    unsafe {
        use windows::Win32::Graphics::Dwm::*;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &2i32 as *const _ as _, 4);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &1i32 as *const _ as _, 4);
    }

    println!("Starting HayateViewer Rust...");

    let renderer = D2DRenderer::new(hwnd)?;
    let mut view_state = ViewState::new();
    let mut app_state = AppState::new();

    app_state.is_spread_view = settings.is_spread_view;
    app_state.binding_direction = if settings.binding_direction == "right" { BindingDirection::Right } else { BindingDirection::Left };
    app_state.spread_view_first_page_single = settings.spread_view_first_page_single;

    let args: Vec<String> = std::env::args().collect();
    let mut image_source = if args.len() > 1 {
        get_image_source(&args[1])
    } else {
        None
    };

    if let Some(ref src) = image_source {
        // AppStateにファイル数を伝える（暫定的に中身は空にするか、何らかのダミーを入れる）
        app_state.image_files = vec!["".to_string(); src.len()];
    }

    let mut current_bitmaps: Vec<(usize, ID2D1Bitmap1)> = Vec::new();

    event_loop.run(move |event, elwt| {
        match event {
            Event::WindowEvent { event, window_id } if window_id == window.id() => match event {
                WindowEvent::CloseRequested => elwt.exit(),
                WindowEvent::Resized(physical_size) => {
                    let _ = renderer.resize(physical_size.width, physical_size.height);
                }
                WindowEvent::HoveredFile(path) => {
                    println!("Hovering over file: {}", path.to_string_lossy());
                }
                WindowEvent::DroppedFile(path) => {
                    let path_str = path.to_string_lossy();
                    println!("Dropped file: {}", path_str);
                    if let Some(new_source) = get_image_source(&path_str) {
                        println!("Source created: {} files/entries", new_source.len());
                        app_state.image_files = vec!["".to_string(); new_source.len()];
                        app_state.current_page_index = 0;
                        image_source = Some(new_source);
                        current_bitmaps.clear();
                        window.request_redraw();
                    } else {
                        println!("Failed to create source from: {}", path_str);
                    }
                }
                WindowEvent::KeyboardInput { event: KeyEvent { physical_key: PhysicalKey::Code(code), state: ElementState::Pressed, .. }, .. } => {
                    match code {
                        KeyCode::ArrowRight => {
                            app_state.navigate(1);
                            current_bitmaps.clear();
                        },
                        KeyCode::ArrowLeft => {
                            app_state.navigate(-1);
                            current_bitmaps.clear();
                        },
                        KeyCode::KeyB => {
                            app_state.is_spread_view = !app_state.is_spread_view;
                            current_bitmaps.clear();
                        }
                        KeyCode::KeyL => {
                            app_state.binding_direction = match app_state.binding_direction {
                                BindingDirection::Left => BindingDirection::Right,
                                BindingDirection::Right => BindingDirection::Left,
                            };
                            current_bitmaps.clear();
                        }
                        _ => (),
                    }
                    window.request_redraw();
                }
                WindowEvent::CursorMoved { position, .. } => {
                    let pos = (position.x as f32, position.y as f32);
                    if view_state.is_dragging {
                        view_state.pan_offset.0 += pos.0 - view_state.last_mouse_pos.0;
                        view_state.pan_offset.1 += pos.1 - view_state.last_mouse_pos.1;
                    }
                    view_state.last_mouse_pos = pos;
                    view_state.cursor_pos = pos;
                }
                WindowEvent::MouseInput { state, button, .. } => {
                    if button == MouseButton::Left {
                        view_state.is_dragging = state == ElementState::Pressed;
                    } else if button == MouseButton::Right && state == ElementState::Pressed {
                        view_state.zoom_level = 1.0;
                        view_state.pan_offset = (0.0, 0.0);
                    }
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    let factor = match delta {
                        MouseScrollDelta::LineDelta(_, y) => 1.1f32.powf(y),
                        MouseScrollDelta::PixelDelta(pos) => 1.1f32.powf(pos.y as f32 / 100.0),
                    };
                    view_state.zoom(factor, view_state.cursor_pos);
                }
                WindowEvent::RedrawRequested => {
                    if let Some(ref mut source) = image_source {
                        let indices = app_state.get_page_indices_to_display();
                        
                        for &idx in &indices {
                            if !current_bitmaps.iter().any(|(i, _)| *i == idx) {
                                if let Ok(decoded) = source.load_image(idx) {
                                    if let Ok(bitmap) = renderer.create_bitmap(decoded.width, decoded.height, &decoded.data) {
                                        current_bitmaps.push((idx, bitmap));
                                    }
                                }
                            }
                        }

                        renderer.begin_draw();
                        
                        let mut bitmaps_to_draw = Vec::new();
                        for &idx in &indices {
                            if let Some((_, bmp)) = current_bitmaps.iter().find(|(i, _)| *i == idx) {
                                bitmaps_to_draw.push(bmp);
                            }
                        }

                        if !bitmaps_to_draw.is_empty() {
                            unsafe {
                                let window_size = window.inner_size();
                                let win_w = window_size.width as f32;
                                let win_h = window_size.height as f32;

                                let mut total_content_w = 0.0;
                                let mut max_content_h = 0.0;
                                for bmp in &bitmaps_to_draw {
                                    let size = bmp.GetSize();
                                    total_content_w += size.width;
                                    if size.height > max_content_h { max_content_h = size.height; }
                                }

                                let scale_fit = (win_w / total_content_w).min(win_h / max_content_h);
                                let total_scale = scale_fit * view_state.zoom_level;

                                let draw_total_w = total_content_w * total_scale;
                                let draw_max_h = max_content_h * total_scale;

                                view_state.clamp_pan_offset((win_w, win_h), (draw_total_w, draw_max_h));

                                let base_x = (win_w - draw_total_w) / 2.0 + view_state.pan_offset.0;
                                let base_y = (win_h - draw_max_h) / 2.0 + view_state.pan_offset.1;

                                let mut current_x = base_x;
                                for bmp in &bitmaps_to_draw {
                                    let size = bmp.GetSize();
                                    let w = size.width * total_scale;
                                    let h = size.height * total_scale;
                                    let y = base_y + (draw_max_h - h) / 2.0;

                                    let dest_rect = D2D_RECT_F {
                                        left: current_x,
                                        top: y,
                                        right: current_x + w,
                                        bottom: y + h,
                                    };
                                    renderer.draw_bitmap(bmp, &dest_rect);
                                    current_x += w;
                                }
                            }
                        }

                        let _ = renderer.end_draw();
                    } else {
                        renderer.begin_draw();
                        let _ = renderer.end_draw();
                    }
                }
                _ => (),
            },
            Event::AboutToWait => {
                window.request_redraw();
            }
            _ => (),
        }
    })?;

    Ok(())
}
