use crate::image::cache::DecodedImage;
use crate::state::BindingDirection;
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

    fn upload_image(
        &self,
        image: &DecodedImage,
    ) -> std::result::Result<TextureHandle, Box<dyn std::error::Error>>;

    /// 抽象化されたテクスチャを描画
    fn draw_image(&self, texture: &TextureHandle, dest_rect: &D2D_RECT_F);

    /// テクスチャのサイズを取得
    fn get_texture_size(&self, texture: &TextureHandle) -> (f32, f32);

    /// 基本的な図形描画
    fn fill_rectangle(&self, rect: &D2D_RECT_F, color: &D2D1_COLOR_F);
    fn draw_rectangle(&self, rect: &D2D_RECT_F, color: &D2D1_COLOR_F, stroke_width: f32);

    // ネイティブダイアログ移行に伴い draw_text, fill_rounded_rectangle は廃止予定
    fn draw_text(&self, text: &str, rect: &D2D_RECT_F, color: &D2D1_COLOR_F, large: bool);

    fn set_interpolation_mode(&mut self, mode: InterpolationMode);
    fn set_text_alignment(&self, alignment: DWRITE_TEXT_ALIGNMENT);

    /// ページめくりアニメーションをサポートするかどうか
    fn supports_page_turn_animation(&self) -> bool {
        false // デフォルトはサポートしない（D2D）
    }

    /// ページめくりアニメーションを描画
    fn draw_page_turn(
        &self,
        _progress: f32,
        _direction: i32,
        _binding: BindingDirection,
        _from_pages: &[PageDrawInfo],
        _to_pages: &[PageDrawInfo],
        _viewport_rect: &D2D_RECT_F,
        _animation_type: &str,
    ) {
        // デフォルト実装は何もしない（D2Dなど非対応バックエンド用）
    }
}

pub struct PageDrawInfo<'a> {
    pub texture: &'a TextureHandle,
    pub dest_rect: D2D_RECT_F,
}

/// バックエンドを跨いでテクスチャを管理するためのハンドル
/// 具体的なオブジェクトはバックエンド側で保持され、IDや列挙型で管理される
pub enum TextureHandle {
    Direct2D(windows::Win32::Graphics::Direct2D::ID2D1Bitmap1),
    #[allow(dead_code)]
    D3D11Rgba(windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView),
    /// YCbCr プレーン（GPU シェーダで RGB に変換）
    D3D11YCbCr {
        y: windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView,
        cb: windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView,
        cr: windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView,
        width: u32,
        height: u32,
        _subsampling: (u8, u8),
        _precision: u8,
        y_is_signed: bool,
        c_is_signed: bool,
    },
    #[allow(dead_code)]
    OpenGL {
        id: u32,
        width: u32,
        height: u32,
    },
    OpenGLYCbCr {
        y: u32,
        cb: u32,
        cr: u32,
        width: u32,
        height: u32,
        _subsampling: (u8, u8),
        _precision: u8,
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
    Lanczos,
}
