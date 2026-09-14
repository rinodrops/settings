# Settings

TOML 形式の設定ファイルを持つアプリケーション向けの，独立した設定ウィンドウ。
別プロセスのバイナリとして動作し，TOML の読み書きのみを担うため，親アプリのライフサイクルに依存しません。

|             | Light                                                          | Dark                                                       |
| ----------- | -------------------------------------------------------------- | ---------------------------------------------------------- |
| **macOS**   | ![macOS Light（日本語）](docs/assets/mac-light-jp.png)         | ![macOS Dark（英語）](docs/assets/mac-dark-en.png)         |
| **Windows** | ![Windows Light（ドイツ語）](docs/assets/windows-light-de.png) | ![Windows Dark（日本語）](docs/assets/windows-dark-jp.png) |
| **Linux**   | ![Linux Light（英語）](docs/assets/linux-light-en.png)         | ![Linux Dark（ドイツ語）](docs/assets/linux-dark-de.png)   |

**English:** [README.md](README.md)

---

## 機能

- **スキーマ駆動** — TOML ファイルで設定を記述するだけ。コード不要
- **16 言語組み込み** — アラビア語・中国語（普通話）・ドイツ語・英語・フランス語・ヒンディー語・イタリア語・日本語・韓国語・オランダ語・ポルトガル語・ロシア語・スペイン語・スウェーデン語・トルコ語・ベトナム語
- **Light / Dark テーマ** — OS 追従または固定。カラーパレットを完全カスタマイズ可能
- **外部変更検知** — 設定ファイルを監視し，競合時にダイアログを表示
- **設定マイグレーション** — キーのリネーム・移動・削除をスキーマに宣言すれば，起動時に旧い設定ファイルを自動移行
- **クロスプラットフォーム** — macOS・Windows・Linux

## クイックスタート

**1. `schema.toml` を作成する:**

```toml
icon_style = "rounded"
theme      = "os"
lang       = "os"

[[tabs]]
id    = "general"
label = { en = "General", ja = "一般" }
icon  = "settings"

[[tabs.fields]]
key    = "server.host"
label  = { en = "Host", ja = "ホスト" }
type   = "string"
widget = "text_input"

[[tabs.fields]]
key    = "server.port"
label  = { en = "Port", ja = "ポート" }
type   = "number"
widget = "drag_value"
min    = 1
max    = 65535
```

**2. ビルド**（[just](https://github.com/casey/just) が必要）:

```bash
cd settings
just init                               # 初回 clone 後・アイコン変更時（下記参照）
SETTINGS_SCHEMA=/path/to/your/schema.toml just binary
```

**3. 親アプリから起動:**

```rust
std::process::Command::new("path/to/settings").spawn()?;
```

## 親アプリへの組み込み

同じ設定ファイルに対して同時に開ける Settings プロセスは 1 つまでです。同じ
config パスで再度起動した場合は何もせず終了します。別の config パスなら同時に
複数のウィンドウを開けます。

### macOS `.app` バンドル

```
MyApp.app/Contents/MacOS/
├── MyApp
└── settings
```

```rust
let settings = std::env::current_exe()?
    .parent().unwrap()
    .join("settings");
std::process::Command::new(settings).spawn()?;
```

### Windows

`Settings.exe` を親アプリの実行ファイルと同じディレクトリに置き，インストーラーで両方を配置します。

### Cargo workspace（Git submodule）

```toml
# 親アプリの Cargo.toml
[workspace]
members = ["myapp", "settings"]
```

スキーマはビルド時に `SETTINGS_SCHEMA` 環境変数で指定します。`just binary` の既定スキーマは `../schema.toml`（クレートルートの一階層上，親アプリのスキーマ）です。`SETTINGS_SCHEMA` を設定せず `cargo build` を直接実行した場合は `demo/schema.toml` にフォールバックします。

## ビルド

ビルドレシピは **`just`** で定義されています（ルート [Justfile](Justfile)、デモは [demo/Justfile](demo/Justfile)）。

| 場面 | 使用されるスキーマ |
| ---- | ------------------ |
| `just binary`（上書きなし） | `../schema.toml` — クレートルートの一階層上，親アプリのスキーマ |
| `SCHEMA=/path/to/schema.toml just binary` | 指定パス |
| `SETTINGS_SCHEMA=/path just binary` | 指定パス（優先） |
| `SETTINGS_SCHEMA` なしの `cargo build` | `demo/schema.toml` — スタンドアロン開発用フォールバック |

### `just init`

リポジトリを clone した直後やアイコン素材を差し替えた直後に **`just init`** を実行します。何度実行しても安全です。各ビルドレシピは `_ensure-*` で不足分を補うため，毎回 `init` は必須ではありません。生成物を明示的に更新したいとき（特にアイコン変更のコミット前）に使います。

| 場所 | コマンド | 更新内容 |
| ---- | -------- | -------- |
| ルート | `just init` | Material アイコンフォント（`assets/icons.ttf`），`assets/appicon.png` から Windows 用 `.ico`，任意のツール案内 |
| `demo/` | `just init` | 上記に加え，macOS では `appicon-macos.png` から `demo/assets/AppIcon.icns` を再生成 |

ルートの `just init` は `SCHEMA` が設定されていればそれを使い，未設定なら `demo/schema.toml` でフォントを取得します。アイコンスタイルの上書き:

```bash
ICON_STYLE=outlined just init
SCHEMA=/path/to/schema.toml just init
```

**macOS デモアイコン:** デモ `.app` は `demo/assets/AppIcon.icns` があればそれをコピーします。`demo/assets/appicon-macos.png` を差し替えたら macOS で `just init` → ビルド確認 → **PNG と `AppIcon.icns` をセットでコミット**してください。Linux / Windows では ICNS 生成はスキップされます（`iconutil` が macOS 専用のため）。Mac 上で生成するか，別環境で作った `.icns` をコミットしてください。

### ルート Justfile（同梱 + checker）

```bash
cd settings

just init                               # 初回 clone 後・アイコン / icon_style 変更時
ICON_STYLE=outlined just init           # rounded / outlined / sharp

# 親アプリ同梱用バイナリ（現在のアーキテクチャ）
just binary
SCHEMA=/path/to/schema.toml just binary

# スキーマ検証のみ（GUI はビルドしない）
just check-schema
SCHEMA=./demo/schema.toml just check-schema

# スタンドアロン配布物（本番スキーマ・任意）
just settings-darwin-build-arm64        # → dist/settings/darwin-arm64/
just settings-win-build                 # → dist/settings/windows-x86_64/
just settings-linux-build               # → dist/settings/linux-x86_64/

# スキーマ検証 CLI（スキーマは埋め込まない）
just checker-darwin-build-arm64         # → dist/settings-schema-checker/darwin-arm64/
just checker-linux-release              # zip + .deb

just clean
```

### デモ Justfile（`demo/`）

ウィジェット試験用スタンドアロン GUI（`demo/schema.toml` 埋め込み，`config.toml` 同梱）。

```bash
cd demo

just init                               # 初回 clone 後・appicon-macos.png 変更時
just dev                                # 現在の PF 向け未署名ビルド
just darwin-build-arm64                 # macOS .app → dist/darwin-arm64/
just linux-release                      # zip + .deb + AppImage
just win-zip                            # Windows .exe + config.toml
just release                            # 署名済み macOS + 全 PF 向けリリースパッケージ
```

### 出力先

| パス | 説明 |
| ---- | ---- |
| `target/release/settings` | 親 macOS / Linux アプリに同梱する生バイナリ |
| `dist/settings/<arch>/` | 本番スキーマのスタンドアロン GUI（`darwin-arm64`, `windows-x86_64` 等） |
| `dist/settings-schema-checker/<arch>/` | スキーマ作者向け `settings-schema-checker` CLI |
| `demo/dist/<arch>/` | `config.toml` 同梱のデモ GUI |

ARCH slug: `darwin-arm64`, `darwin-x86_64`, `windows-x86_64`, `linux-x86_64`。

### GitHub Releases

`v*` タグの push（または **Release** ワークフローの手動実行）で，**checker** と **demo**
を Linux（`ubuntu-24.04`）・macOS（arm64 + x86_64，署名・公証済み）・Windows（未署名 zip）
向けにビルドして draft release に添付します。詳細は [`.github/workflows/release.yml`](.github/workflows/release.yml)。

移行期間中，旧 `Makefile` ターゲット名は Justfile 内の非推奨エイリアスとして残っています。新規作業は `just` を使用してください。

## ドキュメント

スキーマリファレンス・ウィジェットガイド・テーマ・ローカライズの詳細:

**https://emotiongraphics.jp/docs/ja/ref/settings/**

`multiline` の `rows` は表示行数（省略時 4）。溢れた値はウィジェット内でスクロールし，フォーム全体は伸びない。

### `segmented_control` と `type`

`segmented_control` は既定で文字列（`type` 省略または `type = "string"`）を読み書きします。
`type = "number"` にすると TOML の数値として保存します。`options` はセグメントのラベル兼
保存値となる数値の文字列表現です。すべての option に `.` が含まれない場合は整数リテラル
（例: `count = 3`）、いずれかに `.` がある場合は浮動小数点リテラル（例: `weight = 2.0`）
として書き込みます。

## 設定マイグレーション

親アプリが設定キーをリネーム・移動・削除すると，既存のユーザー設定ファイルは旧レイアウトの
まま取り残されます。変更をスキーマに宣言しておくと，Settings が起動時に設定ファイルを移行
します（コメントとフォーマットは保持）。

```toml
schema_version = 2                        # 目標バージョン
# migration_version_key = "schema_version"  # 任意: 進捗を記録する設定キー名

[[migration]]
version = 2
  [[migration.rename]]                    # 値の移動（同一・別テーブル）
  from = "display.font_size"
  to   = "general.font_size"

  [[migration.delete]]                    # キー削除
  key = "display.tick_rate"

  [[migration.transform]]                 # 移動 + 変換
  from  = "mode"
  to    = "vivarium.enabled"
  type  = "enum_to_bool"                  # from が match と一致すれば true，それ以外は false
  match = "vivarium"
```

挙動:

- 起動時に，設定ファイルに記録されたバージョン（トップレベル `schema_version`，または
  `migration_version_key` で指定したキー）とスキーマの `schema_version` を比較し，未適用の
  `[[migration]]` を `version` 昇順で適用し，新しいバージョンを記録して保存します。
- 記録バージョンが無い場合は `0` とみなし，すべてのマイグレーションを適用します。
- 各操作は冪等です。移行元キーが無ければ何もせず，移行先が既に存在する移動はスキップします
  （既存の値を保持）。
- 空になった親テーブル（例: `[display]`）は除去します。
- `settings-schema-checker` がマイグレーションブロックを検証します（バージョン重複，
  `schema_version` を超えるバージョン，`match` の無い `enum_to_bool` 等）。

## ライセンス

MIT License
