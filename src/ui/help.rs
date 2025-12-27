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
use windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_BOLD;


pub struct HelpWindow {
    pub window: Arc<Window>,
    pub _factory: ID2D1Factory1,
    pub _device: ID2D1Device,
    pub context: ID2D1DeviceContext,
    pub swap_chain: IDXGISwapChain1,
    pub brush: ID2D1SolidColorBrush,
    pub text_format: IDWriteTextFormat,
    pub text_format_bold: IDWriteTextFormat, // 追加
    pub text_format_small: IDWriteTextFormat,
}

impl HelpWindow {
    pub fn new<T>(
        elwt: &EventLoopWindowTarget<T>,
        parent_hwnd: HWND,
    ) -> Result<Self> {
        let window = Arc::new(
            WindowBuilder::new()
                .with_title("HayateViewer ヘルプ")
                .with_inner_size(winit::dpi::LogicalSize::new(350.0, 650.0))
                .with_resizable(false)
                .build(elwt)
                .map_err(|e| Error::new(HRESULT(-1), format!("{}", e)))?,
        );

        let hwnd = match window.raw_window_handle() {
            RawWindowHandle::Win32(handle) => HWND(handle.hwnd as _),
            _ => return Err(Error::new(HRESULT(-1), "Unsupported window handle")),
        };

        // 親ウィンドウを設定
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
                None, // pFullscreenDesc (修正)
                None, // pRestrictToOutput (修正)
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
            let text_format_bold = dw_factory.CreateTextFormat( // 追加
                w!("Yu Gothic UI"),
                None,
                DWRITE_FONT_WEIGHT_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                15.0,
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
                text_format_bold,
                text_format_small,
            })
        }
    }

    /// イベント処理。ウィンドウを閉じる必要がある場合に true を返す。
    pub fn handle_event(&self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::KeyboardInput { event: req, .. } => {
                if req.state == ElementState::Pressed {
                    use winit::keyboard::{Key, NamedKey};
                    match req.logical_key {
                        Key::Named(NamedKey::Escape) => true,
                        _ => false,
                    }
                } else {
                    false
                }
            }
            WindowEvent::CloseRequested => true,
            _ => false,
        }
    }

    pub fn draw(&self) {
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
                    right: 350.0,
                    bottom: 60.0,
                },
                &self.brush,
            );

            // タイトル描画
            self.brush.SetColor(&D2D1_COLOR_F {
                r: 0.9,
                g: 0.9,
                b: 0.9,
                a: 1.0, // 修正
            });
            let title = "HayateViewer キーボードショートカット";
            let wide_title: Vec<u16> = title.encode_utf16().collect();
            let title_rect = D2D_RECT_F {
                left: 20.0,
                top: 15.0,
                right: 330.0,
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
                    top: 70.0,
                    right: 330.0,
                    bottom: 630.0,
                },
                &self.brush,
            );

            // ヘルプ項目
            let help_items = [
                ("--- ページ移動 ---", ""),
                ("ホイール / ← →", "次/前のページ"),
                ("Home / End", "最初/最後のページ"),
                ("PgUp / PgDown", "履歴ナビゲーション"),
                ("[ / ]", "前/次のフォルダまたはアーカイブ"),
                ("-----------------", ""),
                ("--- 表示操作 ---", ""),
                ("Ctrl + ホイール", "ズームイン/アウト"),
                ("+ / -", "ズームイン/アウト"),
                ("左ドラッグ (ズーム時)", "パン (画面移動)"),
                ("右クリック押しっぱなし", "ルーペ表示"),
                ("Numpad *", "ズームリセット"),
                ("-----------------", ""),
                ("--- 機能 ---", ""),
                ("O", "設定画面を開く"),
                ("R", "履歴画面を開く"),
                ("S", "シークバー表示切替"),
                ("Shift+S", "ページジャンプ"),
                ("F", "フォルダを開く"),
                ("Shift+F", "ファイルを直接開く"),
                ("H", "ヘルプ画面を開く"),
                ("Esc", "各種ウィンドウを閉じる"),
            ];
            
            let mut y = 80.0;
            let row_height = 20.0;
            let key_width = 150.0;

            for (key, desc) in help_items.iter() {
                // キー（左側、太字）
                let is_section = desc.is_empty();
                
                self.brush.SetColor(&D2D1_COLOR_F {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 1.0,
                });
                
                let text_format = if is_section {
                    &self.text_format_small
                } else {
                    &self.text_format_bold // SetFontWeightの代わりにtext_format_boldを使用
                };
                
                let key_rect = D2D_RECT_F {
                    left: 30.0,
                    top: y,
                    right: 30.0 + key_width,
                    bottom: y + row_height,
                };
                let wide_key: Vec<u16> = key.encode_utf16().collect();
                self.context.DrawText(
                    &wide_key,
                    text_format,
                    &key_rect,
                    &self.brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );

                // 説明（右側、通常）
                if !is_section {
                    self.brush.SetColor(&D2D1_COLOR_F {
                        r: 0.7,
                        g: 0.7,
                        b: 0.7,
                        a: 1.0,
                    });
                    
                    let desc_rect = D2D_RECT_F {
                        left: 30.0 + key_width,
                        top: y,
                        right: 320.0,
                        bottom: y + row_height,
                    };
                    let wide_desc: Vec<u16> = desc.encode_utf16().collect();
                    self.context.DrawText(
                        &wide_desc,
                        &self.text_format, // 通常のtext_formatを使用
                        &desc_rect,
                        &self.brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
                
                // セクションタイトルは行高さを広く取る
                y += if is_section { row_height * 1.5 } else { row_height };
            }

            let _ = self.context.EndDraw(None, None);
            let _ = self.swap_chain.Present(1, DXGI_PRESENT(0));
        }
    }
}
