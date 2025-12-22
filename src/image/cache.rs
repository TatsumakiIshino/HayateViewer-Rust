use lru::LruCache;
use std::sync::{Arc, Mutex};
use std::num::NonZeroUsize;
use std::collections::HashSet;

pub type CacheKey = String;

#[derive(Clone, Debug)]
pub enum PixelData {
    Rgba8(Vec<u8>),
    Ycbcr {
        planes: Vec<Vec<i32>>, // Y, Cb, Cr
        subsampling: (u8, u8), // (dx, dy)
        precision: u8,         // bit深度
        y_is_signed: bool,     // Y が符号付きか
        c_is_signed: bool,     // Cb/Cr が符号付きか
    },
}

impl PixelData {
    pub fn len(&self) -> usize {
        match self {
            Self::Rgba8(data) => data.len(),
            Self::Ycbcr { planes, .. } => planes.iter().map(|p| p.len() * 4).sum(),
        }
    }
}

pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub pixel_data: PixelData,
}

impl DecodedImage {
    pub fn memory_size(&self) -> usize {
        self.pixel_data.len()
    }
}

pub struct ImageCache {
    cache: LruCache<CacheKey, Arc<DecodedImage>>,
    max_bytes: usize,
    current_bytes: usize,
    current_index: usize,
    protected_indices: HashSet<usize>,
}

impl ImageCache {
    pub fn new(capacity_items: usize, max_bytes: usize) -> Self {
        Self {
            // 枚数制限は十分大きく取り、メモリサイズおよび距離で管理する
            cache: LruCache::new(NonZeroUsize::new(capacity_items.max(200)).unwrap()),
            max_bytes,
            current_bytes: 0,
            current_index: 0,
            protected_indices: HashSet::new(),
        }
    }

    pub fn set_current_context(&mut self, current_index: usize, protected: Vec<usize>) {
        self.current_index = current_index;
        self.protected_indices = protected.into_iter().collect();
    }

    fn extract_index(key: &str) -> Option<usize> {
        key.rsplit("::").next()?.parse().ok()
    }

    pub fn get(&mut self, key: &CacheKey) -> Option<Arc<DecodedImage>> {
        self.cache.get(key).cloned()
    }

    pub fn insert(&mut self, key: CacheKey, image: Arc<DecodedImage>) {
        let size = image.memory_size();
        
        // 既に存在する場合はサイズを差し替える
        if let Some(old) = self.cache.get(&key) {
            self.current_bytes -= old.memory_size();
        }
        
        self.current_bytes += size;
        self.cache.put(key, image);

        // メモリ上限を超えている間、現在位置から最も遠いものから削除
        // 表示中のページ（protected_indices）は削除対象から外す
        while self.current_bytes > self.max_bytes && self.cache.len() > 1 {
            let mut farthest_key = None;
            let mut max_dist = -1isize;

            for (k, _) in self.cache.iter() {
                if let Some(idx) = Self::extract_index(k) {
                    if self.protected_indices.contains(&idx) {
                        continue;
                    }
                    let dist = (idx as isize - self.current_index as isize).abs();
                    if dist > max_dist {
                        max_dist = dist;
                        farthest_key = Some(k.clone());
                    }
                }
            }

            if let Some(k) = farthest_key {
                if let Some(old_img) = self.cache.pop(&k) {
                    self.current_bytes -= old_img.memory_size();
                } else {
                    break;
                }
            } else {
                // すべてのキャッシュが表示中またはインデックス抽出不能な場合、
                // LRUの最も古いものを破棄（ただし表示中は守る）
                let mut key_to_pop = None;
                for (k, _) in self.cache.iter().rev() {
                    if let Some(idx) = Self::extract_index(k) {
                        if self.protected_indices.contains(&idx) { continue; }
                    }
                    key_to_pop = Some(k.clone());
                    break;
                }

                if let Some(k) = key_to_pop {
                    if let Some(old_img) = self.cache.pop(&k) {
                        self.current_bytes -= old_img.memory_size();
                    } else { break; }
                } else {
                    // 削除できるものがない
                    break;
                }
            }
        }
    }

    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.cache.clear();
        self.current_bytes = 0;
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    pub fn get_keys(&self) -> Vec<String> {
        self.cache.iter().map(|(k, _)| k.clone()).collect()
    }
}

pub type SharedImageCache = Arc<Mutex<ImageCache>>;

pub fn create_shared_cache(capacity_items: usize, max_bytes: usize) -> SharedImageCache {
    Arc::new(Mutex::new(ImageCache::new(capacity_items, max_bytes)))
}
