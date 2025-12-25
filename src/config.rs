use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HistoryItem {
    pub path: String,
    pub page: usize,
    pub binding: String, // "left", "right", "single"
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Settings {
    pub rendering_backend: String,
    pub is_spread_view: bool,
    pub binding_direction: String,
    pub spread_view_first_page_single: bool,
    pub window_size: (u32, u32),
    pub window_position: (i32, i32),
    pub window_geometry: (i32, i32, u32, u32),
    pub parallel_decoding_workers: usize,
    pub resampling_mode_cpu: String,
    pub resampling_mode_gpu: String,
    pub show_advanced_cache_options: bool,
    pub max_cache_size_mb: u64,
    pub cpu_max_prefetch_pages: usize,
    pub gpu_max_prefetch_pages: usize,
    pub show_status_bar_info: bool,
    pub use_cpu_color_conversion: bool,
    pub magnifier_zoom: f32,
    pub history: Vec<HistoryItem>,
    pub max_history_count: usize,
    /// ページめくりアニメーションを有効にするか（D2Dレンダリング時は無効）
    pub page_turn_animation_enabled: bool,
    /// ページめくりアニメーションの速度（秒単位、0.1〜2.0）
    pub page_turn_duration: f32,
    /// ページめくりアニメーションの種類 ("slide", "curl", "none")
    pub page_turn_animation_type: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            rendering_backend: "direct2d".to_string(), // Rust版のデフォルトは D2D
            is_spread_view: true,
            binding_direction: "left".to_string(),
            spread_view_first_page_single: true,
            window_size: (1280, 768),
            window_position: (100, 100),
            window_geometry: (100, 100, 1280, 768),
            parallel_decoding_workers: 8,
            resampling_mode_cpu: "PIL_LANCZOS".to_string(),
            resampling_mode_gpu: "Lanczos".to_string(),
            show_advanced_cache_options: true,
            max_cache_size_mb: 4096,
            cpu_max_prefetch_pages: 10,
            gpu_max_prefetch_pages: 9,
            show_status_bar_info: true,
            use_cpu_color_conversion: false,
            magnifier_zoom: 2.0,
            history: Vec::new(),
            max_history_count: 50,
            page_turn_animation_enabled: true,
            page_turn_duration: 0.5,
            page_turn_animation_type: "slide".to_string(),
        }
    }
}

impl Settings {
    pub fn load_or_default<P: AsRef<Path>>(path: P) -> Self {
        if let Ok(content) = fs::read_to_string(path) {
            if let Ok(settings) = serde_json::from_str(&content) {
                return settings;
            }
        }
        Self::default()
    }

    pub fn save<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let content = serde_json::to_string_pretty(self).unwrap();
        fs::write(path, content)
    }

    pub fn add_to_history(&mut self, path: String, page: usize, binding: String) {
        // すでに存在する場合は一旦削除して先頭に持ってくる
        self.history.retain(|item| item.path != path);
        self.history.insert(
            0,
            HistoryItem {
                path,
                page,
                binding,
            },
        );
        if self.history.len() > self.max_history_count {
            self.history.truncate(self.max_history_count);
        }
    }

    pub fn remove_from_history(&mut self, index: usize) {
        if index < self.history.len() {
            self.history.remove(index);
        }
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
    }
}
