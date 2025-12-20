pub struct AppState {
    pub image_files: Vec<String>,
    pub folder_start_indices: Vec<usize>,
    pub current_page_index: usize,
    pub is_spread_view: bool,
    pub binding_direction: BindingDirection,
    pub spread_view_first_page_single: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingDirection {
    Left,
    Right,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            image_files: Vec::new(),
            folder_start_indices: Vec::new(),
            current_page_index: 0,
            is_spread_view: true,
            binding_direction: BindingDirection::Right,
            spread_view_first_page_single: true,
        }
    }

    pub fn get_page_indices_to_display(&self) -> Vec<usize> {
        let total_pages = self.image_files.len();
        if total_pages == 0 {
            return Vec::new();
        }

        if !self.is_spread_view {
            return vec![self.current_page_index];
        }

        // 見開き表示モード
        let mut single_page_indices = std::collections::HashSet::new();
        if self.spread_view_first_page_single {
            single_page_indices.insert(0);
            for &idx in &self.folder_start_indices {
                single_page_indices.insert(idx);
            }
        }

        if single_page_indices.contains(&self.current_page_index) {
            return vec![self.current_page_index];
        }

        let page1 = self.current_page_index;
        let page2 = self.current_page_index + 1;

        if page2 >= total_pages || single_page_indices.contains(&page2) {
            return vec![page1];
        }

        match self.binding_direction {
            BindingDirection::Right => vec![page2, page1],
            BindingDirection::Left => vec![page1, page2],
        }
    }

    pub fn navigate(&mut self, direction: i32) {
        let total_pages = self.image_files.len();
        if total_pages == 0 { return; }

        let mut step = if self.is_spread_view { 2 } else { 1 };

        if self.is_spread_view && self.spread_view_first_page_single {
            let mut single_page_indices = std::collections::HashSet::new();
            single_page_indices.insert(0);
            for &idx in &self.folder_start_indices {
                single_page_indices.insert(idx);
            }

            if direction > 0 {
                if single_page_indices.contains(&self.current_page_index) || 
                   single_page_indices.contains(&(self.current_page_index + 1)) {
                    step = 1;
                }
            } else {
                if single_page_indices.contains(&(self.current_page_index.saturating_sub(1))) {
                    step = 1;
                }
            }
        }

        let new_index = if direction > 0 {
            (self.current_page_index + step).min(total_pages - 1)
        } else {
            self.current_page_index.saturating_sub(step)
        };

        self.current_page_index = new_index;
    }
}
