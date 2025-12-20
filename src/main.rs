mod config;
mod render;
mod image;

use crate::config::Settings;
use crate::render::d2d::D2DRenderer;
use crate::image::decoder;
use std::rc::Rc;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
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

    // 引数から画像を読み込む（暫定）
    let args: Vec<String> = std::env::args().collect();
    let mut current_bitmap = None;
    if args.len() > 1 {
        if let Ok(decoded) = decoder::decode_image(&args[1]) {
            if let Ok(bitmap) = renderer.create_bitmap(decoded.width, decoded.height, &decoded.data) {
                current_bitmap = Some(bitmap);
            }
        }
    }

    event_loop.run(move |event, elwt| {
        match event {
            Event::WindowEvent { event, window_id } if window_id == window.id() => match event {
                WindowEvent::CloseRequested => elwt.exit(),
                WindowEvent::Resized(physical_size) => {
                    let _ = renderer.resize(physical_size.width, physical_size.height);
                }
                WindowEvent::RedrawRequested => {
                    renderer.begin_draw();
                    
                    if let Some(ref bitmap) = current_bitmap {
                        unsafe {
                            let size = bitmap.GetSize();
                            let window_size = window.inner_size();
                            
                            // 簡易的なアスペクト比維持スケーリング
                            let scale = (window_size.width as f32 / size.width).min(window_size.height as f32 / size.height);
                            let draw_width = size.width * scale;
                            let draw_height = size.height * scale;
                            let x = (window_size.width as f32 - draw_width) / 2.0;
                            let y = (window_size.height as f32 - draw_height) / 2.0;

                            let dest_rect = D2D_RECT_F {
                                left: x,
                                top: y,
                                right: x + draw_width,
                                bottom: y + draw_height,
                            };
                            renderer.draw_bitmap(bitmap, &dest_rect);
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
