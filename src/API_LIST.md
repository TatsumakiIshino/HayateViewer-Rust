# HayateViewer Rust 内部 API リスト

このドキュメントは、HayateViewer Rust 版の主要な構造体、トレイト、および列挙型の概要をまとめたものです。開発時の参照用として利用してください。

---

## 1. レンダリング基盤 (`src/render/`)

レンダリングエンジン（Direct2D, D3D11, OpenGL）を抽象化するための仕組みです。

### `Renderer` トレイト (`mod.rs`)

レンダリングバックエンドが実装すべき共通のメソッド群です。

- `resize(&self, width: u32, height: u32)`: ウィンドウリサイズ時の処理
- `begin_draw(&self)`: 描画開始
- `end_draw(&self)`: 描画終了
- `upload_image(&self, image: &DecodedImage)`: デコード済み画像をテクスチャとしてGPUへ転送
- `draw_image(&self, texture: &TextureHandle, dest_rect: &D2D_RECT_F)`: テクスチャを表示
- `fill_rectangle(&self, rect: &D2D_RECT_F, color: &D2D1_COLOR_F)`: 塗りつぶし矩形
- `draw_rectangle(&self, rect: &D2D_RECT_F, color: &D2D1_COLOR_F, stroke_width: f32)`: 枠線矩形
- `set_interpolation_mode(&mut self, mode: InterpolationMode)`: 補間モードの設定

### `TextureHandle` 列挙型 (`mod.rs`)

各バックエンドのテクスチャオブジェクトを抽象化したハンドルです。

- `Direct2D(ID2D1Bitmap1)`
- `D3D11Rgba(ID3D11ShaderResourceView)`
- `D3D11YCbCr { y, cb, cr, ... }`: YCbCr プレーン（GPUでRGB変換）
- `OpenGL { id, ... }`
- `OpenGLYCbCr { y, cb, cr, ... }`

---

## 2. 画像処理・デコード (`src/image/`)

画像の読み込み、キャッシュ、およびデコードを管理します。

### `ImageSource` 列挙型 (`mod.rs`)

画像データの供給源を一元化します。

- `Files(Vec<String>)`: 通常のファイルシステム上の画像群
- `Archive(ArchiveLoader)`: 書庫ファイル（ZIP, 7z, RAR等）内の画像群

### `DecodedImage` 構造体 (`cache.rs`)

デコードされたピクセルデータとサイズを保持します。

- `width, height`: 画像サイズ
- `pixel_data`: `PixelData` (RGBA8 または YCbCr)

### `PixelData` 列挙型 (`cache.rs`)

デコード済みデータの持たせ方を定義します。

- `Rgba8(Vec<u8>)`: 標準的なRGBAピクセル
- `Ycbcr { planes, subsampling, ... }`: JPEG2000 等で利用される YCbCr 形式

---

## 3. アプリケーション状態 (`src/state.rs`)

ビューアの再生状態や表示設定を管理します。

### `AppState` 構造体

- `image_files`: 現在の画像パスリスト
- `current_page_index`: 表示中のページ番号（0開始）
- `is_spread_view`: 見開き表示フラグ
- `binding_direction`: 綴じ方向（`Left` or `Right`）
- `spread_view_first_page_single`: 1ページ目を単一表示するか
- `get_page_indices_to_display()`: 現在の状態で表示すべき全インデックスを計算
- `navigate(direction: i32)`: ページの進退処理（見開きを考慮）

---

## 4. 非同期読み込み・キャッシュ (`src/image/loader.rs`, `src/image/cache.rs`)

### `AsyncLoader` 構造体

バックグラウンドスレッドで画像のデコードとキャッシュ管理を行います。

- `LoaderRequest`: 読み込み、ソース変更、クリア等の要求
- `LoaderResponse`: 完了通知（`Loaded`）
- `UserEvent`: `winit` への通知用に変換されたイベント

### `ImageCache` 構造体

LRU（Least Recently Used）ベースのキャッシュ管理に加え、現在ページからの距離による優先度管理を行います。

- `insert(key, image)`: 指定したキー（パス::インデックス）でキャッシュ
- `set_current_context(index, protected)`: 現在のページ位置と絶対に破棄してはいけない（表示中の）インデックスを設定

### `UserEvent` 列挙型 (`loader.rs`)

UIとメインループ間の通信用イベント。

- `PageLoaded(index)`: 画像読み込み完了
- `ToggleSpreadView`: 見開き切り替え
- `RotateDisplayMode`: 表示モード（単一/左綴じ/右綴じ）のトグル
- `SetMagnifierZoom(f32)`: ルーペ倍率の変更

---

## 5. 設定管理 (`src/config.rs`)

### `Settings` 構造体

`config.json` と連動する永続的な設定項目。

- `rendering_backend`: 利用する描画エンジン名
- `max_cache_size_mb`: CPUキャッシュ上限
- `parallel_decoding_workers`: デコード用スレッド数
- `magnifier_zoom`: ルーペ倍率
- `load_or_default()` / `save()`: 設定の読み書き
