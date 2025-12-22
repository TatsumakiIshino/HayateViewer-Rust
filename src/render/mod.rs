use crate::image::cache::DecodedImage;
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_COLOR_F};
use windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT;

pub mod d2d;
pub mod d3d11;
pub mod opengl;

/// レンダラーバックエンドが共通で実装すべきトレイト
pub trait Renderer: Send + Sync {
    fn resize(&self, width: u32, height: u32) -> Result<(), Box<dyn std::error::Error>>;
    fn begin_draw(&self);
    fn end_draw(&self) -> Result<(), Box<dyn std::error::Error>>;

    fn upload_image(&self, image: &DecodedImage) -> std::result::Result<TextureHandle, Box<dyn std::error::Error>>;

    /// 抽象化されたテクスチャを描画
    fn draw_image(&self, texture: &TextureHandle, dest_rect: &D2D_RECT_F);

    /// テクスチャのサイズを取得
    fn get_texture_size(&self, texture: &TextureHandle) -> (f32, f32);

    /// 基本的な図形とテキストの描画（D2D 互換 / フォールバック）
    fn fill_rectangle(&self, rect: &D2D_RECT_F, color: &D2D1_COLOR_F);
    fn fill_rounded_rectangle(&self, rect: &D2D_RECT_F, radius: f32, color: &D2D1_COLOR_F);
    fn draw_rectangle(&self, rect: &D2D_RECT_F, color: &D2D1_COLOR_F, stroke_width: f32);
    fn draw_text(&self, text: &str, rect: &D2D_RECT_F, color: &D2D1_COLOR_F, large: bool);
    
    fn set_interpolation_mode(&mut self, mode: InterpolationMode);
    fn set_text_alignment(&self, alignment: DWRITE_TEXT_ALIGNMENT);
}

/// バックエンドを跨いでテクスチャを管理するためのハンドル
/// 具体的なオブジェクトはバックエンド側で保持され、IDや列挙型で管理される
pub enum TextureHandle {
    Direct2D(windows::Win32::Graphics::Direct2D::ID2D1Bitmap1),
    D3D11Rgba(windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView),
    /// YCbCr プレーン（GPU シェーダで RGB に変換）
    D3D11YCbCr {
        y: windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView,
        cb: windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView,
        cr: windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView,
        width: u32,
        height: u32,
        subsampling: (u8, u8),
        precision: u8,
        y_is_signed: bool,
        c_is_signed: bool,
    },
    OpenGL(u32),
    OpenGLYCbCr {
        y: u32,
        cb: u32,
        cr: u32,
        width: u32,
        height: u32,
        subsampling: (u8, u8),
        precision: u8,
        y_is_signed: bool,
        c_is_signed: bool,
    },
    // 将来的に追加:
    // Wgpu(wgpu::TextureView),
    // Cpu(Arc<Vec<u8>>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InterpolationMode {
    NearestNeighbor,
    Linear,
    Cubic,
    HighQualityCubic,
    Lanczos,
}
