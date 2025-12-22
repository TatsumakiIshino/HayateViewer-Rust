use crate::config::Settings;
use raw_window_handle::{HasRawWindowHandle, RawWindowHandle};
use std::sync::Arc;
use windows::{
    Win32::Foundation::*, Win32::Graphics::Direct2D::Common::*, Win32::Graphics::Direct2D::*,
    Win32::Graphics::Direct3D::*, Win32::Graphics::Direct3D11::*, Win32::Graphics::DirectWrite::*,
    Win32::Graphics::Dxgi::Common::*, Win32::Graphics::Dxgi::*, core::*,
};
use winit::{
    event::*,
    event_loop::EventLoopWindowTarget,
    window::{Window, WindowBuilder},
};

pub struct ModernSettingsWindow {
    pub window: Arc<Window>,
    pub _factory: ID2D1Factory1,
    pub _device: ID2D1Device,
    pub context: ID2D1DeviceContext,
    pub swap_chain: IDXGISwapChain1,
    pub brush: ID2D1SolidColorBrush,
    pub text_format: IDWriteTextFormat,
    // マウス状態
    pub mouse_pos: (f32, f32),
    pub is_clicking: bool,
    pub selected_tab: usize,
}

impl ModernSettingsWindow {
    pub fn new<T>(
        elwt: &EventLoopWindowTarget<T>,
        parent_hwnd: HWND,
        _settings: &Settings,
    ) -> Result<Self> {
        let window = Arc::new(
            WindowBuilder::new()
                .with_title("HayateViewer Settings")
                .with_inner_size(winit::dpi::LogicalSize::new(500.0, 600.0))
                .with_resizable(false)
                .build(elwt)
                .map_err(|e| Error::new(HRESULT(-1), format!("{}", e)))?,
        );

        let hwnd = match window.raw_window_handle() {
            RawWindowHandle::Win32(handle) => HWND(handle.hwnd as _),
            _ => return Err(Error::new(HRESULT(-1), "Unsupported window handle")),
        };

        // 親ウィンドウを設定（モーダル風に動作させるため）
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{GWLP_HWNDPARENT, SetWindowLongPtrW};
            SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, parent_hwnd.0 as isize);
        }

        unsafe {
            let mut d3d_device: Option<ID3D11Device> = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut d3d_device),
                None,
                None,
            )?;
            let d3d_device = d3d_device.unwrap();
            let dxgi_device: IDXGIDevice = d3d_device.cast()?;
            let factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let device = factory.CreateDevice(&dxgi_device)?;
            let context = device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let dxgi_factory: IDXGIFactory2 = CreateDXGIFactory1()?;
            let swap_chain_desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: 0,
                Height: 0,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                Stereo: false.into(),
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                Flags: 0,
            };
            let swap_chain = dxgi_factory.CreateSwapChainForHwnd(
                &d3d_device,
                hwnd,
                &swap_chain_desc,
                None,
                None,
            )?;
            let surface: IDXGISurface = swap_chain.GetBuffer(0)?;
            let back_buffer: ID2D1Bitmap1 = context.CreateBitmapFromDxgiSurface(&surface, None)?;
            context.SetTarget(&back_buffer);
            let brush = context.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 1.0,
                },
                None,
            )?;
            let dw_factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let text_format = dw_factory.CreateTextFormat(
                w!("Segoe UI"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                14.0,
                w!("ja-jp"),
            )?;

            Ok(Self {
                window,
                _factory: factory,
                _device: device,
                context,
                swap_chain,
                brush,
                text_format,
                mouse_pos: (0.0, 0.0),
                is_clicking: false,
                selected_tab: 0,
            })
        }
    }

    pub fn handle_event(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::CloseRequested => true,
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse_pos = (position.x as f32, position.y as f32);
                self.window.request_redraw();
                false
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                self.is_clicking = *state == ElementState::Pressed;
                if !self.is_clicking {
                    self.handle_click();
                }
                self.window.request_redraw();
                false
            }
            _ => false,
        }
    }

    fn handle_click(&mut self) {
        // タブ切り替え判定
        for i in 0..3 {
            let rect = D2D_RECT_F {
                left: 20.0 + i as f32 * 110.0,
                top: 70.0,
                right: 120.0 + i as f32 * 110.0,
                bottom: 105.0,
            };
            if self.is_in_rect(rect) {
                self.selected_tab = i;
            }
        }
    }

    fn is_in_rect(&self, rect: D2D_RECT_F) -> bool {
        self.mouse_pos.0 >= rect.left
            && self.mouse_pos.0 <= rect.right
            && self.mouse_pos.1 >= rect.top
            && self.mouse_pos.1 <= rect.bottom
    }

    pub fn draw(&self, settings: &Settings) {
        unsafe {
            self.context.BeginDraw();
            self.context.Clear(Some(&D2D1_COLOR_F {
                r: 0.1,
                g: 0.11,
                b: 0.12,
                a: 1.0,
            }));

            // ヘッダー背景
            self.brush.SetColor(&D2D1_COLOR_F {
                r: 0.15,
                g: 0.16,
                b: 0.18,
                a: 1.0,
            });
            self.context.FillRectangle(
                &D2D_RECT_F {
                    left: 0.0,
                    top: 0.0,
                    right: 500.0,
                    bottom: 60.0,
                },
                &self.brush,
            );

            // タイトル描画
            self.brush.SetColor(&D2D1_COLOR_F {
                r: 0.9,
                g: 0.9,
                b: 0.9,
                a: 1.0,
            });
            let title = "HayateViewer Settings";
            let wide_title: Vec<u16> = title.encode_utf16().collect();
            let title_rect = D2D_RECT_F {
                left: 20.0,
                top: 18.0,
                right: 480.0,
                bottom: 50.0,
            };
            self.context.DrawText(
                &wide_title,
                &self.text_format,
                &title_rect,
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );

            // タブ描画
            let tabs = ["General", "Rendering", "About"];
            for (i, &name) in tabs.iter().enumerate() {
                let rect = D2D_RECT_F {
                    left: 20.0 + i as f32 * 110.0,
                    top: 70.0,
                    right: 120.0 + i as f32 * 110.0,
                    bottom: 105.0,
                };
                let is_hover = self.is_in_rect(rect);
                let is_selected = self.selected_tab == i;

                let bg_color = if is_selected {
                    D2D1_COLOR_F {
                        r: 0.0,
                        g: 0.47,
                        b: 0.83,
                        a: 1.0,
                    }
                } else if is_hover {
                    D2D1_COLOR_F {
                        r: 0.2,
                        g: 0.22,
                        b: 0.25,
                        a: 1.0,
                    }
                } else {
                    D2D1_COLOR_F {
                        r: 0.15,
                        g: 0.16,
                        b: 0.18,
                        a: 1.0,
                    }
                };
                self.brush.SetColor(&bg_color);
                self.context.FillRectangle(&rect, &self.brush);

                self.brush.SetColor(&D2D1_COLOR_F {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 1.0,
                });
                let wide_name: Vec<u16> = name.encode_utf16().collect();
                self.context.DrawText(
                    &wide_name,
                    &self.text_format,
                    &rect,
                    &self.brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            // 内容エリア背景
            self.brush.SetColor(&D2D1_COLOR_F {
                r: 0.13,
                g: 0.14,
                b: 0.16,
                a: 1.0,
            });
            self.context.FillRectangle(
                &D2D_RECT_F {
                    left: 20.0,
                    top: 120.0,
                    right: 480.0,
                    bottom: 580.0,
                },
                &self.brush,
            );

            match self.selected_tab {
                0 => self.draw_general_tab(settings),
                1 => self.draw_rendering_tab(settings),
                2 => self.draw_about_tab(),
                _ => {}
            }

            let _ = self.context.EndDraw(None, None);
            let _ = self.swap_chain.Present(1, DXGI_PRESENT(0));
        }
    }

    fn draw_general_tab(&self, settings: &Settings) {
        let text = format!(
            "Current Path: (Not yet integrated)\nBinding: {:?}",
            settings.binding_direction
        );
        self.draw_debug_text(&text, 140.0);
    }

    fn draw_rendering_tab(&self, settings: &Settings) {
        let text = format!(
            "Backend: {}\nSpread View: {}",
            settings.rendering_backend, settings.is_spread_view
        );
        self.draw_debug_text(&text, 140.0);
    }

    fn draw_about_tab(&self) {
        self.draw_debug_text("HayateViewer v0.4.4-test\nCreated by Tatsumaki", 140.0);
    }

    fn draw_debug_text(&self, text: &str, top: f32) {
        unsafe {
            self.brush.SetColor(&D2D1_COLOR_F {
                r: 0.8,
                g: 0.8,
                b: 0.8,
                a: 1.0,
            });
            let wide_text: Vec<u16> = text.encode_utf16().collect();
            let rect = D2D_RECT_F {
                left: 40.0,
                top,
                right: 460.0,
                bottom: 560.0,
            };
            self.context.DrawText(
                &wide_text,
                &self.text_format,
                &rect,
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }
}
