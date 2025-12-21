use std::mem::ManuallyDrop;
use windows::{
    core::*, Win32::Foundation::*, Win32::Graphics::Direct2D::Common::*, Win32::Graphics::Direct2D::*,
    Win32::Graphics::Direct3D11::*, Win32::Graphics::Dxgi::Common::*, Win32::Graphics::Dxgi::*,
    Win32::Graphics::Direct3D::*, Win32::Graphics::DirectWrite::*,
};
use crate::image::cache::{DecodedImage, PixelData};
use super::{Renderer, TextureHandle, InterpolationMode};

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
}

impl Renderer for D3D11Renderer {
    fn resize(&self, width: u32, height: u32) -> std::result::Result<(), Box<dyn std::error::Error>> {
        unsafe {
            // D2D ターゲットを解放
            self.d2d_context.SetTarget(None);

            self.swap_chain.ResizeBuffers(0, width, height, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
            
            // D2D ターゲットの再作成
            let surface: IDXGISurface = self.swap_chain.GetBuffer(0)?;
            let d2d_bitmap = self.d2d_context.CreateBitmapFromDxgiSurface(&surface, None)?;
            self.d2d_context.SetTarget(&d2d_bitmap);
        }
        Ok(())
    }

    fn begin_draw(&self) {
        unsafe {
            self.d2d_context.BeginDraw();
            self.d2d_context.Clear(Some(&D2D1_COLOR_F { r: 0.1, g: 0.1, b: 0.1, a: 0.8 }));
        }
    }

    fn end_draw(&self) -> std::result::Result<(), Box<dyn std::error::Error>> {
        unsafe {
            self.d2d_context.EndDraw(None, None)?;
            self.swap_chain.Present(1, DXGI_PRESENT(0)).ok()?;
        }
        Ok(())
    }

    fn upload_image(&self, image: &DecodedImage) -> std::result::Result<TextureHandle, Box<dyn std::error::Error>> {
        match &image.pixel_data {
            PixelData::Rgba8(data) => {
                // D2D ビットマップとして作成（既存の D2D Interop を活用）
                let bitmap: ID2D1Bitmap1 = self.create_d2d_bitmap(image.width, image.height, data).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
                Ok(TextureHandle::Direct2D(bitmap))
            }
            PixelData::Ycbcr { .. } => {
                // TODO: D3D11 ネイティブ実装を完了させる
                // 現時点ではシェーダファイルと TextureHandle の拡張のみ完了
                Err("YCbCr upload not yet fully implemented for D3D11 (shader files and handle types ready)".into())
            }
        }
    }

    fn draw_image(&self, texture: &TextureHandle, dest_rect: &D2D_RECT_F) {
        if let TextureHandle::Direct2D(bitmap) = texture {
            unsafe {
                self.d2d_context.DrawBitmap(
                    bitmap,
                    Some(dest_rect),
                    1.0,
                    self.interpolation_mode,
                    None,
                    None,
                );
            }
        }
    }

    fn get_texture_size(&self, texture: &TextureHandle) -> (f32, f32) {
        if let TextureHandle::Direct2D(bitmap) = texture {
            unsafe {
                let size = bitmap.GetSize();
                (size.width, size.height)
            }
        } else {
            (0.0, 0.0)
        }
    }

    fn fill_rectangle(&self, rect: &D2D_RECT_F, color: &D2D1_COLOR_F) {
        unsafe {
            self.brush.SetColor(color);
            self.d2d_context.FillRectangle(rect, &self.brush);
        }
    }

    fn fill_rounded_rectangle(&self, rect: &D2D_RECT_F, radius: f32, color: &D2D1_COLOR_F) {
        unsafe {
            self.brush.SetColor(color);
            let rounded = D2D1_ROUNDED_RECT {
                rect: *rect,
                radiusX: radius,
                radiusY: radius,
            };
            self.d2d_context.FillRoundedRectangle(&rounded, &self.brush);
        }
    }

    fn draw_rectangle(&self, rect: &D2D_RECT_F, color: &D2D1_COLOR_F, stroke_width: f32) {
        unsafe {
            self.brush.SetColor(color);
            self.d2d_context.DrawRectangle(rect, &self.brush, stroke_width, None);
        }
    }

    fn draw_text(&self, text: &str, rect: &D2D_RECT_F, color: &D2D1_COLOR_F, large: bool) {
        unsafe {
            self.brush.SetColor(color);
            let wide_text: Vec<u16> = text.encode_utf16().collect();
            let format = if large { &self.text_format_large } else { &self.text_format };
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
            InterpolationMode::HighQualityCubic => D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
        };
    }

    fn set_text_alignment(&self, alignment: DWRITE_TEXT_ALIGNMENT) {
        unsafe {
            let _ = self.text_format.SetTextAlignment(alignment);
            let _ = self.text_format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let _ = self.text_format_large.SetTextAlignment(alignment);
            let _ = self.text_format_large.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
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
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                Flags: 0,
            };
            
            let swap_chain = dxgi_factory.CreateSwapChainForHwnd(&device, hwnd, &swap_chain_desc, None, None)?;

            // D2D Interop
            let d2d_factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
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

            let brush = d2d_context.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }, None)?;

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

}
