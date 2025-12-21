use std::io::Read;
use zip::ZipArchive;
use sevenz_rust;
use crate::image::decoder::{DecodedImage, _decode_image_from_memory};

pub enum ArchiveInternal {
    Zip(ZipArchive<std::fs::File>),
    SevenZ {
        archive_path: std::path::PathBuf,
    },
    Rar {
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
            // 7z のファイルリストを取得（高速）
            println!("[Archive] Listing 7z: {}", path);
            let mut file_names = Vec::new();
            let mut reader = sevenz_rust::SevenZReader::open(path_buf.clone(), sevenz_rust::Password::empty())?;
            reader.for_each_entries(|entry, _| {
                let name = entry.name().replace("\\", "/");
                if let Some(ext) = std::path::Path::new(&name).extension().and_then(|s| s.to_str()) {
                    if supported.contains(&ext.to_lowercase().as_str()) {
                        file_names.push(name);
                    }
                }
                Ok(true)
            })?;
            
            file_names.sort_by(|a, b| natord::compare(a, b));
            Ok(Self {
                internal: ArchiveInternal::SevenZ {
                    archive_path: path_buf,
                },
                file_names,
            })
        } else if ext == "rar" || ext == "cbr" {
            // unrar クレートを使用してファイルリストを取得（高速）
            println!("[Archive] Listing RAR: {}", path);
            let mut file_names = Vec::new();
            let mut archive = unrar::Archive::new(path).open_for_listing()?;
            while let Some(header) = archive.read_header()? {
                let entry = header.entry();
                if entry.is_file() {
                    let name = entry.filename.to_string_lossy().replace("\\", "/");
                    if let Some(ext) = std::path::Path::new(&name).extension().and_then(|s| s.to_str()) {
                        if supported.contains(&ext.to_lowercase().as_str()) {
                            file_names.push(name);
                        }
                    }
                }
                archive = header.skip()?;
            }

            file_names.sort_by(|a, b| natord::compare(a, b));
            Ok(Self {
                internal: ArchiveInternal::Rar {
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
            ArchiveInternal::SevenZ { ref archive_path } => {
                // 7z から該当ファイルをメモリへ直接展開
                let mut found = false;
                sevenz_rust::SevenZReader::open(archive_path, sevenz_rust::Password::empty())?
                    .for_each_entries(|entry, reader| {
                        if entry.name().replace("\\", "/") == *name {
                            reader.read_to_end(&mut buffer)?;
                            found = true;
                            return Ok(false); // 目的のファイルが見つかったので中断
                        }
                        Ok(true)
                    })?;
                if !found {
                    return Err(format!("File '{}' not found in 7z archive", name).into());
                }
            }
            ArchiveInternal::Rar { ref archive_path } => {
                // RAR から該当ファイルをメモリへ直接展開（Unidirectional Stream なので該当ファイルまで読み飛ばし）
                let mut archive = unrar::Archive::new(archive_path).open_for_processing()?;
                'outer: loop {
                    match archive.read_header() {
                        Ok(Some(header)) => {
                            let filename = header.entry().filename.to_string_lossy().replace("\\", "/");
                            if filename == *name {
                                let (data, _next_archive) = header.read()?;
                                buffer = data;
                                break 'outer;
                            } else {
                                archive = header.skip()?;
                            }
                        }
                        Ok(None) => break 'outer,
                        Err(e) => return Err(e.into()),
                    }
                }
            }
        }
        
        if buffer.is_empty() {
            return Err(format!("File '{}' not found in archive", name).into());
        }

        let decoded = _decode_image_from_memory(&buffer)?;
        Ok(decoded)
    }
}

impl Drop for ArchiveLoader {
    fn drop(&mut self) {
        // 一時ディレクトリを使用しなくなったため、何もしない
    }
}
