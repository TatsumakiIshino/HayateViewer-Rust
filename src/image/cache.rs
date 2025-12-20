use lru::LruCache;
use std::sync::{Arc, Mutex};
use std::num::NonZeroUsize;
use crate::image::decoder::DecodedImage;

pub type CacheKey = String;

pub struct ImageCache {
    cache: LruCache<CacheKey, Arc<DecodedImage>>,
}

impl ImageCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            cache: LruCache::new(NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::new(10).unwrap())),
        }
    }

    pub fn get(&mut self, key: &CacheKey) -> Option<Arc<DecodedImage>> {
        self.cache.get(key).cloned()
    }

    pub fn insert(&mut self, key: CacheKey, image: Arc<DecodedImage>) {
        self.cache.put(key, image);
    }

    pub fn clear(&mut self) {
        self.cache.clear();
    }

    pub fn len(&self) -> usize {
        self.cache.len()
    }

    pub fn get_keys(&self) -> Vec<String> {
        self.cache.iter().map(|(k, _)| k.clone()).collect()
    }
}

pub type SharedImageCache = Arc<Mutex<ImageCache>>;

pub fn create_shared_cache(capacity: usize) -> SharedImageCache {
    Arc::new(Mutex::new(ImageCache::new(capacity)))
}
