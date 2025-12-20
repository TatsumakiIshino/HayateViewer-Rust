use image::{DynamicImage, GenericImageView, ImageError};
use std::path::Path;

pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>, // BGRA8 形式
}

pub fn decode_image<P: AsRef<Path>>(path: P) -> Result<DecodedImage, ImageError> {
    let img = image::open(path)?;
    Ok(process_dynamic_image(img))
}

pub fn decode_image_from_memory(data: &[u8]) -> Result<DecodedImage, ImageError> {
    let img = image::load_from_memory(data)?;
    Ok(process_dynamic_image(img))
}

fn process_dynamic_image(img: DynamicImage) -> DecodedImage {
    let (width, height) = img.dimensions();
    // 一旦 RGBA8 で取得し、Direct2D が BGRA8 を期待する場合はレンダラー側かここで変換
    let rgba = img.to_rgba8();
    DecodedImage {
        width,
        height,
        data: rgba.into_raw(),
    }
}
