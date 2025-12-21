use image::{DynamicImage, GenericImageView};
use std::path::Path;

pub use crate::image::cache::{DecodedImage, PixelData};

pub fn decode_image<P: AsRef<Path>>(path: P) -> Result<DecodedImage, Box<dyn std::error::Error>> {
    let path_ref = path.as_ref();
    let ext = path_ref.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    
    if ext == "jp2" || ext == "j2k" {
        let data = std::fs::read(path_ref)?;
        return decode_jp2(&data);
    }

    let img = image::open(path_ref)?;
    Ok(process_dynamic_image(img))
}

pub fn _decode_image_from_memory(data: &[u8]) -> Result<DecodedImage, Box<dyn std::error::Error>> {
    println!("[Decoder] Image memory size: {} bytes", data.len());
    if data.len() >= 8 {
        println!("[Decoder] Header: {:02X?}", &data[0..8]);
    }

    if data.len() > 8 && &data[0..8] == &[0x00, 0x00, 0x00, 0x0C, 0x6A, 0x50, 0x20, 0x20] {
        return decode_jp2(data);
    }
    
    let img = image::load_from_memory(data)?;
    Ok(process_dynamic_image(img))
}

fn decode_jp2(data: &[u8]) -> Result<DecodedImage, Box<dyn std::error::Error>> {
    use hayro_jpeg2000::{decode, DecodeSettings, ColorSpace};
    println!("[Decoder] Starting hayro-jpeg2000 decode...");
    
    let img = decode(data, &DecodeSettings::default())
        .map_err(|e| format!("JP2 decode error: {}", e))?;
    
    let width = img.width as u32;
    let height = img.height as u32;
    println!("[Decoder] JP2 ColorSpace: {:?}, Data size: {}", img.color_space, img.data.len());
    if img.data.len() >= 3 {
        println!("[Decoder] First pixel: {:?}", &img.data[0..3]);
    }
    
    use rayon::prelude::*;

    let mut rgba_data = vec![255u8; (width * height * 4) as usize];
    let pixels = &img.data;
    
    match img.color_space {
        ColorSpace::RGB => {
            rgba_data.par_chunks_exact_mut(4)
                .zip(pixels.par_chunks_exact(3))
                .for_each(|(rgba, rgb)| {
                    rgba[0] = rgb[0];
                    rgba[1] = rgb[1];
                    rgba[2] = rgb[2];
                    // rgba[3] is already 255 from vec! init
                });
        }
        ColorSpace::Gray => {
            rgba_data.par_chunks_exact_mut(4)
                .zip(pixels.par_iter())
                .for_each(|(rgba, &p)| {
                    rgba[0] = p;
                    rgba[1] = p;
                    rgba[2] = p;
                });
        }
        _ => {
            return Err(format!("Unsupported color space in JP2: {:?}", img.color_space).into());
        }
    }

    Ok(DecodedImage {
        width,
        height,
        pixel_data: PixelData::Rgba8(rgba_data),
    })
}

fn process_dynamic_image(img: DynamicImage) -> DecodedImage {
    let (width, height) = img.dimensions();
    let rgba = img.to_rgba8();
    DecodedImage {
        width,
        height,
        pixel_data: PixelData::Rgba8(rgba.into_raw()),
    }
}
