use crate::image::cache::SharedImageCache;
use crate::image::ImageSource;
use tokio::sync::mpsc;
use std::sync::{Arc, Mutex};

pub enum LoaderRequest {
    Load { index: usize, priority: i32 },
    SetSource { source: ImageSource, path_key: String },
    Clear,
}

#[derive(Debug)]
pub enum LoaderResponse {
    Loaded { index: usize },
}

// winit のカスタムイベント用
#[derive(Debug, Clone, Copy)]
pub enum UserEvent {
    PageLoaded(usize),
}

pub struct AsyncLoader {
    request_tx: mpsc::Sender<LoaderRequest>,
    response_rx: Mutex<mpsc::Receiver<LoaderResponse>>,
}

impl AsyncLoader {
    pub fn new(cache: SharedImageCache, proxy: winit::event_loop::EventLoopProxy<UserEvent>) -> Arc<Self> {
        let (req_tx, mut req_rx) = mpsc::channel(500);
        let (res_tx, res_rx) = mpsc::channel(500);

        let loader = Arc::new(Self {
            request_tx: req_tx,
            response_rx: Mutex::new(res_rx),
        });

        // バックグラウンドタスクの起動
        tokio::spawn(async move {
            let mut current_source: Option<ImageSource> = None;
            let mut current_path_key: String = String::new();

            while let Some(req) = req_rx.recv().await {
                match req {
                    LoaderRequest::SetSource { source, path_key } => {
                        println!("[Loader] SetSource: path_key={}", path_key);
                        current_source = Some(source);
                        current_path_key = path_key;
                    }
                    LoaderRequest::Clear => {
                        println!("[Loader] Clear");
                        current_source = None;
                        current_path_key.clear();
                        let mut c = cache.lock().unwrap();
                        c.clear();
                    }
                    LoaderRequest::Load { index, priority } => {
                        println!("[Loader] Load: index={}, priority={}", index, priority);
                        if let Some(ref mut source) = current_source {
                            let key = format!("{}::{}", current_path_key, index);
                            
                            // 処理直前にキャッシュを再確認
                            let already_cached = {
                                let mut c = cache.lock().unwrap();
                                c.get(&key).is_some()
                            };

                            if !already_cached {
                                println!("[Loader] Decoding index {}...", index);
                                match source.load_image(index) {
                                    Ok(decoded) => {
                                        println!("[Loader] Decoded index {} ({}x{})", index, decoded.width, decoded.height);
                                        let mut c = cache.lock().unwrap();
                                        c.insert(key.clone(), Arc::new(decoded));
                                    }
                                    Err(e) => {
                                        println!("[Loader] FAILED to decode index {}: {}", index, e);
                                    }
                                }
                            } else {
                                println!("[Loader] Index {} already in cache", index);
                            }
                            
                            let _ = res_tx.send(LoaderResponse::Loaded { index }).await;
                            let _ = proxy.send_event(UserEvent::PageLoaded(index));
                        } else {
                            println!("[Loader] Load failed: source is None");
                        }
                    }
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
