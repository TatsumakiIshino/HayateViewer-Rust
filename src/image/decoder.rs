use image::{DynamicImage, GenericImageView};
use std::path::Path;

pub use crate::image::cache::{DecodedImage, PixelData};

pub fn decode_image<P: AsRef<Path>>(path: P, use_cpu_color_conversion: bool) -> Result<DecodedImage, Box<dyn std::error::Error>> {
    let path_ref = path.as_ref();
    let ext = path_ref.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    
    if ext == "jp2" || ext == "j2k" {
        let data = std::fs::read(path_ref)?;
        return decode_jp2(&data, use_cpu_color_conversion);
    }
 
    let img = image::open(path_ref)?;
    Ok(process_dynamic_image(img))
}

pub fn _decode_image_from_memory(data: &[u8], use_cpu_color_conversion: bool) -> Result<DecodedImage, Box<dyn std::error::Error>> {
    if data.len() > 8 && &data[0..8] == &[0x00, 0x00, 0x00, 0x0C, 0x6A, 0x50, 0x20, 0x20] {
        return decode_jp2(data, use_cpu_color_conversion);
    }
    
    let img = image::load_from_memory(data)?;
    Ok(process_dynamic_image(img))
}

fn decode_jp2(data: &[u8], use_cpu_color_conversion: bool) -> Result<DecodedImage, Box<dyn std::error::Error>> {
    println!("[Decoder] Starting jpeg2k (OpenJPEG bindings) decode...");
 
    // jpeg2k crate v0.10.x API
    let image = jpeg2k::Image::from_bytes(data)?;
 
    let orig_width = image.orig_width();
    let orig_height = image.orig_height();
    let components = image.components();
    let color_space = image.color_space();
    
    println!("[Decoder] Decoded. ColorSpace: {:?}", color_space);
 
    // OpenJPEG が既に RGB と判定している場合、または components が 3 未満の場合は通常パス（RGB）へ
    // SRGB, Gray, YUV などがある。RGB 変換済みなら YCbCr シェーダーを通さない
    let is_rgb_already = matches!(color_space, jpeg2k::ColorSpace::SRGB);
    
    if components.len() == 3 && !is_rgb_already && !use_cpu_color_conversion {
        let c_y = &components[0];
        let c_cb = &components[1];
 
        let dx_y = (orig_width as f32 / c_y.width() as f32).round() as u32;
        let dy_y = (orig_height as f32 / c_y.height() as f32).round() as u32;
        let dx_c = (orig_width as f32 / c_cb.width() as f32).round() as u32;
        let dy_c = (orig_height as f32 / c_cb.height() as f32).round() as u32;
        let precision = components[0].precision();
        
        if dx_y == 1 && dy_y == 1 {
            println!("[Decoder] GPU YCbCr path. Subsampling: ({},{})", dx_c, dy_c);
            let planes: Vec<Vec<i32>> = components.iter().map(|c| c.data().to_vec()).collect();
            return Ok(DecodedImage {
                width: c_y.width(),
                height: c_y.height(),
                pixel_data: PixelData::Ycbcr {
                    planes,
                    subsampling: (dx_c as u8, dy_c as u8),
                    precision: precision as u8,
                    y_is_signed: c_y.is_signed(),
                    c_is_signed: c_cb.is_signed(),
                },
            });
        }
    } else if components.len() == 3 && !is_rgb_already && use_cpu_color_conversion {
        println!("[Decoder] CPU YCbCr -> RGB conversion path.");
        let width = orig_width;
        let height = orig_height;
        let precision = components[0].precision();
        let max_val = ((1u32 << precision) - 1) as f32;
        let scale = 1.0 / max_val;
 
        let c_y = &components[0];
        let c_cb = &components[1];
        let c_cr = &components[2];
        
        let dx_c = (orig_width as f32 / c_cb.width() as f32).round() as u32;
        let dy_c = (orig_height as f32 / c_cb.height() as f32).round() as u32;
 
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        let y_data = c_y.data();
        let cb_data = c_cb.data();
        let cr_data = c_cr.data();
        let c_width = c_cb.width();
 
        let y_is_signed = c_y.is_signed();
        let c_is_signed = c_cb.is_signed();
 
        for y in 0..height {
            for x in 0..width {
                let y_val = y_data[(y * width + x) as usize] as f32 * scale;
                // DC offset for signed Y
                let y_norm = if y_is_signed { y_val + 0.5 } else { y_val };
 
                let cx = x / dx_c;
                let cy = y / dy_c;
                let c_idx = (cy * c_width + cx) as usize;
                
                let cb_val = cb_data[c_idx] as f32 * scale;
                let cr_val = cr_data[c_idx] as f32 * scale;
 
                let cb_norm = if c_is_signed { cb_val } else { cb_val - 0.5 };
                let cr_norm = if c_is_signed { cr_val } else { cr_val - 0.5 };
 
                // ICT Conversion
                let r = y_norm + 1.402 * cr_norm;
                let g = y_norm - 0.34413 * cb_norm - 0.71414 * cr_norm;
                let b = y_norm + 1.772 * cb_norm;
 
                rgba.push((r.clamp(0.0, 1.0) * 255.0) as u8);
                rgba.push((g.clamp(0.0, 1.0) * 255.0) as u8);
                rgba.push((b.clamp(0.0, 1.0) * 255.0) as u8);
                rgba.push(255);
            }
        }
        return Ok(DecodedImage { width, height, pixel_data: PixelData::Rgba8(rgba) });
    }

    // Fallback: use get_pixels (RGB) if not standard 3-component YCbCr
    println!("[Decoder] Falling back to RGB decode (not 3-comp YCbCr or unsupported sampling)");
    let rgb_image = image.get_pixels(None)?; // ImageData
    
    // Convert jpeg2k::ImageData to PixelData::Rgba8
    // Note: get_pixels can return various formats, we need to handle them
    match rgb_image.data {
        jpeg2k::ImagePixelData::Rgb8(data) => {
            let mut rgba = Vec::with_capacity(data.len() / 3 * 4);
            for chunk in data.chunks_exact(3) {
                rgba.push(chunk[0]);
                rgba.push(chunk[1]);
                rgba.push(chunk[2]);
                rgba.push(255);
            }
            Ok(DecodedImage {
                width: rgb_image.width,
                height: rgb_image.height,
                pixel_data: PixelData::Rgba8(rgba),
            })
        },
        jpeg2k::ImagePixelData::Rgba8(data) => {
            Ok(DecodedImage {
                width: rgb_image.width,
                height: rgb_image.height,
                pixel_data: PixelData::Rgba8(data),
            })
        },
        _ => Err(format!("Unsupported color format from OpenJPEG: {:?}", rgb_image.format).into()),
    }
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
