use super::{InterpolationMode, PageDrawInfo, Renderer, TextureHandle};
use crate::image::cache::{DecodedImage, PixelData};
use crate::state::BindingDirection;
use std::mem::ManuallyDrop;
use windows::{
    Win32::Foundation::*, Win32::Graphics::Direct2D::Common::*, Win32::Graphics::Direct2D::*,
    Win32::Graphics::Direct3D::*, Win32::Graphics::Direct3D11::*, Win32::Graphics::DirectWrite::*,
    Win32::Graphics::Dxgi::Common::*, Win32::Graphics::Dxgi::*, core::*,
};

pub struct D3D11Renderer {
    #[allow(dead_code)]
    pub device: ID3D11Device,
    #[allow(dead_code)]
    pub context: ID3D11DeviceContext,
    pub swap_chain: IDXGISwapChain1,
    // D2D Interop
    pub d2d_context: ID2D1DeviceContext,
    pub brush: ID2D1SolidColorBrush,
    #[allow(dead_code)]
    pub dw_factory: IDWriteFactory,
    pub text_format: IDWriteTextFormat,
    pub text_format_large: IDWriteTextFormat,
    pub interpolation_mode: D2D1_INTERPOLATION_MODE,
    pub shader_interpolation_mode: i32, // 0=Nearest, 1=Linear, 2=Cubic, 3=Lanczos

    // D3D11 Resources
    vertex_shader: ID3D11VertexShader,
    input_layout: ID3D11InputLayout,
    _pixel_shader_rgba: ID3D11PixelShader,
    pixel_shader_ycbcr: ID3D11PixelShader,
    vertex_buffer: ID3D11Buffer,
    constant_buffer: ID3D11Buffer,
    sampler_linear: ID3D11SamplerState,
    sampler_nearest: ID3D11SamplerState,
    rasterizer_state: ID3D11RasterizerState,
}

use windows::Win32::Graphics::Direct3D::Fxc::*;

#[repr(C)]
struct Vertex {
    position: [f32; 3],
    tex_coord: [f32; 2],
}

#[repr(C)]
struct YCbCrConstants {
    color_matrix: [[f32; 4]; 4],
    offset: [f32; 4],
    scale: [f32; 4],
    interpolation_mode: i32,
    _padding: [i32; 3], // 16バイトアライメント用パディング
}

fn compile_shader(source: &[u8], entry_point: &str, target: &str) -> Result<ID3DBlob> {
    unsafe {
        let mut error_msgs: Option<ID3DBlob> = None;
        let mut blob: Option<ID3DBlob> = None;

        let entry_point = std::ffi::CString::new(entry_point).unwrap();
        let target = std::ffi::CString::new(target).unwrap();

        let res = D3DCompile(
            source.as_ptr() as _,
            source.len(),
            None,
            None,
            None,
            PCSTR(entry_point.as_ptr() as _),
            PCSTR(target.as_ptr() as _),
            0,
            0,
            &mut blob,
            Some(&mut error_msgs),
        );

        if let Err(e) = res {
            if let Some(errors) = error_msgs {
                let ptr = errors.GetBufferPointer();
                let size = errors.GetBufferSize();
                let slice = std::slice::from_raw_parts(ptr as *const u8, size);
                let msg = String::from_utf8_lossy(slice);
                let error_msg = format!("Shader compile error: {}\n{}", e, msg);
                return Err(Error::new(E_FAIL, &error_msg));
            }
            return Err(e.into());
        }

        Ok(blob.unwrap())
    }
}

impl Renderer for D3D11Renderer {
    fn resize(
        &self,
        width: u32,
        height: u32,
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        unsafe {
            // D2D ターゲットを解放
            self.d2d_context.SetTarget(None);

            self.swap_chain.ResizeBuffers(
                0,
                width,
                height,
                DXGI_FORMAT_UNKNOWN,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;

            // D2D ターゲットの再作成
            let surface: IDXGISurface = self.swap_chain.GetBuffer(0)?;
            let d2d_bitmap = self
                .d2d_context
                .CreateBitmapFromDxgiSurface(&surface, None)?;
            self.d2d_context.SetTarget(&d2d_bitmap);
        }
        Ok(())
    }

    fn begin_draw(&self) {
        unsafe {
            self.d2d_context.BeginDraw();
            self.d2d_context.Clear(Some(&D2D1_COLOR_F {
                r: 0.1,
                g: 0.1,
                b: 0.1,
                a: 0.8,
            }));
        }
    }

    fn end_draw(&self) -> std::result::Result<(), Box<dyn std::error::Error>> {
        unsafe {
            self.d2d_context.EndDraw(None, None)?;
            self.swap_chain.Present(1, DXGI_PRESENT(0)).ok()?;
        }
        Ok(())
    }

    fn upload_image(
        &self,
        image: &DecodedImage,
    ) -> std::result::Result<TextureHandle, Box<dyn std::error::Error>> {
        match &image.pixel_data {
            PixelData::Rgba8(data) => {
                // D2D ビットマップとして作成（既存の D2D Interop を活用）
                let bitmap: ID2D1Bitmap1 = self
                    .create_d2d_bitmap(image.width, image.height, data)
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
                Ok(TextureHandle::Direct2D(bitmap))
            }
            PixelData::Ycbcr {
                planes,
                subsampling,
                precision,
                y_is_signed,
                c_is_signed,
            } => {
                if planes.len() != 3 {
                    return Err("Invalid plane count for YCbCr".into());
                }

                let y_srv = self.create_r32_texture(image.width, image.height, &planes[0])?;
                let (dx, dy) = *subsampling;
                let c_width = (image.width + dx as u32 - 1) / dx as u32;
                let c_height = (image.height + dy as u32 - 1) / dy as u32;

                let cb_srv = self.create_r32_texture(c_width, c_height, &planes[1])?;
                let cr_srv = self.create_r32_texture(c_width, c_height, &planes[2])?;

                Ok(TextureHandle::D3D11YCbCr {
                    y: y_srv,
                    cb: cb_srv,
                    cr: cr_srv,
                    width: image.width,
                    height: image.height,
                    _subsampling: *subsampling,
                    _precision: *precision,
                    y_is_signed: *y_is_signed,
                    c_is_signed: *c_is_signed,
                })
            }
        }
    }

    fn draw_image(&self, texture: &TextureHandle, dest_rect: &D2D_RECT_F) {
        match texture {
            TextureHandle::Direct2D(bitmap) => unsafe {
                self.d2d_context.DrawBitmap(
                    bitmap,
                    Some(dest_rect),
                    1.0,
                    self.interpolation_mode,
                    None,
                    None,
                );
            },
            TextureHandle::D3D11YCbCr {
                y,
                cb,
                cr,
                width: _,
                height: _,
                _subsampling: _,
                _precision: precision,
                y_is_signed,
                c_is_signed,
            } => {
                unsafe {
                    // D2D の描画コンテキストを一時的に閉じる
                    self.d2d_context.EndDraw(None, None).unwrap();

                    let viewport = D3D11_VIEWPORT {
                        TopLeftX: dest_rect.left,
                        TopLeftY: dest_rect.top,
                        Width: dest_rect.right - dest_rect.left,
                        Height: dest_rect.bottom - dest_rect.top,
                        MinDepth: 0.0,
                        MaxDepth: 1.0,
                    };
                    self.context.RSSetViewports(Some(&[viewport]));
                    self.context.RSSetState(&self.rasterizer_state);

                    // Create RTV
                    let back_buffer: ID3D11Texture2D = self.swap_chain.GetBuffer(0).unwrap();
                    let mut rtv: Option<ID3D11RenderTargetView> = None;
                    self.device
                        .CreateRenderTargetView(&back_buffer, None, Some(&mut rtv))
                        .unwrap();

                    let targets = [rtv];
                    self.context.OMSetRenderTargets(Some(&targets), None);

                    // Shaders
                    self.context.VSSetShader(&self.vertex_shader, None);
                    self.context.PSSetShader(&self.pixel_shader_ycbcr, None);

                    // Input Layout
                    self.context.IASetInputLayout(&self.input_layout);
                    self.context
                        .IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);

                    // Resources
                    let stride = std::mem::size_of::<Vertex>() as u32;
                    let offset = 0;
                    let buffers = [Some(self.vertex_buffer.clone())];
                    self.context.IASetVertexBuffers(
                        0,
                        1,
                        Some(buffers.as_ptr()),
                        Some(&stride),
                        Some(&offset),
                    );

                    // Cb/Cr Swap 解除: 元の順序に戻す
                    let views = [Some(y.clone()), Some(cb.clone()), Some(cr.clone())];
                    self.context.PSSetShaderResources(0, Some(&views));

                    let sampler = match self.interpolation_mode {
                        D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR => &self.sampler_nearest,
                        _ => &self.sampler_linear,
                    };
                    let samplers = [Some(sampler.clone())];
                    self.context.PSSetSamplers(0, Some(&samplers));

                    // Constants Update
                    // Scale: input(255) -> 1.0 に正規化
                    let max_val = ((1u32 << precision) - 1) as f32;
                    let scale_val = 1.0 / max_val;

                    // OpenJPEG のデータ形式に合わせたオフセット
                    // Unsigned (0..2^n-1) なら Cb/Cr を -0.5 シフトして 0 中心にする
                    // Signed なら既に 0 中心なのでオフセット不要
                    let y_offset = if *y_is_signed { 0.5 } else { 0.0 }; // Y は符号付きなら +0.5 (DC offset)
                    let c_offset = if *c_is_signed { 0.0 } else { -0.5 };

                    let constants = YCbCrConstants {
                        // D3D11/HLSL のデフォルト（Column-Major）に合わせたパッキング
                        // mul(v, M) は dot(v, Col_i) を行う。
                        // Rust 側の各行[0, 1, 2, 3]がそのまま HLSL の Col_i にマッピングされるため、
                        // 各行に出力チャンネルごとの重みを定義する。
                        color_matrix: [
                            [1.0, 1.772, 0.0, 0.0],           // Weights for Result.x (Blue)
                            [1.0, -0.344136, -0.714136, 0.0], // Weights for Result.y (Green)
                            [1.0, 0.0, 1.402, 0.0],           // Weights for Result.z (Red)
                            [0.0, 0.0, 0.0, 1.0],             // Weights for Result.w (Alpha)
                        ],
                        offset: [y_offset, c_offset, c_offset, 0.0],
                        scale: [scale_val, scale_val, scale_val, 1.0],
                        interpolation_mode: self.shader_interpolation_mode,
                        _padding: [0, 0, 0],
                    };

                    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                    self.context
                        .Map(
                            &self.constant_buffer,
                            0,
                            D3D11_MAP_WRITE_DISCARD,
                            0,
                            Some(&mut mapped),
                        )
                        .unwrap();
                    std::ptr::copy_nonoverlapping(
                        &constants,
                        mapped.pData as *mut YCbCrConstants,
                        1,
                    );
                    self.context.Unmap(&self.constant_buffer, 0);

                    let cbufs = [Some(self.constant_buffer.clone())];
                    self.context.PSSetConstantBuffers(0, Some(&cbufs));

                    self.context.Draw(4, 0);

                    // D2D 描画再開
                    self.d2d_context.BeginDraw();
                }
            }
            _ => {}
        }
    }

    fn get_texture_size(&self, texture: &TextureHandle) -> (f32, f32) {
        match texture {
            TextureHandle::Direct2D(bitmap) => unsafe {
                let size = bitmap.GetSize();
                (size.width, size.height)
            },
            TextureHandle::D3D11YCbCr { width, height, .. } => (*width as f32, *height as f32),
            _ => (0.0, 0.0),
        }
    }

    fn fill_rectangle(&self, rect: &D2D_RECT_F, color: &D2D1_COLOR_F) {
        unsafe {
            self.brush.SetColor(color);
            self.d2d_context.FillRectangle(rect, &self.brush);
        }
    }

    fn draw_rectangle(&self, rect: &D2D_RECT_F, color: &D2D1_COLOR_F, stroke_width: f32) {
        unsafe {
            self.brush.SetColor(color);
            self.d2d_context
                .DrawRectangle(rect, &self.brush, stroke_width, None);
        }
    }

    fn draw_text(&self, text: &str, rect: &D2D_RECT_F, color: &D2D1_COLOR_F, large: bool) {
        unsafe {
            self.brush.SetColor(color);
            let wide_text: Vec<u16> = text.encode_utf16().collect();
            let format = if large {
                &self.text_format_large
            } else {
                &self.text_format
            };
            self.d2d_context.DrawText(
                &wide_text,
                format,
                rect,
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    fn set_interpolation_mode(&mut self, mode: InterpolationMode) {
        self.interpolation_mode = match mode {
            InterpolationMode::NearestNeighbor => D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
            InterpolationMode::Linear => D2D1_INTERPOLATION_MODE_LINEAR,
            InterpolationMode::Cubic => D2D1_INTERPOLATION_MODE_CUBIC,
            InterpolationMode::Lanczos => D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, // D2D用フォールバック
        };
        self.shader_interpolation_mode = match mode {
            InterpolationMode::NearestNeighbor => 0,
            InterpolationMode::Linear => 1,
            InterpolationMode::Cubic => 2,
            InterpolationMode::Lanczos => 3,
        };
    }

    fn set_text_alignment(&self, alignment: DWRITE_TEXT_ALIGNMENT) {
        unsafe {
            let _ = self.text_format.SetTextAlignment(alignment);
            let _ = self
                .text_format
                .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let _ = self.text_format_large.SetTextAlignment(alignment);
            let _ = self
                .text_format_large
                .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        }
    }

    fn supports_page_turn_animation(&self) -> bool {
        true // D3D11はページめくりアニメーションをサポート
    }

    fn draw_page_turn(
        &self,
        progress: f32,
        direction: i32,
        binding: BindingDirection,
        from_pages: &[PageDrawInfo],
        to_pages: &[PageDrawInfo],
        dest_rect: &D2D_RECT_F,
    ) {
        // シンプルなスライドアニメーション（後で3Dカール効果に拡張予定）
        // progress: 0.0 = 遷移開始（from表示）、1.0 = 遷移完了（to表示）

        let width = dest_rect.right - dest_rect.left;
        let eased = 1.0 - (1.0 - progress).powi(3); // ease-out cubic

        // スライド方向の決定
        // ユーザーフィードバックに基づき符号を反転
        let slide_direction = match (binding, direction) {
            (BindingDirection::Right, 1) => 1.0,
            (BindingDirection::Right, _) => -1.0,
            (BindingDirection::Left, 1) => -1.0,
            (BindingDirection::Left, _) => 1.0,
        };

        let offset = width * eased * slide_direction;

        // 遷移前のページを描画（スライドアウト）
        for page in from_pages {
            let mut page_rect = page.dest_rect;
            // X座標をオフセット分ずらす
            page_rect.left += offset;
            page_rect.right += offset;

            // 画面外にスライドアウトしている場合でも描画（クリッピングはレンダラー側で処理）
            if page_rect.right > 0.0 && page_rect.left < dest_rect.right + width {
                self.draw_image(page.texture, &page_rect);
            }
        }

        // 遷移後のページを描画（スライドイン）
        let to_offset = offset - width * slide_direction;
        for page in to_pages {
            let mut page_rect = page.dest_rect;
            page_rect.left += to_offset;
            page_rect.right += to_offset;

            if page_rect.right > 0.0 && page_rect.left < dest_rect.right + width {
                self.draw_image(page.texture, &page_rect);
            }
        }
    }
}

impl D3D11Renderer {
    pub fn new(hwnd: HWND) -> Result<Self> {
        unsafe {
            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;

            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;

            let device = device.unwrap();
            let context = context.unwrap();

            let dxgi_device: IDXGIDevice = device.cast()?;
            let dxgi_adapter: IDXGIAdapter = dxgi_device.GetAdapter()?;
            let dxgi_factory: IDXGIFactory2 = dxgi_adapter.GetParent()?;

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

            let swap_chain =
                dxgi_factory.CreateSwapChainForHwnd(&device, hwnd, &swap_chain_desc, None, None)?;

            // D2D Interop
            let d2d_factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let d2d_device = d2d_factory.CreateDevice(&dxgi_device)?;
            let d2d_context = d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;

            let surface: IDXGISurface = swap_chain.GetBuffer(0)?;
            let d2d_bitmap = d2d_context.CreateBitmapFromDxgiSurface(&surface, None)?;
            d2d_context.SetTarget(&d2d_bitmap);

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

            let text_format_large = dw_factory.CreateTextFormat(
                w!("Segoe UI"),
                None,
                DWRITE_FONT_WEIGHT_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                24.0,
                w!("ja-jp"),
            )?;

            let brush = d2d_context.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 1.0,
                },
                None,
            )?;

            // --- Shader Compilation & Resource Creation ---

            let quad_src = include_bytes!("shaders/texture_quad.hlsl");
            let ycbcr_src = include_bytes!("shaders/ycbcr_to_rgb.hlsl");

            let vs_blob = compile_shader(ycbcr_src, "VSMain", "vs_5_0")?;
            let mut vertex_shader: Option<ID3D11VertexShader> = None;
            device.CreateVertexShader(
                std::slice::from_raw_parts(
                    vs_blob.GetBufferPointer() as *const u8,
                    vs_blob.GetBufferSize(),
                ),
                None,
                Some(&mut vertex_shader),
            )?;
            let vertex_shader = vertex_shader.unwrap();

            let ps_rgba_blob = compile_shader(quad_src, "PSMain", "ps_5_0")?;
            let mut pixel_shader_rgba: Option<ID3D11PixelShader> = None;
            device.CreatePixelShader(
                std::slice::from_raw_parts(
                    ps_rgba_blob.GetBufferPointer() as *const u8,
                    ps_rgba_blob.GetBufferSize(),
                ),
                None,
                Some(&mut pixel_shader_rgba),
            )?;
            let pixel_shader_rgba = pixel_shader_rgba.unwrap();

            let ps_ycbcr_blob = compile_shader(ycbcr_src, "PSMain_Generic", "ps_5_0")?;
            let mut pixel_shader_ycbcr: Option<ID3D11PixelShader> = None;
            device.CreatePixelShader(
                std::slice::from_raw_parts(
                    ps_ycbcr_blob.GetBufferPointer() as *const u8,
                    ps_ycbcr_blob.GetBufferSize(),
                ),
                None,
                Some(&mut pixel_shader_ycbcr),
            )?;
            let pixel_shader_ycbcr = pixel_shader_ycbcr.unwrap();

            // Input Layout
            let input_element_descs = [
                D3D11_INPUT_ELEMENT_DESC {
                    SemanticName: PCSTR(b"POSITION\0".as_ptr()),
                    SemanticIndex: 0,
                    Format: DXGI_FORMAT_R32G32B32_FLOAT,
                    InputSlot: 0,
                    AlignedByteOffset: 0,
                    InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                    InstanceDataStepRate: 0,
                },
                D3D11_INPUT_ELEMENT_DESC {
                    SemanticName: PCSTR(b"TEXCOORD\0".as_ptr()),
                    SemanticIndex: 0,
                    Format: DXGI_FORMAT_R32G32_FLOAT,
                    InputSlot: 0,
                    AlignedByteOffset: 12, // 3 * 4 bytes
                    InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                    InstanceDataStepRate: 0,
                },
            ];

            let mut input_layout: Option<ID3D11InputLayout> = None;
            device.CreateInputLayout(
                &input_element_descs,
                std::slice::from_raw_parts(
                    vs_blob.GetBufferPointer() as *const u8,
                    vs_blob.GetBufferSize(),
                ),
                Some(&mut input_layout),
            )?;
            let input_layout = input_layout.unwrap();

            // Vertex Buffer (Full screen quad, Triangle Strip)
            let vertices = [
                Vertex {
                    position: [-1.0, 1.0, 0.0],
                    tex_coord: [0.0, 0.0],
                }, // Top-Left
                Vertex {
                    position: [1.0, 1.0, 0.0],
                    tex_coord: [1.0, 0.0],
                }, // Top-Right
                Vertex {
                    position: [-1.0, -1.0, 0.0],
                    tex_coord: [0.0, 1.0],
                }, // Bottom-Left
                Vertex {
                    position: [1.0, -1.0, 0.0],
                    tex_coord: [1.0, 1.0],
                }, // Bottom-Right
            ];

            let vb_desc = D3D11_BUFFER_DESC {
                ByteWidth: (std::mem::size_of::<Vertex>() * vertices.len()) as u32,
                Usage: D3D11_USAGE_IMMUTABLE,
                BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32, // ここも cast
                CPUAccessFlags: 0,
                MiscFlags: 0,
                StructureByteStride: 0,
            };

            let vb_data = D3D11_SUBRESOURCE_DATA {
                pSysMem: vertices.as_ptr() as _,
                SysMemPitch: 0,
                SysMemSlicePitch: 0,
            };

            let mut vertex_buffer: Option<ID3D11Buffer> = None;
            device.CreateBuffer(&vb_desc, Some(&vb_data), Some(&mut vertex_buffer))?;
            let vertex_buffer = vertex_buffer.unwrap();

            // Constant Buffer
            let cb_desc = D3D11_BUFFER_DESC {
                ByteWidth: std::mem::size_of::<YCbCrConstants>() as u32, // 16byte alignment check? -> assume it fits 16 bytes multiple
                Usage: D3D11_USAGE_DYNAMIC,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                MiscFlags: 0,
                StructureByteStride: 0,
            };

            let mut constant_buffer: Option<ID3D11Buffer> = None;
            device.CreateBuffer(&cb_desc, None, Some(&mut constant_buffer))?; // created empty
            let constant_buffer = constant_buffer.unwrap();

            // Sampler State
            let sampler_desc = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                MaxAnisotropy: 1,
                ComparisonFunc: D3D11_COMPARISON_ALWAYS,
                MinLOD: 0.0,
                MaxLOD: D3D11_FLOAT32_MAX,
                ..Default::default()
            };

            let mut sampler_linear: Option<ID3D11SamplerState> = None;
            device.CreateSamplerState(&sampler_desc, Some(&mut sampler_linear))?;
            let sampler_linear = sampler_linear.unwrap();

            let mut sampler_desc_nearest = sampler_desc;
            sampler_desc_nearest.Filter = D3D11_FILTER_MIN_MAG_MIP_POINT;
            let mut sampler_nearest: Option<ID3D11SamplerState> = None;
            device.CreateSamplerState(&sampler_desc_nearest, Some(&mut sampler_nearest))?;
            let sampler_nearest = sampler_nearest.unwrap();

            // Rasterizer State (Cull None)
            let rs_desc = D3D11_RASTERIZER_DESC {
                FillMode: D3D11_FILL_SOLID,
                CullMode: D3D11_CULL_NONE,
                FrontCounterClockwise: false.into(),
                DepthBias: 0,
                DepthBiasClamp: 0.0,
                SlopeScaledDepthBias: 0.0,
                DepthClipEnable: true.into(),
                ScissorEnable: false.into(),
                MultisampleEnable: false.into(),
                AntialiasedLineEnable: false.into(),
            };
            let mut rasterizer_state: Option<ID3D11RasterizerState> = None;
            device.CreateRasterizerState(&rs_desc, Some(&mut rasterizer_state))?;
            let rasterizer_state = rasterizer_state.unwrap();

            Ok(Self {
                device,
                context,
                swap_chain,
                d2d_context,
                brush,
                dw_factory,
                text_format,
                text_format_large,
                interpolation_mode: D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                shader_interpolation_mode: 2, // デフォルトは Cubic

                vertex_shader,
                input_layout,
                _pixel_shader_rgba: pixel_shader_rgba,
                pixel_shader_ycbcr,
                vertex_buffer,
                constant_buffer,
                sampler_linear,
                sampler_nearest,
                rasterizer_state,
            })
        }
    }

    pub fn create_d2d_bitmap(&self, width: u32, height: u32, data: &[u8]) -> Result<ID2D1Bitmap1> {
        unsafe {
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_R8G8B8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                colorContext: ManuallyDrop::new(None),
            };

            self.d2d_context.CreateBitmap(
                D2D_SIZE_U { width, height },
                Some(data.as_ptr() as _),
                width * 4,
                &props,
            )
        }
    }

    pub fn create_r32_texture(
        &self,
        width: u32,
        height: u32,
        data: &[i32],
    ) -> Result<ID3D11ShaderResourceView> {
        unsafe {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_R32_SINT,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };

            let init_data = D3D11_SUBRESOURCE_DATA {
                pSysMem: data.as_ptr() as _,
                SysMemPitch: width * 4, // 4 bytes per i32
                SysMemSlicePitch: 0,
            };

            let mut texture: Option<ID3D11Texture2D> = None;
            self.device
                .CreateTexture2D(&desc, Some(&init_data), Some(&mut texture))?;
            let texture = texture.unwrap();

            let srv_desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
                Format: desc.Format,
                ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_SRV {
                        MostDetailedMip: 0,
                        MipLevels: 1,
                    },
                },
            };

            let resource: ID3D11Resource = texture.cast()?;
            let mut srv: Option<ID3D11ShaderResourceView> = None;
            self.device
                .CreateShaderResourceView(&resource, Some(&srv_desc), Some(&mut srv))?;
            Ok(srv.unwrap())
        }
    }
}
