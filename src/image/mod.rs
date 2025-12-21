pub mod decoder;
pub mod archive;
pub mod cache;
pub mod loader;

use crate::image::archive::ArchiveLoader;
use crate::image::decoder::DecodedImage;
use walkdir::WalkDir;

pub enum ImageSource {
    Files(Vec<String>),
    Archive(ArchiveLoader),
}

impl ImageSource {
    pub fn len(&self) -> usize {
        match self {
            Self::Files(f) => f.len(),
            Self::Archive(a) => a.get_file_names().len(),
        }
    }

    pub fn load_image(&mut self, index: usize) -> Result<DecodedImage, Box<dyn std::error::Error>> {
        match self {
            Self::Files(f) => {
                let decoded = decoder::decode_image(&f[index])?;
                Ok(decoded)
            }
            Self::Archive(a) => {
                a.load_image(index)
            }
        }
    }
}

pub fn get_image_source(path: &str) -> Option<ImageSource> {
    let path_buf = std::path::Path::new(path);
    if path_buf.is_dir() {
        let mut files: Vec<String> = Vec::new();
        let supported = ["jpg", "jpeg", "png", "webp", "bmp", "jp2"];
        for entry in WalkDir::new(path).max_depth(1).into_iter().filter_map(|e| e.ok()) {
            if let Some(ext) = entry.path().extension().and_then(|s| s.to_str()) {
                if supported.contains(&ext.to_lowercase().as_str()) {
                    files.push(entry.path().to_string_lossy().to_string());
                }
            }
        }
        files.sort_by(|a, b| natord::compare(a, b));
        return Some(ImageSource::Files(files));
    } else if let Some(ext) = path_buf.extension().and_then(|s| s.to_str()) {
        let ext_lower = ext.to_lowercase();
        if ext_lower == "zip" || ext_lower == "7z" || ext_lower == "cbz" || ext_lower == "rar" || ext_lower == "cbr" {
            if let Ok(loader) = ArchiveLoader::open(path) {
                return Some(ImageSource::Archive(loader));
            }
        } else {
            // 単一ファイル
            return Some(ImageSource::Files(vec![path.to_string()]));
        }
    }
    None
}
