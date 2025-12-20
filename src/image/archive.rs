use std::io::Read;
use zip::ZipArchive;
use sevenz_rust::{SevenZReader, Password};
use crate::image::decoder::{DecodedImage, _decode_image_from_memory};

pub enum ArchiveInternal {
    Zip(ZipArchive<std::fs::File>),
    SevenZ(std::path::PathBuf),
}

pub struct ArchiveLoader {
    internal: ArchiveInternal,
    file_names: Vec<String>,
}

impl ArchiveLoader {
    pub fn open(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let path_buf = std::path::PathBuf::from(path);
        let ext = path_buf.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
        
        let mut file_names = Vec::new();
        let supported = ["jpg", "jpeg", "png", "webp", "bmp", "jp2"];

        if ext == "zip" {
            let file = std::fs::File::open(path)?;
            let mut archive = ZipArchive::new(file)?;
            for i in 0..archive.len() {
                let file = archive.by_index(i)?;
                if file.is_file() {
                    let name = file.name();
                    if let Some(ext) = std::path::Path::new(name).extension().and_then(|s| s.to_str()) {
                        if supported.contains(&ext.to_lowercase().as_str()) {
                            file_names.push(name.to_string());
                        }
                    }
                }
            }
            file_names.sort_by(|a, b| natord::compare(a, b));
            Ok(Self {
                internal: ArchiveInternal::Zip(archive),
                file_names,
            })
        } else if ext == "7z" {
            let file = std::fs::File::open(path)?;
            let len = file.metadata()?.len();
            let mut reader = SevenZReader::new(file, len, Password::empty())?;
            
            reader.for_each_entries(|entry, _| {
                if !entry.is_directory() {
                    let name = entry.name();
                    if let Some(ext) = std::path::Path::new(name).extension().and_then(|s| s.to_str()) {
                        if supported.contains(&ext.to_lowercase().as_str()) {
                            file_names.push(name.to_string());
                        }
                    }
                }
                Ok(true)
            })?;
            
            file_names.sort_by(|a, b| natord::compare(a, b));
            Ok(Self {
                internal: ArchiveInternal::SevenZ(path_buf),
                file_names,
            })
        } else {
            Err("Unsupported archive format".into())
        }
    }

    pub fn get_file_names(&self) -> &[String] {
        &self.file_names
    }

    pub fn load_image(&mut self, index: usize) -> Result<DecodedImage, Box<dyn std::error::Error>> {
        let name = &self.file_names[index];
        let mut buffer = Vec::new();

        match self.internal {
            ArchiveInternal::Zip(ref mut archive) => {
                let mut file = archive.by_name(name)?;
                file.read_to_end(&mut buffer)?;
            }
            ArchiveInternal::SevenZ(ref path) => {
                let file = std::fs::File::open(path)?;
                let len = file.metadata()?.len();
                let mut reader = SevenZReader::new(file, len, Password::empty())?;
                
                let mut found = false;
                reader.for_each_entries(|entry, reader| {
                    if entry.name() == name {
                        reader.read_to_end(&mut buffer)?;
                        found = true;
                        Ok(false) // 停止
                    } else {
                        Ok(true) // 続行
                    }
                })?;
                
                if !found {
                    return Err(format!("File not found in 7z: {}", name).into());
                }
            }
        }
        
        let decoded = _decode_image_from_memory(&buffer)?;
        Ok(decoded)
    }
}
