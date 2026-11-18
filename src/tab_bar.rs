//! Compact egui tab strip. Geometry is returned to the window controller so
//! native drag-to-reorder and cross-window drops keep using physical positions.
use crate::ui_style::{self, Icon};

const MENU_WIDTH: f32 = 42.0;
/// Minimum width of the workspace indicator at either end of the tab strip.
const WORKSPACE_WIDTH: f32 = 32.0;

pub(crate) fn menu_button_rect(height: f32) -> egui::Rect {
    egui::Rect::from_min_max(egui::pos2(3.0, 3.0), egui::pos2(MENU_WIDTH - 3.0, height - 3.0))
}

pub(crate) struct TabFace {
    pub id: usize,
    pub preview: Option<(egui::TextureId, (u32, u32))>,
    pub title: String,
    pub tooltip: String,
    pub group: Option<egui::Color32>,
    pub entangled: bool,
    pub attention: f32,
}

pub(crate) struct Geometry {
    pub menu_clicked: bool,
    pub workspace_clicked: bool,
    /// Where the indicator is, for anything that needs to point at it.
    pub workspace: egui::Rect,
    pub tabs: Vec<egui::Rect>,
    pub close: Vec<egui::Rect>,
    pub add: egui::Rect,
}

impl Default for Geometry {
    fn default() -> Self {
        Self { menu_clicked: false, workspace_clicked: false, workspace: egui::Rect::NOTHING,
            tabs: Vec::new(), close: Vec::new(), add: egui::Rect::NOTHING }
    }
}

pub(crate) fn show(
    root: &mut egui::Ui,
    height: f32,
    font: egui::FontId,
    tabs: &[TabFace],
    active: usize,
    menu_open: bool,
    workspace: usize,
    workspace_open: bool,
    cfg: &crate::config::Config,
) -> Geometry {
    let chrome = ui_style::TabStyle::new(root, cfg, workspace);
    let colors = chrome.base;
    let width = root.max_rect().width();
    let mut geometry = Geometry {
        tabs: vec![egui::Rect::NOTHING; tabs.len()],
        close: vec![egui::Rect::NOTHING; tabs.len()],
        ..Default::default()
    };
    egui::Area::new(egui::Id::unique("terminal_tab_bar"))
        .fixed_pos(egui::Pos2::ZERO)
        .order(egui::Order::Middle)
        .movable(false)
        .constrain(false)
        .fade_in(false)
        .show(root.ctx(), |ui| {
            ui.set_min_size(egui::vec2(width, height));
            let p = ui.painter();
            let bar = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height));
            p.rect_filled(bar, 0.0, colors.bg);
            p.hline(bar.x_range(), height - 0.5, egui::Stroke::new(1.0, colors.border));
            let genre = cfg.workspace_genres.get(workspace).map_or("", |genre| genre.trim());
            let workspace_label = cfg.workspace_button_display.text(workspace + 1, genre);
            let show_icon = cfg.workspace_button_display.shows_icon();
            let workspace_width = if cfg.workspaces_enabled {
                // Leave room for tabs and the add button; long genres are ellipsized.
                (ui_style::text_width(ui, &workspace_label, 11.0) + 22.0
                    + if show_icon && !workspace_label.is_empty() { 24.0 } else { 0.0 })
                    .max(WORKSPACE_WIDTH).min((width * 0.5).clamp(WORKSPACE_WIDTH, 240.0))
            } else { 0.0 };
            let workspace_left = cfg.workspace_button_position == crate::config::WorkspaceButtonPosition::Left;
            let tabs_left = MENU_WIDTH + if workspace_left { workspace_width } else { 0.0 };
            let tabs_right = width - if workspace_left { 0.0 } else { workspace_width };
            let menu = menu_button_rect(height);
            ui_style::record_control(ui, "tab_bar/menu", menu);
            let response = ui.interact(menu, egui::Id::unique("app_menu_button"), egui::Sense::click());
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "mypixels menu"));
            if menu_open || response.hovered() {
                colors.surface(p, menu, 4.0, menu_open, response.hovered());
            }
            let color = if menu_open || response.hovered() { colors.accent } else { colors.muted };
            ui_style::icon(p, Icon::Terminal, egui::pos2(15.0, height * 0.5), color);
            let center = egui::pos2(30.0, height * 0.5);
            p.add(egui::Shape::line(vec![center + egui::vec2(-3.0, -1.5), center + egui::vec2(0.0, 1.5),
                center + egui::vec2(3.0, -1.5)], egui::Stroke::new(1.0, color)));
            geometry.menu_clicked = response.clicked();
            response.on_hover_text("mypixels menu").on_hover_cursor(egui::CursorIcon::PointingHand);
            // Separate the menu from the next strip control.
            p.vline(MENU_WIDTH - 1.0, 0.0..=height, egui::Stroke::new(1.0, colors.border));
            if workspace_width > 0.0 {
                let start = if workspace_left { MENU_WIDTH } else { tabs_right };
                let workspace_rect = egui::Rect::from_min_max(
                    egui::pos2(start + 3.0, 3.0), egui::pos2(start + workspace_width - 3.0, height - 3.0));
                let response = ui.interact(workspace_rect, egui::Id::unique("workspace_button"), egui::Sense::click());
                let description = if genre.is_empty() { format!("Workspace {}", workspace + 1) }
                    else { format!("Workspace {} · {}", workspace + 1, genre) };
                response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &description));
                let text_color = ui_style::black_or_white_text(colors.accent);
                p.rect_filled(workspace_rect, 4.0, colors.accent);
                if workspace_open || response.hovered() || response.has_focus() {
                    p.rect_stroke(workspace_rect, 4.0, egui::Stroke::new(1.0, text_color), egui::StrokeKind::Inside);
                }
                let mut label_rect = workspace_rect.shrink2(egui::vec2(8.0, 0.0));
                if show_icon {
                    let center = if workspace_label.is_empty() { workspace_rect.center() }
                        else { egui::pos2(workspace_rect.left() + 14.0, workspace_rect.center().y) };
                    ui_style::workspace_icon(p, cfg.workspace_icons[workspace], center, text_color);
                    label_rect.min.x += 24.0;
                }
                if !workspace_label.is_empty() {
                    ui_style::text_centered(ui, label_rect, &workspace_label, text_color, 11.0, false);
                }
                geometry.workspace_clicked = response.clicked();
                geometry.workspace = workspace_rect;
                response.on_hover_text(format!("{} · {} · click for the workspace panel",
                    description, crate::config::workspace_shortcut(cfg, workspace)))
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                p.vline(if workspace_left { tabs_left } else { tabs_right },
                    0.0..=height, egui::Stroke::new(1.0, colors.border));
            }
            let gap = if cfg.compact_tabs { 1.0 } else { 4.0 };
            let margin = if cfg.compact_tabs { 1.0 / ui.ctx().pixels_per_point() } else { 3.0 };
            let body_height = if cfg.compact_tabs {
                (height - 2.0 * margin).max(1.0)
            } else { (height - 2.0 * margin).max(16.0) };
            let strip = egui::Rect::from_min_max(egui::pos2(tabs_left + gap, margin),
                egui::pos2((tabs_right - gap).max(tabs_left + gap), margin + body_height));
            // Reserve the add control first. Tabs receive the remaining strip;
            // when they need less space, the button follows the last tab.
            let add_slot = strip.with_min_x((strip.right() - 28.0).max(strip.left()));
            let tab_space = strip.with_max_x((add_slot.left() - gap).max(strip.left()));
            let badge_widths: Vec<_> = (0..tabs.len()).map(|index| if cfg.compact_tabs {
                ui_style::text_width(ui, &format!(" {} ", index + 1), 11.0) + 2.0
            } else { 19.0 }).collect();
            let badge_width = |index: usize| badge_widths[index];
            let close_width = if cfg.compact_tabs { 14.0 } else { 20.0 };
            let close_inset = if cfg.compact_tabs { 8.0 } else { 13.0 };
            let widths: Vec<f32> = tabs.iter().enumerate().map(|(index, tab)| {
                if !cfg.show_tab_titles { return if cfg.compact_tabs { badge_width(index) + 2.0 } else { 36.0 }; }
                let title = ui.painter().layout_no_wrap(tab.title.clone(), font.clone(), colors.fg);
                let (padding, minimum) = if cfg.compact_tabs {
                    let fitted = badge_width(index) + close_width + 5.0;
                    (fitted, fitted)
                } else { (58.0, 96.0) };
                let markers = if tab.entangled { 10.0 } else { 0.0 }
                    + if cfg.compact_tabs && tab.group.is_some() { 6.0 } else { 0.0 };
                let maximum = tab_space.width().clamp(1.0, 236.0);
                (title.size().x + padding + markers).clamp(minimum.min(maximum), maximum)
            }).collect();
            let content_width = widths.iter().sum::<f32>() + gap * tabs.len().saturating_sub(1) as f32;
            let viewport_width = content_width.min(tab_space.width());
            let viewport = tab_space.with_max_x(tab_space.left() + viewport_width);
            let selection = (tabs.get(active).map(|t| t.id), tabs_left.to_bits(), tabs_right.to_bits(), content_width.to_bits());
            let follow = ui.ctx().data_mut(|d| {
                let id = egui::Id::unique("tab_bar_selection");
                let old = d.get_temp::<(Option<usize>, u32, u32, u32)>(id);
                d.insert_temp(id, selection);
                old != Some(selection)
            });
            let mut host = ui.new_child(egui::UiBuilder::new().id_salt("tab_bar_viewport").max_rect(viewport));
            host.set_clip_rect(viewport);
            egui::ScrollArea::horizontal()
                .id_salt("tab_bar_scroll")
                .animated(false)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .auto_shrink([false, false])
                .max_width(viewport_width)
                .max_height(body_height)
                .show(&mut host, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(gap, 0.0);
                    ui.spacing_mut().interact_size.y = body_height;
                    ui.horizontal(|ui| {
                        let content_origin = ui.max_rect().min.to_vec2();
                        let mut selection = None;
                        for (index, tab) in tabs.iter().enumerate() {
                            // Pointer actions stay in the window controller. This egui
                            // widget provides layout, hover, clipping and tooltip behavior.
                            let (rect, response) =
                                ui.allocate_exact_size(egui::vec2(widths[index], body_height), egui::Sense::hover());
                            let target = rect;
                            let rect = ui_style::animated_rect(ui.ctx(), egui::Id::unique("tab_motion").with((tab.id, width.to_bits())), rect.translate(-content_origin)).translate(content_origin);
                            let selected = index == active;
                            let weight = ui_style::transition(ui.ctx(), egui::Id::unique("tab_selected_fill").with(tab.id), selected);
                            chrome.tab(ui.painter(), rect, weight, response.hovered());
                            if selected { selection = Some(rect); }
                            if tab.attention > 0.0 {
                                ui.painter().rect_filled(
                                    rect.shrink(1.0),
                                    5.0,
                                    colors.err.gamma_multiply(tab.attention * 0.35),
                                );
                            }
                            let badge = egui::Rect::from_center_size(
                                egui::pos2(if cfg.show_tab_titles { rect.left() + if cfg.compact_tabs { 1.0 + badge_width(index) * 0.5 } else { 15.0 } } else { rect.center().x }, rect.center().y),
                                egui::vec2(badge_width(index), if cfg.compact_tabs { body_height } else { (body_height - 6.0).max(14.0) }),
                            );
                            let mut badge_style = colors;
                            badge_style.fg = chrome.index;
                            badge_style.number_badge(ui.painter(), badge, index + 1, selected, 11.0);
                            let close = egui::Rect::from_center_size(
                                egui::pos2(rect.right() - close_inset, rect.center().y),
                                egui::vec2(close_width, body_height),
                            );
                            if cfg.show_tab_titles {
                                let title = rect.with_min_x(badge.right() + if cfg.compact_tabs { 1.0 } else { 8.0 })
                                    .with_max_x(close.left() - if cfg.compact_tabs { 1.0 } else { 3.0 });
                                if tab.entangled {
                                    ui_style::text(
                                        ui,
                                        title.with_max_x(title.left() + 10.0),
                                        "*",
                                        chrome.entangle,
                                        13.0,
                                        true,
                                    );
                                }
                                let group_width = if let Some(color) = tab.group {
                                    ui.painter().circle_filled(egui::pos2(close.left() - if cfg.compact_tabs { 3.0 } else { 7.0 }, rect.center().y), 2.5, color);
                                    if cfg.compact_tabs { 6.0 } else { 10.0 }
                                } else { 0.0 };
                                ui_style::text_with_font(
                                    ui,
                                    title.with_min_x(title.left() + if tab.entangled { 10.0 } else { 0.0 })
                                        .with_max_x(title.right() - group_width),
                                    &tab.title,
                                    if selected { chrome.selected_foreground } else { chrome.foreground },
                                    font.clone(),
                                );
                                let close_hover = ui.rect_contains_pointer(close);
                                if selected || response.hovered() {
                                    if close_hover {
                                        ui.painter().rect_filled(close.shrink(2.0), 4.0, colors.card);
                                    }
                                    ui_style::icon(
                                        ui.painter(),
                                        Icon::Close,
                                        close.center(),
                                        if close_hover { colors.accent } else { colors.muted },
                                    );
                                }
                                // Native drag hit targets follow layout, so moving
                                // neighbors do not retrigger a swap mid-animation.
                                geometry.close[index] = egui::Rect::from_center_size(
                                    egui::pos2(target.right() - close_inset, target.center().y), egui::vec2(close_width, body_height))
                                    .intersect(viewport);
                            }
                            geometry.tabs[index] = target.intersect(viewport);
                            ui_style::record_control(ui, format!("tab_bar/tab/{}", index + 1), geometry.tabs[index]);
                            ui_style::record_control(ui, format!("tab_bar/close/{}", index + 1), geometry.close[index]);
                            let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                            let mut tooltip = egui::Tooltip::for_enabled(&response);
                            tooltip.popup = tooltip.popup.frame(colors.menu_frame());
                            tooltip.show(|ui| {
                                colors.apply_controls(ui);
                                ui.label(&tab.tooltip);
                                if let Some((_, (w, h))) = tab.preview {
                                    let width = (w as f32).min((ui.ctx().content_rect().width() - 48.0).max(80.0));
                                    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, width * h as f32 / w.max(1) as f32), egui::Sense::hover());
                                    ui_style::preview_image(ui, rect, tab.preview, &colors);
                                }
                            });
                            if selected && follow {
                                ui.scroll_to_rect(target, None);
                            }
                        }
                        if let Some(rect) = selection {
                            let id = egui::Id::unique("tab_selection_motion")
                                .with((content_origin.x.to_bits(), width.to_bits(), height.to_bits()));
                            let rect = ui_style::animated_rect_in(ui.ctx(), id, rect, viewport);
                            ui.painter().rect_stroke(rect, 5.0,
                                egui::Stroke::new(1.5, colors.accent), egui::StrokeKind::Inside);
                        }
                    });
                });
            let add = add_slot.translate(egui::vec2((viewport.right() + gap).min(add_slot.left()) - add_slot.left(), 0.0));
            ui_style::record_control(ui, "tab_bar/add", add);
            let response = ui.interact(add, egui::Id::unique("tab_bar_add"), egui::Sense::hover());
            if response.hovered() {
                colors.surface(ui.painter(), add, 5.0, false, true);
            }
            ui_style::icon(
                ui.painter(),
                Icon::Plus,
                add.center(),
                if response.hovered() { colors.accent } else { chrome.add },
            );
            response.on_hover_text("New tab · Ctrl+Shift+T").on_hover_cursor(egui::CursorIcon::PointingHand);
            geometry.add = add;
        });
    geometry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_button_can_move_live_without_hiding_tabs_or_stealing_their_hits() {
        use crate::config::WorkspaceButtonPosition::{Left, Right};
        for width in [360.0, 1280.0] {
            for compact in [false, true] {
                for genre in ["", "A long workspace genre that still leaves room for tabs"] {
                    let ctx = egui::Context::default();
                    let mut cfg = crate::config::Config::default();
                    cfg.workspaces_enabled = true;
                    cfg.compact_tabs = compact;
                    cfg.workspace_genres[2] = genre.into();
                    let tabs: Vec<_> = (0..40).map(|id| TabFace {
                        id, preview: None, title: format!("terminal {id}"), tooltip: String::new(),
                        group: None, entangled: false, attention: 0.0,
                    }).collect();
                    let height = ui_style::tab_height(16.0, 1.0, compact);
                    let mut geometry = Geometry::default();
                    for position in [Right, Left, Right] {
                        cfg.workspace_button_position = position;
                        for _ in 0..5 {
                            let mut full = ctx.run_ui(egui::RawInput {
                                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 480.0))),
                                ..Default::default()
                            }, |ui| geometry = show(ui, height, egui::FontId::monospace(14.0), &tabs, 39, false, 2, false, &cfg));
                            full.textures_delta.clear();
                        }
                        let button = geometry.workspace;
                        let selected = geometry.tabs[39];
                        assert!(button.is_positive() && selected.is_positive());
                        assert!(selected.contains_rect(geometry.close[39]), "active tab close target clipped");
                        assert!(geometry.add.left() >= selected.right());
                        assert!(geometry.add.right() <= width);
                        assert!(!button.intersects(selected) && !button.intersects(geometry.add));
                        if position == Left {
                            assert_eq!(button.left(), MENU_WIDTH + 3.0);
                            assert!(selected.left() >= button.right());
                        } else {
                            assert_eq!(button.right(), width - 3.0);
                            assert!(geometry.add.right() <= button.left());
                        }
                        let pos = button.center();
                        let mut events = vec![egui::Event::PointerMoved(pos)];
                        events.extend([true, false].into_iter().map(|pressed| egui::Event::PointerButton {
                            pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
                        }));
                        let mut full = ctx.run_ui(egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 480.0))),
                            events, ..Default::default()
                        }, |ui| geometry = show(ui, height, egui::FontId::monospace(14.0), &tabs, 39, false, 2, false, &cfg));
                        full.textures_delta.clear();
                        assert!(geometry.workspace_clicked, "workspace button lost its hit target after moving");
                        assert!(!geometry.menu_clicked);
                    }
                }
            }
        }
    }

    #[test]
    fn compact_tabs_reduce_padding_without_clipping_controls() {
        for width in [360.0, 1280.0] {
            for show_titles in [false, true] {
                let ctx = egui::Context::default();
                let mut cfg = crate::config::Config::default();
                cfg.show_tab_titles = show_titles;
                let tabs: Vec<_> = (0..40).map(|id| TabFace {
                    id, preview: None, title: "caption".into(), tooltip: "caption".into(),
                    group: None, entangled: false, attention: 0.0,
                }).collect();
                let mut normal_width = 0.0;
                let mut normal_height = 0.0;
                for compact in [false, true] {
                    cfg.compact_tabs = compact;
                    let height = ui_style::tab_height(16.0, 1.0, compact);
                    let mut geometry = Geometry::default();
                    for _ in 0..5 {
                        let mut full = ctx.run_ui(egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 480.0))),
                            ..Default::default()
                        }, |ui| {
                            geometry = show(ui, height, egui::FontId::monospace(14.0), &tabs, 39, false, 0, false, &cfg);
                        });
                        full.textures_delta.clear();
                    }
                    let selected = geometry.tabs[39];
                    assert!(selected.left() >= 0.0 && selected.right() < width);
                    assert!(selected.top() >= 0.0 && selected.bottom() <= height);
                    assert!(geometry.add.right() <= width && geometry.add.left() >= selected.right());
                    if show_titles {
                        let close = geometry.close[39];
                        assert!(close.width() >= if compact { 13.0 } else { 18.0 });
                        assert!(selected.contains_rect(close));
                    } else {
                        assert!(!geometry.close[39].is_positive());
                    }
                    if compact {
                        assert!(selected.width() < normal_width);
                        assert!(height < normal_height);
                    } else {
                        normal_width = selected.width();
                        normal_height = height;
                    }
                }
            }
        }
        // Compact spacing is exactly two physical pixels, including small fonts.
        assert_eq!(ui_style::tab_height(10.0, 1.0, true), 12.0);
        assert_eq!(ui_style::tab_height(11.5, 1.0, true), 14.0);
        assert_eq!(ui_style::tab_height(32.0, 2.0, true), 34.0);
        assert_eq!(ui_style::tab_height(40.0, 1.0, true), 42.0);
        assert_eq!(ui_style::tab_height(10.0, 1.0, false), 28.0);
    }

    #[test]
    fn number_only_tabs_are_compact_and_keep_selection_reachable() {
        for width in [360.0, 1280.0] {
            let ctx = egui::Context::default();
            let mut cfg = crate::config::Config::default();
            let tabs: Vec<_> = (0..10).map(|id| TabFace {
                id, preview: None, title: format!("hidden caption {id}"),
                tooltip: format!("full terminal title {id}"),
                group: Some(egui::Color32::RED), entangled: true, attention: 0.0,
            }).collect();
            for show_titles in [false, true, false] {
                cfg.show_tab_titles = show_titles;
                let mut geometry = Geometry::default();
                for _ in 0..5 {
                    let mut full = ctx.run_ui(egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 480.0))),
                        ..Default::default()
                    }, |ui| {
                        geometry = show(ui, 28.0, egui::FontId::monospace(14.0), &tabs, 9, false, 0, false, &cfg);
                    });
                    if !show_titles {
                        assert!(!full.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t)
                            if t.galley.text().contains("hidden caption") || t.galley.text() == "*")));
                    }
                    full.textures_delta.clear();
                }
                let selected = geometry.tabs[9];
                assert!(selected.left() >= 0.0 && selected.right() < width);
                assert!(geometry.add.left() >= selected.right() && geometry.add.right() <= width);
                if show_titles {
                    assert!(selected.width() >= 90.0);
                    assert!(geometry.close[9].width() >= 18.0);
                } else {
                    assert!((selected.width() - 36.0).abs() < 1.0);
                    assert!(geometry.close.iter().all(|rect| !rect.is_positive()), "invisible close hit targets");
                }
            }
        }
    }

    #[test]
    fn the_workspace_indicator_sits_at_the_right_edge() {
        for width in [360.0, 1280.0] {
            let ctx = egui::Context::default();
            let mut cfg = crate::config::Config::default();
            cfg.workspaces_enabled = true;
            let tabs: Vec<_> = (0..3)
                .map(|id| TabFace {
                    id, preview: None, title: format!("terminal {id}"), tooltip: String::new(),
                    group: None, entangled: false, attention: 0.0,
                })
                .collect();
            let mut geometry = Geometry::default();
            let mut full = ctx.run_ui(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 480.0))),
                ..Default::default()
            }, |ui| {
                geometry = show(ui, 28.0, egui::FontId::monospace(14.0), &tabs, 0, false, 2, false, &cfg);
            });
            full.textures_delta.clear();
            let mark = geometry.workspace;
            assert!(mark.is_positive(), "the indicator has no hit area");
            assert_eq!(mark.right(), width - 3.0, "the indicator is not at the right edge");
            assert_eq!(mark.left(), width - WORKSPACE_WIDTH + 3.0);
            assert!(geometry.tabs[0].left() >= MENU_WIDTH, "the first tab overlaps the menu");
            assert!(geometry.tabs.iter().all(|tab| tab.right() <= mark.left()), "tabs overlap the indicator");
            assert!(geometry.add.right() <= mark.left(), "the add button overlaps the indicator");

            // Hiding the indicator preserves the tabs' left edge.
            cfg.workspaces_enabled = false;
            let mut off = Geometry::default();
            let mut out = ctx.run_ui(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 480.0))),
                ..Default::default()
            }, |ui| {
                off = show(ui, 28.0, egui::FontId::monospace(14.0), &tabs, 0, false, 2, false, &cfg);
            });
            out.textures_delta.clear();
            assert!(!off.workspace.is_positive(), "a hidden indicator still claims a rect");
            assert_eq!(off.tabs[0].left(), geometry.tabs[0].left(), "hiding the indicator moved the tabs' left edge");
        }
    }

    #[test]
    fn workspace_display_modes_draw_the_chosen_identity_and_preserve_tab_space() {
        use crate::config::{WorkspaceButtonDisplay as D, WorkspaceButtonPosition as P};
        for mode in D::ALL {
            for position in [P::Left, P::Right] {
                let ctx = egui::Context::default();
                let mut cfg = crate::config::Config::default();
                cfg.workspaces_enabled = true;
                cfg.workspace_button_display = mode;
                cfg.workspace_button_position = position;
                cfg.workspace_genres[2] = "Development".into();
                cfg.workspace_icons[2] = crate::config::WorkspaceIcon::Code;
                let tabs = [TabFace { id: 0, preview: None, title: "Terminal".into(),
                    tooltip: String::new(), group: None, entangled: false, attention: 0.0 }];
                let mut geometry = Geometry::default();
                for frame in 0..3 {
                    let mut output = ctx.run_ui(egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(500.0, 480.0))),
                        ..Default::default()
                    }, |ui| geometry = show(ui, 28.0, egui::FontId::monospace(14.0), &tabs, 0, false, 2, false, &cfg));
                    output.textures_delta.clear();
                    if frame < 2 { continue; }
                    let label = mode.text(3, "Development");
                    let text: Vec<_> = output.shapes.iter().filter_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if geometry.workspace.contains(text.pos) => Some(text.galley.text()), _ => None,
                    }).collect();
                    if mode == D::Icon { assert!(text.is_empty()); }
                    else { assert_eq!(text, [label.as_str()]); }
                    if mode.shows_icon() {
                        let icon_lines = output.shapes.iter().filter(|shape| matches!(&shape.shape,
                            egui::Shape::Path(path) if path.points.iter().all(|point| geometry.workspace.contains(*point)))).count();
                        assert!(icon_lines >= 3, "missing chosen code icon");
                    }
                    assert!(!geometry.workspace.intersects(geometry.tabs[0]));
                    assert!(!geometry.workspace.intersects(geometry.add));
                    output.textures_delta.clear();
                }
            }
        }
    }

    #[test]
    fn workspace_genres_and_colors_fit_the_indicator() {
        for width in [360.0, 1280.0] {
            for genre in ["", "   ", "Research", "A very long workspace genre that must leave room for tabs"] {
                for (background, foreground) in [
                    (egui::Color32::BLACK, egui::Color32::WHITE),
                    (egui::Color32::WHITE, egui::Color32::BLACK),
                    (egui::Color32::from_gray(118), egui::Color32::BLACK),
                ] {
                    let ctx = egui::Context::default();
                    let mut cfg = crate::config::Config::default();
                    cfg.workspaces_enabled = true;
                    cfg.workspace_genres[2] = genre.into();
                    cfg.workspace_colors[2] = crate::config::Rgb(background.r(), background.g(), background.b());
                    let tabs = [TabFace { id: 0, preview: None, title: "Terminal".into(),
                        tooltip: String::new(), group: None, entangled: false, attention: 0.0 }];
                    let mut geometry = Geometry::default();
                    for frame in 0..3 {
                        let mut output = ctx.run_ui(egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 480.0))),
                            ..Default::default()
                        }, |ui| {
                            geometry = show(ui, 28.0, egui::FontId::monospace(14.0), &tabs, 0, false, 2, false, &cfg);
                        });
                        output.textures_delta.clear();
                        if frame == 2 {
                            let expected = if genre.trim().is_empty() { "3" } else { genre };
                            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
                                egui::Shape::Text(text) if text.galley.job.text == expected
                                    && text.galley.job.sections.iter().all(|section| section.format.color == foreground))),
                                "missing workspace label or wrong contrast: {genre:?}");
                            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
                                egui::Shape::Rect(rect) if rect.rect == geometry.workspace && rect.fill == background)),
                                "workspace background does not match its color");
                        }
                    }
                    assert_eq!(geometry.workspace.right(), width - 3.0);
                    assert!(geometry.workspace.width() <= width * 0.5);
                    assert!(geometry.add.right() <= geometry.workspace.left());
                    assert!(geometry.tabs[0].right() <= geometry.workspace.left());
                    if genre.trim().is_empty() {
                        assert_eq!(geometry.workspace.width(), WORKSPACE_WIDTH - 6.0);
                    } else {
                        assert!(geometry.workspace.width() > WORKSPACE_WIDTH);
                    }
                }
            }
        }
    }

    #[test]
    fn many_tabs_keep_the_active_tab_and_add_button_reachable() {
        for width in [360.0, 1280.0] {
            let ctx = egui::Context::default();
            let cfg = crate::config::Config::default();
            let tabs: Vec<_> = (0..40)
                .map(|id| TabFace {
                    id,
                    preview: None,
                    title: format!("long terminal name {id} 界"),
                    tooltip: String::new(),
                    group: None,
                    entangled: id == 3,
                    attention: 0.0,
                })
                .collect();
            for active in [0, 39, 1] {
                let mut geometry = Geometry::default();
                for _ in 0..5 {
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 480.0))),
                            ..Default::default()
                        },
                        |ui| {
                            geometry =
                                show(ui, 28.0, egui::FontId::monospace(14.0), &tabs, active, false, 0, false, &cfg);
                        },
                    );
                    output.textures_delta.clear();
                }
                let selected = geometry.tabs[active];
                assert!(selected.width() >= 90.0, "active tab is clipped: {selected:?}");
                assert!(selected.left() >= 0.0 && selected.right() < width);
                assert!(geometry.close[active].width() >= 18.0, "close control is clipped: width={width} active={active} selected={selected:?} close={:?}", geometry.close[active]);
                assert!(geometry.add.right() <= width && geometry.add.left() >= selected.right());
                assert!(geometry.add.top() >= 0.0 && geometry.add.bottom() <= 28.0);
            }
        }
    }
}
