use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

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
}
