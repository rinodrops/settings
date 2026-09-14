# Settings

A standalone settings window for applications that use TOML configuration files.
Ships as a separate binary — it reads and writes TOML only, so its lifecycle is fully independent of the parent app.

|             | Light                                                       | Dark                                                        |
| ----------- | ----------------------------------------------------------- | ----------------------------------------------------------- |
| **macOS**   | ![macOS Light (Japanese)](docs/assets/mac-light-jp.png)     | ![macOS Dark (English)](docs/assets/mac-dark-en.png)        |
| **Windows** | ![Windows Light (German)](docs/assets/windows-light-de.png) | ![Windows Dark (Japanese)](docs/assets/windows-dark-jp.png) |
| **Linux**   | ![Linux Light (English)](docs/assets/linux-light-en.png)    | ![Linux Dark (German)](docs/assets/linux-dark-de.png)       |

**日本語:** [README.ja.md](README.ja.md)

---

## Features

- **Schema-driven** — describe your settings in a TOML file; no code required
- **16 built-in languages** — Arabic, Mandarin, German, English, French, Hindi, Italian, Japanese, Korean, Dutch, Portuguese, Russian, Spanish, Swedish, Turkish, Vietnamese
- **Light / Dark** — OS-adaptive or forced; fully customizable color palette
- **External change detection** — watches the config file and prompts on conflict
- **Config migration** — declare key renames/moves/removals in the schema; old config files migrate automatically on launch
- **Cross-platform** — macOS, Windows, Linux

## Quick Start

**1. Write a `schema.toml`:**

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

**2. Build** (requires [just](https://github.com/casey/just)):

```bash
cd settings
just init                               # first clone / after icon changes (see below)
SETTINGS_SCHEMA=/path/to/your/schema.toml just binary
```

**3. Launch from your app:**

```rust
std::process::Command::new("path/to/settings").spawn()?;
```

## Embedding in Your App

At most one Settings process may edit a given config file. Launching Settings again
with the same config path is a no-op (the second instance exits immediately). Different
config paths may each have their own window concurrently.

### macOS `.app` bundle

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

Place `Settings.exe` in the same directory as your executable and install both together.

### Cargo workspace (Git submodule)

```toml
# Parent app's Cargo.toml
[workspace]
members = ["myapp", "settings"]
```

Point the schema via `SETTINGS_SCHEMA` when building. The default schema path for `just binary` is `../schema.toml` (the parent app's schema, one level up from the crate root). When building directly with `cargo build` without `SETTINGS_SCHEMA`, the fallback is `demo/schema.toml`.

## Build

Build recipes live in **`just`** ([Justfile](Justfile) at the repo root; [demo/Justfile](demo/Justfile) for the demo app).

| Scenario | Schema used |
| -------- | ----------- |
| `just binary` (no override) | `../schema.toml` — parent app's schema, one level up from the crate root |
| `SCHEMA=/path/to/schema.toml just binary` | the specified path |
| `SETTINGS_SCHEMA=/path just binary` | the specified path (takes precedence) |
| `cargo build` without `SETTINGS_SCHEMA` | `demo/schema.toml` — standalone dev fallback |

### `just init`

Run **`just init`** after cloning the repo or when icon assets change. It is safe to
re-run. Build recipes also call `_ensure-*` helpers, so everyday builds work without
`init`; use it when you want to refresh derived files explicitly (especially before
committing icon updates).

| Location | Command | What it refreshes |
| -------- | ------- | ----------------- |
| Repo root | `just init` | Material icon font (`assets/icons.ttf`), Windows `.ico` from `assets/appicon.png`, optional tool hints |
| `demo/` | `just init` | Same as root (via demo schema) **plus** `demo/assets/AppIcon.icns` from `appicon-macos.png` on macOS |

`just init` at the repo root uses `SCHEMA` when set; otherwise it falls back to
`demo/schema.toml` for icon-font download. Override icon style:

```bash
ICON_STYLE=outlined just init
SCHEMA=/path/to/schema.toml just init
```

**macOS demo icon:** the demo `.app` copies committed `demo/assets/AppIcon.icns` when
present. After replacing `demo/assets/appicon-macos.png`, run `just init` on macOS,
verify the build, then commit **both** the PNG and `AppIcon.icns`. On Linux/Windows,
`just init` skips ICNS generation (requires macOS `iconutil`); generate on a Mac or
commit the `.icns` produced elsewhere.

### Root Justfile (embed + checker)

```bash
cd settings

just init                               # first clone / icon or icon_style changes
ICON_STYLE=outlined just init           # rounded / outlined / sharp

# Parent-app binary (current arch)
just binary
SCHEMA=/path/to/schema.toml just binary

# Schema validation only (no GUI build)
just check-schema
SCHEMA=./demo/schema.toml just check-schema

# Standalone production GUI (optional; parent-app bundling is the primary use)
just settings-darwin-build-arm64        # → dist/settings/darwin-arm64/
just settings-win-build                 # → dist/settings/windows-x86_64/
just settings-linux-build               # → dist/settings/linux-x86_64/

# Schema checker CLI (no embedded schema)
just checker-darwin-build-arm64         # → dist/settings-schema-checker/darwin-arm64/
just checker-linux-release              # zip + .deb

just clean
```

### Demo Justfile (`demo/`)

Standalone widget showcase (`demo/schema.toml` embedded, `config.toml` bundled).

```bash
cd demo

just init                               # first clone / after changing appicon-macos.png
just dev                                # unsigned build for the current platform
just darwin-build-arm64                 # macOS .app → dist/darwin-arm64/
just linux-release                      # zip + .deb + AppImage
just win-zip                            # Windows .exe + config.toml
just release                            # signed macOS + all platform release packages
```

### Output layout

| Path | Description |
| ---- | ----------- |
| `target/release/settings` | Binary for embedding in a parent macOS/Linux app |
| `dist/settings/<arch>/` | Standalone production GUI (`darwin-arm64`, `windows-x86_64`, …) |
| `dist/settings-schema-checker/<arch>/` | `settings-schema-checker` CLI for schema authors |
| `demo/dist/<arch>/` | Standalone demo GUI with bundled `config.toml` |

ARCH slugs: `darwin-arm64`, `darwin-x86_64`, `windows-x86_64`, `linux-x86_64`.

### GitHub Releases

Pushing a `v*` tag (or running the **Release** workflow manually) builds and publishes
**checker** and **demo** packages for Linux (`ubuntu-24.04`), macOS (arm64 + x86_64,
signed and notarized), and Windows (unsigned zip). See [`.github/workflows/release.yml`](.github/workflows/release.yml).

Legacy `Makefile` targets remain as deprecated aliases during the migration; prefer `just`.

## Documentation

Full schema reference, widget guide, theming, and localization:

**https://emotiongraphics.jp/docs/ref/settings/**

`multiline` fields use `rows` as the visible height (default 4). Longer values scroll inside the widget; they do not stretch the form.

### `segmented_control` and `type`

`segmented_control` reads and writes `string` values by default (`type` omitted or
`type = "string"`). Set `type = "number"` to store TOML numbers instead; `options`
are numeric strings used as segment labels and stored values. When every option
omits `.`, whole numbers are written as integer literals (e.g. `count = 3`); if any
option contains `.`, floats are used (e.g. `weight = 2.0`).

## Config migration

When a parent app renames, moves, or removes config keys, existing user config
files would otherwise keep the stale layout. Declare the changes in the schema and
Settings migrates the config file on launch, preserving comments and formatting.

```toml
schema_version = 2                        # target version
# migration_version_key = "schema_version"  # optional: config key that records progress

[[migration]]
version = 2
  [[migration.rename]]                    # move a value (same or different table)
  from = "display.font_size"
  to   = "general.font_size"

  [[migration.delete]]                    # remove a key
  key = "display.tick_rate"

  [[migration.transform]]                 # move + convert
  from  = "mode"
  to    = "vivarium.enabled"
  type  = "enum_to_bool"                  # true when `from` equals `match`, else false
  match = "vivarium"
```

Behavior:

- On launch, Settings compares the config file's recorded version (top-level
  `schema_version`, or the key named by `migration_version_key`) against the
  schema's `schema_version`, applies pending `[[migration]]` blocks in ascending
  `version` order, records the new version, and saves.
- A missing recorded version is treated as `0`, so every migration applies.
- Operations are idempotent: a missing source key is a no-op, and a move whose
  destination already exists is skipped (existing values are preserved).
- Emptied parent tables (e.g. `[display]`) are pruned.
- `settings-schema-checker` validates the migration block (duplicate versions,
  versions exceeding `schema_version`, and `enum_to_bool` without `match`).

## License

MIT License
