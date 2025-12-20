mod config;
mod render;
mod image;
mod state;

use crate::config::Settings;
use crate::render::d2d::D2DRenderer;
use crate::image::get_image_source;
use crate::image::cache::create_shared_cache;
use crate::image::loader::{AsyncLoader, LoaderRequest, LoaderResponse};
use crate::state::{AppState, BindingDirection};
use std::sync::Arc;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap1;
use winit::{
    event::{Event, WindowEvent, ElementState, MouseButton, MouseScrollDelta, KeyEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
    keyboard::{PhysicalKey, KeyCode, ModifiersState},
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::platform::windows::WindowBuilderExtWindows;
use tokio::runtime::Runtime;

struct ViewState {
    zoom_level: f32,
    pan_offset: (f32, f32),
    is_panning: bool,
    is_loupe: bool,
    loupe_base_zoom: f32,
    loupe_base_pan: (f32, f32),
    last_mouse_pos: (f32, f32),
    cursor_pos: (f32, f32),
}

impl ViewState {
    fn new() -> Self {
        Self {
            zoom_level: 1.0,
            pan_offset: (0.0, 0.0),
            is_panning: false,
            is_loupe: false,
            loupe_base_zoom: 1.0,
            loupe_base_pan: (0.0, 0.0),
            last_mouse_pos: (0.0, 0.0),
            cursor_pos: (0.0, 0.0),
        }
    }

    fn set_zoom(&mut self, new_zoom: f32, center: (f32, f32)) {
        let old_zoom = self.zoom_level;
        self.zoom_level = new_zoom;
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

    fn reset(&mut self) {
        self.zoom_level = 1.0;
        self.pan_offset = (0.0, 0.0);
        self.is_panning = false;
        self.is_loupe = false;
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config_path = "config.json";
    let settings = Settings::load_or_default(config_path);
    if !std::path::Path::new(config_path).exists() { let _ = settings.save(config_path); }

    // Tokio Runtime
    let rt = Runtime::new()?;
    let _guard = rt.enter();

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
    let mut current_path_key = String::new();

    app_state.is_spread_view = settings.is_spread_view;
    app_state.binding_direction = if settings.binding_direction == "right" { BindingDirection::Right } else { BindingDirection::Left };
    app_state.spread_view_first_page_single = settings.spread_view_first_page_single;

    // Cache & Loader
    let cpu_cache = create_shared_cache(100);
    let loader = AsyncLoader::new(cpu_cache.clone());

    // 初期パスの読み込み
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        if let Some(src) = get_image_source(&args[1]) {
            app_state.image_files = vec!["".to_string(); src.len()];
            current_path_key = args[1].clone();
            rt.block_on(loader.send_request(LoaderRequest::SetSource { 
                source: src, 
                path_key: current_path_key.clone() 
            }));
            request_pages_with_prefetch(&app_state, &loader, &rt);
        }
    }

    let mut current_bitmaps: Vec<(usize, ID2D1Bitmap1)> = Vec::new();
    let mut modifiers = ModifiersState::default();

    event_loop.run(move |event, elwt| {
        match event {
            Event::WindowEvent { event, window_id } if window_id == window.id() => match event {
                WindowEvent::CloseRequested => elwt.exit(),
                WindowEvent::Resized(physical_size) => {
                    let _ = renderer.resize(physical_size.width, physical_size.height);
                }
                WindowEvent::DroppedFile(path) => {
                    let path_str = path.to_string_lossy().to_string();
                    println!("Dropped file: {}", path_str);
                    if let Some(new_source) = get_image_source(&path_str) {
                        println!("Source created: {} files/entries", new_source.len());
                        app_state.image_files = vec!["".to_string(); new_source.len()];
                        app_state.current_page_index = 0;
                        current_bitmaps.clear();
                        current_path_key = path_str.clone();
                        
                        rt.block_on(loader.send_request(LoaderRequest::SetSource { 
                            source: new_source, 
                            path_key: path_str 
                        }));
                        request_pages_with_prefetch(&app_state, &loader, &rt);
                        window.request_redraw();
                    }
                }
                WindowEvent::ModifiersChanged(new_modifiers) => {
                    modifiers = new_modifiers.state();
                }
                WindowEvent::KeyboardInput { event: KeyEvent { physical_key: PhysicalKey::Code(code), state: ElementState::Pressed, .. }, .. } => {
                    match code {
                        KeyCode::ArrowRight | KeyCode::ArrowLeft => {
                            let direction = if code == KeyCode::ArrowRight { 1 } else { -1 };
                            if modifiers.shift_key() {
                                app_state.navigate(direction * 10);
                            } else if modifiers.control_key() {
                                let new_idx = (app_state.current_page_index as isize + direction as isize).clamp(0, (app_state.image_files.len() as isize - 1).max(0)) as usize;
                                app_state.current_page_index = new_idx;
                            } else {
                                app_state.navigate(direction);
                            }
                            view_state.reset();
                            request_pages_with_prefetch(&app_state, &loader, &rt);
                        },
                        KeyCode::KeyB => {
                            if !app_state.is_spread_view {
                                app_state.is_spread_view = true;
                                app_state.binding_direction = BindingDirection::Right;
                            } else if app_state.binding_direction == BindingDirection::Right {
                                app_state.binding_direction = BindingDirection::Left;
                            } else {
                                app_state.is_spread_view = false;
                            }
                            view_state.reset();
                            request_pages_with_prefetch(&app_state, &loader, &rt);
                        },
                        KeyCode::NumpadMultiply => {
                            view_state.reset();
                        }
                        _ => (),
                    }
                    window.request_redraw();
                }
                WindowEvent::CursorMoved { position, .. } => {
                    let pos = (position.x as f32, position.y as f32);
                    if view_state.is_panning || view_state.is_loupe {
                        view_state.pan_offset.0 += pos.0 - view_state.last_mouse_pos.0;
                        view_state.pan_offset.1 += pos.1 - view_state.last_mouse_pos.1;
                    }
                    view_state.last_mouse_pos = pos;
                    view_state.cursor_pos = pos;
                    window.request_redraw();
                }
                WindowEvent::MouseInput { state, button, .. } => {
                    match button {
                        MouseButton::Left => {
                            if state == ElementState::Pressed {
                                if view_state.zoom_level > 1.0 {
                                    view_state.is_panning = true;
                                }
                            } else {
                                view_state.is_panning = false;
                            }
                        }
                        MouseButton::Right => {
                            if state == ElementState::Pressed {
                                view_state.is_loupe = true;
                                view_state.loupe_base_zoom = view_state.zoom_level;
                                view_state.loupe_base_pan = view_state.pan_offset;
                                view_state.set_zoom(view_state.zoom_level * 2.0, view_state.cursor_pos);
                            } else {
                                if view_state.is_loupe {
                                    view_state.zoom_level = view_state.loupe_base_zoom;
                                    view_state.pan_offset = view_state.loupe_base_pan;
                                    view_state.is_loupe = false;
                                }
                            }
                        }
                        _ => (),
                    }
                    window.request_redraw();
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    let scroll = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y,
                        MouseScrollDelta::PixelDelta(pos) => (pos.y / 120.0) as f32,
                    };
                    
                    if scroll.abs() > 0.1 {
                        let direction = if scroll > 0.0 { -1 } else { 1 };
                        app_state.navigate(direction);
                        view_state.reset();
                        request_pages_with_prefetch(&app_state, &loader, &rt);
                        window.request_redraw();
                    }
                }
                WindowEvent::RedrawRequested => {
                    // 非同期レスポンスのチェック
                    while let Some(res) = loader.try_recv_response() {
                        match res {
                            LoaderResponse::Loaded { index, .. } => {
                                if app_state.get_page_indices_to_display().contains(&index) {
                                    window.request_redraw();
                                }
                            }
                        }
                    }

                    let indices = app_state.get_page_indices_to_display();
                    
                    // GPU キャッシュの更新
                    {
                        let mut cache = cpu_cache.lock().unwrap();
                        for &idx in &indices {
                            if !current_bitmaps.iter().any(|(i, _)| *i == idx) {
                                let key = format!("{}::{}", current_path_key, idx);
                                if let Some(decoded) = cache.get(&key) {
                                    if let Ok(bitmap) = renderer.create_bitmap(decoded.width, decoded.height, &decoded.data) {
                                        current_bitmaps.push((idx, bitmap));
                                    }
                                }
                            }
                        }
                    }

                    // 描画
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

                            if total_content_w > 0.0 && max_content_h > 0.0 {
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
                    }

                    let _ = renderer.end_draw();
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

fn request_pages_with_prefetch(app_state: &AppState, loader: &AsyncLoader, rt: &Runtime) {
    let display_indices = app_state.get_page_indices_to_display();
    let max_idx = app_state.image_files.len() as isize - 1;
    if max_idx < 0 { return; }

    // 1. 表示対象 (Priority 0)
    for &idx in &display_indices {
        rt.block_on(loader.send_request(LoaderRequest::Load { index: idx, priority: 0 }));
    }

    // 2. 先読み (Priority 1)
    let prefetch_range = 5;
    let mut prefetch_indices = Vec::new();
    for &idx in &display_indices {
        for i in 1..=prefetch_range {
            let next = idx as isize + i;
            let prev = idx as isize - i;
            if next <= max_idx { prefetch_indices.push(next as usize); }
            if prev >= 0 { prefetch_indices.push(prev as usize); }
        }
    }
    
    // 重複と表示対象を除外して送信
    prefetch_indices.sort();
    prefetch_indices.dedup();
    for idx in prefetch_indices {
        if !display_indices.contains(&idx) {
            rt.block_on(loader.send_request(LoaderRequest::Load { index: idx, priority: 1 }));
        }
    }
}
