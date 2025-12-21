use std::io::Read;
use zip::ZipArchive;
use sevenz_rust;
use crate::image::decoder::{DecodedImage, _decode_image_from_memory};

pub enum ArchiveInternal {
    Zip(ZipArchive<std::fs::File>),
    SevenZ {
        temp_dir: std::path::PathBuf,
        #[allow(dead_code)]
        archive_path: std::path::PathBuf,
    },
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

        if ext == "zip" || ext == "cbz" {
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
            let temp_dir = std::env::temp_dir().join(format!("HayateViewer_Rust_{}", crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC).checksum(path.as_bytes())));
            if !temp_dir.exists() {
                std::fs::create_dir_all(&temp_dir)?;
            }
            
            println!("[Archive] Extracting 7z to temp: {} -> {:?}", path, temp_dir);
            sevenz_rust::decompress_file(path, &temp_dir)?;

            // 展開されたファイルをスキャンして file_names を構築
            for entry in walkdir::WalkDir::new(&temp_dir) {
                let entry = entry?;
                if entry.file_type().is_file() {
                    let rel_path = entry.path().strip_prefix(&temp_dir)?;
                    let name = rel_path.to_string_lossy().to_string().replace("\\", "/");
                    if let Some(ext) = entry.path().extension().and_then(|s| s.to_str()) {
                        if supported.contains(&ext.to_lowercase().as_str()) {
                            file_names.push(name);
                        }
                    }
                }
            }

            file_names.sort_by(|a, b| natord::compare(a, b));
            Ok(Self {
                internal: ArchiveInternal::SevenZ {
                    temp_dir,
                    archive_path: path_buf,
                },
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
            ArchiveInternal::SevenZ { ref temp_dir, .. } => {
                let file_path = temp_dir.join(name.replace("/", "\\"));
                println!("[Archive] Reading from temp: {:?}", file_path);
                buffer = std::fs::read(&file_path)?;
                println!("[Archive] Read completed: {}, size={}", name, buffer.len());
            }
        }
        
        let decoded = _decode_image_from_memory(&buffer)?;
        Ok(decoded)
    }
}

impl Drop for ArchiveLoader {
    fn drop(&mut self) {
        if let ArchiveInternal::SevenZ { ref temp_dir, .. } = self.internal {
            println!("[Archive] Cleaning up temp dir: {:?}", temp_dir);
            let _ = std::fs::remove_dir_all(temp_dir);
        }
    }
}
