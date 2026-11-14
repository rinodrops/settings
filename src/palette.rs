//! Named color swatches with a keyboard-accessible HSV editor.

use egui::{Color32, RichText, Stroke};
use settings_schema::{Field, PaletteColor};

use crate::{config::ConfigStore, i18n, theme};

pub(crate) fn sibling(path: &str, key: &str) -> String {
    path.rsplit_once('.')
        .map_or_else(|| key.to_owned(), |(parent, _)| format!("{parent}.{key}"))
}

pub(crate) fn localized<'a>(en: &'a str, ja: &'a str) -> &'a str {
    if i18n::active_lang_code() == "ja" {
        ja
    } else {
        en
    }
}

fn parse_custom(value: &str) -> Option<[u16; 3]> {
    let mut values = value.strip_prefix("custom:")?.split(':');
    let hsv = [
        values.next()?.parse().ok()?,
        values.next()?.parse().ok()?,
        values.next()?.parse().ok()?,
    ];
    (values.next().is_none() && hsv[0] < 360 && hsv[1] <= 100 && hsv[2] <= 100).then_some(hsv)
}

fn rgb([h, s, v]: [u16; 3]) -> Color32 {
    let h = f32::from(h) / 60.0;
    let s = f32::from(s) / 100.0;
    let v = f32::from(v) / 100.0;
    let channel = |offset: f32| {
        let k = (offset + h) % 6.0_f32;
        (255.0 * v * (1.0 - s * k.min(4.0 - k).clamp(0.0, 1.0))).round() as u8
    };
    Color32::from_rgb(channel(5.0), channel(3.0), channel(1.0))
}

pub(crate) fn render(
    ui: &mut egui::Ui,
    field: &Field,
    path: &str,
    config: &mut ConfigStore,
    accent: Color32,
) {
    let Some(default) = field.colors.first() else {
        return;
    };
    let legacy = field
        .legacy_hue_key
        .as_ref()
        .and_then(|key| config.get_number(&sibling(path, key)))
        .filter(|hue| hue.is_finite());
    let selected = config.get_str(path);
    let active = selected.and_then(|id| field.colors.iter().find(|color| color.id == id));
    let custom = selected.and_then(parse_custom);
    let legacy = legacy.filter(|_| active.is_none() && custom.is_none());
    let chosen = active.or_else(|| {
        (custom.is_none() && legacy.is_none_or(|hue| (hue - f64::from(default.hsv[0])).abs() < 0.5))
            .then_some(default)
    });
    let mut hsv = custom
        .or_else(|| chosen.map(|color| color.hsv))
        .unwrap_or_else(|| {
            let mut hsv = default.hsv;
            hsv[0] = legacy.unwrap_or_default().round().rem_euclid(360.0) as u16;
            if let Some(hue) = legacy {
                hsv = crate::hooks::legacy_color(path, hue, default.hsv).unwrap_or(hsv);
            }
            hsv
        });
    // All palettes in a record share one editor, independent of other records.
    let key = path.rsplit_once('.').map_or(path, |(_, key)| key);
    let editor = ui
        .stack()
        .iter()
        .find_map(|stack| {
            stack
                .info
                .tags
                .get_downcast::<egui::Id>("palette_editor")
                .copied()
        })
        .unwrap_or_else(|| egui::Id::new(("palette_editor", path)));
    let mut open = ui.data(|data| data.get_temp::<String>(editor)).as_deref() == Some(key);
    let mut preset = None;
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(chosen.map_or(localized("Custom", "カスタム"), |color| {
                color.label.get()
            }))
            .color(theme::current().muted_text),
        );
        let (sample, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
        ui.painter().rect_filled(sample, 3, rgb(hsv));
        if ui
            .selectable_label(open, localized("Custom…", "カスタム…"))
            .clicked()
        {
            open = !open;
            ui.data_mut(|data| {
                data.insert_temp(editor, if open { key.to_owned() } else { String::new() })
            });
        }
    });
    ui.horizontal_wrapped(|ui| {
        for color in &field.colors {
            let selected = chosen.is_some_and(|chosen| chosen.id == color.id);
            if swatch(ui, color, selected, accent).clicked() {
                preset = Some(color.id.as_str());
            }
        }
    });
    if let Some(id) = preset {
        config.set_str(path, id);
        ui.data_mut(|data| data.remove::<String>(editor));
    } else if open {
        let before = hsv;
        egui::Frame::default()
            .fill(theme::current().header_bg)
            .corner_radius(6)
            .inner_margin(egui::Margin::same(6))
            .show(ui, |ui| {
                let labels = [
                    (localized("Hue", "色相"), "°", 359),
                    (localized("Saturation", "彩度"), "%", 100),
                    (localized("Value", "明度"), "%", 100),
                ];
                let label_width = labels
                    .iter()
                    .map(|(label, _, _)| {
                        ui.painter()
                            .layout_no_wrap(
                                (*label).to_owned(),
                                egui::TextStyle::Body.resolve(ui.style()),
                                Color32::WHITE,
                            )
                            .size()
                            .x
                    })
                    .fold(0.0_f32, f32::max);
                for (component, (label, suffix, max)) in labels.into_iter().enumerate() {
                    let paint = hsv;
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [label_width, 20.0],
                            egui::Label::new(label).halign(egui::Align::Min),
                        );
                        slider(ui, &mut hsv[component], max, component, paint, label);
                        ui.add_sized(
                            [50.0, 20.0],
                            egui::DragValue::new(&mut hsv[component])
                                .range(0..=max)
                                .suffix(suffix),
                        )
                        .on_hover_text(label);
                    });
                }
            });
        if hsv != before {
            config.set_str(path, &format!("custom:{}:{}:{}", hsv[0], hsv[1], hsv[2]));
        }
    }
}

fn swatch(
    ui: &mut egui::Ui,
    color: &PaletteColor,
    selected: bool,
    accent: Color32,
) -> egui::Response {
    let fill = rgb(color.hsv);
    let [r, g, b, _] = fill.to_array();
    let luminance = 0.213 * f32::from(r) + 0.715 * f32::from(g) + 0.072 * f32::from(b);
    let check = if luminance > 135.0 {
        Color32::BLACK
    } else {
        Color32::WHITE
    };
    let response = ui
        .add_sized(
            [18.0, 20.0],
            egui::Button::new("")
                .fill(fill)
                .corner_radius(4)
                .stroke(Stroke::new(1.0, theme::current().field_border)),
        )
        .on_hover_text(color.label.get());
    if selected {
        let center = response.rect.center();
        let stroke = Stroke::new(1.5, check);
        ui.painter().line_segment(
            [
                center + egui::vec2(-4.0, 0.0),
                center + egui::vec2(-1.0, 3.0),
            ],
            stroke,
        );
        ui.painter().line_segment(
            [
                center + egui::vec2(-1.0, 3.0),
                center + egui::vec2(4.0, -3.0),
            ],
            stroke,
        );
        ui.painter().rect_stroke(
            response.rect,
            4,
            Stroke::new(1.0, accent),
            egui::StrokeKind::Inside,
        );
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            ui.is_enabled(),
            selected,
            color.label.get(),
        )
    });
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.expand(2.0),
            5,
            Stroke::new(1.5, accent),
            egui::StrokeKind::Outside,
        );
    }
    response
}

fn slider(
    ui: &mut egui::Ui,
    value: &mut u16,
    max: u16,
    component: usize,
    hsv: [u16; 3],
    label: &str,
) {
    let track = ui.painter().add(egui::Shape::Noop);
    let response = ui
        .scope(|ui| {
            ui.spacing_mut().slider_width = (ui.available_width() - 60.0).clamp(32.0, 240.0);
            let widgets = &mut ui.visuals_mut().widgets;
            widgets.inactive.bg_fill = Color32::TRANSPARENT;
            widgets.inactive.fg_stroke = Stroke::NONE;
            widgets.hovered.bg_fill = Color32::TRANSPARENT;
            widgets.hovered.fg_stroke = Stroke::NONE;
            widgets.active.bg_fill = Color32::TRANSPARENT;
            widgets.active.fg_stroke = Stroke::NONE;
            ui.add(
                egui::Slider::new(value, 0..=max)
                    .show_value(false)
                    .trailing_fill(false),
            )
        })
        .inner
        .on_hover_text(label);
    let rect = egui::Rect::from_center_size(
        response.rect.center(),
        egui::vec2(response.rect.width(), 4.0),
    );
    let mut mesh = egui::Mesh::default();
    let stops = if component == 0 { 6 } else { 1 };
    for i in 0..=stops {
        let mut color = if component == 0 { [0, 90, 95] } else { hsv };
        color[component] = if component == 0 {
            (i * 60) % 360
        } else {
            i * 100
        };
        let x = egui::lerp(rect.x_range(), f32::from(i) / f32::from(stops));
        mesh.colored_vertex(egui::pos2(x, rect.min.y), rgb(color));
        mesh.colored_vertex(egui::pos2(x, rect.max.y), rgb(color));
        if i > 0 {
            let n = u32::from(i) * 2;
            mesh.add_triangle(n - 2, n - 1, n);
            mesh.add_triangle(n, n - 1, n + 1);
        }
    }
    ui.painter().set(track, mesh);
    let pal = theme::current();
    // egui places the marker within the rail by one handle radius on each side.
    let marker_inset = response.rect.height() / 2.5;
    let x = egui::lerp(
        (rect.left() + marker_inset)..=(rect.right() - marker_inset),
        f32::from(*value) / f32::from(max),
    );
    let border = if response.has_focus() || response.hovered() {
        pal.accent
    } else {
        pal.muted_text
    };
    ui.painter().circle(
        egui::pos2(x, rect.center().y),
        6.0,
        pal.surface,
        Stroke::new(1.0, border),
    );
}
