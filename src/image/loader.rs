use crate::image::cache::SharedImageCache;
use crate::image::ImageSource;
use tokio::sync::mpsc;
use std::sync::{Arc, Mutex};

pub enum LoaderRequest {
    Load { index: usize, #[allow(dead_code)] priority: i32 },
    SetSource { source: ImageSource, path_key: String },
    #[allow(dead_code)]
    Clear,
}

pub enum LoaderResponse {
    Loaded { index: usize },
}

pub struct AsyncLoader {
    request_tx: mpsc::Sender<LoaderRequest>,
    response_rx: Mutex<mpsc::Receiver<LoaderResponse>>,
}

impl AsyncLoader {
    pub fn new(cache: SharedImageCache) -> Arc<Self> {
        let (req_tx, mut req_rx) = mpsc::channel(100);
        let (res_tx, res_rx) = mpsc::channel(100);

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
                        current_source = Some(source);
                        current_path_key = path_key;
                    }
                    LoaderRequest::Clear => {
                        current_source = None;
                        current_path_key.clear();
                        let mut c = cache.lock().unwrap();
                        c.clear();
                    }
                    LoaderRequest::Load { index, .. } => {
                        if let Some(ref mut source) = current_source {
                            let key = format!("{}::{}", current_path_key, index);
                            
                            // すでにキャッシュにあるか確認
                            let already_cached = {
                                let mut c = cache.lock().unwrap();
                                c.get(&key).is_some()
                            };

                            if !already_cached {
                                if let Ok(decoded) = source.load_image(index) {
                                    let mut c = cache.lock().unwrap();
                                    c.insert(key.clone(), Arc::new(decoded));
                                }
                            }
                            
                            let _ = res_tx.send(LoaderResponse::Loaded { index }).await;
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
}
