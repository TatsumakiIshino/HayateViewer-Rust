mod config;
mod render;

use crate::config::Settings;
use crate::render::d2d::D2DRenderer;
use std::rc::Rc;
use windows::Win32::Foundation::HWND;
use winit::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 設定のロード（Python版と同じ config.json）
    let config_path = "config.json";
    let settings = Settings::load_or_default(config_path);
    
    // 最初の起動時にデフォルト設定を保存
    if !std::path::Path::new(config_path).exists() {
        settings.save(config_path)?;
    }

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let window = Rc::new(WindowBuilder::new()
        .with_title("HayateViewer Rust")
        .with_inner_size(winit::dpi::LogicalSize::new(
            settings.window_size.0,
            settings.window_size.1,
        ))
        .build(&event_loop)?);

    // HWND の取得
    let hwnd = match window.window_handle()?.as_raw() {
        RawWindowHandle::Win32(handle) => HWND(handle.hwnd.get() as _),
        _ => panic!("Unsupported platform"),
    };

    // レンダラーの初期化
    let renderer = D2DRenderer::new(hwnd)?;

    event_loop.run(move |event, elwt| {
        match event {
            Event::WindowEvent { event, window_id } if window_id == window.id() => match event {
                WindowEvent::CloseRequested => elwt.exit(),
                WindowEvent::Resized(physical_size) => {
                    let _ = renderer.resize(physical_size.width, physical_size.height);
                }
                WindowEvent::RedrawRequested => {
                    renderer.begin_draw();
                    // 未来の実装: 画像の描画ロジック
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
