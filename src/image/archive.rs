use std::io::Read;
use zip::ZipArchive;
use sevenz_rust;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
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
    // メモリキャッシュ: パス名 -> ファイルデータ
    cache: Arc<Mutex<Option<HashMap<String, Vec<u8>>>>>,
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
                cache: Arc::new(Mutex::new(None)),
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
                cache: Arc::new(Mutex::new(None)),
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
                cache: Arc::new(Mutex::new(None)),
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
        
        // 1. キャッシュチェック
        {
            let cache = self.cache.lock().unwrap();
            if let Some(ref map) = *cache {
                if let Some(data) = map.get(name) {
                    println!("[Archive] Cache hit: {}", name);
                    return _decode_image_from_memory(data).map_err(|e| e.into());
                }
            }
        }

        // 2. キャッシュがなければ一括展開
        println!("[Archive] Initial slurping to memory...");
        let mut new_cache = HashMap::new();
        
        match self.internal {
            ArchiveInternal::Zip(ref mut archive) => {
                for i in 0..archive.len() {
                    let mut file = archive.by_index(i)?;
                    if file.is_file() {
                        let fname = file.name().to_string();
                        let mut buffer = Vec::new();
                        file.read_to_end(&mut buffer)?;
                        new_cache.insert(fname, buffer);
                    }
                }
            }
            ArchiveInternal::SevenZ { ref archive_path } => {
                let mut reader = sevenz_rust::SevenZReader::open(archive_path, sevenz_rust::Password::empty())?;
                reader.for_each_entries(|entry, entry_reader| {
                    if !entry.is_directory() {
                        let fname = entry.name().replace("\\", "/");
                        let mut buffer = Vec::new();
                        entry_reader.read_to_end(&mut buffer)?;
                        new_cache.insert(fname, buffer);
                    }
                    Ok(true)
                })?;
            }
            ArchiveInternal::Rar { ref archive_path } => {
                let mut archive = unrar::Archive::new(archive_path).open_for_processing()?;
                while let Some(header) = archive.read_header()? {
                    let filename = header.entry().filename.to_string_lossy().replace("\\", "/");
                    let (data, next_archive) = header.read()?;
                    new_cache.insert(filename, data);
                    archive = next_archive;
                }
            }
        }

        // キャッシュへ格納
        let data = new_cache.get(name).ok_or_else(|| format!("File '{}' not found after slurping", name))?.clone();
        {
            let mut cache = self.cache.lock().unwrap();
            *cache = Some(new_cache);
        }

        println!("[Archive] Slurping complete. Memory items: {}", self.cache.lock().unwrap().as_ref().unwrap().len());
        let decoded = _decode_image_from_memory(&data)?;
        Ok(decoded)
    }
}

impl Drop for ArchiveLoader {
    fn drop(&mut self) {
        // 一時ディレクトリを使用しなくなったため、何もしない
    }
}
