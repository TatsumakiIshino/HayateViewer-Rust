#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod config;
mod render;
mod image;
mod state;
mod ui;

const VERSION: &str = env!("CARGO_PKG_VERSION");

use crate::config::Settings;
use crate::render::Renderer;
use crate::render::d2d::D2DRenderer;
use crate::image::{get_image_source, ImageSource};
use crate::image::cache::{create_shared_cache, SharedImageCache};
use crate::image::loader::{AsyncLoader, LoaderRequest, UserEvent};
use crate::state::{AppState, BindingDirection};
use std::sync::Arc;
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

use windows::Win32::Foundation::{HWND, WPARAM, LPARAM};
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, INITCOMMONCONTROLSEX, ICC_BAR_CLASSES, STATUSCLASSNAMEW,
    SB_SETTEXTW, SB_SETPARTS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, SendMessageW, WS_CHILD, WS_VISIBLE,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_SIZE,
};
use windows::core::w;


fn update_window_title(window: &winit::window::Window, path_key: &str, app_state: &AppState) {
    let archive_name = if !path_key.is_empty() {
        std::path::Path::new(path_key)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path_key.to_string())
    } else {
        String::new()
    };

    let display_indices = app_state.get_page_indices_to_display();
    let mut image_names = Vec::new();
    
    // 見開き順（表示順）にソートしてファイル名を取得
    let mut sorted_indices = display_indices.clone();
    sorted_indices.sort();
    
    for idx in sorted_indices {
        if let Some(path_str) = app_state.image_files.get(idx) {
            let fname = std::path::Path::new(path_str)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path_str.clone());
            image_names.push(fname);
        }
    }
    
    let images_str = image_names.join(" - ");

    let title_text = if !archive_name.is_empty() {
        if !images_str.is_empty() {
            format!("{} / {}", archive_name, images_str)
        } else {
            archive_name
        }
    } else {
        images_str
    };

    let title = if !title_text.is_empty() {
        format!("HayateViewer v{} - {}", VERSION, title_text)
    } else {
        format!("HayateViewer v{}", VERSION)
    };
    window.set_title(&title);
}

/// Windows システムステータスバーを作成する
fn create_status_bar(parent_hwnd: HWND) -> Option<HWND> {
    unsafe {
        // Common Controls を初期化
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_BAR_CLASSES,
        };
        if !InitCommonControlsEx(&icc).as_bool() {
            return None;
        }
        
        // ステータスバーウィンドウを作成
        let status_hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            STATUSCLASSNAMEW,
            w!(""),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0),
            0, 0, 0, 0,
            Some(parent_hwnd),
            None,
            None,
            None,
        );
        
        if status_hwnd.is_ok() {
            let sb_hwnd = status_hwnd.unwrap();
            // パーツ幅をウィンドウ幅全体に設定
            let parts: [i32; 1] = [-1];
            SendMessageW(
                sb_hwnd,
                SB_SETPARTS,
                Some(WPARAM(1)),
                Some(LPARAM(parts.as_ptr() as isize)),
            );
            Some(sb_hwnd)
        } else {
            None
        }
    }
}

/// ステータスバーのテキストを更新する
fn update_status_bar_text(status_hwnd: HWND, text: &str) {
    unsafe {
        let mut wide_text: Vec<u16> = text.encode_utf16().collect();
        wide_text.push(0); // null terminate
        SendMessageW(
            status_hwnd,
            SB_SETTEXTW,
            Some(WPARAM(0)), // Part 0, no flags
            Some(LPARAM(wide_text.as_ptr() as isize)),
        );
    }
}

fn sync_current_state_to_history(settings: &mut Settings, app_state: &AppState, current_path_key: &str) {
    if current_path_key.is_empty() { return; }
    let binding_str = if !app_state.is_spread_view {
        "single"
    } else if app_state.binding_direction == BindingDirection::Left {
        "left"
    } else {
        "right"
    };
    settings.add_to_history(current_path_key.to_string(), app_state.current_page_index, binding_str.to_string());
}

fn load_new_source(
    new_source: ImageSource,
    path_str: String,
    initial_page: usize,
    initial_binding: Option<String>,
    app_state: &mut AppState,
    current_path_key: &mut String,
    window: &winit::window::Window,
    cpu_cache: &SharedImageCache,
    loader: &Arc<AsyncLoader>,
    rt: &Runtime,
    settings: &mut Settings,
    current_bitmaps: &mut Vec<(usize, crate::render::TextureHandle)>,
) {
    println!("ソースを読み込み: {} ({} 個のファイル/エントリ)", path_str, new_source.len());
    
    // 切り替え前に現在のファイルの状態（ページ・綴じ方向）を履歴に保存
    sync_current_state_to_history(settings, app_state, current_path_key);

    if let ImageSource::Files(ref files) = new_source {
        app_state.image_files = files.clone();
    } else if let ImageSource::Archive(ref loader) = new_source {
        app_state.image_files = loader.get_file_names().to_vec();
    }

    // 読み込み先の設定を反映（履歴からの復元用）
    if let Some(binding) = initial_binding {
        match binding.as_str() {
            "single" => app_state.is_spread_view = false,
            "left" => {
                app_state.is_spread_view = true;
                app_state.binding_direction = BindingDirection::Left;
            }
            "right" => {
                app_state.is_spread_view = true;
                app_state.binding_direction = BindingDirection::Right;
            }
            _ => {}
        }
    }

    app_state.current_page_index = initial_page.min(app_state.image_files.len().saturating_sub(1));
    current_bitmaps.clear();
    
    // CPU キャッシュもクリア
    if let Ok(mut cache) = cpu_cache.lock() {
        cache.clear();
    }
    
    *current_path_key = path_str.clone();
    update_window_title(window, current_path_key, app_state);
    
    rt.block_on(loader.send_request(LoaderRequest::Clear));
    let l_prefetch = Arc::clone(loader);
    rt.spawn(async move { let _ = l_prefetch.send_request(LoaderRequest::ClearPrefetch).await; });
    rt.block_on(loader.send_request(LoaderRequest::SetSource { 
        source: new_source, 
        path_key: path_str.clone() 
    }));

    // 新しいファイルを履歴の先頭に追加
    sync_current_state_to_history(settings, app_state, &path_str);
    let _ = settings.save("config.json");
    request_pages_with_prefetch(app_state, loader, rt, cpu_cache, settings, current_path_key);
}

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

    // Windows システムステータスバーを作成
    let status_bar_hwnd = create_status_bar(hwnd);
    if status_bar_hwnd.is_some() {
        println!("[UI] Windows システムステータスバーを作成しました");
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

    println!("[情報] レンダリングエンジン: {}", settings.rendering_backend);
    let mut view_state = ViewState::new();
    let mut app_state = AppState::new();
    let mut current_path_key = String::new();

    app_state.is_spread_view = settings.is_spread_view;
    app_state.binding_direction = if settings.binding_direction == "right" { BindingDirection::Right } else { BindingDirection::Left };
    app_state.spread_view_first_page_single = settings.spread_view_first_page_single;

    // Cache & Loader
    let max_bytes = (settings.max_cache_size_mb as usize) * 1024 * 1024;
    let cpu_cache = create_shared_cache(100, max_bytes);
    let loader = AsyncLoader::new(cpu_cache.clone(), proxy.clone());

    let gpu_mode = match settings.resampling_mode_gpu.as_str() {
        "Nearest" => crate::render::InterpolationMode::NearestNeighbor,
        "Linear" => crate::render::InterpolationMode::Linear,
        "Cubic" => crate::render::InterpolationMode::Cubic,
        "Lanczos" => crate::render::InterpolationMode::Lanczos,
        _ => crate::render::InterpolationMode::Linear,
    };
    renderer.set_interpolation_mode(gpu_mode);

    let mut current_bitmaps: Vec<(usize, crate::render::TextureHandle)> = Vec::new();

    // 初期パスの読み込み
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        if let Some(src) = get_image_source(&args[1]) {
            load_new_source(
                src,
                args[1].clone(),
                0,
                None,
                &mut app_state,
                &mut current_path_key,
                &window,
                &cpu_cache,
                &loader,
                &rt,
                &mut settings,
                &mut current_bitmaps,
            );
        }
    }

    let mut modifiers = ModifiersState::default();
    
    let mut last_dialog_close = std::time::Instant::now();
    let mut modern_settings: Option<ui::modern_settings::ModernSettingsWindow> = None;
    let mut modern_history: Option<ui::history::HistoryWindow> = None;

    event_loop.run(move |event: Event<UserEvent>, elwt: &winit::event_loop::EventLoopWindowTarget<UserEvent>| {
        elwt.set_control_flow(ControlFlow::Wait);
        match event {
            Event::WindowEvent { event, window_id } => {
                // Modern UI ウィンドウのイベント処理
                if let Some(ref mut ms) = modern_settings {
                    if ms.window.id() == window_id {
                        if ms.handle_event(&event, &settings) {
                            modern_settings = None;
                            last_dialog_close = std::time::Instant::now();
                        } else if matches!(event, WindowEvent::RedrawRequested) {
                            ms.draw(&settings);
                        }
                        return;
                    }
                }

                if let Some(ref mut mh) = modern_history {
                    if mh.window.id() == window_id {
                        if mh.handle_event(&event, &settings) {
                            modern_history = None;
                            last_dialog_close = std::time::Instant::now();
                        } else if matches!(event, WindowEvent::RedrawRequested) {
                            mh.draw(&settings);
                        }
                        return;
                    }
                }

                if window_id != window.id() { return; }
                
                match event {
                WindowEvent::CloseRequested => {
                    println!("終了リクエストを受信しました。終了します...");
                    // 終了前に現在の状態を保存
                    sync_current_state_to_history(&mut settings, &app_state, &current_path_key);
                    let _ = settings.save("config.json");
                    elwt.exit();
                    // 非同期タスクがブロッキングしている場合に備え、プロセスを強制終了
                    std::process::exit(0);
                }
                WindowEvent::Resized(physical_size) => {
                    let _ = renderer.resize(physical_size.width, physical_size.height);
                    if let Some(sb_hwnd) = status_bar_hwnd {
                        unsafe {
                            SendMessageW(sb_hwnd, WM_SIZE, Some(WPARAM(0)), Some(LPARAM(0)));
                            // ステータスバーのパーツ幅をウィンドウ幅全体に設定
                            let parts: [i32; 1] = [-1]; // -1 = ウィンドウ幅全体
                            SendMessageW(
                                sb_hwnd,
                                SB_SETPARTS,
                                Some(WPARAM(1)),
                                Some(LPARAM(parts.as_ptr() as isize)),
                            );
                        }
                    }
                }
                WindowEvent::DroppedFile(path) => {
                    let path_str = path.to_string_lossy().to_string();
                    println!("ファイルをドロップ: {}", path_str);
                    if let Some(new_source) = get_image_source(&path_str) {
                        load_new_source(
                            new_source,
                            path_str,
                            0,
                            None,
                            &mut app_state,
                            &mut current_path_key,
                            &window,
                            &cpu_cache,
                            &loader,
                            &rt,
                            &mut settings,
                            &mut current_bitmaps,
                        );
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
                        Key::Character(ref s) if s.to_lowercase() == "o" => {
                            if last_dialog_close.elapsed() < std::time::Duration::from_millis(500) {
                                return;
                            }
                            
                            if modern_settings.is_none() {
                                match ui::modern_settings::ModernSettingsWindow::new(elwt, hwnd, &settings, proxy.clone()) {
                                    Ok(mw) => {
                                        modern_settings = Some(mw);
                                    }
                                    Err(e) => {
                                        println!("Failed to open Modern UI: {:?}", e);
                                    }
                                }
                            }
                            last_dialog_close = std::time::Instant::now();
                        }
                        Key::Character(ref s) if s.to_lowercase() == "s" => {
                            if modifiers.shift_key() {
                                // Shift + S: ページジャンプを開く
                                app_state.is_jump_open = true;
                                app_state.jump_input_buffer.clear();

                            } else {
                                // S: シークバー切り替え
                                app_state.show_seekbar = !app_state.show_seekbar;
                            }
                        }
                        Key::Character(ref s) if s.to_lowercase() == "r" => {
                            // R: 履歴ウィンドウを開く
                            if last_dialog_close.elapsed() < std::time::Duration::from_millis(500) {
                                return;
                            }
                            
                            if modern_history.is_none() {
                                match ui::history::HistoryWindow::new(elwt, hwnd, &settings, proxy.clone()) {
                                    Ok(hw) => {
                                        modern_history = Some(hw);
                                    }
                                    Err(e) => {
                                        println!("Failed to open History Window: {:?}", e);
                                    }
                                }
                            }
                            last_dialog_close = std::time::Instant::now();
                        }
                        Key::Named(NamedKey::ArrowRight) | Key::Named(NamedKey::ArrowLeft) => {
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
                        Key::Character(ref s) if s.to_lowercase() == "b" => {
                                if !app_state.is_spread_view {
                                    app_state.is_spread_view = true;
                                    app_state.binding_direction = BindingDirection::Right;
                                } else if app_state.binding_direction == BindingDirection::Right {
                                    app_state.binding_direction = BindingDirection::Left;
                                } else {
                                    app_state.is_spread_view = false;
                                }
                        }
                        Key::Named(NamedKey::Escape) => {
                            if app_state.is_jump_open {
                                app_state.is_jump_open = false;
                                app_state.jump_input_buffer.clear();
                            }
                        }
                        Key::Character(ref s) if s == "[" || s == "]" => {
                            if !app_state.is_jump_open {
                                let direction = if s == "]" { 1 } else { -1 };
                                if let Some(new_path) = get_neighboring_source(&current_path_key, direction) {
                                    println!("フォルダ/アーカイブ移動: {}", new_path);
                                    if let Some(new_source) = get_image_source(&new_path) {
                                        load_new_source(
                                            new_source,
                                            new_path,
                                            0,
                                            None,
                                            &mut app_state,
                                            &mut current_path_key,
                                            &window,
                                            &cpu_cache,
                                            &loader,
                                            &rt,
                                            &mut settings,
                                            &mut current_bitmaps,
                                        );
                                    }
                                }
                            }
                        }
                        Key::Character(ref s) if s.to_lowercase() == "f" => {
                            let path = if modifiers.shift_key() {
                                ui::dialogs::select_archive_file(hwnd)
                            } else {
                                ui::dialogs::select_folder(hwnd)
                            };

                            if let Some(new_path_buf) = path {
                                let new_path = new_path_buf.to_string_lossy().to_string();
                                if let Some(new_source) = get_image_source(&new_path) {
                                    load_new_source(
                                        new_source,
                                        new_path,
                                        0,
                                        None,
                                        &mut app_state,
                                        &mut current_path_key,
                                        &window,
                                        &cpu_cache,
                                        &loader,
                                        &rt,
                                        &mut settings,
                                        &mut current_bitmaps,
                                    );
                                }
                            }
                        }
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
                            if app_state.is_jump_open {
                                if state == ElementState::Pressed {
                                    let window_size = window.inner_size();
                                    let win_w = window_size.width as f32;
                                    let win_h = window_size.height as f32;
                                    let jump_w = 340.0;
                                    let jump_h = 160.0;
                                    let jump_rect = D2D_RECT_F {
                                        left: (win_w - jump_w) / 2.0,
                                        top: (win_h - jump_h) / 2.0,
                                        right: (win_w + jump_w) / 2.0,
                                        bottom: (win_h + jump_h) / 2.0,
                                    };
                                    
                                    // クリック位置がUI外なら閉じる
                                    if view_state.cursor_pos.0 < jump_rect.left || view_state.cursor_pos.0 > jump_rect.right ||
                                       view_state.cursor_pos.1 < jump_rect.top || view_state.cursor_pos.1 > jump_rect.bottom {
                                        app_state.is_jump_open = false;
                                        app_state.jump_input_buffer.clear();
                                        window.request_redraw();
                                    }
                                }
                                return;
                            }

                            if state == ElementState::Pressed {
                                let window_size = window.inner_size();
                                let win_h = window_size.height as f32;
                                let status_bar_h = 22.0;
                                let seek_bar_h = 8.0;
                                // 描画ロジックと一致させる (win_h - 22.0 - 8.0)
                                let bar_y = win_h - status_bar_h - seek_bar_h;

                                // シークバークリック判定 (少し判定を広げる: 上下 4px)
                                let hit_margin = 4.0;
                                if app_state.show_seekbar && view_state.cursor_pos.1 >= bar_y - hit_margin && view_state.cursor_pos.1 <= bar_y + seek_bar_h + hit_margin {
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
                            if app_state.is_jump_open { return; }
                            if state == ElementState::Pressed {
                                view_state.is_loupe = true;
                                view_state.loupe_base_zoom = view_state.zoom_level;
                                view_state.loupe_base_pan = view_state.pan_offset;

                                let window_size = window.inner_size();
                                let win_size = (window_size.width as f32, window_size.height as f32);
                                view_state.set_zoom(view_state.zoom_level * settings.magnifier_zoom, view_state.cursor_pos, win_size);
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
                    if app_state.is_jump_open { return; }
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
                            // current_bitmaps = to_keep; // 既存コード
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
                                for (_idx, info) in images_info {
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
                                        // 未ロード時は何も描画しない
                                    }
                                    current_x += w_step;
                                }
                            }
                        }
                    }

                    // ステータスバーの更新（Windows システムステータスバーを使用）
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

                    let path_preview: String = current_path_key.clone();
                    
                    let spread_info = if app_state.is_spread_view {
                        let binding = if app_state.binding_direction == BindingDirection::Right { "右" } else { "左" };
                        format!("[見開き:{}]", binding)
                    } else {
                        "[単ページ]".to_string()
                    };

                    let status_text = if settings.show_status_bar_info {
                        format!(
                            "Page: {} / {} {} | Backend: {} | CPU: {}p {} | GPU: {}p {} | Key: {}",
                            current_page_str,
                            total_pages,
                            spread_info,
                            get_backend_display_name(&settings.rendering_backend),
                            cpu_indices.len(),
                            format_page_list(&cpu_indices, app_state.current_page_index),
                            gpu_indices.len(),
                            format_page_list(&gpu_indices, app_state.current_page_index),
                            path_preview
                        )
                    } else {
                        // 簡易表示（キャッシュ詳細なし）
                        format!(
                            "Page: {} / {} {} | Backend: {} | Key: {}",
                            current_page_str,
                            total_pages,
                            spread_info,
                            get_backend_display_name(&settings.rendering_backend),
                            path_preview
                        )
                    };

                    // ステータスバーは常に更新
                    if let Some(sb_hwnd) = status_bar_hwnd {
                        update_status_bar_text(sb_hwnd, &status_text);
                    }

                    // タイトルバー更新（ファイル名を表示、解像度はTODO）
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
                        renderer.fill_rectangle(&jump_rect, &D2D1_COLOR_F { r: 0.05, g: 0.05, b: 0.05, a: 0.95 });
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
                        renderer.fill_rectangle(&input_bg_rect, &D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.6 });

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
                        let status_bar_height = 22.0; // Windows システムステータスバーの高さ
                        let bar_y = win_h - status_bar_height - bar_height;
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
                        renderer.fill_rectangle(&progress_rect, &bar_color);
                    }



                    let _ = renderer.end_draw();
                }
                _ => (),
            }
        },
        Event::UserEvent(user_event) => {
            match user_event {
                UserEvent::PageLoaded(_index) => {
                    window.request_redraw();
                }
                UserEvent::ToggleSpreadView => {
                    app_state.is_spread_view = !app_state.is_spread_view;
                    settings.is_spread_view = app_state.is_spread_view;
                    let _ = settings.save("config.json");
                    view_state.reset();
                    request_pages_with_prefetch(&app_state, &loader, &rt, &cpu_cache, &settings, &current_path_key);
                    window.request_redraw();
                    if let Some(ref mut ms) = modern_settings { ms.window.request_redraw(); }
                }
                UserEvent::ToggleBindingDirection => {
                    app_state.binding_direction = match app_state.binding_direction {
                        BindingDirection::Left => BindingDirection::Right,
                        BindingDirection::Right => BindingDirection::Left,
                    };
                    settings.binding_direction = if app_state.binding_direction == BindingDirection::Right { "right".to_string() } else { "left".to_string() };
                    let _ = settings.save("config.json");
                    view_state.reset();
                    request_pages_with_prefetch(&app_state, &loader, &rt, &cpu_cache, &settings, &current_path_key);
                    window.request_redraw();
                    if let Some(ref mut ms) = modern_settings { ms.window.request_redraw(); }
                }
                UserEvent::ToggleFirstPageSingle => {
                    settings.spread_view_first_page_single = !settings.spread_view_first_page_single;
                    let _ = settings.save("config.json");
                    view_state.reset();
                    window.request_redraw();
                    if let Some(ref mut ms) = modern_settings { ms.window.request_redraw(); }
                }
                UserEvent::ToggleCpuColorConversion => {
                    settings.use_cpu_color_conversion = !settings.use_cpu_color_conversion;
                    let _ = settings.save("config.json");
                    view_state.reset();
                    request_pages_with_prefetch(&app_state, &loader, &rt, &cpu_cache, &settings, &current_path_key);
                    window.request_redraw();
                    if let Some(ref mut ms) = modern_settings { ms.window.request_redraw(); }
                }
                UserEvent::RotateResamplingCpu => {
                    let modes = ["PIL_LANCZOS", "PIL_BILINEAR", "PIL_BICUBIC", "PIL_NEAREST"];
                    let current = settings.resampling_mode_cpu.as_str();
                    let idx = modes.iter().position(|&m| m == current).unwrap_or(0);
                    settings.resampling_mode_cpu = modes[(idx + 1) % modes.len()].to_string();
                    let _ = settings.save("config.json");
                    view_state.reset();
                    window.request_redraw();
                    if let Some(ref mut ms) = modern_settings { ms.window.request_redraw(); }
                }
                UserEvent::RotateResamplingGpu => {
                    let modes = ["Nearest", "Linear", "Cubic", "Lanczos"];
                    let current = settings.resampling_mode_gpu.as_str();
                    let idx = modes.iter().position(|&m| m == current).unwrap_or(0);
                    let new_mode = modes[(idx + 1) % modes.len()];
                    settings.resampling_mode_gpu = new_mode.to_string();
                    
                    // レンダラーに即時反映
                    let mode_enum = match new_mode {
                        "Nearest" => crate::render::InterpolationMode::NearestNeighbor,
                        "Linear" => crate::render::InterpolationMode::Linear,
                        "Cubic" => crate::render::InterpolationMode::Cubic,
                        "Lanczos" => crate::render::InterpolationMode::Lanczos,
                        _ => crate::render::InterpolationMode::Linear,
                    };
                    renderer.set_interpolation_mode(mode_enum);

                    let _ = settings.save("config.json");
                    view_state.reset();
                    window.request_redraw();
                    if let Some(ref mut ms) = modern_settings {
                        ms.window.request_redraw();
                    }
                }
                UserEvent::ToggleStatusBar => {
                    settings.show_status_bar_info = !settings.show_status_bar_info;
                    let _ = settings.save("config.json");
                    window.request_redraw();
                    if let Some(ref mut ms) = modern_settings { ms.window.request_redraw(); }
                }
                UserEvent::RotateRenderingBackend => {
                    let backends = ["direct2d", "direct3d11", "opengl"];
                    let current = settings.rendering_backend.as_str();
                    let idx = backends.iter().position(|&b| b == current).unwrap_or(0);
                    settings.rendering_backend = backends[(idx + 1) % backends.len()].to_string();
                    let _ = settings.save("config.json");
                    println!("[設定] レンダリングバックエンドを {} に変更しました。反映には再起動が必要です。", settings.rendering_backend);
                    if let Some(ref mut ms) = modern_settings { ms.window.request_redraw(); }
                }
                UserEvent::RotateDisplayMode => {
                    // 順序: 単一(false, any) -> 左(true, "left") -> 右(true, "right")
                    if !settings.is_spread_view {
                        settings.is_spread_view = true;
                        settings.binding_direction = "left".to_string();
                    } else if settings.binding_direction == "left" {
                        settings.binding_direction = "right".to_string();
                    } else {
                        settings.is_spread_view = false;
                    }
                    app_state.is_spread_view = settings.is_spread_view;
                    app_state.binding_direction = if settings.binding_direction == "right" { BindingDirection::Right } else { BindingDirection::Left };
                    let _ = settings.save("config.json");
                    view_state.reset();
                    request_pages_with_prefetch(&app_state, &loader, &rt, &cpu_cache, &settings, &current_path_key);
                    window.request_redraw();
                    if let Some(ref mut ms) = modern_settings { ms.window.request_redraw(); }
                }
                UserEvent::SetMagnifierZoom(zoom) => {
                    settings.magnifier_zoom = zoom;
                    let _ = settings.save("config.json");
                    if let Some(ref mut ms) = modern_settings { ms.window.request_redraw(); }
                }
                UserEvent::LoadPath(path) => {
                    if let Some(new_source) = get_image_source(&path) {
                        load_new_source(
                            new_source,
                            path,
                            0, // デフォルト
                            None, // デフォルト
                            &mut app_state,
                            &mut current_path_key,
                            &window,
                            &cpu_cache,
                            &loader,
                            &rt,
                            &mut settings,
                            &mut current_bitmaps,
                        );
                        window.request_redraw();
                    }
                }
                UserEvent::LoadHistory(idx) => {
                    if let Some(item) = settings.history.get(idx).cloned() {
                        if let Some(new_source) = get_image_source(&item.path) {
                            load_new_source(
                                new_source,
                                item.path,
                                item.page,
                                Some(item.binding),
                                &mut app_state,
                                &mut current_path_key,
                                &window,
                                &cpu_cache,
                                &loader,
                                &rt,
                                &mut settings,
                                &mut current_bitmaps,
                            );
                            window.request_redraw();
                        }
                    }
                }
                UserEvent::ClearHistory => {
                    settings.clear_history();
                    let _ = settings.save("config.json");
                    if let Some(ref mut mh) = modern_history { mh.window.request_redraw(); }
                }
                UserEvent::DeleteHistoryItem(idx) => {
                    settings.remove_from_history(idx);
                    let _ = settings.save("config.json");
                    if let Some(ref mut mh) = modern_history { mh.window.request_redraw(); }
                }
                UserEvent::SetMaxHistoryCount(count) => {
                    settings.max_history_count = count;
                    let _ = settings.save("config.json");
                }
            }
        },
            Event::AboutToWait => {
                window.request_redraw();
            }
            _ => (),
        }
    }).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

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
            // println!("[先読み] インデックス {} の即時読み込みをリクエスト", idx);
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
    
    // if !targets_vec.is_empty() {
    //     println!("[先読み] 補充対象インデックス: {:?}", targets_vec);
    // }

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
