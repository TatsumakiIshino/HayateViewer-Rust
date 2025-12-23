use crate::config::Settings;
use raw_window_handle::{HasRawWindowHandle, RawWindowHandle};
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::{
    Win32::Foundation::*, Win32::Graphics::Direct2D::Common::*, Win32::Graphics::Direct2D::*,
    Win32::Graphics::Direct3D::*, Win32::Graphics::Direct3D11::*, Win32::Graphics::DirectWrite::*,
    Win32::Graphics::Dxgi::Common::*, Win32::Graphics::Dxgi::*, Win32::UI::WindowsAndMessaging::*,
    core::*,
};
use winit::{
    event::*,
    event_loop::EventLoopWindowTarget,
    keyboard::{Key, NamedKey},
    window::{Window, WindowBuilder},
};

pub struct HistoryWindow {
    pub window: Arc<Window>,
    pub _factory: ID2D1Factory1,
    pub _device: ID2D1Device,
    pub context: ID2D1DeviceContext,
    pub swap_chain: IDXGISwapChain1,
    pub brush: ID2D1SolidColorBrush,
    pub text_format: IDWriteTextFormat,
    pub event_proxy: winit::event_loop::EventLoopProxy<crate::image::loader::UserEvent>,
    pub selected_index: usize,
    pub mouse_pos: (f32, f32),
    pub last_click_time: Instant,
    pub last_click_idx: Option<usize>,
}

impl HistoryWindow {
    pub fn new<T>(
        elwt: &EventLoopWindowTarget<T>,
        parent_hwnd: HWND,
        _settings: &Settings,
        event_proxy: winit::event_loop::EventLoopProxy<crate::image::loader::UserEvent>,
    ) -> Result<Self> {
        let window = WindowBuilder::new()
            .with_title("閲覧履歴")
            .with_inner_size(winit::dpi::LogicalSize::new(800.0, 400.0))
            .with_min_inner_size(winit::dpi::LogicalSize::new(420.0, 150.0)) // ヘッダーが収まる最小サイズ
            .with_decorations(true)
            .with_resizable(true)
            .build(elwt)
            .map_err(|_| Error::new(HRESULT(-1), "Failed to build window"))?;

        let hwnd = match window.raw_window_handle() {
            RawWindowHandle::Win32(handle) => HWND(handle.hwnd as _),
            _ => return Err(Error::new(HRESULT(-1), "Unsupported window handle")),
        };

        // Parent window setting for Win32
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, parent_hwnd.0 as isize);
        }

        let window = Arc::new(window);

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
            let sc_desc = DXGI_SWAP_CHAIN_DESC1 {
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
                Flags: DXGI_SWAP_CHAIN_FLAG(0).0 as _,
            };
            let swap_chain =
                dxgi_factory.CreateSwapChainForHwnd(&d3d_device, hwnd, &sc_desc, None, None)?;

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
                w!("Yu Gothic UI"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                16.0,
                w!("ja-jp"),
            )?;
            // テキストを左揃えに設定
            text_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;

            Ok(Self {
                window,
                _factory: factory,
                _device: device,
                context,
                swap_chain,
                brush,
                text_format,
                event_proxy,
                selected_index: 0,
                mouse_pos: (0.0, 0.0),
                last_click_time: Instant::now(),
                last_click_idx: None,
            })
        }
    }

    pub fn handle_event(&mut self, event: &WindowEvent, settings: &Settings) -> bool {
        match event {
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key,
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => {
                let history_len = settings.history.len();
                match logical_key {
                    Key::Named(NamedKey::ArrowUp) => {
                        if self.selected_index > 0 {
                            self.selected_index -= 1;
                        } else if history_len > 0 {
                            self.selected_index = history_len - 1;
                        }
                        self.window.request_redraw();
                    }
                    Key::Named(NamedKey::ArrowDown) => {
                        if history_len > 0 {
                            self.selected_index = (self.selected_index + 1) % history_len;
                        }
                        self.window.request_redraw();
                    }
                    Key::Named(NamedKey::Enter) => {
                        self.confirm_selection(settings);
                        return true;
                    }
                    Key::Named(NamedKey::Delete) => {
                        if history_len > 0 {
                            let _ = self.event_proxy.send_event(
                                crate::image::loader::UserEvent::DeleteHistoryItem(
                                    self.selected_index,
                                ),
                            );
                        }
                    }
                    Key::Named(NamedKey::Escape) => {
                        return true;
                    }
                    _ => {}
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse_pos = (position.x as f32, position.y as f32);
                self.window.request_redraw();
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                let now = Instant::now();
                let is_double_click = if let Some(idx) = self.last_click_idx {
                    Some(idx) == self.get_hover_index(settings)
                        && now.duration_since(self.last_click_time) < Duration::from_millis(500)
                } else {
                    false
                };

                if let Some(idx) = self.get_hover_index(settings) {
                    self.selected_index = idx;
                    if is_double_click {
                        self.confirm_selection(settings);
                        return true;
                    }
                    self.last_click_idx = Some(idx);
                } else {
                    self.last_click_idx = None;
                }
                self.last_click_time = now;
                self.window.request_redraw();
            }
            WindowEvent::Resized(size) => {
                unsafe {
                    self.context.SetTarget(None);
                    self.swap_chain
                        .ResizeBuffers(
                            0,
                            size.width,
                            size.height,
                            DXGI_FORMAT_UNKNOWN,
                            DXGI_SWAP_CHAIN_FLAG(0),
                        )
                        .ok();
                    let surface: IDXGISurface = self.swap_chain.GetBuffer(0).ok().unwrap();
                    let back_buffer: ID2D1Bitmap1 = self
                        .context
                        .CreateBitmapFromDxgiSurface(&surface, None)
                        .ok()
                        .unwrap();
                    self.context.SetTarget(&back_buffer);
                }
                self.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                self.draw(settings);
            }
            WindowEvent::CloseRequested => {
                return true; // ウィンドウを閉じる
            }
            _ => {}
        }
        false
    }

    fn get_hover_index(&self, settings: &Settings) -> Option<usize> {
        let item_height = 30.0;
        let start_y = 50.0;
        let scale_factor = self.window.scale_factor() as f32;
        let win_w = self.window.inner_size().width as f32 / scale_factor;

        for (i, _) in settings.history.iter().enumerate() {
            let top = start_y + (i as f32) * item_height;
            let rect = D2D_RECT_F {
                left: 10.0,
                top,
                right: win_w - 10.0,
                bottom: top + item_height,
            };
            if self.is_in_rect(rect) {
                return Some(i);
            }
        }
        None
    }

    fn is_in_rect(&self, rect: D2D_RECT_F) -> bool {
        self.mouse_pos.0 >= rect.left
            && self.mouse_pos.0 <= rect.right
            && self.mouse_pos.1 >= rect.top
            && self.mouse_pos.1 <= rect.bottom
    }

    fn confirm_selection(&self, settings: &Settings) {
        if settings.history.get(self.selected_index).is_some() {
            let _ = self
                .event_proxy
                .send_event(crate::image::loader::UserEvent::LoadHistory(
                    self.selected_index,
                ));
        }
    }

    pub fn draw(&self, settings: &Settings) {
        unsafe {
            self.context.BeginDraw();
            self.context.Clear(Some(&D2D1_COLOR_F {
                r: 0.15,
                g: 0.15,
                b: 0.15,
                a: 1.0,
            }));

            let win_size = self.window.inner_size();
            let scale_factor = self.window.scale_factor() as f32;
            let win_w = win_size.width as f32 / scale_factor;

            // Draw header
            self.brush.SetColor(&D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            });
            let header_rect = D2D_RECT_F {
                left: 10.0,
                top: 10.0,
                right: win_w - 10.0,
                bottom: 40.0,
            };
            let header_text: Vec<u16> = "最近使った項目 (Wクリックで開く / DELで削除)"
                .encode_utf16()
                .collect();
            self.context.DrawText(
                &header_text,
                &self.text_format,
                &header_rect,
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );

            let item_height = 30.0;
            let start_y = 50.0;

            for (i, item) in settings.history.iter().enumerate() {
                let top = start_y + (i as f32) * item_height;
                let rect = D2D_RECT_F {
                    left: 10.0,
                    top,
                    right: win_w - 10.0,
                    bottom: top + item_height,
                };

                let is_hovered = self.is_in_rect(rect);
                let is_selected = i == self.selected_index;

                if is_selected || is_hovered {
                    let bg_color = if is_selected {
                        D2D1_COLOR_F {
                            r: 0.0,
                            g: 0.4,
                            b: 0.8,
                            a: 0.5,
                        }
                    } else {
                        D2D1_COLOR_F {
                            r: 1.0,
                            g: 1.0,
                            b: 1.0,
                            a: 0.1,
                        }
                    };
                    self.brush.SetColor(&bg_color);
                    self.context.FillRectangle(&rect, &self.brush);
                }

                self.brush.SetColor(&D2D1_COLOR_F {
                    r: 0.9,
                    g: 0.9,
                    b: 0.9,
                    a: 1.0,
                });

                let binding_char = match item.binding.as_str() {
                    "left" => "L",
                    "right" => "R",
                    "single" => "S",
                    _ => "?",
                };
                let display_text =
                    format!("({:3} / {})  {}", item.page + 1, binding_char, item.path);
                let text_wide: Vec<u16> = display_text.encode_utf16().collect();
                // テキストは矩形外にもはみ出して描画し、ウィンドウクリッピングに任せる
                let extended_text_rect = D2D_RECT_F {
                    left: 20.0,
                    top: top + 5.0,
                    right: 10000.0, // 非常に広く設定
                    bottom: top + item_height - 5.0,
                };
                self.context.DrawText(
                    &text_wide,
                    &self.text_format,
                    &extended_text_rect,
                    &self.brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            let _ = self.context.EndDraw(None, None);
            let _ = self.swap_chain.Present(1, DXGI_PRESENT(0));
        }
    }
}
