# HayateViewer Rust

Direct2D, D3D11, OpenGL などの多様な描画バックエンドを選択可能な、高速・軽量な画像・自炊コミックビューアです。
Python版 HayateViewer のコンセプトを継承し、更なるパフォーマンスを追求して Rust で再構築されています。

![Settings Screenshot](file:///C:/Users/Tatsumaki/.gemini/antigravity/brain/ce66b745-8a9e-4822-8053-62b75d676c0e/uploaded_image_1766270453034.png)

## 主な特徴

- **マルチ・レンダリングエンジン**:
  - **Direct2D**: 高品質な標準エンジン。
  - **Direct3D 11**: YCbCr シェーダーによる超高速描画。
  - **OpenGL**: クロスプラットフォームを見据えた高性能エンジン。
- **統一されたサンプリング設定**: エンジンを選ばず、常に最適なサンプリングモード（Nearest Neighbor から Lanczos まで）を選択可能。
- **2層キャッシュシステム**: CPU (メモリ) と GPU (VRAM) の両方で画像をキャッシュし、スムーズなページめくりを実現。
- **多様なフォーマットへの対応**:
  - 画像: JPEG, PNG, BMP, WEBP, **JPEG 2000 (JP2)**
  - 書庫: ZIP (CBZ), 7z (CB7), **RAR (CBR)**
- **快適な閲覧機能**:
  - 表示モード（単一ページ / 左綴じ見開き / 右綴じ見開き）
  - リアルタイムルーペ機能（右クリック）
  - スムーズなズーム・パン
  - シークバー表示（マウスドラッグ対応）
  - ページジャンプ UI (Shift+S)
- **Modern UI 設定画面**: デザイン性に優れた半透明オーバーレイによる日本語設定画面。

## GPU リサンプリングの対応状況

現在、設定画面から以下のモードを選択可能です。

| モード名 | 内容 | 対応状況 |
| :--- | :--- | :--- |
| **Nearest Neighbor** | 最近傍補間 | 全エンジン対応 (最速) |
| **Bilinear** | 双線形補間 | 全エンジン対応 (標準) |
| **Bicubic** | 双三次補間 | D2D: フル対応 / GL・D3D11: Bilinear ベース |
| **Lanczos3** | ランツォシュ | D2D: 最高品質対応 / GL・D3D11: Bilinear ベース |

> **Note**: OpenGL および Direct3D 11 (YCbCr 描画) における Cubic/Lanczos3 の本格的なシェーダー実装は、次期テストブランチにて Python 版同等のエンジンとして統合予定です。

## システム要件

- OS: Windows 10 / 11 (64bit)
- グラフィックス: DirectX 11.0 または OpenGL 3.3 以上に対応した GPU

## クイックスタート

### 開発用ビルドと実行

```powershell
cargo run
```

### リリース用ビルド

```powershell
cargo build --release
```

バイナリは `target/release/hayate-viewer-rust.exe` に生成されます。

## 操作方法

| キー / マウス | 動作 |
| :--- | :--- |
| `左右キー` / `ホイール` | ページ移動 |
| `O` (オー) | 設定画面を開く / 閉じる |
| `B` | 表示モードの切り替え (単一 -> 左綴じ -> 右綴じ) |
| `右クリック` | ルーペ（拡大表示） |
| `左ドラッグ` | パン（移動） |
| `S` | シークバーの表示切替 |
| `Shift + S` | ページジャンプ UI を開く |
| `Esc` | 設定画面 / ページジャンプを閉じる |

## ライセンス

[MIT License](LICENSE) (予定)
