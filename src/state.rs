pub struct AppState {
    pub image_files: Vec<String>,
    pub folder_start_indices: Vec<usize>,
    pub current_page_index: usize,
    pub is_spread_view: bool,
    pub binding_direction: BindingDirection,
    pub spread_view_first_page_single: bool,
    pub is_jump_open: bool,
    pub jump_input_buffer: String,
    pub show_seekbar: bool,
    pub is_dragging_seekbar: bool,
    /// 履歴ナビゲーション用: 現在の履歴インデックス (None = 履歴未使用)
    pub current_history_index: Option<usize>,
    /// ステータスバーに表示する一時メッセージ ((メッセージ, 表示開始時刻))
    pub status_message: Option<(String, std::time::Instant)>,
    /// ページめくりアニメーションの状態
    pub page_turn_animation: Option<PageTurnAnimation>,
}

/// ページめくりアニメーションの状態
#[derive(Debug, Clone)]
pub struct PageTurnAnimation {
    /// アニメーション開始時刻
    pub start_time: std::time::Instant,
    /// アニメーションの方向 (1: 前進, -1: 後退)
    pub direction: i32,
    /// アニメーションの持続時間（秒）
    pub duration: f32,
    /// 遷移前のページインデックス
    pub from_pages: Vec<usize>,
    /// 遷移後のページインデックス
    pub to_pages: Vec<usize>,
}

impl PageTurnAnimation {
    /// アニメーションの進行度を取得 (0.0〜1.0)
    pub fn progress(&self) -> f32 {
        let elapsed = self.start_time.elapsed().as_secs_f32();
        (elapsed / self.duration).min(1.0)
    }

    /// アニメーションが完了したかどうか
    pub fn is_complete(&self) -> bool {
        self.progress() >= 1.0
    }
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
            is_jump_open: false,
            jump_input_buffer: String::new(),
            show_seekbar: false,
            is_dragging_seekbar: false,
            current_history_index: None,
            status_message: None,
            page_turn_animation: None,
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
        if total_pages == 0 {
            return;
        }

        let mut step = if self.is_spread_view { 2 } else { 1 };

        if self.is_spread_view && self.spread_view_first_page_single {
            let mut single_page_indices = std::collections::HashSet::new();
            single_page_indices.insert(0);
            for &idx in &self.folder_start_indices {
                single_page_indices.insert(idx);
            }

            if direction > 0 {
                if single_page_indices.contains(&self.current_page_index)
                    || single_page_indices.contains(&(self.current_page_index + 1))
                {
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

        self.current_page_index = self.snap_to_spread(new_index);
    }

    pub fn snap_to_spread(&self, index: usize) -> usize {
        let total_pages = self.image_files.len();
        if total_pages == 0 {
            return 0;
        }
        let index = index.min(total_pages - 1);

        if !self.is_spread_view {
            return index;
        }

        let mut single_page_indices = std::collections::HashSet::new();
        if self.spread_view_first_page_single {
            single_page_indices.insert(0);
            for &idx in &self.folder_start_indices {
                single_page_indices.insert(idx);
            }
        }

        // 単ページ表示すべきインデックスならそのまま
        if single_page_indices.contains(&index) {
            return index;
        }

        // 一つ前のページが単ページ表示対象なら、現在のページはペアの開始（見開き）
        if index > 0 && single_page_indices.contains(&(index - 1)) {
            return index;
        }

        // それ以外の場合（通常の2ページペアの途中など）
        // 最初の単ページ設定がある場合、0を起点に奇数番目がペアの開始
        // ここでは単純化のため、手前の最も近い single_page_index からの距離で判定する
        let last_single = single_page_indices
            .iter()
            .filter(|&&i| i <= index)
            .max()
            .cloned()
            .unwrap_or(0);

        let diff = index - last_single;
        if diff % 2 == 0 {
            // 距離が偶数なら、単ページ開始位置と同じ「偶数/奇数」性を持つため
            // (例: last_single=0, index=2 なら、0(単), 1(ペア始), 2(ペア終) or 0(単), 1-2(ペア))
            // 実際の実装 (get_page_indices_to_display) に合わせると:
            // 0 が single なら、1, 2 がペア。なので 2 なら 1 に戻すべき。
            if diff > 0 { index - 1 } else { index }
        } else {
            // 距離が奇数 (例: last_single=0, index=1 なら 1-2 ペアの開始)
            index
        }
    }
}
