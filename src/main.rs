#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod config;
mod render;
mod image;
mod state;

const VERSION: &str = env!("CARGO_PKG_VERSION");

use crate::config::Settings;
use crate::render::{Renderer, InterpolationMode};
use crate::render::d2d::D2DRenderer;
use crate::image::{get_image_source, ImageSource};
use crate::image::cache::{create_shared_cache, SharedImageCache};
use crate::image::loader::{AsyncLoader, LoaderRequest, UserEvent};
use crate::state::{AppState, BindingDirection};
use std::sync::Arc;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_COLOR_F, D2D_SIZE_F};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
};
use winit::{
    event::{Event, WindowEvent, ElementState, MouseButton, MouseScrollDelta, KeyEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::WindowBuilder,
    keyboard::{PhysicalKey, KeyCode, ModifiersState, Key, NamedKey},
};
use raw_window_handle::{HasRawWindowHandle, RawWindowHandle};
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

    // コマンドライン引数のパース
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--threads") {
        if let Some(val) = args.get(pos + 1) {
            if let Ok(n) = val.parse::<usize>() {
                settings.parallel_decoding_workers = n;
                println!("[設定] スレッド数を引数から {} に設定しました", n);
            }
        }
    }

    // Rayon Global Thread Pool の初期化
    let num_threads = settings.parallel_decoding_workers;
    if num_threads > 0 {
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .build_global();
        println!("[設定] Rayon スレッドプールを {} スレッドで初期化しました", num_threads);
    }

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

    let hwnd = match window.raw_window_handle() {
        RawWindowHandle::Win32(handle) => HWND(handle.hwnd as _),
        _ => return Err("Unsupported window handle".into()),
    };

    unsafe {
        use windows::Win32::Graphics::Dwm::*;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &2i32 as *const _ as _, 4);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &1i32 as *const _ as _, 4);
    }

    println!("HayateViewer Rust を起動中...");
    use std::io::Write;
    let _ = std::io::stdout().flush();

    let mut renderer: Box<dyn Renderer> = match settings.rendering_backend.as_str() {
        "direct3d11" => {
            match crate::render::d3d11::D3D11Renderer::new(hwnd) {
                Ok(r) => Box::new(r),
                Err(e) => {
                    eprintln!("D3D11 レンダラーの初期化に失敗しました。D2D にフォールバックします: {:?}", e);
                    Box::new(D2DRenderer::new(hwnd)?)
                }
            }
        }
        "opengl" => {
            match init_opengl(&window) {
                Ok(r) => Box::new(r),
                Err(e) => {
                    eprintln!("OpenGL レンダラーの初期化に失敗しました。D3D11 にフォールバックします: {:?}", e);
                    match crate::render::d3d11::D3D11Renderer::new(hwnd) {
                        Ok(r) => Box::new(r),
                        Err(_) => Box::new(D2DRenderer::new(hwnd)?),
                    }
                }
            }
        }
        _ => Box::new(D2DRenderer::new(hwnd)?),
    };
    let mut view_state = ViewState::new();
    let mut app_state = AppState::new();
    let mut current_path_key = String::new();

    app_state.is_spread_view = settings.is_spread_view;
    app_state.binding_direction = if settings.binding_direction == "right" { BindingDirection::Right } else { BindingDirection::Left };
    app_state.spread_view_first_page_single = settings.spread_view_first_page_single;

    // Cache & Loader
    let max_bytes = (settings.max_cache_size_mb as usize) * 1024 * 1024;
    let cpu_cache = create_shared_cache(100, max_bytes);
    let loader = AsyncLoader::new(cpu_cache.clone(), proxy);

    {

        let mode = match settings.resampling_mode_dx.as_str() {
            "DX_NEAREST" => InterpolationMode::NearestNeighbor,
            "DX_LINEAR" => InterpolationMode::Linear,
            "DX_CUBIC" => InterpolationMode::Cubic,
            "DX_HQC" => InterpolationMode::HighQualityCubic,
            "DX_LANCZOS" => InterpolationMode::Lanczos,
            _ => InterpolationMode::HighQualityCubic,
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

    let mut current_bitmaps: Vec<(usize, crate::render::TextureHandle)> = Vec::new();
    let mut modifiers = ModifiersState::default();

    event_loop.run(move |event: Event<UserEvent>, elwt| {
        elwt.set_control_flow(ControlFlow::Wait);
        match event {
            Event::WindowEvent { event, window_id } if window_id == window.id() => match event {
                WindowEvent::CloseRequested => {
                    println!("終了リクエストを受信しました。終了します...");
                    elwt.exit();
                    // 非同期タスクがブロッキングしている場合に備え、プロセスを強制終了
                    std::process::exit(0);
                }
                WindowEvent::Resized(physical_size) => {
                    let _ = renderer.resize(physical_size.width, physical_size.height);
                }
                WindowEvent::DroppedFile(path) => {
                    let path_str = path.to_string_lossy().to_string();
                    println!("ファイルをドロップ: {}", path_str);
                    if let Some(new_source) = get_image_source(&path_str) {
                        println!("ソースを作成: {} 個のファイル/エントリ", new_source.len());
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
                                        let l = loader.clone();
                                        rt.spawn(async move { let _ = l.send_request(LoaderRequest::ClearPrefetch).await; });
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
                                let total_options = 10;
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
                                    0 => {
                                        let engines = ["direct2d", "direct3d11", "opengl"];
                                        let current_idx = engines.iter().position(|&e| e == settings.rendering_backend).unwrap_or(0);
                                        let new_idx = (current_idx as isize + direction as isize).rem_euclid(engines.len() as isize) as usize;
                                        settings.rendering_backend = engines[new_idx].to_string();
                                        println!("[設定] レンダリングエンジンを {} に変更しました (再起動後に反映)", get_backend_display_name(&settings.rendering_backend));
                                        let _ = settings.save("config.json");
                                    }
                                    1 => {
                                        app_state.is_spread_view = !app_state.is_spread_view;
                                        settings.is_spread_view = app_state.is_spread_view;
                                        let _ = settings.save("config.json");
                                    },
                                    2 => app_state.binding_direction = if app_state.binding_direction == BindingDirection::Left { BindingDirection::Right } else { BindingDirection::Left },
                                    3 => {
                                        let modes = ["DX_NEAREST", "DX_LINEAR", "DX_CUBIC", "DX_HQC", "DX_LANCZOS"];
                                        let current_idx = modes.iter().position(|&m| m == settings.resampling_mode_dx).unwrap_or(3);
                                        let new_idx = (current_idx as isize + direction as isize).rem_euclid(modes.len() as isize) as usize;
                                        
                                        settings.resampling_mode_dx = modes[new_idx].to_string();
                                        
                                        // レンダラーに即時反映
                                        let new_mode = match modes[new_idx] {
                                            "DX_NEAREST" => InterpolationMode::NearestNeighbor,
                                            "DX_LINEAR" => InterpolationMode::Linear,
                                            "DX_CUBIC" => InterpolationMode::Cubic,
                                            "DX_HQC" => InterpolationMode::HighQualityCubic,
                                            "DX_LANCZOS" => InterpolationMode::Lanczos,
                                            _ => InterpolationMode::HighQualityCubic,
                                        };
                                        renderer.set_interpolation_mode(new_mode);
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
                                        if direction > 0 { settings.gpu_max_prefetch_pages += 1; }
                                        else { settings.gpu_max_prefetch_pages = settings.gpu_max_prefetch_pages.saturating_sub(1); }
                                        let _ = settings.save("config.json");
                                    }
                                    7 => {
                                        settings.show_status_bar_info = !settings.show_status_bar_info;
                                        let _ = settings.save("config.json");
                                    }
                                    8 => {
                                        // デコードスレッド数
                                        if direction > 0 { settings.parallel_decoding_workers += 1; }
                                        else { settings.parallel_decoding_workers = settings.parallel_decoding_workers.saturating_sub(1); }
                                        let _ = settings.save("config.json");
                                    }
                                    9 => {
                                        settings.use_cpu_color_conversion = !settings.use_cpu_color_conversion;
                                        let _ = settings.save("config.json");
                                        // 設定変更時はキャッシュをクリアして再読込
                                        cpu_cache.lock().unwrap().clear();
                                        current_bitmaps.clear();
                                        request_pages_with_prefetch(&app_state, &loader, &rt, &cpu_cache, &settings, &current_path_key);
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
                                let l = loader.clone();
                                rt.spawn(async move { let _ = l.send_request(LoaderRequest::ClearPrefetch).await; });
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
                                    println!("フォルダ/アーカイブ移動: {}", new_path);
                                    if let Some(new_source) = get_image_source(&new_path) {
                                        if let ImageSource::Files(ref files) = new_source {
                                            app_state.image_files = files.clone();
                                        } else if let ImageSource::Archive(ref loader) = new_source {
                                            app_state.image_files = loader.get_file_names().to_vec();
                                        }
                                        app_state.current_page_index = 0;
                                        // フォルダ移動時は一旦クリアしても良いが、
                                        // シームレスな遷移を重視し ClearPrefetch に留める
                                        current_bitmaps.clear(); 
                                        current_path_key = new_path.clone();
                                        update_window_title(&window, &current_path_key, &app_state);
                                        
                                        rt.block_on(loader.send_request(LoaderRequest::Clear));
                                        let l = loader.clone();
                                        rt.spawn(async move { let _ = l.send_request(LoaderRequest::ClearPrefetch).await; });
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
                            match logical_key {
                                Key::Character(ref s) if s == "+" || s == ";" => { // ";" は JP キーボードの "+"
                                    let window_size = window.inner_size();
                                    let win_size = (window_size.width as f32, window_size.height as f32);
                                    let center = (win_size.0 / 2.0, win_size.1 / 2.0);
                                    view_state.set_zoom(view_state.zoom_level * 1.15, center, win_size);
                                }
                                Key::Character(ref s) if s == "-" => {
                                    let window_size = window.inner_size();
                                    let win_size = (window_size.width as f32, window_size.height as f32);
                                    let center = (win_size.0 / 2.0, win_size.1 / 2.0);
                                    view_state.set_zoom(view_state.zoom_level / 1.15, center, win_size);
                                }
                                _ => {
                                    if let PhysicalKey::Code(code) = physical_key {
                                        match code {
                                            KeyCode::NumpadAdd => {
                                                let window_size = window.inner_size();
                                                let win_size = (window_size.width as f32, window_size.height as f32);
                                                let center = (win_size.0 / 2.0, win_size.1 / 2.0);
                                                view_state.set_zoom(view_state.zoom_level * 1.15, center, win_size);
                                            }
                                            KeyCode::NumpadSubtract => {
                                                let window_size = window.inner_size();
                                                let win_size = (window_size.width as f32, window_size.height as f32);
                                                let center = (win_size.0 / 2.0, win_size.1 / 2.0);
                                                view_state.set_zoom(view_state.zoom_level / 1.15, center, win_size);
                                            }
                                            KeyCode::NumpadMultiply => {
                                                view_state.reset();
                                            }
                                            _ => (),
                                        }
                                    }
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
                            let l = loader.clone();
                            rt.spawn(async move { let _ = l.send_request(LoaderRequest::ClearPrefetch).await; });
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
                                        let l = loader.clone();
                                        rt.spawn(async move { let _ = l.send_request(LoaderRequest::ClearPrefetch).await; });
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
                    
                    if scroll.abs() > 0.01 {
                        if modifiers.control_key() {
                            // Ctrl + Wheel: ズーム
                            let factor = if scroll > 0.0 { 1.15 } else { 1.0 / 1.15 };
                            let window_size = window.inner_size();
                            let win_size = (window_size.width as f32, window_size.height as f32);
                            view_state.set_zoom(view_state.zoom_level * factor, view_state.cursor_pos, win_size);
                        } else {
                            // 通常の Wheel: ページ移動
                            let direction = if scroll > 0.0 { -1 } else { 1 };
                            app_state.navigate(direction);
                            let l = loader.clone();
                            rt.spawn(async move { let _ = l.send_request(LoaderRequest::ClearPrefetch).await; });
                            view_state.reset();
                            request_pages_with_prefetch(&app_state, &loader, &rt, &cpu_cache, &settings, &current_path_key);
                        }
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
                    
                    // GPU キャッシュの更新と不要なビットマップの解放
                    {
                        let mut cache = cpu_cache.lock().unwrap();
                        cache.set_current_context(app_state.current_page_index, indices.clone());
                        
                        // 1. 不要なビットマップの解放
                        let max_gpu_bitmaps = settings.gpu_max_prefetch_pages + indices.len();
                        let current_idx = app_state.current_page_index as isize;
                        let max_idx = app_state.image_files.len() as isize - 1;

                        // GPU キャッシュ保持対象範囲の計算 (前後 settings.gpu_max_prefetch_pages)
                        let mut gpu_targets = indices.clone();
                        let prefetch_dist = settings.gpu_max_prefetch_pages as isize;
                        for i in 1..=prefetch_dist {
                            if current_idx - i >= 0 { gpu_targets.push((current_idx - i) as usize); }
                            if current_idx + i <= max_idx { gpu_targets.push((current_idx + i) as usize); }
                        }
                        gpu_targets.sort();
                        gpu_targets.dedup();
                        
                        // 強制解放距離 (先読み設定の2倍強、最低20ページ)
                        let force_evict_dist = (settings.gpu_max_prefetch_pages * 2 + 2).max(20) as isize;

                        if current_bitmaps.len() > max_gpu_bitmaps || current_bitmaps.iter().any(|(idx, _)| (*idx as isize - current_idx).abs() > force_evict_dist) {
                            // 保持対象（先読み範囲内）または距離が近いページを保護し、それ以外を candidates とする
                            let (mut to_keep, mut candidates): (Vec<_>, Vec<_>) = current_bitmaps.drain(..).partition(|(idx, _)| {
                                gpu_targets.contains(idx) || (*idx as isize - current_idx).abs() <= force_evict_dist
                            });

                            // 距離が近い順にソート
                            candidates.sort_by_cached_key(|(idx, _)| (*idx as isize - current_idx).abs());
                            
                            // 枚数上限（または距離制限内の全件）になるまで to_keep に戻す
                            while to_keep.len() < max_gpu_bitmaps && !candidates.is_empty() {
                                to_keep.push(candidates.remove(0));
                            }

                            // 残った candidates は解放対象
                            for (idx, _) in candidates {
                                println!("[GPUキャッシュ] VRAMを解放しました (距離または枚数超過): ページ {}", idx);
                            }
                            current_bitmaps = to_keep;
                        }

                        // 2. 新しいビットマップの生成 (表示中 + 先読み範囲)
                        // 表示中のページを最優先し、次に近い順にアップロードする
                        let mut upload_candidates = gpu_targets.clone();
                        upload_candidates.sort_by_key(|&idx| (idx as isize - current_idx).abs());

                        for &idx in &upload_candidates {
                            if !current_bitmaps.iter().any(|(i, _)| *i == idx) {
                                let key = format!("{}::{}", current_path_key, idx);
                                if let Some(decoded) = cache.get(&key) {
                                    if let Ok(texture) = renderer.upload_image(&decoded) {
                                        current_bitmaps.push((idx, texture));
                                        // 1ループでのアップロード枚数を制限してカクつきを抑えることも可能だが、
                                        // 現状は cache.get できたものはすべてアップロードする
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

                    if !indices.is_empty() {
                        {
                            // 見開き表示で画像が1枚足りない場合でも、2枚分の枠を確保してレイアウトが崩れないようにする
                            let mut images_info = Vec::new();
                            let mut total_content_w = 0.0;
                            let mut max_content_h = 0.0;
                            
                            for &idx in &indices {
                                if let Some((_, bmp)) = current_bitmaps.iter().find(|(i, _)| *i == idx) {
                                    let (w, h) = renderer.get_texture_size(bmp);
                                    let size = D2D_SIZE_F { width: w, height: h };
                                    images_info.push((idx, Some((bmp, size))));
                                    total_content_w += w;
                                    if h > max_content_h { max_content_h = h; }
                                } else {
                                    // 未ロードのページも枠を確保
                                    images_info.push((idx, None));
                                }
                            }

                            // 1枚もロードされていない場合は何もしない
                            if max_content_h == 0.0 {
                                // 仮の高さ（ウィンドウサイズなどから推測）
                                max_content_h = win_h * 0.8;
                            }
                            
                            // 未ロードの画像がある場合、total_content_w を調整
                            if indices.len() == 2 && images_info.iter().any(|info| info.1.is_none()) {
                                if let Some((_, Some((_, size)))) = images_info.iter().find(|info| info.1.is_some()) {
                                    total_content_w = size.width * 2.0;
                                } else {
                                    total_content_w = win_w * 0.8;
                                }
                            }

                            if total_content_w > 0.0 {
                                let scale_fit = (win_w / total_content_w).min(win_h / max_content_h).min(1.0);
                                let total_scale = scale_fit * view_state.zoom_level;

                                let draw_total_w = total_content_w * total_scale;
                                let draw_max_h = max_content_h * total_scale;

                                view_state.clamp_pan_offset((win_w, win_h), (draw_total_w, draw_max_h));

                                let base_x = (win_w - draw_total_w) / 2.0 + view_state.pan_offset.0;
                                let base_y = (win_h - draw_max_h) / 2.0 + view_state.pan_offset.1;

                                let mut current_x = base_x;
                                for (idx, info) in images_info {
                                    // 見開きの場合、個々の画像幅を計算
                                    let w_step = if indices.len() == 2 {
                                        total_content_w / 2.0 * total_scale
                                    } else {
                                        total_content_w * total_scale
                                    };

                                    let y_center = base_y + draw_max_h / 2.0;

                                    if let Some((bmp, size)) = info {
                                        let w = size.width * total_scale;
                                        let h = size.height * total_scale;
                                        let y = y_center - h / 2.0;
                                        // 画像を枠内で中央寄せ
                                        let x = current_x + (w_step - w) / 2.0;

                                        let dest_rect = D2D_RECT_F {
                                            left: x,
                                            top: y,
                                            right: x + w,
                                            bottom: y + h,
                                        };
                                        renderer.draw_image(bmp, &dest_rect);
                                    } else {
                                        // ロード中表示
                                        let text = format!("Loading Page {}...", idx + 1);
                                        let text_rect = D2D_RECT_F {
                                            left: current_x,
                                            top: y_center - 20.0,
                                            right: current_x + w_step,
                                            bottom: y_center + 20.0,
                                        };
                                        renderer.draw_text(
                                            &text,
                                            &text_rect,
                                            &D2D1_COLOR_F { r: 0.6, g: 0.6, b: 0.6, a: 1.0 },
                                            false
                                        );
                                    }
                                    current_x += w_step;
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
                    let display_indices = app_state.get_page_indices_to_display();
                    let current_page_str = if display_indices.len() > 1 {
                        let mut sorted_display = display_indices.clone();
                        sorted_display.sort();
                        format!("{}-{}", sorted_display[0] + 1, sorted_display.last().unwrap() + 1)
                    } else {
                        format!("{}", app_state.current_page_index + 1)
                    };
                    
                    let cpu_indices: Vec<usize> = {
                        let keys = cpu_cache.lock().unwrap().get_keys();
                        keys.iter().filter_map(|k| k.rsplit("::").next()?.parse().ok()).collect()
                    };
                    let gpu_indices: Vec<usize> = current_bitmaps.iter().map(|(idx, _)| *idx).collect();

                    let path_preview: String = current_path_key.chars().take(20).collect();
                    let status_text = format!(
                        " Page: {} / {} | Backend: {} | CPU: {}p {} | GPU: {}p {} | Key: {}",
                        current_page_str,
                        total_pages,
                        get_backend_display_name(&settings.rendering_backend),
                        cpu_indices.len(),
                        format_page_list(&cpu_indices, app_state.current_page_index),
                        gpu_indices.len(),
                        format_page_list(&gpu_indices, app_state.current_page_index),
                        path_preview
                    );
                    if settings.show_status_bar_info {
                        renderer.draw_text(&status_text, &bar_rect, &D2D1_COLOR_F { r: 0.9, g: 0.9, b: 0.9, a: 1.0 }, false);
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
                        renderer.draw_text("ページ指定 (Enterで確定)", &title_rect, &D2D1_COLOR_F { r: 0.6, g: 0.6, b: 0.6, a: 1.0 }, false);

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
                        renderer.draw_text(&full_text, &input_bg_rect, &D2D1_COLOR_F { r: 1.0, g: 0.8, b: 0.0, a: 1.0 }, true);

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

                        renderer.draw_text("--- 設定 ---", &text_rect, &D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }, false);
                        text_rect.top += 40.0;
                        text_rect.bottom += 40.0;

                        let options = [
                            ("レンダリングエンジン", get_backend_display_name(&settings.rendering_backend)),
                            ("見開き表示", if app_state.is_spread_view { "オン" } else { "オフ" }),
                            ("綴じ方向", if app_state.binding_direction == BindingDirection::Right { "右綴じ" } else { "左綴じ" }),
                            ("補間モード (DX)", get_interpolation_display_name(&settings.resampling_mode_dx)),
                            ("最大キャッシュ容量", &format!("{} MB", settings.max_cache_size_mb)),
                            ("CPU 先読み数", &format!("{} ページ", settings.cpu_max_prefetch_pages)),
                            ("GPU 先読み数", &format!("{} ページ", settings.gpu_max_prefetch_pages)),
                            ("ステータスバー", if settings.show_status_bar_info { "表示" } else { "非表示" }),
                            ("デコードスレッド数", &format!("{} (要再起動)", settings.parallel_decoding_workers)),
                            ("CPU 色空間変換", if settings.use_cpu_color_conversion { "オン" } else { "オフ" }),
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
                            renderer.draw_text(&display_text, &text_rect, &color, false);
                            
                            text_rect.top += 35.0;
                            text_rect.bottom += 35.0;
                        }

                        let hint_rect = D2D_RECT_F {
                            left: overlay_rect.left + 20.0,
                            top: overlay_rect.bottom - 40.0,
                            right: overlay_rect.right - 20.0,
                            bottom: overlay_rect.bottom - 10.0,
                        };
                        renderer.draw_text("矢印キーで変更、'O'キーで閉じる", &hint_rect, &D2D1_COLOR_F { r: 0.5, g: 0.5, b: 0.5, a: 1.0 }, false);
                    }

                    let _ = renderer.end_draw();
                }
                _ => (),
            },
            Event::UserEvent(user_event) => {
                match user_event {
                    UserEvent::PageLoaded(_index) => {
                        // 読み込み完了したインデックスをログ出力（デバッグ用）
                        // println!("[イベント] ページ {} のロード完了を受信", _index);
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

fn init_opengl(window: &Arc<winit::window::Window>) -> Result<crate::render::opengl::OpenGLRenderer, Box<dyn std::error::Error>> {
    use glutin::prelude::*;
    use glutin::config::ConfigTemplateBuilder;
    use glutin::context::{ContextAttributesBuilder, ContextApi};
    use glutin::surface::{SurfaceAttributesBuilder, WindowSurface};
    use glutin_winit::GlWindow;
    use raw_window_handle::{HasRawDisplayHandle, HasRawWindowHandle};

    let raw_display_handle = window.raw_display_handle();
    let display = unsafe { glutin::display::Display::new(raw_display_handle, glutin::display::DisplayApiPreference::Wgl(None))? };

    let template = ConfigTemplateBuilder::new()
        .with_alpha_size(8)
        .with_transparency(true);
    let config = unsafe { display.find_configs(template.build())?.next().ok_or("No GL config found")? };

    let raw_window_handle = window.raw_window_handle();
    let context_attributes = ContextAttributesBuilder::new()
        .with_context_api(ContextApi::OpenGl(Some(glutin::context::Version::new(3, 3))))
        .build(Some(raw_window_handle));

    let fallback_context_attributes = ContextAttributesBuilder::new()
        .with_context_api(ContextApi::OpenGl(Some(glutin::context::Version::new(3, 3))))
        .build(None);

    let mut not_current_gl_context = Some(unsafe {
        display.create_context(&config, &context_attributes)
            .or_else(|_| display.create_context(&config, &fallback_context_attributes))?
    });

    let attrs = window.build_surface_attributes(SurfaceAttributesBuilder::<WindowSurface>::new());
    let gl_surface = unsafe { display.create_window_surface(&config, &attrs)? };

    let gl_context = not_current_gl_context.take().unwrap().make_current(&gl_surface)?;
    
    let gl = unsafe { glow::Context::from_loader_function(|s| {
        let name = std::ffi::CString::new(s).unwrap();
        display.get_proc_address(&name) as *const _
    }) };

    crate::render::opengl::OpenGLRenderer::new(Arc::new(gl), gl_context, gl_surface)
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
    let _ = loader_tx.try_send(LoaderRequest::Clear); // 過去のリクエストをクリア

    // 1. 表示対象の即時リクエスト (Priority 0)
    for &idx in &display_indices {
        let key = format!("{}::{}", path_key, idx);
        let cached = cpu_cache.lock().unwrap().get(&key).is_some();
        if !cached {
            println!("[先読み] インデックス {} の即時読み込みをリクエスト", idx);
            let l = loader_tx.clone();
            let cpu_conv = settings.use_cpu_color_conversion;
            rt.spawn(async move {
                let _ = l.send(LoaderRequest::Load { index: idx, priority: 0, use_cpu_color_conversion: cpu_conv }).await;
            });
        }
    }

    // 2. 先読み範囲の計算と「歯抜け」補充 (Priority 1)
    let prefetch_dist = settings.cpu_max_prefetch_pages;
    let mut targets = std::collections::HashSet::new();
    
    // 表示中の全ページについて、その前後 prefetch_dist を先読み対象とする
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
        println!("[先読み] 補充対象インデックス: {:?}", targets_vec);
    }

    for idx in targets_vec {
        let key = format!("{}::{}", path_key, idx);
        let cached = {
            let mut c = cpu_cache.lock().unwrap();
            c.get(&key).is_some()
        };
        
        if !cached {
            let l = loader_tx.clone();
            let cpu_conv = settings.use_cpu_color_conversion;
            rt.spawn(async move {
                let _ = l.send(LoaderRequest::Load { index: idx, priority: 1, use_cpu_color_conversion: cpu_conv }).await;
            });
        }
    }
}

fn get_backend_display_name(backend: &str) -> &str {
    match backend {
        "direct2d" => "Direct2D",
        "direct3d11" => "Direct3D 11",
        "opengl" => "OpenGL",
        _ => backend,
    }
}

fn get_interpolation_display_name(mode: &str) -> &str {
    match mode {
        "DX_NEAREST" => "ニアレストネイバー (高速)",
        "DX_LINEAR" => "バイリニア",
        "DX_CUBIC" => "バイキュービック",
        "DX_HQC" => "高品質バイキュービック",
        "DX_LANCZOS" => "Lanczos (高品質)",
        _ => mode,
    }
}

fn format_page_list(indices: &[usize], current: usize) -> String {
    if indices.is_empty() {
        return "[]".to_string();
    }
    let mut sorted = indices.to_vec();
    sorted.sort();
    
    // 現在ページに近いものを優先して表示する
    if sorted.len() > 8 {
        let first = sorted[0] + 1;
        let last = sorted.last().unwrap() + 1;
        
        // 現在ページ(current+1)の前後を表示したい
        let cur = current + 1;
        let neighbors: Vec<String> = sorted.iter()
            .map(|&i| i + 1)
            .filter(|&p| (p as isize - cur as isize).abs() <= 2 || p == first || p == last)
            .collect::<Vec<_>>()
            .iter().map(|p| p.to_string()).collect();
        
        // 重複を除去して結合
        let mut result = String::from("[");
        let mut last_p = 0;
        for (i, p_str) in neighbors.iter().enumerate() {
            let p: usize = p_str.parse().unwrap();
            if i > 0 {
                if p > last_p + 2 { result.push_str(", [中略] "); }
                else { result.push_str(", "); }
            }
            result.push_str(p_str);
            last_p = p;
        }
        result.push(']');
        result
    } else {
        format!("{:?}", sorted.iter().map(|i| i + 1).collect::<Vec<_>>())
    }
}

fn get_neighboring_source(current_path: &str, direction: isize) -> Option<String> {
    let path = std::path::Path::new(current_path);
    let parent = path.parent()?;
    
    let mut entries = Vec::new();
    let supported_archives = ["zip", "7z", "cbz", "rar", "cbr"];
    
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
