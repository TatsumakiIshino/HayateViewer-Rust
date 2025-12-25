use crate::image::ImageSource;
use crate::image::cache::SharedImageCache;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

pub enum LoaderRequest {
    Load {
        index: usize,
        priority: i32,
        use_cpu_color_conversion: bool,
    },
    SetSource {
        source: ImageSource,
        path_key: String,
    },
    Clear,
    ClearPrefetch,
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum LoaderResponse {
    Loaded { index: usize },
}

// winit のカスタムイベント用
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum UserEvent {
    PageLoaded(usize),
    ToggleSpreadView,
    ToggleBindingDirection,
    ToggleFirstPageSingle,
    ToggleCpuColorConversion,
    RotateResamplingCpu,
    RotateResamplingGpu,
    ToggleStatusBar,
    RotateRenderingBackend,
    RotateDisplayMode,
    SetMagnifierZoom(f32),
    LoadPath(String),
    LoadHistory(usize),
    ClearHistory,
    DeleteHistoryItem(usize),
    SetMaxHistoryCount(usize),
}

pub struct AsyncLoader {
    request_tx: mpsc::Sender<LoaderRequest>,
    response_rx: Mutex<mpsc::Receiver<LoaderResponse>>,
}

impl AsyncLoader {
    pub fn new(
        cache: SharedImageCache,
        proxy: winit::event_loop::EventLoopProxy<UserEvent>,
    ) -> Arc<Self> {
        let (req_tx, mut req_rx) = mpsc::channel(500);
        let (res_tx, res_rx) = mpsc::channel(500);

        let loader = Arc::new(Self {
            request_tx: req_tx,
            response_rx: Mutex::new(res_rx),
        });

        let cache_clone = cache.clone();
        let event_proxy = proxy.clone();

        tokio::spawn(async move {
            let mut current_source: Option<ImageSource> = None;
            let mut current_path_key: String = String::new();
            let mut queue = std::collections::VecDeque::new();

            loop {
                // 1. 新しいリクエストを全てキューに取り込む
                while let Ok(req) = req_rx.try_recv() {
                    match req {
                        LoaderRequest::Clear => {
                            queue.clear();
                        }
                        LoaderRequest::ClearPrefetch => {
                            queue.retain(|r| matches!(r, LoaderRequest::Load { priority: 0, .. }));
                        }
                        LoaderRequest::SetSource { source, path_key } => {
                            println!("[読み込み] ソースを設定: {}", path_key);
                            current_source = Some(source);
                            current_path_key = path_key;
                            queue.clear();
                        }
                        _ => {
                            queue.push_back(req);
                        }
                    }
                }

                // 2. キューが空なら次のメッセージを待機
                if queue.is_empty() {
                    match req_rx.recv().await {
                        Some(req) => match req {
                            LoaderRequest::Clear => {
                                queue.clear();
                                continue;
                            }
                            LoaderRequest::ClearPrefetch => {
                                continue;
                            }
                            LoaderRequest::SetSource { source, path_key } => {
                                println!("[読み込み] ソースを設定: {}", path_key);
                                current_source = Some(source);
                                current_path_key = path_key;
                                queue.clear();
                                continue;
                            }
                            _ => queue.push_back(req),
                        },
                        None => break, // チャンネルクローズ
                    }
                }

                // 3. 最適なリクエストを選択
                // Priority 0 (表示要求) を最優先し、その中でも最新のもの (rposition) を選ぶ
                let next_req_idx = queue
                    .iter()
                    .rposition(|r| matches!(r, LoaderRequest::Load { priority: 0, .. }));
                let next_req = if let Some(pos) = next_req_idx {
                    queue.remove(pos).unwrap()
                } else {
                    queue.pop_front().unwrap()
                };

                match next_req {
                    LoaderRequest::Load {
                        index,
                        priority,
                        use_cpu_color_conversion,
                    } => {
                        if let Some(ref mut _source) = current_source {
                            let key = format!("{}::{}", current_path_key, index);

                            let already_cached = {
                                let mut c = cache_clone.lock().unwrap();
                                c.get(&key).is_some()
                            };

                            if !already_cached {
                                println!(
                                    "[読み込み] デコード中: インデックス {} (優先度 {})...",
                                    index, priority
                                );
                                // 重い処理（特に7z一括展開）をスレッドプールに逃がす
                                let mut source_for_task = current_source.take().unwrap();
                                let (res, returned_source) =
                                    tokio::task::spawn_blocking(move || {
                                        let r = source_for_task
                                            .load_image(index, use_cpu_color_conversion)
                                            .map_err(|e| e.to_string());
                                        (r, source_for_task)
                                    })
                                    .await
                                    .unwrap();

                                current_source = Some(returned_source);

                                match res {
                                    Ok(decoded) => {
                                        {
                                            let mut c = cache_clone.lock().unwrap();
                                            c.insert(key.clone(), Arc::new(decoded));
                                        }
                                        let _ = res_tx.send(LoaderResponse::Loaded { index }).await;
                                        let _ =
                                            event_proxy.send_event(UserEvent::PageLoaded(index));
                                    }
                                    Err(e) => {
                                        println!(
                                            "[読み込み] デコード失敗 インデックス {}: {}",
                                            index, e
                                        );
                                    }
                                }
                            } else {
                                // 既にキャッシュにある場合も完了通知は送る
                                let _ = res_tx.send(LoaderResponse::Loaded { index }).await;
                                let _ = event_proxy.send_event(UserEvent::PageLoaded(index));
                            }
                        }
                    }
                    _ => {}
                }
            }
        });

        loader
    }

    pub async fn send_request(&self, req: LoaderRequest) {
        let _ = self.request_tx.send(req).await;
    }

    pub fn try_recv_response(&self) -> Option<LoaderResponse> {
        let mut rx = self.response_rx.lock().unwrap();
        rx.try_recv().ok()
    }

    pub fn clone_tx(&self) -> mpsc::Sender<LoaderRequest> {
        self.request_tx.clone()
    }
}
