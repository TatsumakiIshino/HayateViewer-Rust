use std::mem::ManuallyDrop;
use windows::{
    core::*, Win32::Foundation::*, Win32::Graphics::Direct2D::Common::*, Win32::Graphics::Direct2D::*,
    Win32::Graphics::Direct3D::*, Win32::Graphics::Direct3D11::*, Win32::Graphics::Dxgi::Common::*,
    Win32::Graphics::Dxgi::*, Win32::Graphics::DirectWrite::*,
};
type D3DResult<T> = windows::core::Result<T>;

use crate::image::cache::{DecodedImage, PixelData};
use super::{Renderer, TextureHandle, InterpolationMode};

// 旧トレイト定義は削除

pub struct D2DRenderer {
    pub _factory: ID2D1Factory1,
    pub _device: ID2D1Device,
    pub context: ID2D1DeviceContext,
    pub swap_chain: IDXGISwapChain1,
    #[allow(dead_code)]
    pub dw_factory: IDWriteFactory,
    pub text_format: IDWriteTextFormat,
    pub text_format_large: IDWriteTextFormat,
    pub brush: ID2D1SolidColorBrush,
    pub interpolation_mode: D2D1_INTERPOLATION_MODE,
}

impl Renderer for D2DRenderer {
    fn resize(&self, width: u32, height: u32) -> std::result::Result<(), Box<dyn std::error::Error>> {
        let res: windows::core::Result<()> = unsafe {
            self.context.SetTarget(None);
            self.swap_chain.ResizeBuffers(0, width, height, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
            let surface: IDXGISurface = self.swap_chain.GetBuffer(0)?;
            let back_buffer: ID2D1Bitmap1 = self.context.CreateBitmapFromDxgiSurface(&surface, None)?;
            self.context.SetTarget(&back_buffer);
            Ok(())
        };
        res.map_err(|e| e.into())
    }

    fn begin_draw(&self) {
        unsafe {
            self.context.BeginDraw();
            self.context.Clear(Some(&D2D1_COLOR_F { r: 0.1, g: 0.1, b: 0.1, a: 0.8 }));
        }
    }

    fn end_draw(&self) -> std::result::Result<(), Box<dyn std::error::Error>> {
        let res: D3DResult<()> = unsafe {
            self.context.EndDraw(None, None)?;
            self.swap_chain.Present(1, DXGI_PRESENT(0)).ok()
        };
        res.map_err(|e| e.into())
    }

    fn upload_image(&self, image: &DecodedImage) -> std::result::Result<TextureHandle, Box<dyn std::error::Error>> {
        match image.pixel_data {
            PixelData::Rgba8(ref data) => {
                let bitmap: ID2D1Bitmap1 = self.create_bitmap(image.width, image.height, data).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
                Ok(TextureHandle::Direct2D(bitmap))
            }
            PixelData::Ycbcr { .. } => {
                Err("YCbCr upload not yet implemented for D2D".into())
            }
        }
    }

    fn draw_image(&self, texture: &TextureHandle, dest_rect: &D2D_RECT_F) {
        let TextureHandle::Direct2D(bitmap) = texture;
        unsafe {
            self.context.DrawBitmap(
                bitmap,
                Some(dest_rect),
                1.0,
                self.interpolation_mode,
                None,
                None,
            );
        }
    }

    fn get_texture_size(&self, texture: &TextureHandle) -> (f32, f32) {
        let TextureHandle::Direct2D(bitmap) = texture;
        unsafe {
            let size = bitmap.GetSize();
            (size.width, size.height)
        }
    }

    fn fill_rectangle(&self, rect: &D2D_RECT_F, color: &D2D1_COLOR_F) {
        unsafe {
            self.brush.SetColor(color);
            self.context.FillRectangle(rect, &self.brush);
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
            self.context.FillRoundedRectangle(&rounded, &self.brush);
        }
    }

    fn draw_rectangle(&self, rect: &D2D_RECT_F, color: &D2D1_COLOR_F, stroke_width: f32) {
        unsafe {
            self.brush.SetColor(color);
            self.context.DrawRectangle(rect, &self.brush, stroke_width, None);
        }
    }

    fn draw_text(&self, text: &str, rect: &D2D_RECT_F, color: &D2D1_COLOR_F, large: bool) {
        unsafe {
            self.brush.SetColor(color);
            let wide_text: Vec<u16> = text.encode_utf16().collect();
            let format = if large { &self.text_format_large } else { &self.text_format };
            self.context.DrawText(
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

impl D2DRenderer {
    pub fn new(hwnd: HWND) -> Result<Self> {
        unsafe {
            // Direct3D 11 デバイスの作成
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

            // Direct2D デバイスとコンテキストの作成
            let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let device = factory.CreateDevice(&dxgi_device)?;
            let context = device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;

            // スワップチェーンの作成
            let dxgi_factory: IDXGIFactory2 = CreateDXGIFactory1()?;
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

            let swap_chain = dxgi_factory.CreateSwapChainForHwnd(&d3d_device, hwnd, &swap_chain_desc, None, None)?;

            // レンダーターゲットの設定
            let surface: IDXGISurface = swap_chain.GetBuffer(0)?;
            let back_buffer: ID2D1Bitmap1 = context.CreateBitmapFromDxgiSurface(&surface, None)?;
            context.SetTarget(&back_buffer);

            // DirectWrite と ブラシの作成
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

            let brush = context.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }, None)?;

            Ok(Self {
                _factory: factory,
                _device: device,
                context,
                swap_chain,
                dw_factory,
                text_format,
                text_format_large,
                brush,
                interpolation_mode: D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
            })
        }
    }

    pub fn create_bitmap(&self, width: u32, height: u32, data: &[u8]) -> Result<ID2D1Bitmap1> {
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

            self.context.CreateBitmap(
                D2D_SIZE_U { width, height },
                Some(data.as_ptr() as _),
                width * 4,
                &props,
            )
        }
    }
}
