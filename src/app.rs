use std::collections::HashMap;
use std::sync::mpsc;

use crate::config::{ConfigStore, NumberRepr};
use crate::i18n::t;
use crate::schema::{Field, FieldType, SaveButtonMode, SectionMap, SectionTabStyle, Schema, ThemeMode as SchemaThemeMode, WidgetKind};
use crate::theme::{self, Palette, Variant};
use settings_schema::runtime::ValidationEngine;

/// Pending background file-picker: (config key path, result receiver).
type PendingFilePick = Option<(String, mpsc::Receiver<Option<std::path::PathBuf>>)>;

// ---------------------------------------------------------------------------
// Tab bar layout constants — adjust here to tune appearance.

/// Height of the icon tab bar (px).  Text-only fallback always uses 40 px.
const TAB_BAR_H:      f32 = 64.0;

/// Height of the text-only tab bar (px).
const TAB_BAR_H_TEXT: f32 = 40.0;

/// Height of the bottom action bar (px) when a Save button is shown.
const BOTTOM_BAR_H:   f32 = 40.0;

/// Fixed width of each icon tab button (px).
const TAB_BTN_W:    f32 = 76.0;

/// Icon glyph size (pt).
const TAB_ICON_PX:  f32 = 32.0;

/// Top margin inside a tab button, above the icon (px).
const TAB_ICON_TOP: f32 = 8.0;

/// Gap between icon bottom edge and label top edge (px).
const TAB_GAP:      f32 = 4.0;

/// Label font size (pt).
const TAB_LABEL_PX: f32 = 10.0;

/// Corner radius of the selection / hover highlight (px).
const TAB_ROUNDING: f32 = 8.0;

// ---------------------------------------------------------------------------
// Field / content layout constants

/// Body font size for field labels, input widgets, and option text (pt).
const FIELD_FONT_PX: f32 = 13.0;

/// Hint / comment font size, e.g. "例: 5m, 30s, 1h" (pt).
const HINT_FONT_PX:  f32 = 12.0;

/// Content-area padding inside the central panel (px).
const CONTENT_PAD_T: i8 = 12;
const CONTENT_PAD_R: i8 = 24;
const CONTENT_PAD_B: i8 = 12;
const CONTENT_PAD_L: i8 = 24;

/// Corner radius for input fields and interactive widgets (px).
const FIELD_ROUNDING: u8 = 6;

/// Border stroke width when a field is idle or hovered (px).
const FIELD_BORDER_W_IDLE: f32 = 1.0;

/// Gap between the field's outer edge and the inner edge of the focus ring (px).
const FOCUS_RING_GAP:      f32 = 2.0;

/// Stroke width of the focus ring (px).  The ring occupies [gap, gap+width] outside the field.
const FOCUS_RING_W:        f32 = 4.0;

/// Corner radius of the focus ring (px).  Intentionally smaller than FIELD_ROUNDING
/// so the ring appears visually distinct from (not concentric with) the field border.
const FOCUS_RING_ROUNDING: u8  = 3;

/// Horizontal inset applied to the widget column so the focus ring fits inside the grid
/// cell on both sides (px).  Must be ≥ FOCUS_RING_GAP + FOCUS_RING_W.
const FOCUS_RING_PAD: f32 = FOCUS_RING_GAP + FOCUS_RING_W;

/// Space between the underline tab bar and the content area below it.
const SUBTAB_CONTENT_PAD: f32 = 12.0;

/// Vertical space added above and below each separator line (px).
const SEPARATOR_PAD: f32 = 6.0;

/// Top inset of egui's default single-line `TextEdit` text, in px.
/// egui lays the text out `Margin::symmetric(4, 2)` inside the frame, so the
/// first text line starts this far below the widget's top edge.  We mirror the
/// value here to compute where a field's value text sits (see
/// [`label_baseline_offset`]).  The widgets do not override this margin.
const TEXTEDIT_MARGIN_TOP: f32 = 2.0;
/// Total vertical margin (top + bottom) of egui's default `TextEdit`.
const TEXTEDIT_MARGIN_Y: f32 = 4.0;

// ---------------------------------------------------------------------------
// App state

pub struct SettingsApp {
    schema: Schema,
    config: ConfigStore,
    selected_tab: usize,
    /// Previous frame's selected tab — used to detect tab switches and trigger window resize.
    prev_tab: usize,
    /// Per-tab index of the selected sub-section (for `section_map` tabs).
    selected_sub: HashMap<usize, usize>,
    /// Tracks which `secret_input` fields are shown in plain text (key_path -> bool).
    show_secrets: HashMap<String, bool>,
    add_dialog: Option<AddDialog>,
    delete_confirm: Option<DeleteConfirm>,
    /// Resolved codepoints for Material Symbols (empty when icons not embedded).
    icon_map: HashMap<String, char>,
    /// Light-variant palette (built from the schema at startup).
    light: theme::Palette,
    /// Dark-variant palette (built from the schema at startup).
    dark: theme::Palette,
    /// Theme preference resolved against the OS each frame.
    theme_mode: SchemaThemeMode,
    /// Currently active palette (light or dark), refreshed every frame.
    palette: theme::Palette,
    /// Last variant applied to the native window decorations (title bar). Used
    /// to push a `SetTheme` command only when the resolved variant changes, so
    /// the OS-drawn title bar matches the schema-defined content theme.
    applied_window_variant: Option<Variant>,
    /// Whether a Save button is shown or edits are written automatically.
    save_button_mode: SaveButtonMode,
    /// Pending file-picker result: (config key path, receiver).
    /// Spawned in a background thread to avoid blocking the render loop on Linux.
    pending_file_pick: PendingFilePick,
    /// File-system watcher for the config file; `None` when initialization failed.
    /// Held for its RAII side effect (watching stops when dropped).
    _watcher: Option<notify::RecommendedWatcher>,
    /// Receives raw file-system events from the watcher background thread.
    watch_rx: Option<std::sync::mpsc::Receiver<notify::Result<notify::Event>>>,
    /// Time of the most recent save; used to suppress the watcher event that
    /// follows our own atomic-rename write (debounce window: 2 s).
    last_save: Option<std::time::Instant>,
    /// Set when an external file change is detected while unsaved edits exist.
    file_conflict: bool,
    /// Compiled CEL programs for field validation and option_states.
    validation: ValidationEngine,
}

struct AddDialog {
    key_prefix: String,
    input: String,
    error: Option<String>,
}

struct DeleteConfirm {
    key_prefix: String,
    section_key: String,
}

// ---------------------------------------------------------------------------

impl SettingsApp {
    pub fn new(cc: &eframe::CreationContext, schema: Schema, config: ConfigStore) -> Self {
        let light = Palette::from_schema(&schema, Variant::Light);
        let dark = Palette::from_schema(&schema, Variant::Dark);
        let theme_mode = schema.theme;
        // Initial palette: resolve against the OS theme known at creation time.
        let variant = theme::resolve_variant(theme_mode, cc.egui_ctx.system_theme());
        let palette = match variant {
            Variant::Light => light,
            Variant::Dark => dark,
        };
        setup_fonts(&cc.egui_ctx);
        setup_style(&cc.egui_ctx);
        setup_visuals(&cc.egui_ctx, &palette);
        #[cfg(has_icons)]
        let icon_map = parse_codepoints(crate::schema::ICON_CODEPOINTS);
        #[cfg(not(has_icons))]
        let icon_map = HashMap::new();
        let save_button_mode = schema.save_button;
        let validation = ValidationEngine::compile(&schema)
            .expect("schema CEL expressions must compile (checked at build time)");
        let config_path = config.path().to_path_buf();
        let (watch_tx, watch_rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
        let watcher = {
            use notify::Watcher;
            let mut w = notify::recommended_watcher(move |event| {
                let _ = watch_tx.send(event);
            })
            .ok();
            if let Some(w) = w.as_mut() {
                let _ = w.watch(&config_path, notify::RecursiveMode::NonRecursive);
            }
            w
        };
        Self {
            schema,
            config,
            selected_tab: 0,
            prev_tab: usize::MAX,
            selected_sub: HashMap::new(),
            show_secrets: HashMap::new(),
            add_dialog: None,
            delete_confirm: None,
            icon_map,
            light,
            dark,
            theme_mode,
            palette,
            applied_window_variant: None,
            save_button_mode,
            pending_file_pick: None,
            _watcher: watcher,
            watch_rx: Some(watch_rx),
            last_save: None,
            file_conflict: false,
            validation,
        }
    }
}

// ---------------------------------------------------------------------------
// Window size helper (called from main.rs before the window is created)

/// Computes the logical `inner_size` (client area, excluding OS title bar) from
/// the schema's `content_width` / `content_height` fields.
///
/// The inner height is: `tab_bar_h + content_height + bottom_bar_h`.
///
/// - `tab_bar_h`   — 0 when there are no tabs, 64 px with icon tabs, 40 px otherwise.
/// - `bottom_bar_h`— 0 when auto-save is active, 40 px when a Save button is shown.
pub fn compute_window_size(schema: &crate::schema::Schema) -> [f32; 2] {
    let cw = schema.content_width.unwrap_or(700.0);
    let ch = schema.content_height.unwrap_or(370.0);

    // Tab bar height.
    let tab_h = if schema.tabs.len() <= 1 {
        0.0
    } else {
        #[cfg(has_icons)]
        let has_icons = schema.tabs.iter().any(|t| t.icon.is_some());
        #[cfg(not(has_icons))]
        let has_icons = false;
        if has_icons { TAB_BAR_H } else { TAB_BAR_H_TEXT }
    };

    // Bottom bar height.
    let bottom_h = match schema.save_button {
        crate::schema::SaveButtonMode::Show => BOTTOM_BAR_H,
        crate::schema::SaveButtonMode::Hide => 0.0,
        crate::schema::SaveButtonMode::Os =>
            if cfg!(target_os = "macos") { 0.0 } else { BOTTOM_BAR_H },
    };

    [cw, tab_h + ch + bottom_h]
}

// ---------------------------------------------------------------------------

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Resolve the active palette against the OS theme (it can change while
        // the app is running) and re-apply the theme every frame so eframe's
        // system-theme following doesn't override the schema-defined colors or
        // text-style sizes.
        let variant = theme::resolve_variant(self.theme_mode, ctx.system_theme());
        self.palette = match variant {
            Variant::Light => self.light,
            Variant::Dark => self.dark,
        };
        theme::set_current(self.palette);
        setup_visuals(ctx, &self.palette);
        setup_style(ctx);

        // Match the native window decorations (title bar) to the active variant.
        // Without this, the OS draws the title bar in its own light/dark style,
        // which clashes with a schema that forces `theme = "dark"` under a light
        // OS (or vice versa). Only sent on change to avoid redundant commands.
        if self.applied_window_variant != Some(variant) {
            ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(match variant {
                Variant::Light => egui::SystemTheme::Light,
                Variant::Dark => egui::SystemTheme::Dark,
            }));
            self.applied_window_variant = Some(variant);
        }

        // Poll for a completed background file-picker.
        if let Some((key_path, rx)) = &self.pending_file_pick {
            if let Ok(result) = rx.try_recv() {
                if let Some(p) = result {
                    self.config.set_str(key_path, &p.to_string_lossy());
                }
                self.pending_file_pick = None;
                ctx.request_repaint();
            } else {
                // Still waiting — keep repainting so we poll every frame.
                ctx.request_repaint();
            }
        }

        // Poll for external config-file changes from the watcher background thread.
        if let Some(rx) = &self.watch_rx {
            let debounce = std::time::Duration::from_millis(2000);
            let is_own_save = self.last_save
                .map(|t| t.elapsed() < debounce)
                .unwrap_or(false);
            let mut has_external_event = false;
            while rx.try_recv().is_ok() {
                if !is_own_save {
                    has_external_event = true;
                }
            }
            if has_external_event {
                if self.config.is_dirty() {
                    self.file_conflict = true;
                } else if let Err(e) = self.config.reload() {
                    eprintln!("Reload error: {e}");
                }
            }
        }

        // Dialogs (rendered as floating windows before panels).
        handle_add_dialog(ctx, &mut self.add_dialog, &mut self.config, self.palette.accent);
        handle_delete_confirm(
            ctx,
            &mut self.delete_confirm,
            &mut self.config,
            &mut self.selected_sub,
            self.palette.accent,
        );
        handle_file_conflict(ctx, &mut self.file_conflict, &mut self.config, self.palette.accent);

        // Top: main tab bar.
        // When icon assets were embedded at build time and at least one tab
        // defines an `icon`, use a taller bar with icon-above-label buttons.
        let has_icon_tabs = !self.icon_map.is_empty()
            && self.schema.tabs.iter().any(|t| t.icon.is_some());
        let tab_bar_h = if self.schema.tabs.len() <= 1 {
            0.0
        } else if has_icon_tabs {
            TAB_BAR_H
        } else {
            TAB_BAR_H_TEXT
        };

        // Resolve per-tab content dimensions and resize window when the tab changes.
        {
            let tab = &self.schema.tabs[self.selected_tab];
            let content_w = tab.content_width.or(self.schema.content_width).unwrap_or(700.0);
            let content_h = tab.content_height.or(self.schema.content_height).unwrap_or(370.0);
            if self.prev_tab != self.selected_tab {
                self.prev_tab = self.selected_tab;
                let bottom_h = match self.save_button_mode {
                    SaveButtonMode::Show => BOTTOM_BAR_H,
                    SaveButtonMode::Hide => 0.0,
                    SaveButtonMode::Os   => if cfg!(target_os = "macos") { 0.0 } else { BOTTOM_BAR_H },
                };
                let window_h = tab_bar_h + content_h + bottom_h;
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(
                    egui::Vec2::new(content_w, window_h),
                ));
            }
        }

        // Top: main tab bar — hidden when there is only one tab.
        if self.schema.tabs.len() > 1 {
        // Remove the panel separator stroke and inner margins so the full
        // TAB_BAR_H is available to the button rects (default frame has 8 px
        // top/bottom margins which would clip the label at the bottom).
        let mut tab_bar_frame = egui::Frame::side_top_panel(&ctx.style());
        tab_bar_frame.stroke = egui::Stroke::NONE;
        tab_bar_frame.inner_margin = egui::Margin::ZERO;
        egui::TopBottomPanel::top("tab_bar")
            .frame(tab_bar_frame)
            .exact_height(tab_bar_h)
            .show(ctx, |ui| {
            if has_icon_tabs {
                // Center the tab group; each button has a fixed width.
                let n = self.schema.tabs.len() as f32;
                let spacing = ui.spacing().item_spacing.x;
                let total_w = n * TAB_BTN_W + (n - 1.0) * spacing;
                let lead = ((ui.available_width() - total_w) / 2.0).max(0.0);
                // Capture panel height now — ui.available_height() inside
                // ui.horizontal() returns only the current row height, not the
                // full panel height, which would make the selection rect tiny.
                let btn_h = ui.available_height();
                ui.horizontal(|ui| {
                    ui.add_space(lead);
                    for (i, tab) in self.schema.tabs.iter().enumerate() {
                        let selected = self.selected_tab == i;
                        let icon_char = tab
                            .icon
                            .as_ref()
                            .and_then(|name| self.icon_map.get(name))
                            .copied();
                        let (rect, resp) = ui.allocate_exact_size(
                            egui::Vec2::new(TAB_BTN_W, btn_h),
                            egui::Sense::click(),
                        );
                        if resp.clicked() {
                            self.selected_tab = i;
                        }
                        // Selection / hover background.
                        let vis = ui.visuals();
                        if selected {
                            ui.painter().rect_filled(
                                rect.shrink2(egui::Vec2::new(4.0, 6.0)),
                                TAB_ROUNDING,
                                vis.selection.bg_fill,
                            );
                        } else if resp.hovered() {
                            ui.painter().rect_filled(
                                rect.shrink2(egui::Vec2::new(4.0, 6.0)),
                                TAB_ROUNDING,
                                vis.widgets.hovered.bg_fill,
                            );
                        }
                        let color =
                            if selected { self.palette.accent } else { self.palette.tab_text };
                        if let Some(c) = icon_char {
                            // Icon: absolute pixel offset from button top.
                            let icon_cy = TAB_ICON_TOP + TAB_ICON_PX * 0.5;
                            ui.painter().text(
                                rect.left_top()
                                    + egui::Vec2::new(TAB_BTN_W * 0.5, icon_cy),
                                egui::Align2::CENTER_CENTER,
                                c.to_string(),
                                egui::FontId::new(
                                    TAB_ICON_PX,
                                    egui::FontFamily::Name("icons".into()),
                                ),
                                color,
                            );
                            // Label: directly below the icon — no overlap.
                            let label_cy =
                                TAB_ICON_TOP + TAB_ICON_PX + TAB_GAP + TAB_LABEL_PX * 0.5;
                            ui.painter().text(
                                rect.left_top()
                                    + egui::Vec2::new(TAB_BTN_W * 0.5, label_cy),
                                egui::Align2::CENTER_CENTER,
                                tab.label.get(),
                                egui::FontId::new(
                                    TAB_LABEL_PX,
                                    egui::FontFamily::Proportional,
                                ),
                                color,
                            );
                        } else {
                            // Fallback: label only (icon name not in codepoints map).
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                tab.label.get(),
                                egui::FontId::proportional(13.0),
                                color,
                            );
                        }
                    }
                });
            } else {
                // Text-only tabs (icon assets not embedded or no icons defined).
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    for (i, tab) in self.schema.tabs.iter().enumerate() {
                        if ui
                            .selectable_label(self.selected_tab == i, tab.label.get())
                            .clicked()
                        {
                            self.selected_tab = i;
                        }
                    }
                });
                ui.add_space(4.0);
            }
        });
        } // end single-tab hide

        // Bottom: action bar — shown when explicit Save buttons are requested.
        // Presents the standard Windows-style trio: OK / Cancel / Apply.
        // Changes are held in memory and only written to disk on OK or Apply;
        // Cancel (and closing the window) discards them. Apply is disabled until
        // a change has been made; OK can always be pressed.
        let show_save_btn = match self.save_button_mode {
            SaveButtonMode::Show => true,
            SaveButtonMode::Hide => false,
            SaveButtonMode::Os  => !cfg!(target_os = "macos"),
        };
        if show_save_btn {
            let accent = self.palette.accent;
            let dirty = self.config.is_dirty();
            let valid = self.validation.is_valid(&self.schema, &self.config);
            egui::TopBottomPanel::bottom("bottom_bar")
                .exact_height(BOTTOM_BAR_H)
                .show(ctx, |ui| {
                ui.add_space(4.0);
                // Buttons are right-aligned. Add them right-to-left so the visual
                // left-to-right order matches the Windows convention: OK, Cancel,
                // Apply (Apply rightmost).
                ui.horizontal(|ui| {
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            // Apply (rightmost): persist changes, stay open.
                            // Disabled while there is nothing to apply.
                            let apply = ui.add_enabled(
                                dirty && valid,
                                action_button(t().apply, false, accent),
                            );
                            if apply.clicked() {
                                if let Err(e) = self.config.save() {
                                    eprintln!("Save error: {e}");
                                }
                                self.last_save = Some(std::time::Instant::now());
                            }
                            // Cancel (middle): discard in-memory changes by closing.
                            if ui.add(action_button(t().cancel, false, accent)).clicked() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            // OK (leftmost, primary): persist if needed, then close.
                            let ok = ui.add_enabled(
                                valid,
                                action_button(t().ok, true, accent),
                            );
                            if ok.clicked() {
                                if dirty {
                                    if let Err(e) = self.config.save() {
                                        eprintln!("Save error: {e}");
                                    }
                                    self.last_save = Some(std::time::Instant::now());
                                }
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        },
                    );
                });
                ui.add_space(4.0);
            });
        }

        // Auto-save: if the mode does not use an explicit Save button, write
        // changes to disk whenever the in-memory state was mutated this frame.
        let auto_save = match self.save_button_mode {
            SaveButtonMode::Hide => true,
            SaveButtonMode::Show => false,
            SaveButtonMode::Os  => cfg!(target_os = "macos"),
        };
        if auto_save
            && self.config.is_dirty()
            && self.validation.is_valid(&self.schema, &self.config)
        {
            if let Err(e) = self.config.save() {
                eprintln!("Auto-save error: {e}");
            }
            self.last_save = Some(std::time::Instant::now());
        }

        // Center: tab content.
        // Split borrows: schema (immutable) vs. mutable state fields is safe in
        // Rust 2021 because the closure captures disjoint struct fields.
        egui::CentralPanel::default().show(ctx, |ui| {
            let mut content_frame = egui::Frame::default();
            content_frame.inner_margin = egui::Margin {
                left:   CONTENT_PAD_L,
                right:  CONTENT_PAD_R,
                top:    CONTENT_PAD_T,
                bottom: CONTENT_PAD_B,
            };
            content_frame.show(ui, |ui| {
                let tab_idx = self.selected_tab;
                let accent = self.palette.accent;
                let tab = &self.schema.tabs[tab_idx];
                let max_height = tab.content_height
                    .or(self.schema.content_height)
                    .unwrap_or(440.0);
                if let Some(fields) = self.schema.tabs[tab_idx].fields.as_deref() {
                    show_flat_fields(
                        ui,
                        fields,
                        tab_idx,
                        max_height,
                        &self.schema,
                        &mut self.validation,
                        &mut self.config,
                        &mut self.show_secrets,
                        &mut self.pending_file_pick,
                        accent,
                    );
                } else if let Some(sm) = self.schema.tabs[tab_idx].section_map.as_ref() {
                    show_section_map(
                        ui,
                        tab_idx,
                        max_height,
                        sm,
                        &self.schema,
                        &mut self.validation,
                        &mut self.config,
                        &mut self.show_secrets,
                        &mut self.selected_sub,
                        &mut self.add_dialog,
                        &mut self.delete_confirm,
                        &mut self.pending_file_pick,
                        accent,
                    );
                }
            });
        });
    }
}

// ---------------------------------------------------------------------------
// Flat-fields renderer

fn show_flat_fields(
    ui: &mut egui::Ui,
    fields: &[Field],
    tab_id: usize,
    max_height: f32,
    schema: &Schema,
    validation: &mut ValidationEngine,
    config: &mut ConfigStore,
    show_secrets: &mut HashMap<String, bool>,
    pending: &mut PendingFilePick,
    accent: egui::Color32,
) {
    egui::ScrollArea::vertical()
        .id_salt(("flat_fields", tab_id))
        .max_height(max_height)
        .show(ui, |ui| {
        ui.add_space(FOCUS_RING_PAD);
        egui::Grid::new(("flat_fields", tab_id))
            .num_columns(2)
            .min_col_width(120.0)
            .spacing([8.0, 8.0])
            .show(ui, |ui| {
                for field in fields {
                    if field.widget == WidgetKind::Separator {
                        render_separator_row(ui);
                        continue;
                    }
                    let key_path = field.key.as_str();
                    let errors = validation.field_errors(schema, field, key_path, config);
                    render_field(
                        ui,
                        field,
                        key_path,
                        schema,
                        validation,
                        &errors,
                        config,
                        show_secrets,
                        pending,
                        accent,
                    );
                    ui.end_row();
                    render_field_feedback(ui, &errors, field_hint(field, key_path, config));
                }
            });
    });
}

// ---------------------------------------------------------------------------
// Section-map renderer

#[allow(clippy::too_many_arguments)]
fn show_section_map(
    ui: &mut egui::Ui,
    tab_idx: usize,
    max_height: f32,
    sm: &SectionMap,
    schema: &Schema,
    validation: &mut ValidationEngine,
    config: &mut ConfigStore,
    show_secrets: &mut HashMap<String, bool>,
    selected_sub: &mut HashMap<usize, usize>,
    add_dialog: &mut Option<AddDialog>,
    delete_confirm: &mut Option<DeleteConfirm>,
    pending: &mut PendingFilePick,
    accent: egui::Color32,
) {
    let sections = config.section_keys(&sm.key_prefix);

    let raw = *selected_sub.entry(tab_idx).or_insert(0);
    let sub_idx = if !sections.is_empty() && raw >= sections.len() {
        selected_sub.insert(tab_idx, 0);
        0
    } else {
        raw
    };

    let mut new_sub: Option<usize> = None;
    let mut request_delete: Option<String> = None;

    // Sub-section tab bar — style is chosen by sm.tab_style.
    match sm.tab_style {
        SectionTabStyle::Segmented => {
            show_subtab_segmented(ui, &sections, sub_idx, &mut new_sub, sm.max_width);
        }
        SectionTabStyle::Underline => {
            show_subtab_underline(
                ui, &sections, sub_idx, sm.allow_add_remove,
                &mut new_sub, &mut request_delete, add_dialog, &sm.key_prefix,
                accent,
            );
        }
    }

    if let Some(i) = new_sub {
        selected_sub.insert(tab_idx, i);
    }
    if let Some(key) = request_delete {
        *delete_confirm = Some(DeleteConfirm {
            key_prefix: sm.key_prefix.clone(),
            section_key: key,
        });
    }

    if sections.is_empty() {
        ui.label(t().no_sections);
        return;
    }

    let section_key = &sections[sub_idx];
    let section_path = format!("{}.{}", sm.key_prefix, section_key);

    egui::ScrollArea::vertical()
        .id_salt(("section_fields", tab_idx))
        .max_height(max_height)
        .show(ui, |ui| {
        ui.add_space(SUBTAB_CONTENT_PAD);

        egui::Grid::new(("section_fields", tab_idx, sub_idx))
                .num_columns(2)
                .min_col_width(120.0)
                .spacing([8.0, 8.0])
                .show(ui, |ui| {
                    for field in &sm.fields {
                        if field.widget == WidgetKind::Separator {
                            render_separator_row(ui);
                            continue;
                        }
                        let key_path = format!("{section_path}.{}", field.key);
                        let errors = validation.field_errors(schema, field, &key_path, config);
                        render_field(
                            ui,
                            field,
                            &key_path,
                            schema,
                            validation,
                            &errors,
                            config,
                            show_secrets,
                            pending,
                            accent,
                        );
                        ui.end_row();
                        render_field_feedback(ui, &errors, field_hint(field, &key_path, config));
                    }
                });
    });
}

// ---------------------------------------------------------------------------
// Sub-tab bar helpers

/// Underline style: accent thick underline on active, thin grey on others.
/// Also handles add/delete buttons when `allow_add_remove` is true.
#[allow(clippy::too_many_arguments)]
fn show_subtab_underline(
    ui: &mut egui::Ui,
    sections: &[String],
    sub_idx: usize,
    allow_add_remove: bool,
    new_sub: &mut Option<usize>,
    request_delete: &mut Option<String>,
    add_dialog: &mut Option<AddDialog>,
    key_prefix: &str,
    accent: egui::Color32,
) {
    const TAB_H:          f32 = 32.0;
    const UNDERLINE_W:    f32 = 3.0;
    const TAB_H_PAD:      f32 = 8.0;
    let inactive_text = theme::current().muted_text;
    let baseline_col  = theme::current().divider;
    // Extra tab width reserved for the hover-reveal × icon.
    const CLOSE_W:        f32 = 16.0;
    // Hit-test radius around the × center.
    const CLOSE_R:        f32 =  5.5;

    // Capture the full available width before the horizontal layout narrows it.
    let full_w = ui.available_width();

    // Collect the active tab's rect so we can paint the accent line AFTER the
    // full-width baseline (later painter calls sit on top).
    let mut active_rect: Option<egui::Rect> = None;

    ui.horizontal(|ui| {
        for (i, key) in sections.iter().enumerate() {
            let is_active = sub_idx == i;
            let text_color = if is_active { accent } else { inactive_text };

            let galley = ui.painter().layout_no_wrap(
                key.clone(),
                egui::FontId::proportional(FIELD_FONT_PX),
                text_color,
            );
            // When allow_add_remove, reserve CLOSE_W on the left (macOS) or right (others).
            let close_extra = if allow_add_remove { CLOSE_W } else { 0.0 };
            let tab_w = galley.size().x + TAB_H_PAD * 2.0 + close_extra;

            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(tab_w, TAB_H),
                egui::Sense::click(),
            );
            let tab_hovered = resp.hovered();

            // × icon position: left edge on macOS, right edge elsewhere.
            #[cfg(target_os = "macos")]
            let close_center = egui::pos2(
                rect.min.x + CLOSE_W * 0.5,
                rect.center().y,
            );
            #[cfg(not(target_os = "macos"))]
            let close_center = egui::pos2(
                rect.max.x - CLOSE_W * 0.5,
                rect.center().y,
            );

            // Route the click: × area → delete, elsewhere → select.
            if resp.clicked() {
                let click_on_close = allow_add_remove
                    && resp.interact_pointer_pos()
                        .map_or(false, |p| p.distance(close_center) <= CLOSE_R);
                if click_on_close {
                    *request_delete = Some(key.clone());
                } else {
                    *new_sub = Some(i);
                }
            }
            if is_active { active_rect = Some(rect); }

            if ui.is_rect_visible(rect) {
                // Text: centered in the label portion, shifted away from the × side.
                #[cfg(target_os = "macos")]
                let label_cx = rect.min.x + close_extra + (tab_w - close_extra) * 0.5;
                #[cfg(not(target_os = "macos"))]
                let label_cx = rect.min.x + (tab_w - close_extra) * 0.5;
                ui.painter().add(egui::epaint::TextShape::new(
                    egui::pos2(
                        label_cx - galley.size().x * 0.5,
                        rect.center().y - galley.size().y * 0.5,
                    ),
                    galley,
                    text_color,
                ));

                // × icon: rendered only when this tab is hovered.
                if allow_add_remove && tab_hovered {
                    let pointer_on_close = ui.input(|inp| {
                        inp.pointer.hover_pos()
                            .map_or(false, |p| p.distance(close_center) <= CLOSE_R)
                    });
                    let close_col = if pointer_on_close {
                        theme::current().icon
                    } else {
                        theme::current().icon_weak
                    };
                    let d = 3.5_f32;
                    let p = ui.painter();
                    p.line_segment(
                        [egui::pos2(close_center.x - d, close_center.y - d),
                         egui::pos2(close_center.x + d, close_center.y + d)],
                        egui::Stroke::new(1.5, close_col),
                    );
                    p.line_segment(
                        [egui::pos2(close_center.x + d, close_center.y - d),
                         egui::pos2(close_center.x - d, close_center.y + d)],
                        egui::Stroke::new(1.5, close_col),
                    );
                }
            }
        }

        if allow_add_remove {
            // Borderless "+" icon — same ghost style as the × close icon.
            const ADD_W: f32 = 28.0;
            const ADD_R: f32 =  5.5;
            let (add_rect, add_resp) = ui.allocate_exact_size(
                egui::vec2(ADD_W, TAB_H),
                egui::Sense::click(),
            );
            if add_resp.clicked() {
                *add_dialog = Some(AddDialog {
                    key_prefix: key_prefix.to_owned(),
                    input: String::new(),
                    error: None,
                });
            }
            if ui.is_rect_visible(add_rect) {
                let add_col = if add_resp.hovered() {
                    theme::current().icon
                } else {
                    theme::current().icon_weak
                };
                let c  = add_rect.center();
                let p  = ui.painter();
                p.line_segment(
                    [egui::pos2(c.x - ADD_R, c.y), egui::pos2(c.x + ADD_R, c.y)],
                    egui::Stroke::new(1.5, add_col),
                );
                p.line_segment(
                    [egui::pos2(c.x, c.y - ADD_R), egui::pos2(c.x, c.y + ADD_R)],
                    egui::Stroke::new(1.5, add_col),
                );
            }
            add_resp.on_hover_text(t().add_section);
        }
    });

    let bar_rect = ui.min_rect();
    let baseline_y = bar_rect.max.y;
    let painter = ui.painter();

    // 1. Full-width light gray baseline — spans the full available width.
    painter.line_segment(
        [egui::pos2(bar_rect.min.x, baseline_y),
         egui::pos2(bar_rect.min.x + full_w, baseline_y)],
        egui::Stroke::new(1.0, baseline_col),
    );

    // 2. Active-tab thick accent line painted on top of the baseline.
    if let Some(rect) = active_rect {
        painter.line_segment(
            [egui::pos2(rect.min.x, baseline_y),
             egui::pos2(rect.max.x, baseline_y)],
            egui::Stroke::new(UNDERLINE_W, accent),
        );
    }
}

/// Segmented-control style: all sub-tabs as a single pill-based control.
/// No add/delete support (intended for fixed, small section counts).
fn show_subtab_segmented(
    ui: &mut egui::Ui,
    sections: &[String],
    sub_idx: usize,
    new_sub: &mut Option<usize>,
    max_width: Option<f32>,
) {
    if sections.is_empty() { return; }

    let n      = sections.len();
    let seg_h  = 28.0_f32;
    let inset  = 2.0_f32;
    let r      = FIELD_ROUNDING;
    let pill_r = r.saturating_sub(inset as u8);

    let avail_w = ui.available_width();
    let ctrl_w  = match max_width {
        Some(max) => max.min(avail_w),
        None      => avail_w,
    };
    let offset_x = ((avail_w - ctrl_w) * 0.5).floor();
    let seg_w    = ctrl_w / n as f32;

    // Allocate full width so the row occupies the correct height,
    // but only draw the (possibly narrower) control centered within it.
    let (outer_rect, _) = ui.allocate_exact_size(
        egui::vec2(avail_w, seg_h),
        egui::Sense::hover(),
    );
    // The actual control rect, centered.
    let ctrl_rect = egui::Rect::from_min_size(
        egui::pos2(outer_rect.min.x + offset_x, outer_rect.min.y),
        egui::vec2(ctrl_w, seg_h),
    );

    // Interaction pass.
    let mut segs: Vec<(egui::Rect, bool, bool)> = Vec::with_capacity(n);
    // Interaction pass — use ctrl_rect origin so click targets align with the painted control.
    for i in 0..n {
        let seg_rect = egui::Rect::from_min_size(
            egui::pos2(ctrl_rect.min.x + i as f32 * seg_w, ctrl_rect.min.y),
            egui::vec2(seg_w, seg_h),
        );
        let resp = ui.interact(
            seg_rect,
            egui::Id::new("subtab_seg").with(ctrl_rect.min.x as i32).with(i),
            egui::Sense::click(),
        );
        if resp.clicked() { *new_sub = Some(i); }
        segs.push((seg_rect, i == sub_idx, resp.hovered()));
    }

    // Paint pass.
    if ui.is_rect_visible(ctrl_rect) {
        let pal = theme::current();
        let track_fill = pal.control_track;
        let sel_fill   = pal.surface;
        let sel_border = pal.control_track_border;
        let shadow     = pal.shadow;

        let painter  = ui.painter();
        let track_cr = egui::CornerRadius::same(r);
        let pill_cr  = egui::CornerRadius::same(pill_r);

        painter.rect_filled(ctrl_rect, track_cr, track_fill);

        for (seg_rect, is_sel, _) in &segs {
            if *is_sel {
                let pill = seg_rect.shrink(inset);
                painter.rect_filled(pill.translate(egui::vec2(0.0, 1.0)), pill_cr, shadow);
                painter.rect_filled(pill, pill_cr, sel_fill);
                painter.rect_stroke(pill, pill_cr,
                    egui::Stroke::new(0.5, sel_border), egui::StrokeKind::Middle);
            }
        }

        for (i, (seg_rect, _, _)) in segs.iter().enumerate() {
            painter.text(
                seg_rect.center(),
                egui::Align2::CENTER_CENTER,
                sections[i].as_str(),
                egui::FontId::proportional(FIELD_FONT_PX),
                ui.visuals().text_color(),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Per-field widget renderer

/// Returns the baseline of the first text line, measured from the top of the
/// galley, for `font_id` at the current font configuration.  This reflects any
/// per-font `y_offset` tweak (see [`setup_fonts`]) because it is read from a
/// laid-out galley rather than from nominal metrics.
fn first_line_baseline(ctx: &egui::Context, font_id: egui::FontId) -> f32 {
    ctx.fonts(|f| {
        let galley = f.layout_no_wrap("Mg".to_owned(), font_id.clone(), egui::Color32::WHITE);
        galley
            .rows
            .first()
            .and_then(|row| row.glyphs.first())
            .map(|glyph| glyph.pos.y)
            // No glyph (shouldn't happen for "Mg"); fall back to a typical ascent.
            .unwrap_or_else(|| f.row_height(&font_id) * 0.8)
    })
}

/// Where a widget anchors its value text vertically, used to place the field
/// label's baseline on the matching line.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LabelAnchor {
    /// The value text is vertically centered within the widget (most widgets:
    /// `TextEdit`, `ComboBox`, sliders, segmented control, toggles, …).  The
    /// label is centered on the same row.
    Center,
    /// The widget is a tall stack whose first row carries the meaningful text
    /// (`exclusive_radio`, `multiline`).  The label aligns to that first line
    /// instead of the vertical middle of the whole widget.
    FirstLine,
}

/// Classifies how a widget positions its value text so the label can match it.
fn label_anchor(kind: &WidgetKind) -> LabelAnchor {
    match kind {
        // A vertical list of options; align the label to the first one.
        WidgetKind::ExclusiveRadio => LabelAnchor::FirstLine,
        // A multi-row box; align the label to the first visible text line.
        WidgetKind::Multiline => LabelAnchor::FirstLine,
        // Everything else vertically centers its text within the widget box.
        _ => LabelAnchor::Center,
    }
}

fn render_field(
    ui: &mut egui::Ui,
    field: &Field,
    key_path: &str,
    schema: &Schema,
    validation: &mut ValidationEngine,
    validation_errors: &[String],
    config: &mut ConfigStore,
    show_secrets: &mut HashMap<String, bool>,
    pending: &mut PendingFilePick,
    accent: egui::Color32,
) {
    let invalid = !validation_errors.is_empty();
    // --- Label vertical alignment -----------------------------------------
    //
    // egui's `Grid` vertically centers each cell (`Align2::LEFT_CENTER`), and
    // every value widget vertically centers its own text.  To make the bold
    // label's baseline land on the value text's baseline we:
    //
    //   1. Measure both baselines from laid-out galleys, so per-font `y_offset`
    //      tweaks (CJK faces) and OS-specific font metrics are accounted for.
    //   2. Force the label cell to the *measured* row height (the value widget
    //      from the previous frame, never below a single-line input) and place
    //      the label by an explicit `add_space`.  Filling the row means the cell
    //      is not re-centered, so the label position no longer depends on the
    //      bold label font's own row height — which differs from the body font
    //      and varies by OS, and was the source of the residual drift.
    let body_id = egui::FontId::new(FIELD_FONT_PX, egui::FontFamily::Proportional);
    let label_id = egui::FontId::new(FIELD_FONT_PX, egui::FontFamily::Name("bold".into()));
    let r = ui.ctx().fonts(|f| f.row_height(&body_id));
    let b_body = first_line_baseline(ui.ctx(), body_id);
    let b_label = first_line_baseline(ui.ctx(), label_id);
    let line_input_h = r + TEXTEDIT_MARGIN_Y;

    // Widget height measured on the previous frame (layout is stable, so this
    // is exact after the first frame).  Falls back to a single-line input.
    let height_id = egui::Id::new(("field_widget_h", key_path));
    let measured = ui
        .ctx()
        .data(|d| d.get_temp::<f32>(height_id))
        .unwrap_or(line_input_h);
    let row_h = measured.max(line_input_h);

    // Target baseline of the label, measured from the row's top edge.
    let target = match label_anchor(&field.widget) {
        // Centered text: the value text box (height `r`) is centered in `row_h`.
        LabelAnchor::Center => (row_h - r) * 0.5 + b_body,
        // First line: align to a single-line input's text line at the top.
        LabelAnchor::FirstLine => TEXTEDIT_MARGIN_TOP + b_body,
    };
    let label_space = (target - b_label).max(0.0);

    // Label column: bold, right-aligned, baseline placed at `target`.
    ui.vertical(|ui| {
        ui.set_min_height(row_h);
        ui.add_space(label_space);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            ui.label(egui::RichText::new(field.label.get())
                .font(egui::FontId::new(FIELD_FONT_PX, egui::FontFamily::Name("bold".into()))));
        });
    });

    // Widget column — FOCUS_RING_PAD inset on each side provides room for the focus ring.
    // Pack from the top: Grid cells inherit `left_to_right(Align::Center)` and the
    // Frame would otherwise vertically center its child in leftover panel height,
    // which inflates `min_rect` and floats tall widgets (multiline) down the tab.
    let cell = egui::Frame::default()
        .inner_margin(egui::Margin::symmetric(FOCUS_RING_PAD as i8, 0))
        .show(ui, |ui| {
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                if let Some(sl) = &field.sublabel {
                    ui.horizontal(|ui| {
                        render_widget_inner(
                            ui,
                            field,
                            key_path,
                            schema,
                            validation,
                            invalid,
                            config,
                            show_secrets,
                            pending,
                            accent,
                        );
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(sl.get())
                                .size(FIELD_FONT_PX)
                                .color(ui.visuals().text_color()),
                        );
                    });
                } else {
                    render_widget_inner(
                        ui,
                        field,
                        key_path,
                        schema,
                        validation,
                        invalid,
                        config,
                        show_secrets,
                        pending,
                        accent,
                    );
                }
            });
        });

    // Remember the rendered widget height for the next frame's label placement.
    ui.ctx()
        .data_mut(|d| d.insert_temp(height_id, cell.response.rect.height()));
}

fn render_widget_inner(
    ui: &mut egui::Ui,
    field: &Field,
    key_path: &str,
    schema: &Schema,
    validation: &mut ValidationEngine,
    invalid: bool,
    config: &mut ConfigStore,
    show_secrets: &mut HashMap<String, bool>,
    pending: &mut PendingFilePick,
    accent: egui::Color32,
) {
    match &field.widget {
        WidgetKind::TextInput => {
            let current = config.get_str(key_path).unwrap_or("").to_owned();
            let mut buf = current.clone();
            let w = clamped_width(ui.available_width(), field.min_width, field.max_width);
            let resp = ui.add(egui::TextEdit::singleline(&mut buf).desired_width(w));
            paint_field_border(ui, &resp, accent, invalid);
            retain_focus_after_ime(ui, &resp);
            if resp.changed() {
                config.set_str(key_path, &buf);
            }
        }

        WidgetKind::Hotkey => {
            render_hotkey(ui, field, key_path, config, accent);
        }

        WidgetKind::Slider => {
            render_numeric(ui, field, key_path, config, accent, true);
        }

        WidgetKind::DragValue => {
            render_numeric(ui, field, key_path, config, accent, false);
        }

        // Separators are handled at the loop level (they split the Grid);
        // this branch is a no-op safety fallback.
        WidgetKind::Separator => {}

        WidgetKind::SecretInput => {
            render_secret_input(ui, key_path, config, show_secrets, accent, invalid);
        }

        WidgetKind::Checkbox => {
            render_checkbox(ui, key_path, config, accent);
        }

        WidgetKind::Toggle => {
            render_toggle(ui, key_path, config, accent);
        }

        WidgetKind::Multiline => {
            render_multiline(ui, field, key_path, config, accent, invalid);
        }

        WidgetKind::Select => {
            let current = config.get_str(key_path).unwrap_or("").to_owned();
            let options: Vec<String> = if let Some(opts) = &field.options {
                opts.clone()
            } else if let Some(from) = &field.options_from {
                config.section_keys(from)
            } else {
                vec![]
            };
            let mut selected = current.clone();
            egui::ComboBox::from_id_salt(key_path)
                .selected_text(selected.as_str())
                .show_ui(ui, |ui| {
                    for opt in &options {
                        let enabled =
                            validation.option_enabled(schema, field, key_path, opt, config);
                        ui.add_enabled_ui(enabled, |ui| {
                            ui.selectable_value(&mut selected, opt.clone(), opt.as_str());
                        });
                    }
                });
            if selected != current {
                config.set_str(key_path, &selected);
            }
        }

        WidgetKind::SegmentedControl => {
            render_segmented_control(ui, field, key_path, schema, validation, config, accent);
        }

        WidgetKind::ExclusiveRadio => {
            render_exclusive_radio(ui, field, key_path, config, show_secrets, accent);
        }

        WidgetKind::FilePath => {
            if pending.is_none() {
                *pending = render_file_path(ui, field, key_path, config, accent, invalid);
            } else {
                // Another pick is still in flight — show the text field but disable the button.
                render_file_path(ui, field, key_path, config, accent, invalid);
            }
        }

        WidgetKind::ColorPicker => {
            render_color_picker(ui, key_path, config);
        }

        WidgetKind::KeyValueMap => {
            render_key_value_map(ui, field, key_path, config, accent);
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: multiline

/// Fixed-height multiline editor.  `rows` is the visible height; overflow
/// scrolls inside the widget so the form Grid is not stretched by long values.
fn render_multiline(
    ui: &mut egui::Ui,
    field: &Field,
    key_path: &str,
    config: &mut ConfigStore,
    accent: egui::Color32,
    invalid: bool,
) {
    let current = config.get_str(key_path).unwrap_or("").to_owned();
    let mut buf = current.clone();
    let rows = field.rows.unwrap_or(4);
    let w = clamped_width(ui.available_width(), field.min_width, field.max_width);
    let row_h = ui.text_style_height(&egui::TextStyle::Body);
    let h = row_h * rows as f32 + TEXTEDIT_MARGIN_Y;

    let mut changed = false;
    let mut focused = false;
    let pal = theme::current();
    // Idle fill + border live on this outer frame so they do not scroll away with
    // the inner TextEdit.  The TextEdit itself is frameless for the same reason.
    let viewport = egui::Frame::NONE
        .fill(pal.surface)
        .stroke(egui::Stroke::new(FIELD_BORDER_W_IDLE, pal.field_border))
        .corner_radius(egui::CornerRadius::same(FIELD_ROUNDING))
        .show(ui, |ui| {
            ui.set_min_size(egui::vec2(w, h));
            ui.set_max_size(egui::vec2(w, h));
            egui::ScrollArea::vertical()
                .id_salt(("multiline", key_path))
                .max_height(h)
                .min_scrolled_height(h)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let resp = ui.add(
                        egui::TextEdit::multiline(&mut buf)
                            .frame(false)
                            .desired_rows(rows)
                            .desired_width(ui.available_width()),
                    );
                    changed = resp.changed();
                    focused = resp.has_focus();
                });
        });
    paint_field_border_at(ui, viewport.response.rect, focused, accent, invalid);
    if changed {
        config.set_str(key_path, &buf);
    }
}

// ---------------------------------------------------------------------------
// Helper: file_path

/// Returns `Some((key_path, rx))` when the user clicked the browse button and a
/// background file-picker thread was spawned.  The caller should store this in
/// `SettingsApp::pending_file_pick`.
fn render_file_path(
    ui: &mut egui::Ui,
    field: &Field,
    key_path: &str,
    config: &mut ConfigStore,
    accent: egui::Color32,
    invalid: bool,
) -> Option<(String, mpsc::Receiver<Option<std::path::PathBuf>>)> {
    let current = config.get_str(key_path).unwrap_or("").to_owned();
    let mut buf  = current.clone();

    let mut pending = None;

    // Use right_to_left layout: button is placed first (anchored to the right),
    // then the TextEdit fills the remaining width.  This prevents the row from
    // overflowing when the button label is wider in some locales (e.g. "Browse…"
    // vs "参照…"), which would otherwise corrupt subsequent Grid cell widths.
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let clicked = ui.button(t().browse).clicked();
        let resp = ui.add(
            egui::TextEdit::singleline(&mut buf).desired_width(ui.available_width())
        );
        paint_field_border(ui, &resp, accent, invalid);
        retain_focus_after_ime(ui, &resp);
        if resp.changed() {
            config.set_str(key_path, &buf);
        }

        if clicked {
            let is_dir = field.is_directory;
            // These are only consumed by the filter block below, which is
            // compiled out on macOS (see the comment there). Gate the bindings
            // to the same platforms so they are not flagged as unused.
            #[cfg(not(target_os = "macos"))]
            let filter = field.file_filter.as_ref().map(|s| s.get().to_owned());
            #[cfg(not(target_os = "macos"))]
            let exts   = field.file_extensions.clone();
            #[cfg(not(target_os = "macos"))]
            let all_files = t().all_files;
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let result = if is_dir {
                    rfd::FileDialog::new().pick_folder()
                } else {
                    // On macOS, NSOpenPanel has no filter dropdown and silently grays out
                    // non-matching files with no way to override — skip the filter entirely
                    // so all files remain selectable.
                    // On Linux portal backends, add an "all files" entry so the user
                    // can bypass the filter via the dropdown.
                    #[cfg(target_os = "macos")]
                    let dlg = rfd::FileDialog::new();
                    #[cfg(not(target_os = "macos"))]
                    let mut dlg = rfd::FileDialog::new();
                    #[cfg(not(target_os = "macos"))]
                    if let (Some(f), Some(e)) = (filter.as_deref(), exts.as_deref()) {
                        let exts_ref: Vec<&str> = e.iter().map(String::as_str).collect();
                        dlg = dlg.add_filter(f, &exts_ref);
                        dlg = dlg.add_filter(all_files, &["*"]);
                    }
                    dlg.pick_file()
                };
                let _ = tx.send(result);
            });
            pending = Some((key_path.to_owned(), rx));
        }
    });

    pending
}

// ---------------------------------------------------------------------------
// Helper: color_picker

fn render_color_picker(
    ui: &mut egui::Ui,
    key_path: &str,
    config: &mut ConfigStore,
) {
    let hex = config.get_str(key_path).unwrap_or("#000000").to_owned();
    let mut rgb = hex_to_rgb(&hex).unwrap_or([0, 0, 0]);
    let resp = ui.color_edit_button_srgb(&mut rgb);
    if resp.changed() {
        config.set_str(key_path, &rgb_to_hex(rgb));
    }
}

fn hex_to_rgb(hex: &str) -> Option<[u8; 3]> {
    let s = hex.strip_prefix('#').unwrap_or(hex);
    if s.len() != 6 { return None; }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some([r, g, b])
}

fn rgb_to_hex(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

// ---------------------------------------------------------------------------
// Helper: key_value_map

fn render_key_value_map(
    ui: &mut egui::Ui,
    field: &Field,
    key_path: &str,
    config: &mut ConfigStore,
    accent: egui::Color32,
) {
    let key_label   = field.key_label.as_ref().map(|s| s.get()).unwrap_or("Key");
    let value_label = field.value_label.as_ref().map(|s| s.get()).unwrap_or("Value");

    #[derive(Clone, Default)]
    struct State {
        selected:  Option<String>,
        adding:    bool,
        new_key:   String,
        new_val:   String,
        focus_key: bool,  // request focus on the key field when the row first opens
    }

    let id = egui::Id::new(("kv_map", key_path));
    let mut state: State = ui.data(|d| d.get_temp(id).unwrap_or_default());

    let keys    = config.section_keys(key_path);
    let avail_w = ui.available_width();

    const ROW_H:    f32 = 28.0;
    const HDR_H:    f32 = 28.0;
    const FOOT_H:   f32 = 28.0;  // footer bar that holds the +/− buttons
    const KEY_FRAC: f32 = 0.42;
    const PAD_X:    f32 = 8.0;
    const BTN_W:    f32 = 28.0;

    let key_col_w = avail_w * KEY_FRAC;
    let val_col_w = avail_w - key_col_w;
    let pal       = theme::current();
    let border    = pal.field_border;
    let hdr_bg    = pal.header_bg;
    let sel_bg    = {
        let [r, g, b, _] = accent.to_array();
        egui::Color32::from_rgba_premultiplied(r, g, b, 70)
    };

    let n_data  = keys.len();
    let n_add   = if state.adding { 1 } else { 0 };
    let table_h = HDR_H + (n_data + n_add) as f32 * ROW_H + FOOT_H;

    // ---- Allocate the whole table rect up front ----
    let (table_rect, _) =
        ui.allocate_exact_size(egui::vec2(avail_w, table_h), egui::Sense::hover());

    // Outer border
    if ui.is_rect_visible(table_rect) {
        ui.painter().rect_stroke(
            table_rect,
            egui::CornerRadius::same(FIELD_ROUNDING),
            egui::Stroke::new(1.0, border),
            egui::StrokeKind::Outside,
        );
    }

    // ---- Header ----
    let hdr_rect = egui::Rect::from_min_size(table_rect.min, egui::vec2(avail_w, HDR_H));
    if ui.is_rect_visible(hdr_rect) {
        let top_r = egui::CornerRadius { nw: FIELD_ROUNDING, ne: FIELD_ROUNDING, sw: 0, se: 0 };
        ui.painter().rect_filled(hdr_rect, top_r, hdr_bg);
        ui.painter().hline(
            table_rect.min.x..=table_rect.max.x,
            hdr_rect.max.y,
            egui::Stroke::new(1.0, border),
        );
        ui.painter().vline(
            table_rect.min.x + key_col_w,
            hdr_rect.y_range(),
            egui::Stroke::new(1.0, border),
        );
        let bold = egui::FontId::new(FIELD_FONT_PX, egui::FontFamily::Name("bold".into()));
        let txt = ui.visuals().text_color();
        ui.painter().text(
            egui::pos2(hdr_rect.min.x + PAD_X, hdr_rect.center().y),
            egui::Align2::LEFT_CENTER, key_label, bold.clone(), txt,
        );
        ui.painter().text(
            egui::pos2(hdr_rect.min.x + key_col_w + PAD_X, hdr_rect.center().y),
            egui::Align2::LEFT_CENTER, value_label, bold, txt,
        );
    }

    // ---- Data rows ----
    let mut to_remove: Option<String> = None;
    let mut to_select: Option<Option<String>> = None;

    for (i, k) in keys.iter().enumerate() {
        let is_last = i + 1 == n_data && !state.adding;
        let is_sel  = state.selected.as_deref() == Some(k.as_str());
        let row_min = egui::pos2(table_rect.min.x, table_rect.min.y + HDR_H + i as f32 * ROW_H);
        let row_rect = egui::Rect::from_min_size(row_min, egui::vec2(avail_w, ROW_H));

        if ui.is_rect_visible(row_rect) {
            if is_sel {
                ui.painter().rect_filled(row_rect, egui::CornerRadius::ZERO, sel_bg);
            }
            if !is_last {
                ui.painter().hline(
                    table_rect.min.x..=table_rect.max.x,
                    row_rect.max.y,
                    egui::Stroke::new(1.0, border),
                );
            }
            ui.painter().vline(
                row_min.x + key_col_w,
                row_rect.y_range(),
                egui::Stroke::new(1.0, border),
            );
            ui.painter().text(
                egui::pos2(row_min.x + PAD_X, row_rect.center().y),
                egui::Align2::LEFT_CENTER,
                k.as_str(),
                egui::FontId::proportional(FIELD_FONT_PX),
                ui.visuals().text_color(),
            );
        }

        // Click the key column to select the row.
        // (The value column is covered by the TextEdit which handles its own clicks.)
        let key_col_rect = egui::Rect::from_min_size(row_min, egui::vec2(key_col_w, ROW_H));
        let click_resp = ui.interact(key_col_rect, id.with(("row", k.as_str())), egui::Sense::click());
        if click_resp.clicked() {
            to_select = Some(if is_sel { None } else { Some(k.clone()) });
        }

        // Value TextEdit via put
        let val_path = format!("{}.{}", key_path, k);
        let val = config.get_str(&val_path).unwrap_or("").to_owned();
        let mut val_buf = val.clone();
        let edit_rect = egui::Rect::from_min_size(
            egui::pos2(row_min.x + key_col_w + PAD_X, row_rect.center().y - 10.0),
            egui::vec2(val_col_w - PAD_X * 2.0, 20.0),
        );
        let edit_resp = ui.put(
            edit_rect,
            egui::TextEdit::singleline(&mut val_buf).frame(false),
        );
        retain_focus_after_ime(ui, &edit_resp);
        if edit_resp.changed() {
            config.set_str(&val_path, &val_buf);
        }
        // Focusing the value TextEdit also selects the row.
        if edit_resp.gained_focus() {
            to_select = Some(Some(k.clone()));
        }
    }

    // Apply deferred row selection
    if let Some(sel) = to_select {
        state.selected = sel;
    }

    // ---- "Adding" input row ----
    if state.adding {
        let row_min = egui::pos2(
            table_rect.min.x,
            table_rect.min.y + HDR_H + n_data as f32 * ROW_H,
        );
        let row_rect = egui::Rect::from_min_size(row_min, egui::vec2(avail_w, ROW_H));
        if ui.is_rect_visible(row_rect) {
            // White background — same as data rows.
            ui.painter().rect_filled(row_rect, egui::CornerRadius::ZERO, pal.surface);
            ui.painter().hline(
                table_rect.min.x..=table_rect.max.x,
                row_rect.max.y,
                egui::Stroke::new(1.0, border),
            );
            ui.painter().vline(
                row_min.x + key_col_w,
                row_rect.y_range(),
                egui::Stroke::new(1.0, border),
            );
        }
        let key_edit_id = id.with("new_key_edit");
        let val_edit_id = id.with("new_val_edit");
        let key_rect = egui::Rect::from_min_size(
            egui::pos2(row_min.x + PAD_X, row_rect.center().y - 10.0),
            egui::vec2(key_col_w - PAD_X * 2.0, 20.0),
        );
        let val_rect = egui::Rect::from_min_size(
            egui::pos2(row_min.x + key_col_w + PAD_X, row_rect.center().y - 10.0),
            egui::vec2(val_col_w - PAD_X * 2.0, 20.0),
        );

        // Consume Enter BEFORE TextEdit renders — TextEdit swallows Key::Enter and
        // prevent us from seeing it via ui.input().  Consuming it here lets us detect
        // it reliably while still keeping Escape available after rendering.
        let either_focused =
            ui.memory(|m| m.has_focus(key_edit_id) || m.has_focus(val_edit_id));
        let enter = either_focused
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));

        let key_resp = ui.put(
            key_rect,
            egui::TextEdit::singleline(&mut state.new_key)
                .id(key_edit_id)
                .frame(false)
                .hint_text("key"),
        );
        // Auto-focus the key field when the row first opens.
        if state.focus_key {
            ui.ctx().memory_mut(|m| m.request_focus(key_edit_id));
            state.focus_key = false;
        }
        let val_resp = ui.put(
            val_rect,
            egui::TextEdit::singleline(&mut state.new_val)
                .id(val_edit_id)
                .frame(false)
                .hint_text("value"),
        );
        let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));

        if escape {
            state.new_key.clear();
            state.new_val.clear();
            state.adding = false;
        } else if enter {
            // Enter pressed while one of the fields had focus.
            if !state.new_key.is_empty() {
                config.set_str(
                    &format!("{}.{}", key_path, &state.new_key),
                    &state.new_val,
                );
            }
            state.new_key.clear();
            state.new_val.clear();
            state.adding = false;
        } else if (val_resp.lost_focus() || key_resp.lost_focus())
            && !state.new_key.is_empty()
            // Don't commit if focus merely moved between the two input fields.
            && !(key_resp.lost_focus() && val_resp.has_focus())
        {
            config.set_str(
                &format!("{}.{}", key_path, &state.new_key),
                &state.new_val,
            );
            state.new_key.clear();
            state.new_val.clear();
            state.adding = false;
        }
    }

    // ---- Footer bar with "+"/"-" buttons (drawn inside the table rect) ----
    let foot_y   = table_rect.max.y - FOOT_H;
    let foot_rect = egui::Rect::from_min_max(
        egui::pos2(table_rect.min.x, foot_y),
        table_rect.max,
    );
    let can_remove  = state.selected.is_some() || state.adding;
    // Two square buttons tiled at the left of the footer, separated by a vline.
    let mid_x     = table_rect.min.x + BTN_W;
    let plus_rect = egui::Rect::from_min_max(
        foot_rect.min,
        egui::pos2(mid_x, foot_rect.max.y),
    );
    let minus_rect = egui::Rect::from_min_max(
        egui::pos2(mid_x + 1.0, foot_rect.min.y),
        egui::pos2(mid_x + 1.0 + BTN_W, foot_rect.max.y),
    );
    let plus_resp  = ui.interact(plus_rect,  id.with("btn_add"),    egui::Sense::click());
    let minus_resp = ui.interact(minus_rect, id.with("btn_remove"), egui::Sense::click());

    if ui.is_rect_visible(foot_rect) {
        // Footer separator line
        ui.painter().hline(
            table_rect.min.x..=table_rect.max.x,
            foot_y,
            egui::Stroke::new(1.0, border),
        );
        // Bottom corners rounding for the footer
        let bot_r = egui::CornerRadius { sw: FIELD_ROUNDING, se: FIELD_ROUNDING, nw: 0, ne: 0 };
        ui.painter().rect_filled(foot_rect, bot_r, hdr_bg);
        // Divider between the two buttons
        ui.painter().vline(
            mid_x,
            plus_rect.y_range(),
            egui::Stroke::new(1.0, border),
        );
        let hover_bg  = pal.control_track;
        let lft_r = egui::CornerRadius { sw: FIELD_ROUNDING, nw: 0, ne: 0, se: 0 };
        if plus_resp.hovered()                { ui.painter().rect_filled(plus_rect,  lft_r, hover_bg); }
        if minus_resp.hovered() && can_remove { ui.painter().rect_filled(minus_rect, egui::CornerRadius::ZERO, hover_bg); }
        let btn_font  = egui::FontId::proportional(16.0);
        let dim_color = pal.icon_disabled;
        let txt = ui.visuals().text_color();
        ui.painter().text(plus_rect.center(),  egui::Align2::CENTER_CENTER, "+", btn_font.clone(), txt);
        ui.painter().text(minus_rect.center(), egui::Align2::CENTER_CENTER, "−", btn_font, if can_remove { txt } else { dim_color });
    }

    if plus_resp.clicked() { state.adding = true; state.focus_key = true; }
    if minus_resp.clicked() && can_remove {
        if state.adding {
            state.new_key.clear();
            state.new_val.clear();
            state.adding = false;
        } else {
            to_remove = state.selected.clone();
            state.selected = None;
        }
    }

    // Apply deferred removal
    if let Some(k) = to_remove {
        config.remove(&format!("{}.{}", key_path, k));
    }

    ui.data_mut(|d| d.insert_temp(id, state));
}

// ---------------------------------------------------------------------------
// Helper: checkbox (custom-drawn for accent fill + small corner radius)

fn render_checkbox(
    ui: &mut egui::Ui,
    key_path: &str,
    config: &mut ConfigStore,
    accent: egui::Color32,
) {
    const BOX_SIZE: f32 = 16.0;
    const BOX_R:    u8  = 3;     // small corner radius — looks square, not circular

    let val = config.get_bool(key_path).unwrap_or(false);

    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(BOX_SIZE, BOX_SIZE),
        egui::Sense::click(),
    );

    let new_val = if resp.clicked() { !val } else { val };
    if new_val != val {
        config.set_bool(key_path, new_val);
    }

    if ui.is_rect_visible(rect) {
        let cr      = egui::CornerRadius::same(BOX_R);
        let painter = ui.painter();

        if new_val {
            // ON: accent fill
            painter.rect_filled(rect, cr, accent);
            // Checkmark — two line segments forming a ✓
            let stroke = egui::Stroke::new(1.8, theme::current().on_accent);
            let p0 = rect.min + egui::vec2(BOX_SIZE * 0.18, BOX_SIZE * 0.50);
            let p1 = rect.min + egui::vec2(BOX_SIZE * 0.40, BOX_SIZE * 0.72);
            let p2 = rect.min + egui::vec2(BOX_SIZE * 0.78, BOX_SIZE * 0.28);
            painter.line_segment([p0, p1], stroke);
            painter.line_segment([p1, p2], stroke);
        } else {
            // OFF: surface fill + border
            painter.rect_filled(rect, cr, theme::current().surface);
            painter.rect_stroke(
                rect,
                cr,
                egui::Stroke::new(FIELD_BORDER_W_IDLE, theme::current().field_border),
                egui::StrokeKind::Middle,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: toggle (iOS-style switch)

fn render_toggle(
    ui: &mut egui::Ui,
    key_path: &str,
    config: &mut ConfigStore,
    accent: egui::Color32,
) {
    const TRACK_W:  f32 = 44.0;
    const TRACK_H:  f32 = 24.0;
    const KNOB_D:   f32 = 20.0;
    const INSET:    f32 = 2.0;

    let val = config.get_bool(key_path).unwrap_or(false);

    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(TRACK_W, TRACK_H),
        egui::Sense::click(),
    );

    let new_val = if resp.clicked() { !val } else { val };
    if new_val != val {
        config.set_bool(key_path, new_val);
    }

    if ui.is_rect_visible(rect) {
        let pal = theme::current();
        let track_color = if new_val { accent } else { pal.control_off };
        let track_r     = egui::CornerRadius::same((TRACK_H * 0.5) as u8);
        let knob_r      = egui::CornerRadius::same((KNOB_D * 0.5) as u8);

        let painter = ui.painter();

        // Track
        painter.rect_filled(rect, track_r, track_color);

        // Knob shadow
        let knob_x = if new_val {
            rect.max.x - INSET - KNOB_D
        } else {
            rect.min.x + INSET
        };
        let knob_rect = egui::Rect::from_min_size(
            egui::pos2(knob_x, rect.min.y + INSET),
            egui::vec2(KNOB_D, KNOB_D),
        );
        painter.rect_filled(
            knob_rect.translate(egui::vec2(0.0, 1.0)),
            knob_r,
            pal.shadow_strong,
        );

        // Knob
        painter.rect_filled(knob_rect, knob_r, pal.surface);
    }
}

// ---------------------------------------------------------------------------
// Helper: segmented_control

fn segmented_control_options(field: &Field, config: &ConfigStore) -> Vec<String> {
    if let Some(opts) = &field.options {
        opts.clone()
    } else if field.field_type != FieldType::Number {
        field.options_from
            .as_ref()
            .map(|from| config.section_keys(from))
            .unwrap_or_default()
    } else {
        vec![]
    }
}

fn parse_numeric_segment_options(options: &[String]) -> Vec<(String, f64)> {
    use std::sync::Once;

    let mut parsed = Vec::with_capacity(options.len());
    let mut invalid = Vec::new();
    for opt in options {
        match opt.parse::<f64>() {
            Ok(v) => parsed.push((opt.clone(), v)),
            Err(_) => invalid.push(opt.as_str()),
        }
    }
    if !invalid.is_empty() {
        static WARN: Once = Once::new();
        WARN.call_once(|| {
            eprintln!(
                "segmented_control: skipping invalid numeric options: {invalid:?}"
            );
        });
    }
    parsed
}

fn numeric_segment_matches(current: f64, opt: f64) -> bool {
    (current - opt).abs() < 1e-9
}

enum SegmentedWrite {
    String,
    Number { value: f64, repr: NumberRepr },
}

struct SegmentedSegments {
    labels: Vec<String>,
    selected: Vec<bool>,
    on_select: Vec<SegmentedWrite>,
}

fn render_segmented_control(
    ui: &mut egui::Ui,
    field: &Field,
    key_path: &str,
    schema: &Schema,
    validation: &mut ValidationEngine,
    config: &mut ConfigStore,
    _accent: egui::Color32,
) {
    let options = segmented_control_options(field, config);
    if options.is_empty() {
        return;
    }

    let segments = match field.field_type {
        FieldType::Number => {
            let parsed = parse_numeric_segment_options(&options);
            if parsed.is_empty() {
                return;
            }
            let repr = NumberRepr::from_options(&options);
            let current = config.get_number(key_path).unwrap_or_else(|| {
                field
                    .min
                    .or_else(|| parsed.first().map(|(_, v)| *v))
                    .unwrap_or(0.0)
            });
            SegmentedSegments {
                labels: parsed.iter().map(|(l, _)| l.clone()).collect(),
                selected: parsed
                    .iter()
                    .map(|(_, v)| numeric_segment_matches(current, *v))
                    .collect(),
                on_select: parsed
                    .iter()
                    .map(|(_, v)| SegmentedWrite::Number { value: *v, repr })
                    .collect(),
            }
        }
        _ => {
            let current = config.get_str(key_path).unwrap_or("");
            SegmentedSegments {
                labels: options.clone(),
                selected: options.iter().map(|o| o == current).collect(),
                on_select: options
                    .iter()
                    .map(|_| SegmentedWrite::String)
                    .collect(),
            }
        }
    };

    let n = segments.labels.len();
    let seg_h = 28.0_f32;
    let inset = 2.0_f32;
    let r = FIELD_ROUNDING;
    let pill_r = r.saturating_sub(inset as u8);
    let avail_w = ui.available_width();
    let control_w = clamped_width(avail_w, field.min_width, field.max_width);
    let seg_w = control_w / n as f32;
    let (outer_rect, _) = ui.allocate_exact_size(
        egui::vec2(control_w, seg_h),
        egui::Sense::hover(),
    );

    let enabled: Vec<bool> = segments
        .labels
        .iter()
        .map(|label| validation.option_enabled(schema, field, key_path, label, config))
        .collect();

    let mut changed_index = None;
    paint_segmented_control_track(
        ui,
        outer_rect,
        &segments.labels,
        &segments.selected,
        &enabled,
        key_path,
        seg_w,
        seg_h,
        inset,
        r,
        pill_r,
        &mut changed_index,
    );

    if let Some(i) = changed_index {
        match &segments.on_select[i] {
            SegmentedWrite::String => {
                config.set_str(key_path, &segments.labels[i]);
            }
            SegmentedWrite::Number { value, repr } => {
                config.set_number(key_path, *value, *repr);
            }
        }
    }
}

fn paint_segmented_control_track(
    ui: &mut egui::Ui,
    outer_rect: egui::Rect,
    labels: &[String],
    selected: &[bool],
    enabled: &[bool],
    key_path: &str,
    seg_w: f32,
    seg_h: f32,
    inset: f32,
    r: u8,
    pill_r: u8,
    changed_index: &mut Option<usize>,
) -> Vec<(egui::Rect, bool, bool)> {
    let pal = theme::current();
    let track_fill = pal.control_track;
    let sel_fill = pal.surface;
    let sel_border = pal.control_track_border;
    let shadow = pal.shadow;

    let n = labels.len();
    let mut segs: Vec<(egui::Rect, bool, bool)> = Vec::with_capacity(n);

    for i in 0..n {
        let seg_rect = egui::Rect::from_min_size(
            egui::pos2(outer_rect.min.x + i as f32 * seg_w, outer_rect.min.y),
            egui::vec2(seg_w, seg_h),
        );
        let sense = if enabled[i] {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        let resp = ui.interact(seg_rect, egui::Id::new(key_path).with(i), sense);
        if enabled[i] && resp.clicked() {
            *changed_index = Some(i);
        }
        segs.push((seg_rect, selected[i], resp.hovered()));
    }

    if ui.is_rect_visible(outer_rect) {
        let painter = ui.painter();
        let track_cr = egui::CornerRadius::same(r);
        let pill_cr = egui::CornerRadius::same(pill_r);

        painter.rect_filled(outer_rect, track_cr, track_fill);

        for (seg_rect, is_sel, _) in &segs {
            if *is_sel {
                let pill = seg_rect.shrink(inset);
                painter.rect_filled(pill.translate(egui::vec2(0.0, 1.0)), pill_cr, shadow);
                painter.rect_filled(pill, pill_cr, sel_fill);
                painter.rect_stroke(
                    pill,
                    pill_cr,
                    egui::Stroke::new(0.5, sel_border),
                    egui::StrokeKind::Middle,
                );
            }
        }

        for (i, (seg_rect, _, _)) in segs.iter().enumerate() {
            let color = if enabled[i] {
                ui.visuals().text_color()
            } else {
                pal.icon_disabled
            };
            painter.text(
                seg_rect.center(),
                egui::Align2::CENTER_CENTER,
                labels[i].as_str(),
                egui::FontId::proportional(FIELD_FONT_PX),
                color,
            );
        }
    }

    segs
}

// ---------------------------------------------------------------------------
// Helper: hotkey recorder

fn render_hotkey(
    ui: &mut egui::Ui,
    field: &Field,
    key_path: &str,
    config: &mut ConfigStore,
    accent: egui::Color32,
) {
    const CLEAR_W:     f32 = 24.0;
    const CLEAR_R:     f32 =  4.5;
    const GAP:         f32 =  4.0;
    let placeholder = t().click_to_input;

    let recorded = config.get_str(key_path).unwrap_or("").to_owned();

    let id = ui.make_persistent_id(("hotkey", key_path));
    let is_recording: bool = ui.data(|d| d.get_temp(id).unwrap_or(false));
    let has_value = !recorded.is_empty();

    ui.horizontal(|ui| {
        // Width: reserve room for the clear (×) button when a value is set.
        let clear_room = if has_value { CLEAR_W + GAP } else { 0.0 };
        let avail = (ui.available_width() - clear_room).max(20.0);
        let w = clamped_width(avail, field.min_width, field.max_width);

        // Height: match egui's singleline TextEdit — row height + 2 × button_padding.y,
        // floored at interact_size.y (exactly what TextEdit::allocate_exact_size does).
        let row_h  = ui.text_style_height(&egui::TextStyle::Body);
        let btn_pad = ui.spacing().button_padding.y;
        let h = (row_h + 2.0 * btn_pad).max(ui.spacing().interact_size.y);

        // The display box.
        let desired = egui::vec2(w, h);
        let (rect, resp) = ui.allocate_exact_size(desired, egui::Sense::click());

        // Toggle recording mode on click.
        if resp.clicked() {
            ui.data_mut(|d| d.insert_temp(id, !is_recording));
        }

        // Capture keypress while recording.
        // NOTE: Do NOT call request_focus() here — passing a custom ID with no
        // associated widget rect causes an internal egui panic in 0.31 when it
        // tries to position the IME window.  Key events are read from the raw
        // input queue which is always accessible via ui.input() regardless of
        // focus state.
        let mut new_value:    Option<String> = None;
        let mut exit_recording = false;
        if is_recording {
            ui.input(|inp| {
                for evt in &inp.events {
                    if let egui::Event::Key { key, modifiers, pressed: true, .. } = evt {
                        match key {
                            egui::Key::Escape => {
                                // Cancel — leave recording without saving.
                                exit_recording = true;
                            }
                            egui::Key::Backspace | egui::Key::Delete => {
                                new_value = Some("__clear__".to_owned());
                            }
                            // Ignore bare Tab (used for UI navigation).
                            egui::Key::Tab => {}
                            _ => {
                                new_value = Some(format_hotkey(*key, *modifiers));
                            }
                        }
                        break; // Process only the first matching key per frame.
                    }
                }
            });

            if let Some(ref v) = new_value {
                if v == "__clear__" {
                    config.remove(key_path);
                } else {
                    config.set_str(key_path, v);
                }
            }
            if new_value.is_some() || exit_recording {
                ui.data_mut(|d| d.insert_temp(id, false));
            }
        }

        // Paint the box.
        if ui.is_rect_visible(rect) {
            let border_col = if is_recording { accent } else { theme::current().field_border };
            let border_w   = if is_recording { 1.5 } else { FIELD_BORDER_W_IDLE };
            let bg         = ui.visuals().extreme_bg_color;
            let cr         = egui::CornerRadius::same(FIELD_ROUNDING);
            let painter    = ui.painter();

            painter.rect_filled(rect, cr, bg);
            painter.rect_stroke(rect, cr,
                egui::Stroke::new(border_w, border_col), egui::StrokeKind::Middle);

            let (label, label_col) = if is_recording {
                (t().press_key.to_owned(),
                 theme::current().icon_muted)
            } else if recorded.is_empty() {
                (placeholder.to_owned(), theme::current().placeholder)
            } else {
                (recorded.clone(), ui.visuals().text_color())
            };

            painter.text(
                egui::pos2(rect.min.x + 8.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                label,
                egui::FontId::proportional(FIELD_FONT_PX),
                label_col,
            );
        }

        // Focus ring while recording.
        if is_recording {
            let half_w = FOCUS_RING_W * 0.5;
            ui.painter().rect_stroke(
                rect.expand(FOCUS_RING_GAP + half_w),
                egui::CornerRadius::same(FOCUS_RING_ROUNDING),
                egui::Stroke::new(FOCUS_RING_W, accent),
                egui::StrokeKind::Middle,
            );
        }

        // Clear (×) button — ghost style, visible only when a value is set.
        if has_value {
            ui.add_space(GAP);
            let (clear_rect, clear_resp) = ui.allocate_exact_size(
                egui::vec2(CLEAR_W, desired.y),
                egui::Sense::click(),
            );
            if clear_resp.clicked() {
                config.remove(key_path);
                ui.data_mut(|d| d.insert_temp(id, false));
            }
            if ui.is_rect_visible(clear_rect) {
                let col = if clear_resp.hovered() {
                    theme::current().icon
                } else {
                    theme::current().icon_weak
                };
                let c = clear_rect.center();
                let p = ui.painter();
                p.line_segment(
                    [egui::pos2(c.x - CLEAR_R, c.y - CLEAR_R),
                     egui::pos2(c.x + CLEAR_R, c.y + CLEAR_R)],
                    egui::Stroke::new(1.5, col),
                );
                p.line_segment(
                    [egui::pos2(c.x + CLEAR_R, c.y - CLEAR_R),
                     egui::pos2(c.x - CLEAR_R, c.y + CLEAR_R)],
                    egui::Stroke::new(1.5, col),
                );
            }
            clear_resp.on_hover_text(t().clear);
        }
    });
}

/// Renders a numeric field as either a `Slider` (`use_slider=true`) or a `DragValue`.
/// Reads / writes the value as a TOML float. Falls back to `default` when the key
/// is absent and `default` is set; otherwise starts at `min` (or 0).
fn render_numeric(
    ui: &mut egui::Ui,
    field: &Field,
    key_path: &str,
    config: &mut ConfigStore,
    accent: egui::Color32,
    use_slider: bool,
) {
    let min   = field.min.unwrap_or(0.0);
    let max   = field.max.unwrap_or(100.0);
    let step  = field.step.unwrap_or(1.0);
    let suffix = field.suffix.as_deref().unwrap_or("");

    let mut val: f64 = config.get_number(key_path).unwrap_or(min);

    // Target height: same as singleline TextEdit.
    let row_h  = ui.text_style_height(&egui::TextStyle::Body);
    let btn_y  = ui.spacing().button_padding.y;
    let h = (row_h + 2.0 * btn_y).max(ui.spacing().interact_size.y);

    let pad_x = 8.0; // match text_input left/right padding
    let w = clamped_width(ui.available_width(), field.min_width, field.max_width);

    // Pre-compute slider track width.
    let track_w = if use_slider {
        let font_id   = egui::FontId::proportional(FIELD_FONT_PX);
        let wide_str  = numeric_display_string(max, step, suffix);
        let text_w    = ui.fonts(|f| {
            f.layout_no_wrap(wide_str, font_id, egui::Color32::BLACK)
                .rect.width()
        });
        let val_box_w = (text_w + 2.0 * pad_x)
            .max(ui.spacing().interact_size.x + 2.0 * pad_x);
        let gap = ui.spacing().item_spacing.x;
        (w - gap - val_box_w).max(1.0)
    } else {
        0.0
    };

    let resp = ui.scope(|ui| {
        let bg    = ui.visuals().extreme_bg_color;
        let white = theme::current().surface;
        ui.spacing_mut().button_padding.x          = pad_x;
        ui.visuals_mut().widgets.inactive.weak_bg_fill = bg;
        ui.visuals_mut().widgets.hovered.weak_bg_fill  = bg;
        ui.visuals_mut().widgets.active.weak_bg_fill   = bg;

        if use_slider {
            ui.spacing_mut().slider_width = track_w;
            // trailing fill uses selection.bg_fill for the left track portion.
            ui.visuals_mut().selection.bg_fill = accent;
            // handle: white circle with accent border.
            let border = egui::Stroke::new(2.0, accent);
            ui.visuals_mut().widgets.inactive.bg_fill = white;
            ui.visuals_mut().widgets.inactive.fg_stroke = border;
            ui.visuals_mut().widgets.hovered.bg_fill = white;
            ui.visuals_mut().widgets.hovered.fg_stroke = egui::Stroke::new(2.5, accent);
            ui.visuals_mut().widgets.active.bg_fill = white;
            ui.visuals_mut().widgets.active.fg_stroke = egui::Stroke::new(2.5, accent);

            let decimals = if step >= 1.0 {
                0
            } else {
                ((-step.abs().log10()).ceil() as usize).min(10)
            };
            let slider = egui::Slider::new(&mut val, min..=max)
                .step_by(step)
                .max_decimals(decimals)
                .suffix(suffix)
                .trailing_fill(true);
            ui.add(slider)
        } else {
            let dv = egui::DragValue::new(&mut val)
                .range(min..=max)
                .speed(step)
                .suffix(suffix);
            ui.add_sized([w, h], dv)
        }
    }).inner;

    if !use_slider {
        paint_field_border(ui, &resp, accent, false);
    }

    // Draw the right-side track (handle → right end) as a thin gray line.
    // We set widgets.inactive.bg_fill = WHITE for the handle, which also
    // made the native rail invisible, so we draw the remaining portion manually.
    if use_slider {
        let base_r  = h / 2.5;
        let t       = ((val - min) / (max - min)) as f32;
        let left    = resp.rect.left();
        let handle_x = egui::lerp(
            (left + base_r)..=(left + track_w - base_r),
            t.clamp(0.0, 1.0),
        );
        let track_right = left + track_w;
        let cy = resp.rect.center().y;
        let rail_h  = 4.0_f32; // match egui default rail height
        let rail_r  = egui::CornerRadius::same((rail_h / 2.0) as u8);
        let right_rail = egui::Rect::from_min_max(
            egui::pos2(handle_x + base_r + 1.0, cy - rail_h / 2.0),
            egui::pos2(track_right, cy + rail_h / 2.0),
        );
        let rail_color = theme::current().control_off;
        ui.painter().rect_filled(right_rail, rail_r, rail_color);
    }

    if resp.changed() {
        config.set_number(key_path, val, NumberRepr::from_step(step));
    }
}

/// Formats `val` the way egui's DragValue/Slider would display it.
/// Used to measure the value-box width before rendering.
fn numeric_display_string(val: f64, step: f64, suffix: &str) -> String {
    if step >= 1.0 {
        format!("{}{}", val as i64, suffix)
    } else {
        let prec = ((-step.abs().log10()).ceil() as usize).min(10);
        format!("{:.prec$}{}", val, suffix, prec = prec)
    }
}

/// Formats a key + modifier combo as a human-readable string (e.g. `"cmd+shift+b"`).
fn format_hotkey(key: egui::Key, mods: egui::Modifiers) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if mods.ctrl  { parts.push("ctrl");  }
    if mods.alt   { parts.push("alt");   }
    if mods.shift { parts.push("shift"); }
    if mods.mac_cmd || mods.command { parts.push("cmd"); }
    let key_str = key_name(key);
    parts.push(key_str);
    parts.join("+")
}

fn key_name(key: egui::Key) -> &'static str {
    match key {
        egui::Key::A => "a", egui::Key::B => "b", egui::Key::C => "c",
        egui::Key::D => "d", egui::Key::E => "e", egui::Key::F => "f",
        egui::Key::G => "g", egui::Key::H => "h", egui::Key::I => "i",
        egui::Key::J => "j", egui::Key::K => "k", egui::Key::L => "l",
        egui::Key::M => "m", egui::Key::N => "n", egui::Key::O => "o",
        egui::Key::P => "p", egui::Key::Q => "q", egui::Key::R => "r",
        egui::Key::S => "s", egui::Key::T => "t", egui::Key::U => "u",
        egui::Key::V => "v", egui::Key::W => "w", egui::Key::X => "x",
        egui::Key::Y => "y", egui::Key::Z => "z",
        egui::Key::Num0 => "0", egui::Key::Num1 => "1", egui::Key::Num2 => "2",
        egui::Key::Num3 => "3", egui::Key::Num4 => "4", egui::Key::Num5 => "5",
        egui::Key::Num6 => "6", egui::Key::Num7 => "7", egui::Key::Num8 => "8",
        egui::Key::Num9 => "9",
        egui::Key::F1  => "f1",  egui::Key::F2  => "f2",  egui::Key::F3  => "f3",
        egui::Key::F4  => "f4",  egui::Key::F5  => "f5",  egui::Key::F6  => "f6",
        egui::Key::F7  => "f7",  egui::Key::F8  => "f8",  egui::Key::F9  => "f9",
        egui::Key::F10 => "f10", egui::Key::F11 => "f11", egui::Key::F12 => "f12",
        egui::Key::Space     => "space",
        egui::Key::Enter     => "enter",
        egui::Key::ArrowUp   => "up",   egui::Key::ArrowDown  => "down",
        egui::Key::ArrowLeft => "left", egui::Key::ArrowRight => "right",
        egui::Key::Home  => "home",  egui::Key::End    => "end",
        egui::Key::PageUp => "pageup", egui::Key::PageDown => "pagedown",
        _ => "?",
    }
}

// ---------------------------------------------------------------------------
// Helper: secret_input

fn render_secret_input(
    ui: &mut egui::Ui,
    key_path: &str,
    config: &mut ConfigStore,
    show_secrets: &mut HashMap<String, bool>,
    accent: egui::Color32,
    invalid: bool,
) {
    // Material Symbols codepoints for visibility / visibility_off.
    const ICON_VISIBLE:  char = '\u{e8f4}';
    const ICON_HIDDEN:   char = '\u{e8f5}';
    const BTN_SIZE:      f32  = 28.0;   // square hit area for the eye button
    const ICON_FONT_PX:  f32  = 18.0;
    const GAP:           f32  =  4.0;   // gap between text field and button

    let show = show_secrets.entry(key_path.to_owned()).or_insert(false);
    let current = config.get_str(key_path).unwrap_or("").to_owned();
    let mut buf = current.clone();
    let mut changed = false;

    ui.horizontal(|ui| {
        // Reserve space for the eye button so the text field doesn't overflow.
        let input_w = (ui.available_width() - BTN_SIZE - GAP).max(40.0);
        let resp = ui.add(
            egui::TextEdit::singleline(&mut buf)
                .password(!*show)
                .desired_width(input_w),
        );
        paint_field_border(ui, &resp, accent, invalid);
        retain_focus_after_ime(ui, &resp);
        changed = resp.changed();

        ui.add_space(GAP);

        // Eye icon — flat/borderless, same ghost style as × and +.
        let (btn_rect, btn_resp) = ui.allocate_exact_size(
            egui::vec2(BTN_SIZE, BTN_SIZE),
            egui::Sense::click(),
        );
        if btn_resp.clicked() { *show = !*show; }

        if ui.is_rect_visible(btn_rect) {
            let icon_col = if btn_resp.hovered() {
                theme::current().icon
            } else {
                theme::current().icon_muted
            };
            let icon_char = if *show { ICON_VISIBLE } else { ICON_HIDDEN };
            ui.painter().text(
                btn_rect.center(),
                egui::Align2::CENTER_CENTER,
                icon_char,
                egui::FontId::new(ICON_FONT_PX, egui::FontFamily::Name("icons".into())),
                icon_col,
            );
        }
        let tip = if *show { t().hide } else { t().show };
        btn_resp.on_hover_text(tip);
    });

    if changed {
        config.set_str(key_path, &buf);
    }
}

// ---------------------------------------------------------------------------
// Helper: exclusive_radio

/// Renders a single radio option (circle + label). Returns true when clicked.
fn render_radio_option(
    ui: &mut egui::Ui,
    label: &str,
    selected: bool,
    accent: egui::Color32,
) -> bool {
    const R:        f32 = 8.0;   // outer circle radius
    const DOT_R:    f32 = 3.5;   // inner dot radius when selected
    const DIAMETER: f32 = R * 2.0;

    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        egui::FontId::proportional(FIELD_FONT_PX),
        ui.visuals().text_color(),
    );
    let total_w = DIAMETER + 6.0 + galley.size().x;
    let total_h = DIAMETER.max(galley.size().y);

    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(total_w, total_h),
        egui::Sense::click(),
    );

    if ui.is_rect_visible(rect) {
        let cx = rect.min.x + R;
        let cy = rect.center().y;
        let center = egui::pos2(cx, cy);
        let painter = ui.painter();

        if selected {
            // ON: accent fill + light dot
            painter.circle_filled(center, R, accent);
            painter.circle_filled(center, DOT_R, theme::current().on_accent);
        } else {
            // OFF: surface fill + border
            painter.circle_filled(center, R, theme::current().surface);
            painter.circle_stroke(
                center, R,
                egui::Stroke::new(FIELD_BORDER_W_IDLE, theme::current().field_border),
            );
        }

        // Label to the right of the circle.
        painter.add(egui::epaint::TextShape::new(
            egui::pos2(rect.min.x + DIAMETER + 6.0,
                       cy - galley.size().y * 0.5),
            galley,
            ui.visuals().text_color(),
        ));
    }

    resp.clicked()
}

fn render_exclusive_radio(
    ui: &mut egui::Ui,
    field: &Field,
    key_path: &str,
    config: &mut ConfigStore,
    show_secrets: &mut HashMap<String, bool>,
    accent: egui::Color32,
) {
    let Some(exc) = field.exclusive.as_ref() else {
        return;
    };

    let parent = parent_path(key_path);
    let mode_path = format!("{parent}.{}", exc.mode_key);

    let current_mode = config
        .get_str(&mode_path)
        .unwrap_or(&exc.mode_default)
        .to_owned();
    let mut new_mode = current_mode.clone();

    ui.vertical(|ui| {
        for variant in &exc.variants {
            let is_sel = variant.value == new_mode;
            let clicked = render_radio_option(ui, variant.label.get(), is_sel, accent);
            if clicked { new_mode = variant.value.clone(); }
        }
        if let Some(v) = exc.variants.iter().find(|v| v.value == new_mode) {
            let vpath = format!("{parent}.{}", v.field_key);
            match v.widget {
                WidgetKind::SecretInput => {
                    render_secret_input(ui, &vpath, config, show_secrets, accent, false);
                }
                _ => {
                    let current = config.get_str(&vpath).unwrap_or("").to_owned();
                    let mut buf = current.clone();
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut buf)
                            .desired_width(f32::INFINITY),
                    );
                    paint_field_border(ui, &resp, accent, false);
                    retain_focus_after_ime(ui, &resp);
                    if resp.changed() {
                        config.set_str(&vpath, &buf);
                    }
                }
            }
        }
    });

    // If mode changed: update mode_key and remove the inactive variant's key.
    if new_mode != current_mode {
        for v in &exc.variants {
            if v.value == current_mode {
                config.remove(&format!("{parent}.{}", v.field_key));
            }
        }
        config.set_str(&mode_path, &new_mode);
    }
}

// ---------------------------------------------------------------------------
// Hint resolution

fn field_hint<'a>(field: &'a Field, key_path: &str, config: &ConfigStore) -> Option<&'a str> {
    if let Some(h) = &field.hint {
        return Some(h.get());
    }
    if field.widget == WidgetKind::ExclusiveRadio {
        if let Some(exc) = &field.exclusive {
            let parent = parent_path(key_path);
            let mode_path = format!("{parent}.{}", exc.mode_key);
            let mode = config
                .get_str(&mode_path)
                .unwrap_or(&exc.mode_default);
            return match mode {
                "direct" => field.hint_direct.as_ref().map(|s| s.get()),
                "env" => field.hint_env.as_ref().map(|s| s.get()),
                _ => None,
            };
        }
    }
    None
}

fn parent_path(path: &str) -> &str {
    path.rfind('.').map(|i| &path[..i]).unwrap_or(path)
}

// ---------------------------------------------------------------------------
// Focus border helper
// ---------------------------------------------------------------------------
// Width clamping helper

/// Clamps `avail` to `[min_width, max_width]`, either of which may be omitted.
fn clamped_width(avail: f32, min_width: Option<f32>, max_width: Option<f32>) -> f32 {
    let mut w = avail;
    if let Some(max) = max_width { w = w.min(max); }
    if let Some(min) = min_width { w = w.max(min); }
    w
}

// Separator helper

/// Renders a separator as a Grid row (2 cells) with a full-width horizontal rule.
/// Must be called from inside a 2-column `egui::Grid`, followed by no `ui.end_row()` call
/// (this function calls it internally).
fn render_separator_row(ui: &mut egui::Ui) {
    let sep_h = SEPARATOR_PAD * 2.0 + 1.0;
    // Label column: invisible height reservation only.
    ui.allocate_space(egui::vec2(0.0, sep_h));
    // Widget column: paint a line that spans the full clip rect (both columns).
    let (rect, _resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), sep_h), egui::Sense::hover());
    ui.painter().hline(
        ui.clip_rect().x_range(),
        rect.center().y,
        egui::Stroke::new(1.0, theme::current().separator),
    );
    ui.end_row();
}
//
// Paints the accent-colored border over a TextEdit when it has keyboard focus.
// egui's `widgets.active` state tracks pointer-button-down, not keyboard focus,
// so we must paint the focus ring manually after adding the widget.

/// On Windows, pressing Enter to confirm an IME candidate (かな漢字変換) causes
/// [`egui::TextEdit`] to lose focus.  This happens because egui's
/// `remove_ime_incompatible_events` suppresses `Key::Backspace` and arrow keys
/// during composition but not `Key::Enter`.  The sequence is:
///   1. `ImeEvent::Commit` is processed first (egui sorts IME events first) →
///      `ime_enabled` is set to `false`.
///   2. `Key::Enter` is processed → `surrender_focus` is called on the TextEdit.
///
/// Workaround: if the TextEdit just lost focus AND an `ImeEvent::Commit` occurred
/// in the same frame, re-request focus so the field stays active for further editing.
/// The user must press Enter once more (without active IME) to actually confirm.
///
/// `resp` must be the direct [`egui::Response`] from `ui.add(TextEdit::...)`.
fn retain_focus_after_ime(ui: &egui::Ui, resp: &egui::Response) {
    let _ = (ui, resp); // suppress unused-variable warnings on non-Windows builds
    #[cfg(target_os = "windows")]
    if resp.lost_focus() {
        let ime_committed = ui.input(|i| {
            i.events
                .iter()
                .any(|e| matches!(e, egui::Event::Ime(egui::ImeEvent::Commit(_))))
        });
        if ime_committed {
            ui.memory_mut(|mem| mem.request_focus(resp.id));
        }
    }
}

fn paint_field_border(
    ui: &egui::Ui,
    resp: &egui::Response,
    accent: egui::Color32,
    invalid: bool,
) {
    paint_field_border_at(ui, resp.rect, resp.has_focus(), accent, invalid);
}

fn paint_field_border_at(
    ui: &egui::Ui,
    rect: egui::Rect,
    has_focus: bool,
    accent: egui::Color32,
    invalid: bool,
) {
    let color = if invalid {
        theme::current().error
    } else if has_focus {
        accent
    } else {
        return;
    };
    // StrokeKind::Middle: the stroke is centered on the rect boundary.
    let half_w = FOCUS_RING_W * 0.5;
    ui.painter().rect_stroke(
        rect.expand(FOCUS_RING_GAP + half_w),
        egui::CornerRadius::same(FOCUS_RING_ROUNDING),
        egui::Stroke::new(FOCUS_RING_W, color),
        egui::StrokeKind::Middle,
    );
}

/// Validation error rows (or hint when no errors). Must follow `ui.end_row()` for the field.
fn render_field_feedback(ui: &mut egui::Ui, errors: &[String], hint: Option<&str>) {
    if !errors.is_empty() {
        ui.label("");
        egui::Frame::default()
            .inner_margin(egui::Margin {
                left: FOCUS_RING_PAD as i8,
                ..Default::default()
            })
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    for err in errors {
                        ui.add(egui::Label::new(
                            egui::RichText::new(err)
                                .size(HINT_FONT_PX)
                                .color(theme::current().error),
                        ));
                    }
                });
            });
        ui.end_row();
    } else if let Some(hint) = hint {
        ui.label("");
        egui::Frame::default()
            .inner_margin(egui::Margin {
                left: FOCUS_RING_PAD as i8,
                ..Default::default()
            })
            .show(ui, |ui| {
                ui.add(egui::Label::new(
                    egui::RichText::new(hint).size(HINT_FONT_PX),
                ));
            });
        ui.end_row();
    }
}

// ---------------------------------------------------------------------------
// Font setup: load the first available OS CJK font and append it to egui's
// fallback chain so that Japanese / Chinese / Korean text renders correctly.

/// Returns the appropriate CJK `y_offset` tweak value for the current language
/// and platform.
///
/// | Condition            | Value | Rationale                                      |
/// |----------------------|-------|------------------------------------------------|
/// | Linux (ja or en)     | 0.0   | Linux CJK fonts align naturally; no shift      |
/// | macOS / Windows + ja | 2.0   | CJK is primary typeface; small shift needed    |
/// | Windows + en         | 1.0   | Windows CJK glyphs sit lower on Latin baseline |
/// | macOS + en           | 3.0   | macOS CJK fonts need more downward compensation|
fn cjk_y_offset_for_en_or_ja() -> f32 {
    if cfg!(target_os = "linux") {
        0.0
    } else if crate::i18n::active_lang_code() == "ja" {
        2.0
    } else if cfg!(target_os = "windows") {
        1.0
    } else {
        3.0 // macOS
    }
}

fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    // CJK font — optional, warns if missing.
    match load_cjk_font() {
        Some(bytes) => {
            let mut font_data = egui::FontData::from_owned(bytes);
            // Shift CJK glyphs to align with the Latin baseline. See the
            // `cjk_y_offset_for_en_or_ja` doc table for per-platform values.
            font_data.tweak.y_offset = cjk_y_offset_for_en_or_ja();
            fonts.font_data.insert("cjk".to_owned(), font_data.into());
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                if let Some(list) = fonts.families.get_mut(&family) {
                    list.push("cjk".to_owned());
                }
            }
        }
        None => eprintln!("Warning: no CJK font found — Japanese text will show as boxes."),
    }

    // When the UI language is Japanese, put the CJK face first so that labels
    // and TextEdit widgets use it as their primary typeface rather than falling
    // back from a Latin face.  For other languages (e.g. English) the CJK face
    // stays at the end of the fallback list so that any incidental CJK glyphs
    // still render, but Latin text uses the system face (Ubuntu-Light, etc.).
    if crate::i18n::active_lang_code() == "ja" && fonts.font_data.contains_key("cjk") {
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            if let Some(list) = fonts.families.get_mut(&family) {
                list.retain(|name| name != "cjk");
                list.insert(0, "cjk".to_owned());
            }
        }
    }

    // Bold CJK font — for field labels (e.g. Hiragino W6 on macOS).
    // Falls back to the regular CJK font when no bold variant is found.
    {
        let cjk_bold_name = if let Some(bytes) = load_cjk_bold_font() {
            let mut fd = egui::FontData::from_owned(bytes);
            fd.tweak.y_offset = cjk_y_offset_for_en_or_ja();
            fonts.font_data.insert("cjk_bold".to_owned(), fd.into());
            Some("cjk_bold")
        } else if fonts.font_data.contains_key("cjk") {
            Some("cjk")
        } else {
            None
        };
        // Load a Latin bold font for non-Japanese mode so that labels actually
        // look bold.  For Japanese mode the CJK bold face serves as the primary
        // typeface; Ubuntu-Light is the last-resort fallback in both cases.
        let latin_bold_primary = if let Some((bytes, face_index)) = load_latin_bold_font() {
            let mut fd = egui::FontData::from_owned(bytes);
            fd.index = face_index;
            fonts.font_data.insert("latin_bold".to_owned(), fd.into());
            Some("latin_bold")
        } else {
            None
        };

        // For Japanese, put the CJK bold face first so labels use it as the
        // primary typeface.  For other languages, put the Latin bold face first
        // and keep the CJK bold only as a fallback for incidental CJK glyphs.
        let bold_list = if crate::i18n::active_lang_code() == "ja" {
            let mut list = vec!["Ubuntu-Light".to_owned()];
            if let Some(name) = cjk_bold_name {
                list.insert(0, name.to_owned());
            }
            list
        } else {
            // Latin bold → CJK bold (fallback for CJK glyphs) → Ubuntu-Light
            let mut list = vec!["Ubuntu-Light".to_owned()];
            if let Some(name) = cjk_bold_name {
                list.insert(0, name.to_owned());
            }
            if let Some(name) = latin_bold_primary {
                list.insert(0, name.to_owned());
            }
            list
        };
        fonts.families.insert(
            egui::FontFamily::Name("bold".into()),
            bold_list,
        );
    }

    // Icon font — present only when `make icons` was run before building.
    #[cfg(has_icons)]
    {
        fonts.font_data.insert(
            "icons".to_owned(),
            egui::FontData::from_static(crate::schema::ICON_FONT).into(),
        );
        fonts.families.insert(
            egui::FontFamily::Name("icons".into()),
            vec!["icons".to_owned()],
        );
    }

    ctx.set_fonts(fonts);
}

/// Applies the active [`Palette`] to the egui visuals.
fn setup_visuals(ctx: &egui::Context, pal: &Palette) {
    // Pick a light or dark base so egui-derived colors (scrollbars, text
    // selection, etc.) match the palette, then override the specific colors we
    // care about.
    let dark = is_dark(pal.bg);
    let mut vis = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    vis.panel_fill          = pal.bg;
    vis.window_fill         = pal.bg;
    vis.extreme_bg_color    = pal.surface;
    vis.hyperlink_color     = pal.accent;
    vis.selection.bg_fill   = pal.selection_bg;
    vis.override_text_color = Some(pal.text);

    // Input field borders and corner radius.
    let rounding     = egui::CornerRadius::same(FIELD_ROUNDING);
    let stroke_idle  = egui::Stroke::new(FIELD_BORDER_W_IDLE, pal.field_border);
    let stroke_hover = egui::Stroke::new(FIELD_BORDER_W_IDLE, pal.field_border);
    vis.widgets.noninteractive.corner_radius = rounding;
    vis.widgets.noninteractive.bg_stroke =
        egui::Stroke::new(1.0, pal.separator);
    vis.widgets.inactive.corner_radius       = rounding;
    vis.widgets.inactive.bg_stroke           = stroke_idle;
    vis.widgets.hovered.corner_radius        = rounding;
    vis.widgets.hovered.bg_stroke            = stroke_hover;
    vis.widgets.active.corner_radius         = rounding;
    vis.widgets.active.bg_stroke             = stroke_idle; // focus border is painted manually
    vis.widgets.open.corner_radius           = rounding;
    vis.widgets.open.bg_stroke               = stroke_idle;

    ctx.set_visuals(vis);
}

/// Returns `true` when `c` is dark enough that the UI should use a dark base
/// (simple perceived-luminance threshold).
fn is_dark(c: egui::Color32) -> bool {
    let [r, g, b, _] = c.to_array();
    let lum = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    lum < 128.0
}

fn setup_style(ctx: &egui::Context) {
    ctx.style_mut(|s| {
        s.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(FIELD_FONT_PX, egui::FontFamily::Proportional),
        );
        s.text_styles.insert(
            egui::TextStyle::Small,
            egui::FontId::new(HINT_FONT_PX, egui::FontFamily::Proportional),
        );
    });
}

fn load_cjk_font() -> Option<Vec<u8>> {
    for path in cjk_font_candidates() {
        if let Ok(bytes) = std::fs::read(path) {
            return Some(bytes);
        }
    }
    None
}

fn cjk_font_candidates() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        &[
            "/System/Library/Fonts/\u{30d2}\u{30e9}\u{30ae}\u{30ce}\u{89d2}\u{30b4}\u{30b7}\u{30c3}\u{30af} W3.ttc",
            "/System/Library/Fonts/\u{30d2}\u{30e9}\u{30ae}\u{30ce}\u{89d2}\u{30b4}\u{30b7}\u{30c3}\u{30af} W6.ttc",
            "/Library/Fonts/\u{30d2}\u{30e9}\u{30ae}\u{30ce}\u{89d2}\u{30b4}\u{30b7}\u{30c3}\u{30af} ProN W3.otf",
        ]
    } else if cfg!(target_os = "windows") {
        &[
            "C:\\Windows\\Fonts\\meiryo.ttc",
            "C:\\Windows\\Fonts\\YuGothM.ttc",
            "C:\\Windows\\Fonts\\msgothic.ttc",
        ]
    } else {
        // Linux
        &[
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJKjp-Regular.otf",
            "/usr/share/fonts/truetype/vlgothic/VL-Gothic-Regular.ttf",
        ]
    }
}

fn load_cjk_bold_font() -> Option<Vec<u8>> {
    for path in cjk_bold_font_candidates() {
        if let Ok(bytes) = std::fs::read(path) {
            return Some(bytes);
        }
    }
    None
}

fn cjk_bold_font_candidates() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        &[
            "/System/Library/Fonts/\u{30d2}\u{30e9}\u{30ae}\u{30ce}\u{89d2}\u{30b4}\u{30b7}\u{30c3}\u{30af} W6.ttc",
        ]
    } else if cfg!(target_os = "windows") {
        &[
            "C:\\Windows\\Fonts\\meiryob.ttc",
            "C:\\Windows\\Fonts\\YuGothB.ttc",
        ]
    } else {
        // Linux
        &[
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJKjp-Bold.otf",
        ]
    }
}

fn load_latin_bold_font() -> Option<(Vec<u8>, u32)> {
    for (path, index) in latin_bold_font_candidates() {
        if let Ok(bytes) = std::fs::read(path) {
            return Some((bytes, *index));
        }
    }
    None
}

/// Returns (path, face_index_in_collection) for Latin bold font candidates.
///
/// face_index is the TTC/OTC face index (0 for standalone TTF/OTF files).
/// macOS: Helvetica.ttc index 0 = Regular, index 1 = Bold.
fn latin_bold_font_candidates() -> &'static [(&'static str, u32)] {
    if cfg!(target_os = "macos") {
        &[
            ("/System/Library/Fonts/Helvetica.ttc", 1),
            ("/System/Library/Fonts/Supplemental/Arial Bold.ttf", 0),
        ]
    } else if cfg!(target_os = "windows") {
        &[
            ("C:\\Windows\\Fonts\\arialbd.ttf", 0),
            ("C:\\Windows\\Fonts\\calibrib.ttf", 0),
        ]
    } else {
        // Linux
        &[
            ("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf", 0),
            ("/usr/share/fonts/truetype/ubuntu/Ubuntu-B.ttf", 0),
            ("/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf", 0),
        ]
    }
}

// ---------------------------------------------------------------------------
// Codepoints parser (only compiled when icon assets are embedded)

/// Parses a Material Symbols `.codepoints` file into a name → char map.
/// Each line has the format: `icon_name HEXCODEPOINT`
#[cfg(has_icons)]
fn parse_codepoints(src: &str) -> HashMap<String, char> {
    let mut map = HashMap::new();
    for line in src.lines() {
        let mut parts = line.split_ascii_whitespace();
        if let (Some(name), Some(hex)) = (parts.next(), parts.next()) {
            if let Ok(cp) = u32::from_str_radix(hex, 16) {
                if let Some(c) = char::from_u32(cp) {
                    map.insert(name.to_owned(), c);
                }
            }
        }
    }
    map
}

// ---------------------------------------------------------------------------
// Add-section dialog

/// Builds a dialog-style action button matching the delete-confirm dialog
/// (72x28 with 6 px rounded corners). When `primary`, it is filled with the
/// accent color and uses the on-accent text color; otherwise it uses the
/// default button styling.
fn action_button(label: &'static str, primary: bool, accent: egui::Color32) -> egui::Button<'static> {
    let btn = if primary {
        egui::Button::new(egui::RichText::new(label).color(theme::current().on_accent))
            .fill(accent)
    } else {
        egui::Button::new(label)
    };
    btn.min_size(egui::vec2(72.0, 28.0))
        .corner_radius(egui::CornerRadius::same(6))
}

fn handle_add_dialog(
    ctx: &egui::Context,
    add_dialog: &mut Option<AddDialog>,
    config: &mut ConfigStore,
    accent: egui::Color32,
) {
    let mut action: Option<(String, String)> = None; // (prefix, key)
    let mut cancel = false;

    if let Some(dialog) = add_dialog.as_mut() {
        let frame = egui::Frame::popup(&ctx.style())
            .inner_margin(egui::Margin::same(16));
        egui::Window::new("##add_section")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .min_width(260.0)
            .frame(frame)
            .show(ctx, |ui| {
                ui.set_min_width(220.0);
                ui.label(
                    egui::RichText::new(t().add_section).size(FIELD_FONT_PX),
                );
                ui.add_space(4.0);
                ui.label(t().section_name_label);
                let resp = ui.text_edit_singleline(&mut dialog.input);
                if let Some(err) = &dialog.error {
                    ui.colored_label(theme::current().error, err.as_str());
                }
                ui.add_space(FIELD_FONT_PX + ui.spacing().item_spacing.y);
                let enter = resp.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if ui.add(action_button(t().add, true, accent)).clicked()
                                || enter
                            {
                                let key = dialog.input.trim().to_owned();
                                if key.is_empty() {
                                    dialog.error = Some(t().enter_name.into());
                                } else {
                                    action = Some((dialog.key_prefix.clone(), key));
                                }
                            }
                            if ui.add(action_button(t().cancel, false, accent)).clicked() {
                                cancel = true;
                            }
                        },
                    );
                });
            });
    }

    if let Some((prefix, key)) = action {
        config.set_table(&format!("{prefix}.{key}"));
        *add_dialog = None;
    } else if cancel {
        *add_dialog = None;
    }
}

// ---------------------------------------------------------------------------
// Delete-confirm dialog

fn handle_delete_confirm(
    ctx: &egui::Context,
    delete_confirm: &mut Option<DeleteConfirm>,
    config: &mut ConfigStore,
    selected_sub: &mut HashMap<usize, usize>,
    accent: egui::Color32,
) {
    let mut do_delete = false;
    let mut cancel = false;

    if let Some(confirm) = delete_confirm.as_ref() {
        let frame = egui::Frame::popup(&ctx.style())
            .inner_margin(egui::Margin::same(16));
        egui::Window::new("##delete_confirm")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .min_width(260.0)
            .frame(frame)
            .show(ctx, |ui| {
                ui.set_min_width(220.0);
                ui.label(
                    egui::RichText::new(t().delete_confirm(&confirm.section_key))
                        .size(FIELD_FONT_PX),
                );
                // one blank line (≈ FIELD_FONT_PX + item_spacing.y)
                ui.add_space(FIELD_FONT_PX + ui.spacing().item_spacing.y);
                // Use horizontal first to constrain height, then right_to_left
                // inside it so buttons sit at the right edge.
                ui.horizontal(|ui| {
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new(t().delete)
                                            .color(theme::current().on_accent),
                                    )
                                    .fill(accent)
                                    .min_size(egui::vec2(72.0, 28.0))
                                    .corner_radius(egui::CornerRadius::same(6)),
                                )
                                .clicked()
                            {
                                do_delete = true;
                            }
                            if ui
                                .add(
                                    egui::Button::new(t().cancel)
                                        .min_size(egui::vec2(72.0, 28.0))
                                        .corner_radius(egui::CornerRadius::same(6)),
                                )
                                .clicked()
                            {
                                cancel = true;
                            }
                        },
                    );
                });
            });
    }

    if do_delete {
        if let Some(confirm) = delete_confirm.take() {
            config.remove(&format!("{}.{}", confirm.key_prefix, confirm.section_key));
            selected_sub.clear();
        }
    } else if cancel {
        *delete_confirm = None;
    }
}

// ---------------------------------------------------------------------------
// File-conflict dialog

fn handle_file_conflict(
    ctx: &egui::Context,
    file_conflict: &mut bool,
    config: &mut ConfigStore,
    accent: egui::Color32,
) {
    if !*file_conflict {
        return;
    }

    let mut do_reload = false;
    let mut keep = false;

    let frame = egui::Frame::popup(&ctx.style())
        .inner_margin(egui::Margin::same(16));
    egui::Window::new("##file_conflict")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .min_width(300.0)
        .frame(frame)
        .show(ctx, |ui| {
            ui.set_min_width(260.0);
            ui.label(egui::RichText::new(t().file_changed).size(FIELD_FONT_PX));
            ui.add_space(FIELD_FONT_PX + ui.spacing().item_spacing.y);
            ui.horizontal(|ui| {
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        if ui.add(action_button(t().keep_editing, false, accent)).clicked() {
                            keep = true;
                        }
                        if ui.add(action_button(t().reload, true, accent)).clicked() {
                            do_reload = true;
                        }
                    },
                );
            });
        });

    if do_reload {
        if let Err(e) = config.reload() {
            eprintln!("Reload error: {e}");
        }
        *file_conflict = false;
    } else if keep {
        *file_conflict = false;
    }
}
