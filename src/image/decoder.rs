use image::{DynamicImage, GenericImageView};
use std::path::Path;

pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>, // RGBA8 形式
}

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
    if data.len() > 8 && &data[0..8] == &[0x00, 0x00, 0x00, 0x0C, 0x6A, 0x50, 0x20, 0x20] {
        return decode_jp2(data);
    }
    
    let img = image::load_from_memory(data)?;
    Ok(process_dynamic_image(img))
}

fn decode_jp2(data: &[u8]) -> Result<DecodedImage, Box<dyn std::error::Error>> {
    use hayro_jpeg2000::{decode, DecodeSettings, ColorSpace};
    
    let img = decode(data, &DecodeSettings::default())
        .map_err(|e| format!("JP2 decode error: {}", e))?;
    
    let width = img.width as u32;
    let height = img.height as u32;
    
    let mut rgba_data = Vec::with_capacity((width * height * 4) as usize);
    let pixels = &img.data;
    
    match img.color_space {
        ColorSpace::RGB => {
            for i in (0..pixels.len()).step_by(3) {
                if i + 2 < pixels.len() {
                    rgba_data.push(pixels[i]);
                    rgba_data.push(pixels[i+1]);
                    rgba_data.push(pixels[i+2]);
                    rgba_data.push(255);
                }
            }
        }
        ColorSpace::Gray => {
            for &p in pixels {
                rgba_data.push(p);
                rgba_data.push(p);
                rgba_data.push(p);
                rgba_data.push(255);
            }
        }
        _ => {
            // 他のカラースペースは暫定的に無視するかエラーにする
            return Err(format!("Unsupported color space in JP2: {:?}", img.color_space).into());
        }
    }

    Ok(DecodedImage {
        width,
        height,
        data: rgba_data,
    })
}

fn process_dynamic_image(img: DynamicImage) -> DecodedImage {
    let (width, height) = img.dimensions();
    let rgba = img.to_rgba8();
    DecodedImage {
        width,
        height,
        data: rgba.into_raw(),
    }
}
