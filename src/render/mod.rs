use crate::image::cache::DecodedImage;
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_COLOR_F};
use windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT;

pub mod d2d;
pub mod d3d11;
pub mod opengl;

/// レンダラーバックエンドが共通で実装すべきトレイト
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

    /// 基本的な図形描画
    fn fill_rectangle(&self, rect: &D2D_RECT_F, color: &D2D1_COLOR_F);
    fn draw_rectangle(&self, rect: &D2D_RECT_F, color: &D2D1_COLOR_F, stroke_width: f32);
    
    // ネイティブダイアログ移行に伴い draw_text, fill_rounded_rectangle は廃止予定
    fn draw_text(&self, text: &str, rect: &D2D_RECT_F, color: &D2D1_COLOR_F, large: bool);
    
    fn set_interpolation_mode(&mut self, mode: InterpolationMode);
    fn set_text_alignment(&self, alignment: DWRITE_TEXT_ALIGNMENT);
}

/// メモリ上のダイアログテンプレート（Win32 DialogBoxIndirect 用）
#[repr(C, packed(2))]
pub struct DLGTEMPLATE {
    pub style: u32,
    pub dw_ext_style: u32,
    pub c_dit: u16,
    pub x: i16,
    pub y: i16,
    pub cx: i16,
    pub cy: i16,
}

#[repr(C, packed(2))]
pub struct DLGITEMTEMPLATE {
    pub style: u32,
    pub dw_ext_style: u32,
    pub x: i16,
    pub y: i16,
    pub cx: i16,
    pub cy: i16,
    pub id: u16,
}

pub struct DialogTemplate {
    pub data: Vec<u8>,
    pub items_count: u16,
}

impl DialogTemplate {
    pub fn new(title: &str, x: i16, y: i16, cx: i16, cy: i16, style: u32) -> Self {
        let mut data = Vec::new();
        let header = DLGTEMPLATE {
            style,
            dw_ext_style: 0,
            c_dit: 0,
            x, y, cx, cy,
        };
        data.extend_from_slice(unsafe {
            std::slice::from_raw_parts(&header as *const _ as *const u8, std::mem::size_of::<DLGTEMPLATE>())
        });

        // Menu, Class, Title
        data.extend_from_slice(&[0, 0]); // Menu: none
        data.extend_from_slice(&[0, 0]); // Class: none
        for c in title.encode_utf16() {
            data.extend_from_slice(&c.to_le_bytes());
        }
        data.extend_from_slice(&[0, 0]); // Null terminator

        Self { data, items_count: 0 }
    }

    pub fn add_item(&mut self, class_id: u16, text: &str, id: u16, x: i16, y: i16, cx: i16, cy: i16, style: u32) {
        // DWORD alignment
        while self.data.len() % 4 != 0 {
            self.data.push(0);
        }

        let item = DLGITEMTEMPLATE {
            style,
            dw_ext_style: 0,
            x, y, cx, cy, id,
        };
        self.data.extend_from_slice(unsafe {
            std::slice::from_raw_parts(&item as *const _ as *const u8, std::mem::size_of::<DLGITEMTEMPLATE>())
        });

        // Class (0xFFFF + ID)
        self.data.extend_from_slice(&[0xFF, 0xFF]);
        self.data.extend_from_slice(&class_id.to_le_bytes());

        // Text
        for c in text.encode_utf16() {
            self.data.extend_from_slice(&c.to_le_bytes());
        }
        self.data.extend_from_slice(&[0, 0]); // Null terminator
        self.data.extend_from_slice(&[0, 0]); // Creation data: none

        self.items_count += 1;
        // Update header count
        let count_offset = 8; // DLGTEMPLATE.c_dit is at offset 8
        self.data[count_offset..count_offset+2].copy_from_slice(&self.items_count.to_le_bytes());
    }
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
    HighQualityCubic,
    Lanczos,
}
