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
    pub text_format_title: IDWriteTextFormat,
    pub text_format_small: IDWriteTextFormat,
    // マウス状態
    pub mouse_pos: (f32, f32),
    pub is_clicking: bool,
    pub selected_tab: usize,
    pub focus_index: usize,
    pub is_focus_on_tabs: bool,
    pub event_proxy: winit::event_loop::EventLoopProxy<crate::image::loader::UserEvent>,
}

impl ModernSettingsWindow {
    pub fn new<T>(
        elwt: &EventLoopWindowTarget<T>,
        parent_hwnd: HWND,
        _settings: &Settings,
        event_proxy: winit::event_loop::EventLoopProxy<crate::image::loader::UserEvent>,
    ) -> Result<Self> {
        let window = Arc::new(
            WindowBuilder::new()
                .with_title("HayateViewer 設定")
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
                w!("Yu Gothic UI"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                15.0,
                w!("ja-jp"),
            )?;
            let text_format_title = dw_factory.CreateTextFormat(
                w!("Yu Gothic UI"),
                None,
                DWRITE_FONT_WEIGHT_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                22.0,
                w!("ja-jp"),
            )?;
            let text_format_small = dw_factory.CreateTextFormat(
                w!("Yu Gothic UI"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                13.0,
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
                text_format_title,
                text_format_small,
                mouse_pos: (0.0, 0.0),
                is_clicking: false,
                selected_tab: 0,
                focus_index: 0,
                is_focus_on_tabs: true,
                event_proxy,
            })
        }
    }

    pub fn handle_event(&mut self, event: &WindowEvent, settings: &Settings) -> bool {
        match event {
            WindowEvent::KeyboardInput { event: req, .. } => {
                if req.state == ElementState::Pressed {
                    use winit::keyboard::{Key, NamedKey};
                    match req.logical_key {
                        Key::Named(NamedKey::ArrowLeft) => {
                            if self.is_focus_on_tabs {
                                self.selected_tab = (self.selected_tab + 2) % 3;
                            } else {
                                self.handle_action_at(self.focus_index, settings);
                            }
                        }
                        Key::Named(NamedKey::ArrowRight) => {
                            if self.is_focus_on_tabs {
                                self.selected_tab = (self.selected_tab + 1) % 3;
                            } else {
                                self.handle_action_at(self.focus_index, settings);
                            }
                        }
                        Key::Named(NamedKey::ArrowDown) => {
                            if self.is_focus_on_tabs {
                                self.is_focus_on_tabs = false;
                                self.focus_index = 0;
                            } else {
                                let count = self.get_item_count();
                                if count > 0 {
                                    self.focus_index = (self.focus_index + 1) % count;
                                }
                            }
                        }
                        Key::Named(NamedKey::ArrowUp) => {
                            if !self.is_focus_on_tabs {
                                if self.focus_index == 0 {
                                    self.is_focus_on_tabs = true;
                                } else {
                                    self.focus_index -= 1;
                                }
                            }
                        }
                        Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Space) => {
                            if !self.is_focus_on_tabs {
                                self.handle_action_at(self.focus_index, settings);
                            }
                        }
                        Key::Named(NamedKey::Tab) => {
                            self.is_focus_on_tabs = !self.is_focus_on_tabs;
                            self.focus_index = 0;
                        }
                        Key::Named(NamedKey::Escape) => return true,
                        _ => {}
                    }
                }
                self.window.request_redraw();
                false
            }
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
                    self.handle_click(settings);
                }
                self.window.request_redraw();
                false
            }
            _ => false,
        }
    }

    fn handle_click(&mut self, settings: &Settings) {
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
                return;
            }
        }

        // 全般タブ内のクリック判定
        if self.selected_tab == 0 {
            let items = [210.0, 250.0, 290.0, 330.0];
            for (idx, &top) in items.iter().enumerate() {
                let rect = D2D_RECT_F {
                    left: 40.0,
                    top,
                    right: 200.0,
                    bottom: top + 30.0,
                };
                if self.is_in_rect(rect) {
                    self.is_focus_on_tabs = false;
                    self.focus_index = idx;
                    self.handle_action_at(idx, settings);
                    return;
                }
            }
        } else if self.selected_tab == 1 {
            let items = [210.0, 250.0, 290.0, 330.0];
            for (idx, &top) in items.iter().enumerate() {
                let rect = D2D_RECT_F {
                    left: 40.0,
                    top,
                    right: 200.0,
                    bottom: top + 30.0,
                };
                if self.is_in_rect(rect) {
                    self.is_focus_on_tabs = false;
                    self.focus_index = idx;
                    self.handle_action_at(idx, settings);
                    return;
                }
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
                b: 0.13,
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

            // タイトル描画 (日本語)
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
                top: 15.0,
                right: 480.0,
                bottom: 50.0,
            };
            self.context.DrawText(
                &wide_title,
                &self.text_format_title,
                &title_rect,
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );

            // タブ描画 (日本語)
            let tabs = ["全般", "レンダリング", "情報"];
            for (i, &name) in tabs.iter().enumerate() {
                let rect = D2D_RECT_F {
                    left: 20.0 + i as f32 * 110.0,
                    top: 70.0,
                    right: 120.0 + i as f32 * 110.0,
                    bottom: 105.0,
                };
                let is_hover = self.is_in_rect(rect);
                let is_selected = self.selected_tab == i;
                let is_focused = self.is_focus_on_tabs && is_selected;

                let bg_color = if is_selected {
                    D2D1_COLOR_F {
                        r: 0.0,
                        g: 0.47,
                        b: 0.83,
                        a: 1.0,
                    }
                } else if is_hover {
                    D2D1_COLOR_F {
                        r: 0.25,
                        g: 0.26,
                        b: 0.28,
                        a: 1.0,
                    }
                } else {
                    D2D1_COLOR_F {
                        r: 0.18,
                        g: 0.19,
                        b: 0.21,
                        a: 1.0,
                    }
                };
                self.brush.SetColor(&bg_color);
                let rounded_rect = D2D1_ROUNDED_RECT {
                    rect,
                    radiusX: 6.0,
                    radiusY: 6.0,
                };
                self.context
                    .FillRoundedRectangle(&rounded_rect, &self.brush);

                if is_focused {
                    self.brush.SetColor(&D2D1_COLOR_F {
                        r: 1.0,
                        g: 1.0,
                        b: 1.0,
                        a: 1.0,
                    });
                    self.context
                        .DrawRoundedRectangle(&rounded_rect, &self.brush, 2.0, None);
                }

                self.brush.SetColor(&D2D1_COLOR_F {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 1.0,
                });
                let wide_name: Vec<u16> = name.encode_utf16().collect();

                // テキストを中央揃えにするために一時的にプロパティを変更
                self.text_format
                    .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)
                    .unwrap();
                self.text_format
                    .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)
                    .unwrap();

                self.context.DrawText(
                    &wide_name,
                    &self.text_format,
                    &rect,
                    &self.brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );

                // 元に戻す
                self.text_format
                    .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)
                    .unwrap();
                self.text_format
                    .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)
                    .unwrap();
            }

            // 内容エリア背景
            self.brush.SetColor(&D2D1_COLOR_F {
                r: 0.14,
                g: 0.15,
                b: 0.17,
                a: 1.0,
            });
            self.context.FillRectangle(
                &D2D_RECT_F {
                    left: 20.0,
                    top: 110.0,
                    right: 480.0,
                    bottom: 580.0,
                },
                &self.brush,
            );

            match self.selected_tab {
                0 => self.draw_general_tab(settings),
                1 => self.draw_rendering_tab(settings),
                2 => self.draw_about_tab(settings),
                _ => {}
            }

            let _ = self.context.EndDraw(None, None);
            let _ = self.swap_chain.Present(1, DXGI_PRESENT(0));
        }
    }

    fn draw_general_tab(&self, settings: &Settings) {
        // ボタン描画
        let focus_idx = if !self.is_focus_on_tabs {
            Some(self.focus_index)
        } else {
            None
        };

        let guide_text = "■ 基本設定\n\n(※ 項目をクリック、または矢印キーとEnterで変更できます)";
        self.draw_debug_text(guide_text, 130.0);

        let display_mode_text = if !settings.is_spread_view {
            "単一ページ"
        } else if settings.binding_direction == "left" {
            "見開き・左綴じ（左開き）"
        } else {
            "見開き・右綴じ（右開き）"
        };
        let first_page_text = if settings.spread_view_first_page_single {
            "有効"
        } else {
            "無効"
        };
        let status_text = if settings.show_status_bar_info {
            "表示"
        } else {
            "非表示"
        };

        self.draw_button(
            "表示モード",
            display_mode_text,
            40.0,
            210.0,
            160.0,
            30.0,
            settings.is_spread_view,
            focus_idx == Some(0),
        );
        self.draw_button(
            "先頭単一表示",
            first_page_text,
            40.0,
            250.0,
            160.0,
            30.0,
            settings.spread_view_first_page_single,
            focus_idx == Some(1),
        );
        self.draw_button(
            "ステータスバー",
            status_text,
            40.0,
            290.0,
            160.0,
            30.0,
            settings.show_status_bar_info,
            focus_idx == Some(2),
        );
        self.draw_button(
            "ルーペ倍率",
            &format!("{:.1}x", settings.magnifier_zoom),
            40.0,
            330.0,
            160.0,
            30.0,
            false,
            focus_idx == Some(3),
        );
        self.draw_button(
            "履歴件数",
            &format!("{} 件", settings.max_history_count),
            40.0,
            370.0,
            160.0,
            30.0,
            false,
            focus_idx == Some(4),
        );
    }

    fn draw_button(
        &self,
        label: &str,
        value: &str,
        left: f32,
        top: f32,
        width: f32,
        height: f32,
        active: bool,
        focused: bool,
    ) {
        unsafe {
            let rect = D2D_RECT_F {
                left,
                top,
                right: left + width,
                bottom: top + height,
            };
            let is_hover = self.is_in_rect(rect);

            let bg_color = if active {
                D2D1_COLOR_F {
                    r: 0.0,
                    g: 0.45,
                    b: 0.85,
                    a: 1.0,
                }
            } else if is_hover || focused {
                D2D1_COLOR_F {
                    r: 0.3,
                    g: 0.32,
                    b: 0.35,
                    a: 1.0,
                }
            } else {
                D2D1_COLOR_F {
                    r: 0.22,
                    g: 0.23,
                    b: 0.25,
                    a: 1.0,
                }
            };

            self.brush.SetColor(&bg_color);
            self.context.FillRectangle(&rect, &self.brush);

            if focused {
                self.brush.SetColor(&D2D1_COLOR_F {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 1.0,
                });
                self.context.DrawRectangle(&rect, &self.brush, 1.5, None);
            }

            self.brush.SetColor(&D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            });
            let wide_label: Vec<u16> = label.encode_utf16().collect();
            self.context.DrawText(
                &wide_label,
                &self.text_format,
                &D2D_RECT_F {
                    left: rect.left + 5.0,
                    top: rect.top + 5.0,
                    right: rect.right - 5.0,
                    bottom: rect.bottom - 5.0,
                },
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );

            // 値の描画 (ボタンの右側)
            if !value.is_empty() {
                self.brush.SetColor(&D2D1_COLOR_F {
                    r: 0.8,
                    g: 0.8,
                    b: 0.8,
                    a: 1.0,
                });
                let wide_value: Vec<u16> = format!(": {}", value).encode_utf16().collect();
                let val_rect = D2D_RECT_F {
                    left: rect.right + 15.0,
                    top: rect.top + 5.0,
                    right: rect.right + 300.0,
                    bottom: rect.bottom - 5.0,
                };
                self.context.DrawText(
                    &wide_value,
                    &self.text_format,
                    &val_rect,
                    &self.brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
        }
    }

    fn draw_rendering_tab(&self, settings: &Settings) {
        // ボタン描画
        let focus_idx = if !self.is_focus_on_tabs {
            Some(self.focus_index)
        } else {
            None
        };

        let backend_display = match settings.rendering_backend.as_str() {
            "direct2d" => "Direct2D",
            "direct3d11" => "Direct3D 11",
            "opengl" => "OpenGL",
            b => b,
        };

        let guide_text = "■ レンダリング設定\n\n(※ バックエンド変更の反映には再起動が必要です)";
        self.draw_debug_text(guide_text, 130.0);

        self.draw_button(
            "レンダリングエンジン",
            backend_display,
            40.0,
            210.0,
            160.0,
            30.0,
            false,
            focus_idx == Some(0),
        );
        let cpu_res_text = match settings.resampling_mode_cpu.as_str() {
            "PIL_NEAREST" => "Nearest Neighbor (最近傍補間) [推奨]",
            "PIL_BILINEAR" => "Bilinear (双線形補間)",
            "PIL_BICUBIC" => "Bicubic (双三次補間)",
            "PIL_LANCZOS" => "Lanczos3 (ランツォシュ)",
            _ => &settings.resampling_mode_cpu,
        };
        self.draw_button(
            "CPUサンプリング",
            cpu_res_text,
            40.0,
            250.0,
            160.0,
            30.0,
            false,
            focus_idx == Some(1),
        );
        let gpu_res_text = match settings.resampling_mode_gpu.as_str() {
            "Nearest" => "Nearest Neighbor (最近傍補間)",
            "Linear" => "Bilinear (双線形補間)",
            "Cubic" => "Bicubic (双三次補間)",
            "Lanczos" => "Lanczos3 (ランツォシュ) [最高品質]",
            _ => &settings.resampling_mode_gpu,
        };
        self.draw_button(
            "GPUサンプリング",
            gpu_res_text,
            40.0,
            290.0,
            160.0,
            30.0,
            false,
            focus_idx == Some(2),
        );
        self.draw_button(
            "CPU色変換",
            if settings.use_cpu_color_conversion {
                "有効"
            } else {
                "無効"
            },
            40.0,
            330.0,
            160.0,
            30.0,
            settings.use_cpu_color_conversion,
            focus_idx == Some(3),
        );
    }

    fn draw_about_tab(&self, settings: &Settings) {
        let version = env!("CARGO_PKG_VERSION");

        unsafe {
            let mut ellipse = D2D1_ELLIPSE {
                point: std::mem::zeroed(),
                radiusX: 25.0,
                radiusY: 25.0,
            };
            ellipse.point.X = 70.0;
            ellipse.point.Y = 170.0;
            let icon_center = ellipse.point;

            // 青い輪
            self.brush.SetColor(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.5,
                b: 1.0,
                a: 1.0,
            });
            self.context.DrawEllipse(&ellipse, &self.brush, 3.0, None);

            // 中央の "i"
            let i_rect = D2D_RECT_F {
                left: icon_center.X - 10.0,
                top: icon_center.Y - 15.0,
                right: icon_center.X + 10.0,
                bottom: icon_center.Y + 15.0,
            };
            let wide_i: Vec<u16> = "i".encode_utf16().collect();
            self.brush.SetColor(&D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            });
            self.text_format_title
                .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)
                .unwrap();
            self.context.DrawText(
                &wide_i,
                &self.text_format_title,
                &i_rect,
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            self.text_format_title
                .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)
                .unwrap();

            // 2. タイトル
            let title_rect = D2D_RECT_F {
                left: 115.0,
                top: 153.0,
                right: 460.0,
                bottom: 200.0,
            };
            let title_text = "HayateViewer Rust";
            let wide_title: Vec<u16> = title_text.encode_utf16().collect();
            self.context.DrawText(
                &wide_title,
                &self.text_format_title,
                &title_rect,
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );

            // 3. 詳細リスト
            let start_x = 115.0;
            let start_y = 205.0;
            let row_height = 24.0;
            let label_width = 130.0;

            let infos = [
                ("Version", version),
                ("Renderer", &settings.rendering_backend),
                (
                    "Parallel Workers",
                    &settings.parallel_decoding_workers.to_string(),
                ),
                (
                    "CPU Resampling",
                    self.get_resampling_name(&settings.resampling_mode_cpu),
                ),
                (
                    "GPU Resampling",
                    self.get_resampling_name(&settings.resampling_mode_gpu),
                ),
                (
                    "Max Cache Size",
                    &format!("{} MB", settings.max_cache_size_mb),
                ),
                (
                    "Prefetch (CPU)",
                    &format!("{} pages", settings.cpu_max_prefetch_pages),
                ),
                (
                    "Prefetch (GPU)",
                    &format!("{} pages", settings.gpu_max_prefetch_pages),
                ),
                (
                    "Magnifier Zoom",
                    &format!("{:.1}x", settings.magnifier_zoom),
                ),
                ("OS", "Windows (x86_64)"),
                ("Developed by", "Tatsumaki Ishino\nKID Project Team"),
            ];

            for (i, (label, value)) in infos.iter().enumerate() {
                let y = start_y + i as f32 * row_height;

                // ラベル (グレー)
                self.brush.SetColor(&D2D1_COLOR_F {
                    r: 0.6,
                    g: 0.6,
                    b: 0.6,
                    a: 1.0,
                });
                let label_rect = D2D_RECT_F {
                    left: start_x,
                    top: y,
                    right: start_x + label_width,
                    bottom: y + row_height,
                };
                let wide_label: Vec<u16> = label.encode_utf16().collect();
                self.context.DrawText(
                    &wide_label,
                    &self.text_format,
                    &label_rect,
                    &self.brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );

                // 値 (白)
                self.brush.SetColor(&D2D1_COLOR_F {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 1.0,
                });
                let val_rect = D2D_RECT_F {
                    left: start_x + label_width,
                    top: y,
                    right: 460.0,
                    bottom: y + row_height * 2.0, // 改行に対応するため高さを確保
                };
                let wide_val: Vec<u16> = value.encode_utf16().collect();
                self.context.DrawText(
                    &wide_val,
                    &self.text_format,
                    &val_rect,
                    &self.brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            // 4. フッタークレジット
            let footer_text = "© 2024 Tatsumaki Ishino. All rights reserved.";
            let footer_rect = D2D_RECT_F {
                left: 40.0,
                top: 545.0,
                right: 460.0,
                bottom: 570.0,
            };
            let wide_footer: Vec<u16> = footer_text.encode_utf16().collect();
            self.brush.SetColor(&D2D1_COLOR_F {
                r: 0.4,
                g: 0.4,
                b: 0.4,
                a: 1.0,
            });
            self.context.DrawText(
                &wide_footer,
                &self.text_format_small,
                &footer_rect,
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    fn get_resampling_name(&self, mode: &str) -> &'static str {
        match mode {
            "PIL_NEAREST" | "Nearest" => "Nearest Neighbor",
            "PIL_BILINEAR" | "Bilinear" => "Bilinear",
            "PIL_BICUBIC" | "Bicubic" => "Bicubic",
            "Lanczos" | "Lanczos3" => "Lanczos3",
            _ => "Unknown",
        }
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

    fn get_item_count(&self) -> usize {
        match self.selected_tab {
            0 => 5, // 全般: 表示モード, 先頭単一, ステータスバー, ルーペ倍率, 履歴件数
            1 => 4, // レンダリング: エンジン, CPUサンプリング, GPUサンプリング, CPU色変換
            _ => 0,
        }
    }

    fn handle_action_at(&self, index: usize, settings: &Settings) {
        if self.selected_tab == 0 {
            match index {
                0 => {
                    let _ = self
                        .event_proxy
                        .send_event(crate::image::loader::UserEvent::RotateDisplayMode);
                }
                1 => {
                    let _ = self
                        .event_proxy
                        .send_event(crate::image::loader::UserEvent::ToggleFirstPageSingle);
                }
                2 => {
                    let _ = self
                        .event_proxy
                        .send_event(crate::image::loader::UserEvent::ToggleStatusBar);
                }
                3 => {
                    let next_zoom = if settings.magnifier_zoom >= 5.0 {
                        2.0
                    } else {
                        settings.magnifier_zoom + 0.5
                    };
                    let _ = self
                        .event_proxy
                        .send_event(crate::image::loader::UserEvent::SetMagnifierZoom(next_zoom));
                }
                4 => {
                    let next_count = if settings.max_history_count >= 50 {
                        10
                    } else {
                        settings.max_history_count + 10
                    };
                    let _ = self.event_proxy.send_event(
                        crate::image::loader::UserEvent::SetMaxHistoryCount(next_count),
                    );
                }
                _ => {}
            }
        } else if self.selected_tab == 1 {
            match index {
                0 => {
                    let _ = self
                        .event_proxy
                        .send_event(crate::image::loader::UserEvent::RotateRenderingBackend);
                }
                1 => {
                    let _ = self
                        .event_proxy
                        .send_event(crate::image::loader::UserEvent::RotateResamplingCpu);
                }
                2 => {
                    let _ = self
                        .event_proxy
                        .send_event(crate::image::loader::UserEvent::RotateResamplingGpu);
                }
                3 => {
                    let _ = self
                        .event_proxy
                        .send_event(crate::image::loader::UserEvent::ToggleCpuColorConversion);
                }
                _ => {}
            }
        }
    }
}
