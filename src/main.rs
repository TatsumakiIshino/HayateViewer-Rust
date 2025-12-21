#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod config;
mod render;
mod image;
mod state;

const VERSION: &str = env!("CARGO_PKG_VERSION");

use crate::config::Settings;
use crate::render::d2d::{D2DRenderer, Renderer};
use crate::image::{get_image_source, ImageSource};
use crate::image::cache::{create_shared_cache, SharedImageCache};
use crate::image::loader::{AsyncLoader, LoaderRequest, UserEvent};
use crate::state::{AppState, BindingDirection};
use std::sync::Arc;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_COLOR_F};
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap1;
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
};
use winit::{
    event::{Event, WindowEvent, ElementState, MouseButton, MouseScrollDelta, KeyEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::WindowBuilder,
    keyboard::{PhysicalKey, KeyCode, ModifiersState, Key, NamedKey},
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

    fn set_zoom(&mut self, new_zoom: f32, center: (f32, f32), window_size: (f32, f32)) {
        let old_zoom = self.zoom_level;
        if (new_zoom - old_zoom).abs() < 1e-4 { return; }

        self.zoom_level = new_zoom.clamp(0.1, 50.0);
        let actual_factor = self.zoom_level / old_zoom;

        // 指定した座標 (center) がズーム前後で同じウィンドウ位置に留まるようにパンを調整
        // P_win = (win_w / 2) + pan + x_rel * zoom
        // pan_new = pan_old + (P_win - win_w / 2 - pan_old) * (1 - actual_factor)
        self.pan_offset.0 += (center.0 - window_size.0 / 2.0 - self.pan_offset.0) * (1.0 - actual_factor);
        self.pan_offset.1 += (center.1 - window_size.1 / 2.0 - self.pan_offset.1) * (1.0 - actual_factor);
    }

    fn clamp_pan_offset(&mut self, window_size: (f32, f32), content_size: (f32, f32)) {
        if self.is_loupe {
            // ルーペ中はマウス位置を保持するために制限を緩める
            return;
        }
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
    let mut settings = Settings::load_or_default(config_path);
    if !std::path::Path::new(config_path).exists() { let _ = settings.save(config_path); }

    // Tokio Runtime
    let rt = Runtime::new()?;
    let _guard = rt.enter();

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();

    let window = Arc::new(WindowBuilder::new()
        .with_title(format!("HayateViewer Rust v{}", VERSION))
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
    use std::io::Write;
    let _ = std::io::stdout().flush();

    let mut renderer = D2DRenderer::new(hwnd)?;
    let mut view_state = ViewState::new();
    let mut app_state = AppState::new();
    let mut current_path_key = String::new();

    app_state.is_spread_view = settings.is_spread_view;
    app_state.binding_direction = if settings.binding_direction == "right" { BindingDirection::Right } else { BindingDirection::Left };
    app_state.spread_view_first_page_single = settings.spread_view_first_page_single;

    // Cache & Loader
    let cache_capacity = (settings.max_cache_size_mb / 20).max(50) as usize; // 1枚 20MB と仮定して概算、最低50枚
    let cpu_cache = create_shared_cache(cache_capacity);
    let loader = AsyncLoader::new(cpu_cache.clone(), proxy);

    {
        use windows::Win32::Graphics::Direct2D::D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC;
        use windows::Win32::Graphics::Direct2D::D2D1_INTERPOLATION_MODE_LINEAR;
        use windows::Win32::Graphics::Direct2D::D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR;
        use windows::Win32::Graphics::Direct2D::D2D1_INTERPOLATION_MODE_CUBIC;

        let mode = match settings.resampling_mode_dx.as_str() {
            "DX_NEAREST" => D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
            "DX_LINEAR" => D2D1_INTERPOLATION_MODE_LINEAR,
            "DX_CUBIC" => D2D1_INTERPOLATION_MODE_CUBIC,
            "DX_HQC" => D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
            _ => D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
        };
        renderer.set_interpolation_mode(mode);
    }

    // 初期パスの読み込み
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        if let Some(src) = get_image_source(&args[1]) {
            if let ImageSource::Files(ref files) = src {
                app_state.image_files = files.clone();
            } else if let ImageSource::Archive(ref loader) = src {
                app_state.image_files = loader.get_file_names().to_vec();
            }
            current_path_key = args[1].clone();
            update_window_title(&window, &current_path_key, &app_state);
            rt.block_on(loader.send_request(LoaderRequest::Clear));
            rt.block_on(loader.send_request(LoaderRequest::SetSource { 
                source: src, 
                path_key: current_path_key.clone() 
            }));
            request_pages_with_prefetch(&app_state, &loader, &rt, &cpu_cache, &settings, &current_path_key);
        }
    }

    let mut current_bitmaps: Vec<(usize, ID2D1Bitmap1)> = Vec::new();
    let mut modifiers = ModifiersState::default();

    event_loop.run(move |event: Event<UserEvent>, elwt| {
        elwt.set_control_flow(ControlFlow::Wait);
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
                        if let ImageSource::Files(ref files) = new_source {
                            app_state.image_files = files.clone();
                        } else if let ImageSource::Archive(ref loader) = new_source {
                            app_state.image_files = loader.get_file_names().to_vec();
                        }
                        app_state.current_page_index = 0;
                        current_bitmaps.clear();
                        current_path_key = path_str.clone();
                        update_window_title(&window, &current_path_key, &app_state);
                        
                        rt.block_on(loader.send_request(LoaderRequest::Clear));
                        rt.block_on(loader.send_request(LoaderRequest::SetSource { 
                            source: new_source, 
                            path_key: path_str 
                        }));
                        request_pages_with_prefetch(&app_state, &loader, &rt, &cpu_cache, &settings, &current_path_key);
                        window.request_redraw();
                    }
                }
                WindowEvent::ModifiersChanged(new_modifiers) => {
                    modifiers = new_modifiers.state();
                }
                WindowEvent::KeyboardInput { 
                    event: KeyEvent { 
                        logical_key, 
                        physical_key,
                        state: ElementState::Pressed, 
                        .. 
                    }, .. 
                } => {
                    if app_state.is_jump_open {
                        match logical_key {
                            Key::Character(ref s) if s.chars().all(|c| c.is_ascii_digit()) => {
                                if app_state.jump_input_buffer.len() < 5 {
                                    app_state.jump_input_buffer.push_str(s.as_str());
                                }
                            }
                            Key::Named(NamedKey::Backspace) => {
                                app_state.jump_input_buffer.pop();
                            }
                            Key::Named(NamedKey::Enter) => {
                                if let Ok(page_num) = app_state.jump_input_buffer.parse::<usize>() {
                                    if page_num > 0 && page_num <= app_state.image_files.len() {
                                        app_state.current_page_index = page_num - 1;
                                        view_state.reset();
                                        request_pages_with_prefetch(&app_state, &loader, &rt, &cpu_cache, &settings, &current_path_key);
                                    }
                                }
                                app_state.is_jump_open = false;
                                app_state.jump_input_buffer.clear();
                            }
                            Key::Named(NamedKey::Escape) => {
                                app_state.is_jump_open = false;
                                app_state.jump_input_buffer.clear();
                            }
                            _ => (),
                        }
                        window.request_redraw();
                        return;
                    }

                    match logical_key {
                        Key::Character(ref s) if s.to_lowercase() == "s" => {
                            if modifiers.shift_key() {
                                // Shift + S: ページジャンプを開く
                                app_state.is_jump_open = true;
                                app_state.jump_input_buffer.clear();
                                app_state.is_options_open = false;
                            } else {
                                // S: シークバー切り替え
                                app_state.show_seekbar = !app_state.show_seekbar;
                            }
                            window.request_redraw();
                        }
                        Key::Named(NamedKey::ArrowUp) | Key::Named(NamedKey::ArrowDown) => {
                            if app_state.is_options_open {
                                let total_options = 7;
                                if logical_key == Key::Named(NamedKey::ArrowUp) {
                                    app_state.options_selected_index = (app_state.options_selected_index + total_options - 1) % total_options;
                                } else {
                                    app_state.options_selected_index = (app_state.options_selected_index + 1) % total_options;
                                }
                            }
                        }
                        Key::Named(NamedKey::ArrowRight) | Key::Named(NamedKey::ArrowLeft) => {
                            if app_state.is_options_open {
                                let direction = if logical_key == Key::Named(NamedKey::ArrowRight) { 1 } else { -1 };
                                match app_state.options_selected_index {
                                    1 => app_state.is_spread_view = !app_state.is_spread_view,
                                    2 => app_state.binding_direction = if app_state.binding_direction == BindingDirection::Left { BindingDirection::Right } else { BindingDirection::Left },
                                    3 => {
                                        let modes = ["DX_NEAREST", "DX_LINEAR", "DX_CUBIC", "DX_HQC"];
                                        let current_idx = modes.iter().position(|&m| m == settings.resampling_mode_dx).unwrap_or(3);
                                        let new_idx = (current_idx as isize + direction as isize).rem_euclid(modes.len() as isize) as usize;
                                        
                                        settings.resampling_mode_dx = modes[new_idx].to_string();
                                        
                                        // レンダラーに即時反映
                                        use windows::Win32::Graphics::Direct2D::*;
                                        let d2d_mode = match modes[new_idx] {
                                            "DX_NEAREST" => D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                                            "DX_LINEAR" => D2D1_INTERPOLATION_MODE_LINEAR,
                                            "DX_CUBIC" => D2D1_INTERPOLATION_MODE_CUBIC,
                                            _ => D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                                        };
                                        renderer.set_interpolation_mode(d2d_mode);
                                        let _ = settings.save("config.json");
                                    }
                                    4 => {
                                        if direction > 0 { settings.max_cache_size_mb += 512; }
                                        else { settings.max_cache_size_mb = settings.max_cache_size_mb.saturating_sub(512); }
                                        let _ = settings.save("config.json");
                                    }
                                    5 => {
                                        if direction > 0 { settings.cpu_max_prefetch_pages += 1; }
                                        else { settings.cpu_max_prefetch_pages = settings.cpu_max_prefetch_pages.saturating_sub(1); }
                                        let _ = settings.save("config.json");
                                    }
                                    6 => {
                                        settings.show_status_bar_info = !settings.show_status_bar_info;
                                        let _ = settings.save("config.json");
                                    }
                                    _ => (),
                                }
                            } else {
                                // ページ移動
                                let direction = if logical_key == Key::Named(NamedKey::ArrowRight) { 1 } else { -1 };
                                if modifiers.shift_key() {
                                    app_state.navigate(direction * 10);
                                } else if modifiers.control_key() {
                                    let new_idx = (app_state.current_page_index as isize + direction as isize).clamp(0, (app_state.image_files.len() as isize - 1).max(0)) as usize;
                                    app_state.current_page_index = new_idx;
                                } else {
                                    app_state.navigate(direction);
                                }
                                view_state.reset();
                                request_pages_with_prefetch(&app_state, &loader, &rt, &cpu_cache, &settings, &current_path_key);
                            }
                        }
                        Key::Character(ref s) if s.to_lowercase() == "b" => {
                            if !app_state.is_options_open {
                                if !app_state.is_spread_view {
                                    app_state.is_spread_view = true;
                                    app_state.binding_direction = BindingDirection::Right;
                                } else if app_state.binding_direction == BindingDirection::Right {
                                    app_state.binding_direction = BindingDirection::Left;
                                } else {
                                    app_state.is_spread_view = false;
                                }
                                view_state.reset();
                                request_pages_with_prefetch(&app_state, &loader, &rt, &cpu_cache, &settings, &current_path_key);
                            }
                        }
                        Key::Character(ref s) if s.to_lowercase() == "o" => {
                            app_state.is_options_open = !app_state.is_options_open;
                            window.request_redraw();
                        }
                        Key::Character(ref s) if s == "[" || s == "]" => {
                            if !app_state.is_options_open && !app_state.is_jump_open {
                                let direction = if s == "]" { 1 } else { -1 };
                                if let Some(new_path) = get_neighboring_source(&current_path_key, direction) {
                                    println!("Navigating to folder: {}", new_path);
                                    if let Some(new_source) = get_image_source(&new_path) {
                                        if let ImageSource::Files(ref files) = new_source {
                                            app_state.image_files = files.clone();
                                        } else if let ImageSource::Archive(ref loader) = new_source {
                                            app_state.image_files = loader.get_file_names().to_vec();
                                        }
                                        app_state.current_page_index = 0;
                                        current_bitmaps.clear();
                                        current_path_key = new_path.clone();
                                        update_window_title(&window, &current_path_key, &app_state);
                                        
                                        rt.block_on(loader.send_request(LoaderRequest::Clear));
                                        rt.block_on(loader.send_request(LoaderRequest::SetSource { 
                                            source: new_source, 
                                            path_key: new_path 
                                        }));
                                        request_pages_with_prefetch(&app_state, &loader, &rt, &cpu_cache, &settings, &current_path_key);
                                    }
                                }
                            }
                        }
                        Key::Named(NamedKey::Escape) => {
                            if app_state.is_options_open {
                                app_state.is_options_open = false;
                                window.request_redraw();
                            }
                        }
                        _ => {
                            if let PhysicalKey::Code(code) = physical_key {
                                match code {
                                    KeyCode::NumpadMultiply => {
                                        view_state.reset();
                                    }
                                    _ => (),
                                }
                            }
                        }
                    }
                    window.request_redraw();
                }
                WindowEvent::CursorMoved { position, .. } => {
                    let pos = (position.x as f32, position.y as f32);
                    let window_size = window.inner_size();
                    let win_w = window_size.width as f32;

                    // シークバーのドラッグ処理
                    if app_state.is_dragging_seekbar && !app_state.image_files.is_empty() {
                        let progress = (pos.0 / win_w).clamp(0.0, 1.0);
                        let total_pages = app_state.image_files.len();
                        let target_progress = if app_state.binding_direction == BindingDirection::Right {
                            1.0 - progress
                        } else {
                            progress
                        };
                        let idx = (target_progress * (total_pages - 1) as f32).round() as usize;
                        let new_idx = app_state.snap_to_spread(idx);
                        if new_idx != app_state.current_page_index {
                            app_state.current_page_index = new_idx;
                            view_state.reset();
                            request_pages_with_prefetch(&app_state, &loader, &rt, &cpu_cache, &settings, &current_path_key);
                        }
                    }

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
                                let window_size = window.inner_size();
                                let win_h = window_size.height as f32;
                                let bar_h = 25.0;
                                let seek_bar_h = 8.0;
                                let bar_y = if settings.show_status_bar_info { win_h - bar_h - seek_bar_h } else { win_h - seek_bar_h };

                                // シークバークリック判定
                                if app_state.show_seekbar && view_state.cursor_pos.1 >= bar_y && view_state.cursor_pos.1 <= bar_y + seek_bar_h {
                                    app_state.is_dragging_seekbar = true;
                                    // 即座に位置を反映させるために CursorMoved と同じロジックを実行
                                    let win_w = window_size.width as f32;
                                    let progress = (view_state.cursor_pos.0 / win_w).clamp(0.0, 1.0);
                                    let total_pages = app_state.image_files.len();
                                    if total_pages > 0 {
                                        let target_progress = if app_state.binding_direction == BindingDirection::Right {
                                            1.0 - progress
                                        } else {
                                            progress
                                        };
                                        let idx = (target_progress * (total_pages - 1) as f32).round() as usize;
                                        app_state.current_page_index = app_state.snap_to_spread(idx);
                                        view_state.reset();
                                        request_pages_with_prefetch(&app_state, &loader, &rt, &cpu_cache, &settings, &current_path_key);
                                    }
                                } else if view_state.zoom_level > 1.0 {
                                    view_state.is_panning = true;
                                }
                            } else {
                                view_state.is_panning = false;
                                app_state.is_dragging_seekbar = false;
                            }
                        }
                        MouseButton::Right => {
                            if state == ElementState::Pressed {
                                view_state.is_loupe = true;
                                view_state.loupe_base_zoom = view_state.zoom_level;
                                view_state.loupe_base_pan = view_state.pan_offset;

                                let window_size = window.inner_size();
                                let win_size = (window_size.width as f32, window_size.height as f32);
                                view_state.set_zoom(view_state.zoom_level * 2.0, view_state.cursor_pos, win_size);
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
                        current_bitmaps.clear();
                        view_state.reset();
                        request_pages_with_prefetch(&app_state, &loader, &rt, &cpu_cache, &settings, &current_path_key);
                        window.request_redraw();
                    }
                }
                WindowEvent::RedrawRequested => {
                    // 非同期レスポンスのチェック
                    while let Some(_) = loader.try_recv_response() {
                        window.request_redraw();
                    }

                    let window_size = window.inner_size();
                    let win_w = window_size.width as f32;
                    let win_h = window_size.height as f32;

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

                    // ステータスバーの描画
                    let bar_h = 25.0;
                    let bar_rect = D2D_RECT_F {
                        left: 0.0,
                        top: win_h - bar_h,
                        right: win_w,
                        bottom: win_h,
                    };
                    if settings.show_status_bar_info {
                        renderer.fill_rectangle(&bar_rect, &D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.5 });
                    }

                    let total_pages = app_state.image_files.len();
                    let current_page = app_state.current_page_index + 1;
                    
                    let cpu_indices: Vec<usize> = {
                        let keys = cpu_cache.lock().unwrap().get_keys();
                        keys.iter().filter_map(|k| k.rsplit("::").next()?.parse().ok()).collect()
                    };
                    let gpu_indices: Vec<usize> = current_bitmaps.iter().map(|(idx, _)| *idx).collect();

                    let path_preview: String = current_path_key.chars().take(20).collect();
                    let status_text = format!(
                        " Page: {} / {} | Backend: Direct2D | CPU: {}p {} | GPU: {}p {} | Key: {}",
                        current_page,
                        total_pages,
                        cpu_indices.len(),
                        format_page_list(&cpu_indices),
                        gpu_indices.len(),
                        format_page_list(&gpu_indices),
                        path_preview
                    );
                    if settings.show_status_bar_info {
                        renderer.draw_text(&status_text, &bar_rect, &D2D1_COLOR_F { r: 0.9, g: 0.9, b: 0.9, a: 1.0 });
                    }

                    update_window_title(&window, &current_path_key, &app_state);

                    // ページジャンプオーバーレイの描画
                    if app_state.is_jump_open {
                        let jump_w = 340.0;
                        let jump_h = 160.0;
                        let jump_rect = D2D_RECT_F {
                            left: (win_w - jump_w) / 2.0,
                            top: (win_h - jump_h) / 2.0,
                            right: (win_w + jump_w) / 2.0,
                            bottom: (win_h + jump_h) / 2.0,
                        };
                        
                        // メインパネル
                        renderer.fill_rounded_rectangle(&jump_rect, 12.0, &D2D1_COLOR_F { r: 0.05, g: 0.05, b: 0.05, a: 0.95 });
                        renderer.draw_rectangle(&jump_rect, &D2D1_COLOR_F { r: 0.3, g: 0.3, b: 0.3, a: 1.0 }, 1.0);

                        renderer.set_text_alignment(DWRITE_TEXT_ALIGNMENT_CENTER);
                        
                        // タイトルラベル
                        let mut title_rect = jump_rect.clone();
                        title_rect.top += 15.0;
                        title_rect.bottom = title_rect.top + 30.0;
                        renderer.draw_text("ページ指定 (Enterで確定)", &title_rect, &D2D1_COLOR_F { r: 0.6, g: 0.6, b: 0.6, a: 1.0 });

                        // 入力エリア背景（サブパネル）
                        let input_bg_w = 280.0;
                        let input_bg_h = 60.0;
                        let input_bg_rect = D2D_RECT_F {
                            left: (win_w - input_bg_w) / 2.0,
                            top: jump_rect.top + 55.0,
                            right: (win_w + input_bg_w) / 2.0,
                            bottom: jump_rect.top + 55.0 + input_bg_h,
                        };
                        renderer.fill_rounded_rectangle(&input_bg_rect, 6.0, &D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.6 });

                        // 入力中の文字と合計を一つの文字列として中央揃えで描画
                        let input_val = if app_state.jump_input_buffer.is_empty() { "---" } else { &app_state.jump_input_buffer };
                        let cursor = if (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() / 500) % 2 == 0 { "|" } else { " " };
                        
                        // カーソルを入力値の直後に表示したいため、文字列を工夫
                        let full_text = if app_state.jump_input_buffer.is_empty() {
                            format!("{} / {}", input_val, total_pages) // 空の時はカーソル無しでも良いが、一応
                        } else {
                            format!("{}{} / {}", app_state.jump_input_buffer, cursor, total_pages)
                        };

                        renderer.set_text_alignment(DWRITE_TEXT_ALIGNMENT_CENTER);
                        renderer.draw_text_large(&full_text, &input_bg_rect, &D2D1_COLOR_F { r: 1.0, g: 0.8, b: 0.0, a: 1.0 });

                        renderer.set_text_alignment(DWRITE_TEXT_ALIGNMENT_LEADING);
                    }

                    // シークバーの描画
                    if app_state.show_seekbar && total_pages > 0 {
                        let bar_height = if app_state.is_dragging_seekbar { 12.0 } else { 8.0 };
                        let bar_y = if settings.show_status_bar_info { win_h - bar_h - bar_height } else { win_h - bar_height };
                        let full_rect = D2D_RECT_F {
                            left: 0.0,
                            top: bar_y,
                            right: win_w,
                            bottom: bar_y + bar_height,
                        };
                        renderer.fill_rectangle(&full_rect, &D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.5 });

                        let progress = (app_state.current_page_index as f32) / ((total_pages - 1) as f32).max(1.0);
                        let progress_rect = if app_state.binding_direction == BindingDirection::Right {
                            D2D_RECT_F {
                                left: win_w * (1.0 - progress),
                                top: bar_y,
                                right: win_w,
                                bottom: bar_y + bar_height,
                            }
                        } else {
                            D2D_RECT_F {
                                left: 0.0,
                                top: bar_y,
                                right: win_w * progress,
                                bottom: bar_y + bar_height,
                            }
                        };
                        let bar_color = if app_state.is_dragging_seekbar {
                            D2D1_COLOR_F { r: 0.0, g: 0.6, b: 1.0, a: 1.0 }
                        } else {
                            D2D1_COLOR_F { r: 0.0, g: 0.4, b: 0.8, a: 0.9 }
                        };
                        renderer.fill_rounded_rectangle(&progress_rect, 4.0, &bar_color);
                    }

                    // 設定オーバーレイの描画
                    if app_state.is_options_open {
                        let overlay_w = 400.0;
                        let overlay_h = 450.0;
                        let overlay_rect = D2D_RECT_F {
                            left: (win_w - overlay_w) / 2.0,
                            top: (win_h - overlay_h) / 2.0,
                            right: (win_w + overlay_w) / 2.0,
                            bottom: (win_h + overlay_h) / 2.0,
                        };

                        // 背景（半透明の黒）
                        renderer.fill_rectangle(&overlay_rect, &D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.8 });
                        
                        let mut text_rect = D2D_RECT_F {
                            left: overlay_rect.left + 20.0,
                            top: overlay_rect.top + 20.0,
                            right: overlay_rect.right - 20.0,
                            bottom: overlay_rect.top + 50.0,
                        };

                        renderer.draw_text("--- 設定 ---", &text_rect, &D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 });
                        text_rect.top += 40.0;
                        text_rect.bottom += 40.0;

                        let options = [
                            ("レンダリングエンジン", settings.rendering_backend.as_str()),
                            ("見開き表示", if app_state.is_spread_view { "オン" } else { "オフ" }),
                            ("綴じ方向", if app_state.binding_direction == BindingDirection::Right { "右綴じ" } else { "左綴じ" }),
                            ("補間モード (DX)", settings.resampling_mode_dx.as_str()),
                            ("最大キャッシュ容量", &format!("{} MB", settings.max_cache_size_mb)),
                            ("CPU 先読み数", &format!("{} ページ", settings.cpu_max_prefetch_pages)),
                            ("ステータスバー", if settings.show_status_bar_info { "表示" } else { "非表示" }),
                        ];

                        for (i, (label, value)) in options.iter().enumerate() {
                            let is_selected = i == app_state.options_selected_index;
                            let color = if is_selected {
                                D2D1_COLOR_F { r: 0.2, g: 0.6, b: 1.0, a: 1.0 }
                            } else {
                                D2D1_COLOR_F { r: 0.8, g: 0.8, b: 0.8, a: 1.0 }
                            };

                            if is_selected {
                                let sel_rect = D2D_RECT_F {
                                    left: overlay_rect.left + 10.0,
                                    top: text_rect.top - 2.0,
                                    right: overlay_rect.right - 10.0,
                                    bottom: text_rect.bottom + 2.0,
                                };
                                renderer.fill_rectangle(&sel_rect, &D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 0.2 });
                            }

                            let display_text = format!("{}: {}", label, value);
                            renderer.draw_text(&display_text, &text_rect, &color);
                            
                            text_rect.top += 35.0;
                            text_rect.bottom += 35.0;
                        }

                        let hint_rect = D2D_RECT_F {
                            left: overlay_rect.left + 20.0,
                            top: overlay_rect.bottom - 40.0,
                            right: overlay_rect.right - 20.0,
                            bottom: overlay_rect.bottom - 10.0,
                        };
                        renderer.draw_text("矢印キーで変更、'O'キーで閉じる", &hint_rect, &D2D1_COLOR_F { r: 0.5, g: 0.5, b: 0.5, a: 1.0 });
                    }

                    let _ = renderer.end_draw();
                }
                _ => (),
            },
            Event::UserEvent(user_event) => {
                match user_event {
                    UserEvent::PageLoaded(_) => {
                        window.request_redraw();
                    }
                }
            }
            Event::AboutToWait => {
                window.request_redraw();
            }
            _ => (),
        }
    })?;

    Ok(())
}

fn update_window_title(window: &winit::window::Window, path_key: &str, app_state: &AppState) {
    if path_key.is_empty() {
        window.set_title(&format!("HayateViewer Rust v{}", VERSION));
        return;
    }

    let base_name = std::path::Path::new(path_key)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path_key.to_string());

    let indices = app_state.get_page_indices_to_display();
    let mut names = Vec::new();
    for &idx in &indices {
        if idx < app_state.image_files.len() {
            let name = std::path::Path::new(&app_state.image_files[idx])
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| format!("Page {}", idx + 1));
            if !name.is_empty() {
                names.push(name);
            }
        }
    }

    if names.is_empty() {
        window.set_title(&format!("{} - HayateViewer Rust v{}", base_name, VERSION));
    } else {
        window.set_title(&format!("{} - {} - HayateViewer Rust v{}", base_name, names.join(" / "), VERSION));
    }
}

fn request_pages_with_prefetch(app_state: &AppState, loader: &AsyncLoader, rt: &Runtime, cpu_cache: &SharedImageCache, settings: &Settings, path_key: &str) {
    let display_indices = app_state.get_page_indices_to_display();
    let max_idx = app_state.image_files.len() as isize - 1;
    if max_idx < 0 { return; }

    let loader_tx = loader.clone_tx();

    // 1. 表示対象の即時リクエスト (Priority 0)
    for &idx in &display_indices {
        let key = format!("{}::{}", path_key, idx);
        let cached = cpu_cache.lock().unwrap().get(&key).is_some();
        if !cached {
            println!("[Prefetch] Requesting immediate load for index {}", idx);
            let l = loader_tx.clone();
            rt.spawn(async move {
                let _ = l.send(LoaderRequest::Load { index: idx, priority: 0 }).await;
            });
        }
    }

    // 2. 先読み範囲の計算と「歯抜け」補充 (Priority 1)
    let prefetch_dist = settings.cpu_max_prefetch_pages;
    let mut targets = std::collections::HashSet::new();
    
    for &idx in &display_indices {
        let start = (idx as isize - prefetch_dist as isize).max(0) as usize;
        let end = (idx as isize + prefetch_dist as isize).min(max_idx) as usize;
        for i in start..=end {
            if !display_indices.contains(&i) {
                targets.insert(i);
            }
        }
    }

    let mut targets_vec: Vec<_> = targets.into_iter().collect();
    // 現在のページに近い順にソート（効率的な補充のため）
    let current = app_state.current_page_index as isize;
    targets_vec.sort_by_key(|&idx| (idx as isize - current).abs());
    
    if !targets_vec.is_empty() {
        println!("[Prefetch] Gap filling targets: {:?}", targets_vec);
    }

    for idx in targets_vec {
        let key = format!("{}::{}", path_key, idx);
        let cached = {
            let mut c = cpu_cache.lock().unwrap();
            c.get(&key).is_some()
        };
        
        if !cached {
            let l = loader_tx.clone();
            rt.spawn(async move {
                let _ = l.send(LoaderRequest::Load { index: idx, priority: 1 }).await;
            });
        }
    }
}

fn format_page_list(indices: &[usize]) -> String {
    if indices.is_empty() {
        return "[]".to_string();
    }
    let mut sorted = indices.to_vec();
    sorted.sort();
    
    if sorted.len() > 5 {
        format!("[{}, {}, {}, ..., {}]", sorted[0] + 1, sorted[1] + 1, sorted[2] + 1, sorted.last().unwrap() + 1)
    } else {
        format!("{:?}", sorted.iter().map(|i| i + 1).collect::<Vec<_>>())
    }
}

fn get_neighboring_source(current_path: &str, direction: isize) -> Option<String> {
    let path = std::path::Path::new(current_path);
    let parent = path.parent()?;
    
    let mut entries = Vec::new();
    let supported_archives = ["zip", "7z", "cbz"];
    
    if let Ok(dir) = std::fs::read_dir(parent) {
        for entry in dir.flatten() {
            let p = entry.path();
            if p.is_dir() {
                entries.push(p);
            } else if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
                if supported_archives.contains(&ext.to_lowercase().as_str()) {
                    entries.push(p);
                }
            }
        }
    }
    
    if entries.is_empty() { return None; }
    
    entries.sort_by(|a, b| natord::compare(&a.to_string_lossy(), &b.to_string_lossy()));
    
    let current_abs = std::fs::canonicalize(path).ok()?;
    let current_idx = entries.iter().position(|e| {
        std::fs::canonicalize(e).map(|abs| abs == current_abs).unwrap_or(false)
    });

    if let Some(idx) = current_idx {
        let next_idx = idx as isize + direction;
        if next_idx >= 0 && next_idx < entries.len() as isize {
            return Some(entries[next_idx as usize].to_string_lossy().to_string());
        }
    }
    
    None
}
