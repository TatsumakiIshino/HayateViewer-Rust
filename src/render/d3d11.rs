use super::{InterpolationMode, Renderer, TextureHandle};
use crate::image::cache::{DecodedImage, PixelData};

use windows::{
    Win32::Foundation::*, Win32::Graphics::Direct2D::Common::*, Win32::Graphics::Direct3D::*,
    Win32::Graphics::Direct3D11::*, Win32::Graphics::DirectWrite::*,
    Win32::Graphics::Dxgi::Common::*, Win32::Graphics::Dxgi::*, core::*,
};

pub struct D3D11Renderer {
    #[allow(dead_code)]
    pub device: ID3D11Device,
    #[allow(dead_code)]
    pub context: ID3D11DeviceContext,
    pub swap_chain: IDXGISwapChain1,

    // D3D11 Resources
    render_target_view: ID3D11RenderTargetView,
    vertex_shader: ID3D11VertexShader,
    input_layout: ID3D11InputLayout,
    pixel_shader_rgba: ID3D11PixelShader,
    pixel_shader_ycbcr: ID3D11PixelShader,
    vertex_buffer: ID3D11Buffer,
    constant_buffer: ID3D11Buffer,
    sampler_linear: ID3D11SamplerState,
    sampler_nearest: ID3D11SamplerState,
    rasterizer_state: ID3D11RasterizerState,

    // Settings
    pub interpolation_mode: InterpolationMode,
    pub text_alignment: std::sync::atomic::AtomicI32, // GDI 用
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
        _width: u32,
        _height: u32,
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        // スワップチェーンの自動スケーリング (DXGI_SCALING_STRETCH) に任せるため何もしない
        Ok(())
    }

    fn begin_draw(&self) {
        unsafe {
            let rtv = self.render_target_view.clone();
            // 背景色 (ダークグレー)
            let clear_color = [0.1, 0.1, 0.1, 1.0];
            self.context.ClearRenderTargetView(&rtv, &clear_color);

            // ビューポートをバックバッファ全体に設定
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            let back_buffer: ID3D11Texture2D = self.swap_chain.GetBuffer(0).unwrap();
            back_buffer.GetDesc(&mut desc);

            let viewport = D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: desc.Width as f32,
                Height: desc.Height as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            };
            self.context.RSSetViewports(Some(&[viewport]));
        }
    }

    fn end_draw(&self) -> std::result::Result<(), Box<dyn std::error::Error>> {
        unsafe {
            // VSync ON で待機
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
                let srv = self.create_rgba_texture(image.width, image.height, data)?;
                Ok(TextureHandle::D3D11Rgba(srv))
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
                    _c_is_signed: *c_is_signed,
                })
            }
        }
    }

    fn draw_image(&self, texture: &TextureHandle, dest_rect: &D2D_RECT_F) {
        unsafe {
            // ビューポートを描画領域に合わせて設定
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

            // レンダーターゲット設定
            let rtv = [Some(self.render_target_view.clone())];
            self.context.OMSetRenderTargets(Some(&rtv), None);

            // シェーダー設定
            self.context.VSSetShader(&self.vertex_shader, None);

            // Input Layout
            self.context.IASetInputLayout(&self.input_layout);
            self.context
                .IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);

            // Vertex Buffer
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

            // Sampler
            let sampler = match self.interpolation_mode {
                InterpolationMode::NearestNeighbor => &self.sampler_nearest,
                _ => &self.sampler_linear,
            };
            self.context
                .PSSetSamplers(0, Some(&[Some(sampler.clone())]));

            match texture {
                TextureHandle::D3D11Rgba(srv) => {
                    self.context.PSSetShader(&self.pixel_shader_rgba, None);
                    self.context
                        .PSSetShaderResources(0, Some(&[Some(srv.clone())]));
                    self.context.Draw(4, 0);
                }
                TextureHandle::D3D11YCbCr {
                    y,
                    cb,
                    cr,
                    _precision: precision,
                    y_is_signed,
                    ..
                } => {
                    self.context.PSSetShader(&self.pixel_shader_ycbcr, None);

                    let views = [Some(y.clone()), Some(cb.clone()), Some(cr.clone())];
                    self.context.PSSetShaderResources(0, Some(&views));

                    // Constants
                    let max_val = ((1u32 << precision) - 1) as f32;
                    let scale_val = 1.0 / max_val;
                    let y_offset = 0.0;
                    let c_offset = -128.0;

                    let constants = YCbCrConstants {
                        color_matrix: [
                            [1.0, 1.0, 1.0, 0.0],           // Y contribution to RGB
                            [0.0, -0.344136, 1.772, 0.0],  // Cb contribution to RGB
                            [1.402, -0.714136, 0.0, 0.0],  // Cr contribution to RGB
                            [0.0, 0.0, 0.0, 1.0],          // Constant
                        ],
                        offset: [y_offset, c_offset, c_offset, 0.0],
                        scale: [scale_val, scale_val, scale_val, 1.0],
                        interpolation_mode: match self.interpolation_mode {
                            InterpolationMode::NearestNeighbor => 0,
                            InterpolationMode::Linear => 1,
                            InterpolationMode::Cubic => 2,
                            InterpolationMode::Lanczos => 3,
                        },
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

                    self.context
                        .PSSetConstantBuffers(0, Some(&[Some(self.constant_buffer.clone())]));

                    self.context.Draw(4, 0);
                }
                _ => {}
            }
        }
    }

    fn get_texture_size(&self, texture: &TextureHandle) -> (f32, f32) {
        match texture {
            TextureHandle::D3D11Rgba(srv) => unsafe {
                if let Ok(res) = srv.GetResource() {
                    let texture2d: ID3D11Texture2D = res.cast().unwrap();
                    let mut desc = D3D11_TEXTURE2D_DESC::default();
                    texture2d.GetDesc(&mut desc);
                    (desc.Width as f32, desc.Height as f32)
                } else {
                    (0.0, 0.0)
                }
            },
            TextureHandle::D3D11YCbCr { width, height, .. } => (*width as f32, *height as f32),
            _ => (0.0, 0.0),
        }
    }

    fn fill_rectangle(&self, _rect: &D2D_RECT_F, _color: &D2D1_COLOR_F) {}

    fn draw_rectangle(&self, _rect: &D2D_RECT_F, _color: &D2D1_COLOR_F, _stroke_width: f32) {}

    fn draw_text(&self, text: &str, rect: &D2D_RECT_F, color: &D2D1_COLOR_F, large: bool) {
        self.draw_text_internal(text, rect, color, large);
    }

    fn set_interpolation_mode(&mut self, mode: InterpolationMode) {
        self.interpolation_mode = mode;
    }

    fn set_text_alignment(&self, alignment: DWRITE_TEXT_ALIGNMENT) {
        self.text_alignment
            .store(alignment.0, std::sync::atomic::Ordering::Relaxed);
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
                Format: DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
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

            // Create RenderTargetView
            let back_buffer: ID3D11Texture2D = swap_chain.GetBuffer(0)?;
            let mut rtv: Option<ID3D11RenderTargetView> = None;
            device.CreateRenderTargetView(&back_buffer, None, Some(&mut rtv))?;
            let render_target_view = rtv.unwrap();

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
                    AlignedByteOffset: 12,
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

            // Vertex Buffer (Full screen quad)
            let vertices = [
                Vertex {
                    position: [-1.0, 1.0, 0.0],
                    tex_coord: [0.0, 0.0],
                },
                Vertex {
                    position: [1.0, 1.0, 0.0],
                    tex_coord: [1.0, 0.0],
                },
                Vertex {
                    position: [-1.0, -1.0, 0.0],
                    tex_coord: [0.0, 1.0],
                },
                Vertex {
                    position: [1.0, -1.0, 0.0],
                    tex_coord: [1.0, 1.0],
                },
            ];
            let vb_desc = D3D11_BUFFER_DESC {
                ByteWidth: (std::mem::size_of::<Vertex>() * vertices.len()) as u32,
                Usage: D3D11_USAGE_IMMUTABLE,
                BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32,
                ..Default::default()
            };
            let vb_data = D3D11_SUBRESOURCE_DATA {
                pSysMem: vertices.as_ptr() as _,
                ..Default::default()
            };
            let mut vertex_buffer: Option<ID3D11Buffer> = None;
            device.CreateBuffer(&vb_desc, Some(&vb_data), Some(&mut vertex_buffer))?;
            let vertex_buffer = vertex_buffer.unwrap();

            // Constant Buffer
            let cb_desc = D3D11_BUFFER_DESC {
                ByteWidth: std::mem::size_of::<YCbCrConstants>() as u32,
                Usage: D3D11_USAGE_DYNAMIC,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                ..Default::default()
            };
            let mut constant_buffer: Option<ID3D11Buffer> = None;
            device.CreateBuffer(&cb_desc, None, Some(&mut constant_buffer))?;
            let constant_buffer = constant_buffer.unwrap();

            // Samplers
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

            // Rasterizer State
            let rs_desc = D3D11_RASTERIZER_DESC {
                FillMode: D3D11_FILL_SOLID,
                CullMode: D3D11_CULL_NONE,
                DepthClipEnable: true.into(),
                ..Default::default()
            };
            let mut rasterizer_state: Option<ID3D11RasterizerState> = None;
            device.CreateRasterizerState(&rs_desc, Some(&mut rasterizer_state))?;
            let rasterizer_state = rasterizer_state.unwrap();

            Ok(Self {
                device,
                context,
                swap_chain,
                render_target_view,
                vertex_shader,
                input_layout,
                pixel_shader_rgba,
                pixel_shader_ycbcr,
                vertex_buffer,
                constant_buffer,
                sampler_linear,
                sampler_nearest,
                rasterizer_state,
                interpolation_mode: InterpolationMode::Linear,
                text_alignment: std::sync::atomic::AtomicI32::new(
                    windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT_LEADING.0,
                ),
            })
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
                SysMemPitch: width * 4,
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

            let mut srv: Option<ID3D11ShaderResourceView> = None;
            self.device
                .CreateShaderResourceView(&texture, Some(&srv_desc), Some(&mut srv))?;
            Ok(srv.unwrap())
        }
    }

    // ヘルパー: RGBA データからテクスチャを作成
    fn create_rgba_texture(
        &self,
        width: u32,
        height: u32,
        data: &[u8],
    ) -> Result<ID3D11ShaderResourceView> {
        unsafe {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM_SRGB, // sRGB ガンマ補正を適用
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
                ..Default::default()
            };
            let init_data = D3D11_SUBRESOURCE_DATA {
                pSysMem: data.as_ptr() as _,
                SysMemPitch: width * 4,
                SysMemSlicePitch: 0,
            };
            let mut texture: Option<ID3D11Texture2D> = None;
            self.device
                .CreateTexture2D(&desc, Some(&init_data), Some(&mut texture))?;

            let mut srv: Option<ID3D11ShaderResourceView> = None;
            self.device
                .CreateShaderResourceView(&texture.unwrap(), None, Some(&mut srv))?;
            Ok(srv.unwrap())
        }
    }

    fn draw_text_internal(&self, text: &str, rect: &D2D_RECT_F, color: &D2D1_COLOR_F, large: bool) {
        use windows::Win32::Graphics::Gdi::*;
        use windows::core::w;

        let width = (rect.right - rect.left).ceil() as i32;
        let height = (rect.bottom - rect.top).ceil() as i32;
        if width <= 0 || height <= 0 {
            return;
        }

        unsafe {
            let hdc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height, // Top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };

            let mut p_bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let hbitmap =
                CreateDIBSection(Some(hdc), &info, DIB_RGB_COLORS, &mut p_bits, None, 0).unwrap();
            let old_bitmap = SelectObject(hdc, HGDIOBJ(hbitmap.0));

            // バッファをクリア (完全透過)
            std::ptr::write_bytes(p_bits, 0, (width * height * 4) as usize);

            let font_height = if large { 32 } else { 18 };
            let weight = if large { FW_BOLD } else { FW_NORMAL };
            let hfont = CreateFontW(
                font_height,
                0,
                0,
                0,
                weight.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                DEFAULT_QUALITY,
                DEFAULT_PITCH.0 as u32,
                w!("Yu Gothic UI"),
            );
            let old_font = SelectObject(hdc, HGDIOBJ(hfont.0));

            // GDI はアルファチャンネルをサポートしないため、テキスト色を白で描画し
            // 後でシェーダーまたはブレンドステートで色を付けるか、ここでピクセル操作する。
            // ここではピクセル操作で色とアルファを適用する。
            SetTextColor(hdc, COLORREF(0x00FFFFFF)); // 白
            SetBkMode(hdc, TRANSPARENT);

            let mut wide_text: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let mut rect_gdi = windows::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            };

            // アライメント (Atomic からロード)
            let alignment = self
                .text_alignment
                .load(std::sync::atomic::Ordering::Relaxed) as u32;
            let mut format = DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX;

            // DWRITE_TEXT_ALIGNMENT と GDI フラグのマッピング
            // LEADING (0) -> LEFT
            // TRAILING (1) -> RIGHT
            // CENTER (2) -> CENTER
            if alignment == 2 {
                format |= DT_CENTER;
            } else if alignment == 1 {
                format |= DT_RIGHT;
            } else {
                format |= DT_LEFT;
            }

            DrawTextW(hdc, &mut wide_text, &mut rect_gdi, format);

            // ピクセル操作: GDI が描画した白(R=G=B=255)を元に、指定色のアルファ付きピクセルにする
            // GDI の DrawText はアンチエイリアスで中間色を出力する可能性がある。
            // 背景が黒(0)で前景色が白(255)なので、Rチャンネルの値をそのままアルファとして使用できる。
            let r_target = (color.r * 255.0) as u8;
            let g_target = (color.g * 255.0) as u8;
            let b_target = (color.b * 255.0) as u8;

            let pixel_sl =
                std::slice::from_raw_parts_mut(p_bits as *mut u32, (width * height) as usize);
            for p in pixel_sl {
                // BGRA 順序 (Windows GDI)
                let intensity = (*p & 0xFF) as u8; // Blue channel (White text -> all channels same)
                if intensity > 0 {
                    // pre-multiplied alpha
                    // intensity(0-255) をアルファとして扱う
                    let alpha = intensity;
                    let r = (r_target as u32 * alpha as u32) / 255;
                    let g = (g_target as u32 * alpha as u32) / 255;
                    let b = (b_target as u32 * alpha as u32) / 255;

                    *p = ((alpha as u32) << 24) | (r << 16) | (g << 8) | b;
                } else {
                    *p = 0;
                }
            }

            // D3D11 テクスチャ作成
            let texture_srv = self
                .create_rgba_texture(
                    width as u32,
                    height as u32,
                    std::slice::from_raw_parts(p_bits as *const u8, (width * height * 4) as usize),
                )
                .unwrap();
            let texture_handle = TextureHandle::D3D11Rgba(texture_srv);

            // 描画
            self.draw_image(&texture_handle, rect);

            // cleanup
            let _ = SelectObject(hdc, old_font);
            let _ = DeleteObject(HGDIOBJ(hfont.0));
            let _ = SelectObject(hdc, old_bitmap);
            let _ = DeleteObject(HGDIOBJ(hbitmap.0));
            let _ = DeleteDC(hdc);
        }
    }
}
