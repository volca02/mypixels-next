//! Shared visual language for the terminal UI. Palette, typography, icons,
//! surfaces and control treatments live here; screens own content and layout.

#[derive(Clone, Copy)]
pub(crate) struct TermStyle {
    pub(crate) bg: egui::Color32,
    pub(crate) fg: egui::Color32,
    pub(crate) muted: egui::Color32,
    pub(crate) border: egui::Color32,
    pub(crate) accent: egui::Color32,
    pub(crate) card: egui::Color32,
    pub(crate) ok: egui::Color32,
    pub(crate) err: egui::Color32,
}

impl TermStyle {
    /// Shared popup surface for application, context and overflow menus.
    pub(crate) fn menu_frame(&self) -> egui::Frame {
        egui::Frame::new()
            .fill(self.bg)
            .stroke(egui::Stroke::new(1.0, self.border))
            .corner_radius(9.0)
            .inner_margin(6.0)
    }

    pub(crate) fn menu_contents<R>(&self, ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
        ui.scope(|ui| {
            self.apply_controls(ui);
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 2.0);
            contents(ui)
        }).inner
    }

    pub(crate) fn new(ui: &egui::Ui, ok: egui::Color32, err: egui::Color32) -> Self {
        let v = ui.visuals();
        let dark = v.text_color().intensity() > v.window_fill.intensity();
        let bg =
            if dark { v.window_fill.lerp_to_gamma(egui::Color32::from_rgb(10, 18, 23), 0.65) } else { v.window_fill };
        let fg = if dark { v.text_color().lerp_to_gamma(egui::Color32::WHITE, 0.4) } else { v.text_color() };
        Self {
            bg,
            fg,
            muted: bg.lerp_to_gamma(fg, 0.65),
            border: bg.lerp_to_gamma(fg, 0.16),
            accent: v.hyperlink_color,
            card: bg.lerp_to_gamma(fg, 0.025),
            ok,
            err,
        }
    }

    pub(crate) fn apply_controls(&self, ui: &mut egui::Ui) {
        let style = ui.style_mut();
        style.text_styles.insert(egui::TextStyle::Body, egui::FontId::monospace(13.0));
        style.text_styles.insert(egui::TextStyle::Button, egui::FontId::monospace(13.0));
        style.text_styles.insert(egui::TextStyle::Small, egui::FontId::monospace(12.0));
        style.spacing.item_spacing = egui::vec2(12.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 7.0);
        style.spacing.interact_size.y = 30.0;
        style.spacing.slider_width = 110.0;
        let v = &mut style.visuals;
        v.override_text_color = Some(self.fg);
        v.weak_text_color = Some(self.muted);
        v.panel_fill = self.bg;
        v.window_fill = self.card;
        v.window_stroke = egui::Stroke::new(1.0, self.border);
        v.window_corner_radius = egui::CornerRadius::same(9);
        v.window_shadow = egui::epaint::Shadow::NONE;
        v.extreme_bg_color = self.bg;
        v.faint_bg_color = self.card;
        v.selection.bg_fill = self.bg.lerp_to_gamma(self.accent, 0.18);
        v.selection.stroke = egui::Stroke::new(1.0, self.accent);
        v.widgets.noninteractive.fg_stroke.color = self.fg;
        v.widgets.noninteractive.bg_stroke.color = self.border;
        for (widget, amount) in [
            (&mut v.widgets.inactive, 0.03),
            (&mut v.widgets.hovered, 0.10),
            (&mut v.widgets.active, 0.18),
            (&mut v.widgets.open, 0.13),
        ] {
            widget.corner_radius = egui::CornerRadius::same(5);
            widget.bg_fill = self.bg.lerp_to_gamma(self.accent, amount);
            widget.weak_bg_fill = widget.bg_fill;
            widget.bg_stroke = egui::Stroke::new(1.0, if amount > 0.03 { self.accent } else { self.border });
            widget.fg_stroke.color = if amount > 0.03 { self.accent } else { self.fg };
        }
    }

    pub(crate) fn status(&self, done: bool, exit: Option<i32>) -> egui::Color32 {
        if !done {
            self.accent
        } else if exit.is_some_and(|e| e != 0) {
            self.err
        } else {
            self.ok
        }
    }
}

/// Shared action treatment: unframed at rest, accent outline on hover/focus,
/// muted disabled state, optional vector icon and right-aligned shortcut.
pub(crate) fn action_row(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: egui::Id,
    label: &str,
    shortcut: &str,
    glyph: Option<Icon>,
    enabled: bool,
    c: &TermStyle,
) -> egui::Response {
    action_row_tinted(ui, rect, id, label, shortcut, glyph, enabled, c, None)
}

/// As `action_row`, with the glyph forced to an identity colour: a workspace
/// row's marker carries the workspace's own colour, which is not the row's
/// accent. Text, hover and shortcut treatment stay shared.
pub(crate) fn action_row_tinted(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: egui::Id,
    label: &str,
    shortcut: &str,
    glyph: Option<Icon>,
    enabled: bool,
    c: &TermStyle,
    tint: Option<egui::Color32>,
) -> egui::Response {
    let response = ui.interact(rect, id, if enabled { egui::Sense::click() } else { egui::Sense::hover() });
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    let highlight = enabled && (response.hovered() || response.has_focus());
    if highlight {
        ui.painter().rect(rect, 5.0, c.bg.lerp_to_gamma(c.accent, 0.14),
            egui::Stroke::new(1.0, c.accent), egui::StrokeKind::Inside);
    }
    let color = if !enabled { c.bg.lerp_to_gamma(c.muted, 0.65) } else if highlight { c.accent } else { c.fg };
    if let Some(glyph) = glyph {
        icon(ui.painter(), glyph, egui::pos2(rect.left() + 18.0, rect.center().y), tint.unwrap_or(color));
    }
    let shortcut_width = if shortcut.is_empty() { 0.0 } else {
        (ui.painter().layout_no_wrap(shortcut.to_string(), egui::FontId::monospace(12.0), c.muted).size().x + 20.0)
            .min(rect.width() * 0.52)
    };
    text(ui, rect.with_min_x(rect.left() + if glyph.is_some() { 39.0 } else { 10.0 })
        .with_max_x(rect.right() - shortcut_width - 10.0), label, color, 13.0, false);
    if !shortcut.is_empty() {
        text(ui, rect.with_min_x(rect.right() - shortcut_width).with_max_x(rect.right() - 10.0),
            shortcut, if enabled { c.muted } else { color }, 12.0, false);
    }
    if enabled { response.on_hover_cursor(egui::CursorIcon::PointingHand) } else { response }
}

pub(crate) fn menu_item(
    ui: &mut egui::Ui, label: &str, shortcut: &str, glyph: Option<Icon>, enabled: bool, colors: &TermStyle,
) -> egui::Response {
    let (id, rect) = ui.allocate_space(egui::vec2(ui.available_width(), 32.0));
    action_row(ui, rect, id, label, shortcut, glyph, enabled, colors)
}

pub(crate) fn menu_separator(ui: &mut egui::Ui, colors: &TermStyle) {
    let (_, rect) = ui.allocate_space(egui::vec2(ui.available_width(), 10.0));
    ui.painter().hline(rect.shrink2(egui::vec2(8.0, 0.0)).x_range(), rect.center().y,
        egui::Stroke::new(1.0, colors.border));
}

/// Shared motion policy. egui owns interpolation, interruption and repainting;
/// components supply only stable identities and target states/geometry.
const MOTION_SECONDS: f32 = 0.16;

pub(crate) fn apply_motion(ctx: &egui::Context, speed: f32) {
    let speed = if speed.is_finite() { speed.clamp(0.0, 4.0) } else { 1.0 };
    let seconds = if speed > 0.0 { MOTION_SECONDS / speed.max(0.25) } else { 0.0 };
    if ctx.global_style().animation_time != seconds {
        ctx.global_style_mut(|style| style.animation_time = seconds);
    }
}

pub(crate) fn transition(ctx: &egui::Context, id: egui::Id, target: bool) -> f32 {
    ctx.animate_bool_with_time_and_easing(id, target, ctx.global_style().animation_time, egui::emath::easing::cubic_out)
}

pub(crate) fn animated_rect(ctx: &egui::Context, id: egui::Id, target: egui::Rect) -> egui::Rect {
    let now = ctx.input(|i| i.time);
    let reset = ctx.data_mut(|data| {
        let old = data.get_temp::<f64>(id);
        data.insert_temp(id, now);
        old.is_none_or(|seen| now - seen > 0.3)
    });
    let duration = if reset { 0.0 } else { ctx.global_style().animation_time };
    let x = ctx.animate_value_with_time(id.with("x"), target.left(), duration);
    let y = ctx.animate_value_with_time(id.with("y"), target.top(), duration);
    let width = ctx.animate_value_with_time(id.with("width"), target.width(), duration);
    let height = ctx.animate_value_with_time(id.with("height"), target.height(), duration);
    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(width, height))
}

/// Keep an animated selection visible when a scroll viewport changes.
pub(crate) fn animated_rect_in(ctx: &egui::Context, id: egui::Id, target: egui::Rect, viewport: egui::Rect) -> egui::Rect {
    let rect = animated_rect(ctx, id, target);
    let min = egui::pos2(
        rect.left().clamp(viewport.left(), (viewport.right() - rect.width()).max(viewport.left())),
        rect.top().clamp(viewport.top(), (viewport.bottom() - rect.height()).max(viewport.top())),
    );
    egui::Rect::from_min_size(min, rect.size()).intersect(viewport)
}

/// Shared bitmap well for switcher and hover previews; always keeps aspect.
pub(crate) fn preview_image(ui: &egui::Ui, rect: egui::Rect, image: Option<(egui::TextureId, (u32, u32))>, c: &TermStyle) {
    ui.painter().rect_filled(rect, 7.0, c.bg);
    if let Some((texture, (w, h))) = image {
        let native = egui::vec2(w.max(1) as f32, h.max(1) as f32);
        let scale = (rect.width() / native.x).min(rect.height() / native.y);
        let size = native * scale;
        egui::Image::new(egui::load::SizedTexture::new(texture, native))
            .fit_to_exact_size(size).corner_radius(7.0)
            .paint_at(ui, egui::Rect::from_center_size(rect.center(), size));
    } else {
        text(ui, rect.shrink(10.0), "Preparing preview…", c.muted, 12.0, false);
    }
    ui.painter().rect_stroke(rect, 7.0, egui::Stroke::new(1.0, c.border), egui::StrokeKind::Inside);
}

/// A single-line galley with an ellipsis, so long commands never move the
/// clock column or enlarge the sidebar. The full command remains in a tooltip.
pub(crate) fn text(ui: &egui::Ui, rect: egui::Rect, text: &str, color: egui::Color32, size: f32, strong: bool) {
    draw_text(ui, rect, text, color, size, strong, Place::Left)
}

/// As `text`, centred in the rect. Indicator cells have no left edge to hang a
/// label from: a workspace number reads as a mark, so it sits in the middle of
/// the space the strip gave it.
pub(crate) fn text_centered(ui: &egui::Ui, rect: egui::Rect, text: &str, color: egui::Color32, size: f32, strong: bool) {
    draw_text(ui, rect, text, color, size, strong, Place::Center)
}

/// As `text`, flush with the rect's right edge. A line's left side names the
/// thing and its right side carries the status, which is why the switcher header
/// puts its workspace there: the count stays where the eye starts and the answer
/// to "which one" waits at the end.
pub(crate) fn text_right(ui: &egui::Ui, rect: egui::Rect, text: &str, color: egui::Color32, size: f32, strong: bool) {
    draw_text(ui, rect, text, color, size, strong, Place::Right)
}

/// How wide `text` would be in the font `text*` draws with. Measuring rather than
/// estimating lets a label decide its own form instead of a hard-coded guess about
/// the font's advance.
pub(crate) fn text_width(ui: &egui::Ui, text: &str, size: f32) -> f32 {
    egui::WidgetText::from(egui::RichText::new(text).font(egui::FontId::monospace(size)))
        .into_galley(ui, None, f32::INFINITY, egui::FontId::monospace(size))
        .size()
        .x
}

/// Where text sits inside its rect when the rect is wider than the glyphs.
enum Place {
    Left,
    Center,
    Right,
}

fn draw_text(ui: &egui::Ui, rect: egui::Rect, text: &str, color: egui::Color32, size: f32, strong: bool, place: Place) {
    if rect.width() <= 0.0 {
        return;
    }
    let mut text = egui::RichText::new(text).font(egui::FontId::monospace(size)).color(color);
    if strong {
        text = text.strong();
    }
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        rect.width(),
        egui::FontId::monospace(size),
    );
    let x = match place {
        Place::Left => rect.left(),
        Place::Center => (rect.center().x - galley.size().x * 0.5).max(rect.left()),
        Place::Right => (rect.right() - galley.size().x).max(rect.left()),
    };
    let pos = egui::pos2(x, rect.center().y - galley.size().y * 0.5);
    ui.painter().with_clip_rect(ui.clip_rect().intersect(rect)).galley(pos, galley, color);
}

#[derive(Clone, Copy)]
pub(crate) enum Icon {
    Archive,
    Terminal,
    Plus,
    Settings,
    Dismiss,
    Layers,
    Search,
    Filter,
    ChevronLeft,
    ChevronRight,
    More,
    Clock,
    Check,
    Error,
    Close,
    Copy,
    Pipe,
    Rerun,
    View,
    Tombstone,
}

pub(crate) fn icon_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    glyph: Icon,
    label: &str,
    c: &TermStyle,
) -> egui::Response {
    record_control(ui, label, rect);
    let response = ui.interact(rect, ui.scope_id().with(label), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    if response.hovered() || response.has_focus() {
        ui.painter().rect(rect, 16.0, c.card, egui::Stroke::new(1.0, c.border), egui::StrokeKind::Inside);
    }
    icon(ui.painter(), glyph, rect.center(), if response.hovered() || response.has_focus() { c.accent } else { c.fg });
    response.on_hover_text(label).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Row-level control revealed with its row: `reveal` (0..1) fades it in, it
/// rests muted, and takes the standard hot treatment on hover. Inert while
/// faded out, so a hidden button never eats a click meant for the row.
pub(crate) fn icon_button_faded(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    glyph: Icon,
    label: &str,
    c: &TermStyle,
    reveal: f32,
) -> egui::Response {
    let live = reveal > 0.5;
    let response = ui.interact(
        rect,
        ui.scope_id().with(label),
        if live { egui::Sense::click() } else { egui::Sense::hover() },
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, live, label));
    if reveal <= 0.01 {
        return response;
    }
    let hot = response.hovered() || response.has_focus();
    if hot {
        ui.painter().rect(rect, 16.0, c.card, egui::Stroke::new(1.0, c.border), egui::StrokeKind::Inside);
    }
    let rest = egui::Color32::from_rgba_unmultiplied(c.muted.r(), c.muted.g(), c.muted.b(), (c.muted.a() as f32 * reveal) as u8);
    icon(ui.painter(), glyph, rect.center(), if hot { c.accent } else { rest });
    response.on_hover_text(label).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Workspace identities use vectors so every choice works with every terminal font.
pub(crate) fn workspace_icon(p: &egui::Painter, glyph: crate::config::WorkspaceIcon, center: egui::Pos2, color: egui::Color32) {
    use crate::config::WorkspaceIcon as W;
    let stroke = egui::Stroke::new(1.5, color);
    let pt = |x, y| center + egui::vec2(x, y);
    let line = |points: &[(f32, f32)]| p.add(egui::Shape::line(points.iter().map(|&(x, y)| pt(x, y)).collect(), stroke));
    match glyph {
        W::Layers => icon(p, Icon::Layers, center, color),
        W::Terminal => icon(p, Icon::Terminal, center, color),
        W::Code => {
            line(&[(-4.0, -5.0), (-9.0, 0.0), (-4.0, 5.0)]);
            line(&[(4.0, -5.0), (9.0, 0.0), (4.0, 5.0)]);
            line(&[(2.0, -8.0), (-2.0, 8.0)]);
        }
        W::Grid => for x in [-8.0, 2.0] { for y in [-8.0, 2.0] {
            p.rect_stroke(egui::Rect::from_min_size(pt(x, y), egui::vec2(6.0, 6.0)), 1.0, stroke, egui::StrokeKind::Inside);
        } },
        W::Cube => {
            line(&[(0.0, -9.0), (8.0, -5.0), (8.0, 5.0), (0.0, 9.0), (-8.0, 5.0), (-8.0, -5.0), (0.0, -9.0)]);
            line(&[(-8.0, -5.0), (0.0, 0.0), (8.0, -5.0)]);
            line(&[(0.0, 0.0), (0.0, 9.0)]);
        }
        W::Branch => {
            line(&[(-5.0, -5.0), (-5.0, 5.0)]);
            line(&[(-5.0, 2.0), (5.0, -2.0), (5.0, -5.0)]);
            for (x, y) in [(-5.0, -7.0), (-5.0, 7.0), (5.0, -7.0)] { p.circle_stroke(pt(x, y), 2.0, stroke); }
        }
        W::Database => {
            p.add(egui::epaint::EllipseShape::stroke(pt(0.0, -6.0), egui::vec2(8.0, 3.0), stroke));
            line(&[(-8.0, -6.0), (-8.0, 6.0), (-4.0, 8.0), (4.0, 8.0), (8.0, 6.0), (8.0, -6.0)]);
            line(&[(-8.0, 0.0), (-4.0, 2.0), (4.0, 2.0), (8.0, 0.0)]);
        }
        W::Flask => {
            line(&[(-4.0, -9.0), (4.0, -9.0)]);
            line(&[(-3.0, -9.0), (-3.0, -2.0), (-8.0, 6.0), (-7.0, 8.0), (7.0, 8.0), (8.0, 6.0), (3.0, -2.0), (3.0, -9.0)]);
            line(&[(-5.0, 3.0), (5.0, 3.0)]);
        }
        W::Stack => for y in [-7.0, -1.0, 5.0] {
            p.rect_stroke(egui::Rect::from_min_size(pt(-8.0, y), egui::vec2(16.0, 4.0)), 1.0, stroke, egui::StrokeKind::Inside);
        },
        W::Sliders => for (x, y) in [(-7.0, 3.0), (0.0, -3.0), (7.0, 1.0)] {
            line(&[(x, -8.0), (x, y - 2.0)]);
            line(&[(x, y + 2.0), (x, 8.0)]);
            p.circle_stroke(pt(x, y), 2.0, stroke);
        },
    }
}

/// Small vector icons avoid missing-glyph boxes in the bundled egui font.
pub(crate) fn icon(p: &egui::Painter, icon: Icon, center: egui::Pos2, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.4, color);
    let pt = |x, y| center + egui::vec2(x, y);
    let line =
        |points: &[(f32, f32)]| p.add(egui::Shape::line(points.iter().map(|&(x, y)| pt(x, y)).collect(), stroke));
    match icon {
        Icon::Archive => {
            p.rect_stroke(egui::Rect::from_center_size(pt(0.0, 2.0), egui::vec2(18.0, 15.0)),
                2.0, stroke, egui::StrokeKind::Inside);
            p.rect_stroke(egui::Rect::from_center_size(pt(0.0, -6.0), egui::vec2(21.0, 4.0)),
                1.0, stroke, egui::StrokeKind::Inside);
            line(&[(-3.0, -1.0), (3.0, -1.0)]);
        }
        Icon::Plus => {
            line(&[(-6.0, 0.0), (6.0, 0.0)]);
            line(&[(0.0, -6.0), (0.0, 6.0)]);
        }
        Icon::Terminal => {
            p.rect_stroke(
                egui::Rect::from_center_size(center, egui::vec2(18.0, 16.0)),
                3.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            line(&[(-5.0, -4.0), (-1.0, 0.0), (-5.0, 4.0)]);
            line(&[(1.0, 4.0), (5.0, 4.0)]);
        }
        Icon::Settings => {
            p.circle_stroke(center, 5.5, stroke);
            p.circle_stroke(center, 2.0, stroke);
            for i in 0..8 {
                let angle = i as f32 * std::f32::consts::TAU / 8.0;
                let d = egui::vec2(angle.cos(), angle.sin());
                p.line_segment([center + d * 5.5, center + d * 8.5], stroke);
            }
        }
        Icon::Dismiss => {
            line(&[(-5.0, -5.0), (5.0, 5.0)]);
            line(&[(-5.0, 5.0), (5.0, -5.0)]);
        }

        Icon::Layers => {
            for y in [-5.0, 0.0, 5.0] {
                line(&[(-8.0, y), (0.0, y + 4.0), (8.0, y)]);
            }
            line(&[(-8.0, -5.0), (0.0, -9.0), (8.0, -5.0)]);
        }
        Icon::ChevronLeft => { line(&[(2.0, -4.0), (-2.0, 0.0), (2.0, 4.0)]); }
        Icon::ChevronRight => { line(&[(-2.0, -4.0), (2.0, 0.0), (-2.0, 4.0)]); }
        Icon::Filter => {
            line(&[(-7.0, -6.0), (7.0, -6.0), (2.0, 0.0), (2.0, 6.0), (-2.0, 8.0), (-2.0, 0.0), (-7.0, -6.0)]);
        }
        Icon::Search => {
            p.circle_stroke(pt(-2.0, -2.0), 6.0, stroke);
            line(&[(2.5, 2.5), (8.0, 8.0)]);
        }
        Icon::More => {
            for x in [-6.0, 0.0, 6.0] {
                p.circle_filled(pt(x, 0.0), 1.3, color);
            }
        }
        Icon::Clock => {
            p.circle_stroke(center, 7.0, stroke);
            line(&[(0.0, -4.5), (0.0, 0.0), (3.5, 2.0)]);
        }
        Icon::Check => {
            line(&[(-4.0, 0.0), (-1.0, 3.0), (5.0, -4.0)]);
        }
        Icon::Error => {
            p.circle_stroke(center, 7.0, stroke);
            line(&[(0.0, -4.0), (0.0, 1.0)]);
            p.circle_filled(pt(0.0, 4.0), 0.8, color);
        }
        Icon::Close => {
            line(&[(-2.5, -2.5), (2.5, 2.5)]);
            line(&[(-2.5, 2.5), (2.5, -2.5)]);
        }
        Icon::Copy => {
            p.rect_stroke(
                egui::Rect::from_min_max(pt(-6.0, -3.0), pt(3.0, 7.0)),
                1.0,
                stroke,
                egui::StrokeKind::Middle,
            );
            line(&[(-2.0, -4.0), (-2.0, -7.0), (7.0, -7.0), (7.0, 3.0), (5.0, 3.0)]);
        }
        Icon::Pipe => {
            line(&[(-5.0, -7.0), (-5.0, 7.0)]);
            line(&[(-5.0, 2.0), (4.0, -1.0), (4.0, -7.0)]);
            line(&[(1.0, -4.0), (4.0, -7.0), (7.0, -4.0)]);
            p.circle_filled(pt(-5.0, 7.0), 1.8, color);
        }
        Icon::Rerun => {
            line(&[
                (5.0, -3.0),
                (2.0, -7.0),
                (-4.0, -6.0),
                (-7.0, -1.0),
                (-6.0, 4.0),
                (-1.0, 7.0),
                (4.0, 5.0),
                (7.0, 1.0),
            ]);
            line(&[(5.0, -7.0), (5.0, -3.0), (1.0, -3.0)]);
        }
        Icon::View => {
            p.rect_stroke(
                egui::Rect::from_center_size(center, egui::vec2(13.0, 13.0)),
                1.0,
                stroke,
                egui::StrokeKind::Middle,
            );
            line(&[(-3.0, -2.0), (0.0, 0.0), (-3.0, 2.0)]);
            line(&[(1.0, 3.0), (4.0, 3.0)]);
        }
        Icon::Tombstone => {
            // Slab with a domed top, seated on a ground line.
            line(&[
                (-5.0, 7.0),
                (-5.0, -2.0),
                (-3.5, -5.5),
                (0.0, -7.0),
                (3.5, -5.5),
                (5.0, -2.0),
                (5.0, 7.0),
            ]);
            line(&[(-8.0, 7.0), (8.0, 7.0)]);
            line(&[(-2.0, -2.0), (2.0, -2.0)]);
        }
    }
}

pub fn theme(cfg: &crate::config::Config, bg: [f32; 3], fg: [f32; 3]) -> egui::Visuals {
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| -> egui::Color32 {
        let c = |i: usize| ((a[i] * (1.0 - t) + b[i] * t) * 255.0).round().clamp(0.0, 255.0) as u8;
        egui::Color32::from_rgb(c(0), c(1), c(2))
    };
    // bg/fg are the terminal's *effective* colours (theme, palette and
    // overrides all resolved upstream) - exactly what the screen paints
    // with. cfg.background_color alone would ignore a themed session and
    // render the UI in default-dark while the terminal wore gruvbox.
    let accent = cfg.accent_color.to_f32();
    let tablist = cfg.tablist_background.to_f32();
    let black = [0.0f32; 3];
    let mut v = egui::Visuals::dark();
    // Panels sit between the terminal background and the tab-strip colour;
    // windows (config panel, popups) nudge up from the background so they
    // read as raised without needing an opinionated grey.
    v.panel_fill = mix(bg, tablist, 0.5);
    v.window_fill = mix(bg, fg, 0.05);
    v.window_stroke = egui::Stroke::new(1.0, mix(bg, fg, 0.25));
    v.faint_bg_color = mix(bg, fg, 0.08);
    // Text inputs darker than the background: they must read as wells.
    v.extreme_bg_color = mix(bg, black, 0.35);
    v.hyperlink_color = mix(accent, fg, 0.2);
    v.selection.bg_fill = mix(bg, accent, 0.4);
    v.selection.stroke = egui::Stroke::new(1.0, mix(bg, fg, 0.9));
    {
        let mut w = v.widgets.clone();
        let base = mix(bg, fg, 0.06);
        w.inactive.bg_fill = base;
        w.inactive.weak_bg_fill = base;
        w.inactive.bg_stroke = egui::Stroke::new(1.0, mix(bg, fg, 0.2));
        w.hovered.bg_fill = mix(bg, accent, 0.18);
        w.hovered.weak_bg_fill = mix(bg, accent, 0.18);
        w.hovered.bg_stroke = egui::Stroke::new(1.0, mix(bg, accent, 0.6));
        w.active.bg_fill = mix(bg, accent, 0.3);
        w.active.weak_bg_fill = mix(bg, accent, 0.3);
        w.active.bg_stroke = egui::Stroke::new(1.0, mix(bg, accent, 0.8));
        w.open.bg_fill = base;
        w.open.weak_bg_fill = base;
        w.open.bg_stroke = egui::Stroke::new(1.0, mix(bg, accent, 0.6));
        v.widgets = w;
    }
    {
        let non = &mut v.widgets.noninteractive;
        non.fg_stroke = egui::Stroke::new(
            1.0,
            egui::Color32::from_rgb(
                (fg[0].clamp(0.0, 1.0) * 255.0) as u8,
                (fg[1].clamp(0.0, 1.0) * 255.0) as u8,
                (fg[2].clamp(0.0, 1.0) * 255.0) as u8,
            ),
        );
        // 0.37 derives weak/disabled text by tinting fg_stroke towards
        // weak_bg_fill, so setting the base above themes all three at once.
    }
    v
}

/// Convert a config RGB to egui colour (clamped: user YAML can be bold).
pub fn rgb_c32(rgb: crate::config::Rgb) -> egui::Color32 {
    let c = rgb.to_f32();
    egui::Color32::from_rgb(
        (c[0].clamp(0.0, 1.0) * 255.0) as u8,
        (c[1].clamp(0.0, 1.0) * 255.0) as u8,
        (c[2].clamp(0.0, 1.0) * 255.0) as u8,
    )
}

/// Shared endings for settings and its font picker: a neutral Cancel and
/// an accent-outlined primary action, matching history's selected actions.
pub(crate) fn actions(ui: &mut egui::Ui, width: f32, primary: &str, cancel: &str) -> (bool, bool) {
    let accent = ui.visuals().hyperlink_color;
    let base = ui.visuals().panel_fill;
    let border = ui.visuals().widgets.inactive.bg_stroke;
    let half = ((width - 10.0) / 2.0).max(0.0);
    let mut result = (false, false);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let cancel_response = ui.add(egui::Button::new(cancel).min_size(egui::vec2(half, 34.0))
            .fill(base).stroke(border).corner_radius(5.0));
        record_control(ui, cancel, cancel_response.rect);
        result.1 = cancel_response.clicked();
        let primary_response = ui.add(
            egui::Button::new(egui::RichText::new(primary).color(accent)).min_size(egui::vec2(half, 34.0))
                .fill(base.lerp_to_gamma(accent, 0.14)).stroke(egui::Stroke::new(1.0, accent)).corner_radius(5.0));
        record_control(ui, primary, primary_response.rect);
        result.0 = primary_response.clicked();
    });
    result
}

/// Measure the same component that will be painted, without registering live
/// interactions. Callers supply a width; the child owns its natural height.
pub(crate) fn measure_section(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, width: f32,
    contents: impl FnOnce(&mut egui::Ui)) -> egui::Vec2 {
    let mut child = ui.new_child(egui::UiBuilder::new().id_salt(("measure", id))
        .max_rect(egui::Rect::from_min_size(ui.cursor().min, egui::vec2(width, f32::INFINITY)))
        .layout(egui::Layout::top_down(egui::Align::Min)).sizing_pass().invisible());
    child.set_width(width);
    contents(&mut child);
    child.min_size()
}

/// Paint a naturally sized footer at the bottom of a bounded container and
/// return its remaining body rectangle. No caller needs to know footer internals.
pub(crate) fn bottom_section<R>(ui: &mut egui::Ui, id: impl std::hash::Hash + Copy + std::fmt::Debug,
    mut contents: impl FnMut(&mut egui::Ui) -> R) -> (egui::Rect, R) {
    let available = ui.available_rect_before_wrap();
    let size = measure_section(ui, id, available.width(), |ui| { contents(ui); });
    let footer = available.with_min_y((available.bottom() - size.y).max(available.top()));
    let body = available.with_max_y(footer.top());
    let mut child = ui.new_child(egui::UiBuilder::new().id_salt(id).max_rect(footer));
    child.set_clip_rect(ui.clip_rect().intersect(footer));
    let result = contents(&mut child);
    ui.expand_to_include_rect(footer);
    (body, result)
}

#[cfg(test)]
pub(crate) fn control_rect(ctx: &egui::Context, name: &str) -> Option<egui::Rect> {
    ctx.data_mut(|data| data.get_temp(egui::Id::unique("control_geometry").with(name)))
}

/// Actual, clipped control bounds for diagnostics and interaction tests.
/// Coordinates in the log are native pixels, like the application's pointer.
pub(crate) fn record_control(ui: &egui::Ui, name: impl Into<String>, rect: egui::Rect) {
    static LOG_GEOMETRY: std::sync::LazyLock<bool> = std::sync::LazyLock::new(||
        std::env::var_os("MYPIXELS_UI_GEOMETRY").is_some());
    if !cfg!(test) && !*LOG_GEOMETRY { return; }
    if ui.is_sizing_pass() || !ui.is_visible() { return; }
    let name = name.into();
    let rect = rect.intersect(ui.clip_rect());
    if !rect.is_positive() { return; }
    let pixels_per_point = ui.pixels_per_point();
    ui.ctx().data_mut(|data| {
        let id = egui::Id::unique("control_geometry").with(&name);
        if data.get_temp::<egui::Rect>(id) != Some(rect) {
            data.insert_temp(id, rect);
            if *LOG_GEOMETRY {
                let rect = rect * pixels_per_point;
                log::info!("ui control {name}: [{:.2}, {:.2}, {:.2}, {:.2}]", rect.left(), rect.top(), rect.right(), rect.bottom());
            }
        }
    });
}

pub(crate) fn section(ui: &mut egui::Ui, title: &str, colors: &TermStyle, contents: impl FnOnce(&mut egui::Ui)) {
    let width = ui.available_width();
    let frame = egui::Frame::new()
        .fill(colors.card)
        .stroke(egui::Stroke::new(1.0, colors.border))
        .corner_radius(9.0)
        .inner_margin(14.0);
    frame.show(ui, |ui| {
            ui.set_width((width - frame.total_margin().sum().x).max(0.0));
            ui.label(egui::RichText::new(title).font(egui::FontId::monospace(15.0)).strong().color(colors.fg));
            ui.add_space(6.0);
            contents(ui);
        });
}

pub(crate) fn form_grid(id: &str) -> egui::Grid {
    egui::Grid::new(id).num_columns(2).spacing(egui::vec2(12.0, 4.0)).min_row_height(32.0)
}

/// Compact chrome stays readable while reserving terminal rows.
pub(crate) fn tab_height(text_height: f32, scale: f32, compact: bool) -> f32 {
    if compact {
        // One physical pixel above and below the text, without the normal
        // strip's minimum height imposing extra space on small tab fonts.
        text_height.ceil() + 2.0
    } else {
        (text_height + 8.0 * scale).max(28.0 * scale).round()
    }
}

/// Choose the greater contrast ratio against an opaque background.
pub(crate) fn black_or_white_text(background: egui::Color32) -> egui::Color32 {
    let linear = egui::Rgba::from(background);
    let luminance = 0.2126 * linear.r() + 0.7152 * linear.g() + 0.0722 * linear.b();
    if (luminance + 0.05) / 0.05 >= 1.05 / (luminance + 0.05) {
        egui::Color32::BLACK
    } else {
        egui::Color32::WHITE
    }
}

/// Keep theme text when readable; otherwise choose black or white for the fill.
fn contrasting_text(foreground: egui::Color32, background: egui::Color32) -> egui::Color32 {
    let luminance = |color: egui::Color32| {
        let linear = egui::Rgba::from(color);
        0.2126 * linear.r() + 0.7152 * linear.g() + 0.0722 * linear.b()
    };
    let bg = luminance(background);
    let fg = luminance(background.blend(foreground));
    if (fg.max(bg) + 0.05) / (fg.min(bg) + 0.05) >= 4.5 {
        foreground
    } else if (bg + 0.05) / 0.05 >= 1.05 / (bg + 0.05) {
        egui::Color32::BLACK
    } else {
        egui::Color32::WHITE
    }
}

impl TermStyle {
    pub(crate) fn surface(&self, p: &egui::Painter, rect: egui::Rect, radius: f32, selected: bool, hovered: bool) {
        let fill = self.bg.lerp_to_gamma(
            if selected { self.accent } else { self.fg },
            if selected {
                0.11
            } else if hovered {
                0.09
            } else {
                0.055
            },
        );
        p.rect(
            rect,
            radius,
            fill,
            egui::Stroke::new(if selected { 1.5 } else { 1.0 }, if selected { self.accent } else { self.border }),
            egui::StrokeKind::Inside,
        );
    }

    pub(crate) fn number_badge(&self, p: &egui::Painter, rect: egui::Rect, number: usize, selected: bool, size: f32) {
        let fill = if selected {
            self.bg.blend(self.accent.gamma_multiply(0.8))
        } else {
            self.bg.lerp_to_gamma(self.fg, 0.14)
        };
        p.rect(
            rect,
            4.0,
            fill,
            egui::Stroke::new(1.0, self.border),
            egui::StrokeKind::Inside,
        );
        p.text(rect.center(), egui::Align2::CENTER_CENTER, number.to_string(), egui::FontId::monospace(size),
            contrasting_text(self.fg, fill));
    }
}

/// Truncated, vertically centered labels using the configured tab face or UI font.
pub(crate) fn text_with_font(ui: &egui::Ui, rect: egui::Rect, label: &str, color: egui::Color32, font: egui::FontId) {
    if rect.width() <= 0.0 {
        return;
    }
    let galley = egui::WidgetText::from(egui::RichText::new(label).font(font.clone()).color(color)).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        rect.width(),
        font,
    );
    ui.painter().with_clip_rect(ui.clip_rect().intersect(rect)).galley(
        egui::pos2(rect.left(), rect.center().y - galley.size().y * 0.5),
        galley,
        color,
    );
}

/// Tab-specific state colors resolved alongside the shared UI palette.
/// Legacy defaults adopt the new chrome; explicit color overrides still work.
pub(crate) struct TabStyle {
    pub base: TermStyle,
    pub foreground: egui::Color32,
    pub selected_foreground: egui::Color32,
    pub entangle: egui::Color32,
    pub index: egui::Color32,
    pub add: egui::Color32,
    background: egui::Color32,
    selected_background: egui::Color32,
}

impl TabStyle {
    pub fn new(ui: &egui::Ui, cfg: &crate::config::Config, workspace: usize) -> Self {
        static DEFAULTS: std::sync::LazyLock<crate::config::Config> =
            std::sync::LazyLock::new(crate::config::Config::default);
        let defaults = &*DEFAULTS;
        let resolve = |value, default, fallback| if value == default { fallback } else { rgb_c32(value) };
        let mut base = TermStyle::new(ui, rgb_c32(cfg.col[2]), rgb_c32(cfg.col[1]));
        if cfg.tablist_background != defaults.tablist_background {
            base.bg = rgb_c32(cfg.tablist_background);
            base.card = base.bg.lerp_to_gamma(base.fg, 0.025);
            base.border = base.bg.lerp_to_gamma(base.fg, 0.16);
            base.muted = base.bg.lerp_to_gamma(base.fg, 0.65);
        }
        // With workspaces off, tabs use the neutral accent (col[2]) instead
        // of a workspace identity color.
        let color = if cfg.workspaces_enabled {
            rgb_c32(cfg.workspace_colors.get(workspace).copied().unwrap_or(cfg.workspace_colors[0]))
        } else {
            base.accent
        };
        base.accent = color;
        let background = base.bg.lerp_to_gamma(color, 0.18);
        let selected_background = base.bg.lerp_to_gamma(color, 0.42);
        let foreground = contrasting_text(color.lerp_to_gamma(base.fg, 0.75), background);
        Self {
            foreground,
            selected_foreground: contrasting_text(color.lerp_to_gamma(base.fg, 0.85), selected_background),
            background,
            selected_background,
            entangle: color,
            index: foreground,
            add: resolve(cfg.add_button_color, defaults.add_button_color, base.muted),
            base,
        }
    }

    pub fn tab(&self, p: &egui::Painter, rect: egui::Rect, selected: f32, hovered: bool) {
        let background = self.background.lerp_to_gamma(self.selected_background, selected);
        p.rect(
            rect,
            5.0,
            if hovered { background.lerp_to_gamma(self.base.fg, 0.035) } else { background },
            egui::Stroke::new(1.0, self.base.bg.lerp_to_gamma(self.base.accent, 0.45)),
            egui::StrokeKind::Inside,
        );
    }
}

pub(crate) fn picker_style(ui: &egui::Ui) -> egui::Style {
    let mut style = (**ui.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 4.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    style.spacing.interact_size.y = 24.0;
    style.wrap_mode = Some(egui::TextWrapMode::Truncate);
    style
}

impl TermStyle {
    pub(crate) fn card_surface(&self, p: &egui::Painter, rect: egui::Rect, selected: bool, hovered: bool) {
        let fill = if selected {
            self.bg.lerp_to_gamma(self.accent, 0.11)
        } else if hovered {
            self.bg.lerp_to_gamma(self.fg, 0.09)
        } else {
            self.bg.lerp_to_gamma(self.fg, 0.055)
        };
        if selected {
            p.rect_stroke(
                rect.expand(2.0),
                12.0,
                egui::Stroke::new(1.5, self.accent.gamma_multiply(0.35)),
                egui::StrokeKind::Outside,
            );
        }
        p.rect(
            rect,
            11.0,
            fill,
            egui::Stroke::new(if selected { 2.5 } else { 1.0 }, if selected { self.accent } else { self.border }),
            egui::StrokeKind::Inside,
        );
    }
}

pub(crate) fn toggle(ui: &mut egui::Ui, label: &str, v: &mut bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(40.0, 30.0), egui::Sense::click());
    let changed = response.clicked();
    if changed {
        *v = !*v;
    }
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), *v, label));
    let accent = ui.visuals().hyperlink_color;
    let track = egui::Rect::from_center_size(rect.center(), egui::vec2(36.0, 20.0));
    let border = if *v || response.hovered() || response.has_focus() {
        egui::Stroke::new(1.0, accent)
    } else {
        ui.visuals().widgets.inactive.bg_stroke
    };
    let fill = if *v { ui.visuals().panel_fill.lerp_to_gamma(accent, 0.24) } else { ui.visuals().panel_fill };
    ui.painter().rect(track, 10.0, fill, border, egui::StrokeKind::Inside);
    ui.painter().circle_filled(
        egui::pos2(track.left() + if *v { 26.0 } else { 10.0 }, track.center().y),
        6.0,
        if *v { accent } else { ui.visuals().weak_text_color() },
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand);
    changed
}

#[cfg(test)]
mod motion_tests {
    use super::*;

    fn workspace_style(cfg: &crate::config::Config, workspace: usize, light: bool) -> TabStyle {
        let ctx = egui::Context::default();
        if light { ctx.set_visuals(egui::Visuals::light()); }
        let mut style = None;
        let mut full = ctx.run_ui(egui::RawInput::default(), |ui| {
            style = Some(TabStyle::new(ui, cfg, workspace));
        });
        full.textures_delta.clear();
        style.unwrap()
    }

    #[test]
    fn workspace_tab_styles_follow_only_their_workspace_color() {
        let mut cfg = crate::config::Config::default();
        cfg.workspaces_enabled = true;
        let first = workspace_style(&cfg, 0, false);
        let second = workspace_style(&cfg, 1, false);
        assert_ne!(first.background, second.background);
        assert_ne!(first.selected_background, second.selected_background);
        assert_ne!(first.base.accent, second.base.accent);
        cfg.workspace_colors[1] = crate::config::Rgb(30, 200, 80);
        let changed = workspace_style(&cfg, 1, false);
        assert_ne!(changed.background, second.background);
        assert_ne!(changed.selected_background, second.selected_background);
        assert_eq!(changed.base.accent, rgb_c32(cfg.workspace_colors[1]));
        assert_eq!(workspace_style(&cfg, 0, false).selected_background, first.selected_background);
        // Legacy global tab colors must not override workspace-derived chrome.
        cfg.tab_color_bg = crate::config::Rgb(255, 0, 0);
        cfg.tab_color_bg_selected = crate::config::Rgb(0, 255, 0);
        cfg.tab_color_fg = crate::config::Rgb(0, 0, 255);
        cfg.tab_color_fg_selected = crate::config::Rgb(255, 0, 255);
        let legacy = workspace_style(&cfg, 1, false);
        assert_eq!(legacy.background, changed.background);
        assert_eq!(legacy.selected_background, changed.selected_background);
        assert_eq!(legacy.foreground, changed.foreground);
        assert_eq!(legacy.selected_foreground, changed.selected_foreground);
    }

    #[test]
    fn workspace_text_is_readable_on_light_and_dark_tab_fills() {
        let mut cfg = crate::config::Config::default();
        for light in [false, true] {
            for color in cfg.workspace_colors.into_iter().chain([crate::config::Rgb(0, 0, 0), crate::config::Rgb(255, 255, 255)]) {
                cfg.workspace_colors[0] = color;
                let style = workspace_style(&cfg, 0, light);
                for (fg, bg) in [(style.foreground, style.background), (style.selected_foreground, style.selected_background)] {
                    let luminance = |c| {
                        let c = egui::Rgba::from(c);
                        0.2126 * c.r() + 0.7152 * c.g() + 0.0722 * c.b()
                    };
                    let (fg, bg) = (luminance(fg), luminance(bg));
                    let contrast = (fg.max(bg) + 0.05) / (fg.min(bg) + 0.05);
                    assert!(contrast >= 4.5, "unreadable tab text: light={light} workspace={color:?} contrast={contrast}");
                }
            }
        }
    }

    fn positions(ctx: &egui::Context, time: f64, swapped: bool) -> (f32, f32) {
        let mut result = (0.0, 0.0);
        let mut full = ctx.run_ui(egui::RawInput { time: Some(time), ..Default::default() }, |ui| {
            let target = |x| egui::Rect::from_min_size(egui::pos2(x, 0.0), egui::vec2(80.0, 40.0));
            result.0 = animated_rect(ui.ctx(), egui::Id::unique("first_tab"), target(if swapped { 100.0 } else { 0.0 })).left();
            result.1 = animated_rect(ui.ctx(), egui::Id::unique("second_tab"), target(if swapped { 0.0 } else { 100.0 })).left();
        });
        full.textures_delta.clear();
        result
    }

    #[test]
    fn configured_speed_changes_actual_motion_and_off_finishes_in_flight_transitions() {
        let slow = egui::Context::default();
        let fast = egui::Context::default();
        apply_motion(&slow, 0.5);
        apply_motion(&fast, 2.0);
        assert_eq!(slow.global_style().animation_time, 0.32);
        assert_eq!(fast.global_style().animation_time, 0.08);
        for ctx in [&slow, &fast] {
            positions(ctx, 0.0, false);
            positions(ctx, 0.016, true);
        }
        assert!(positions(&fast, 0.07, true).0 > positions(&slow, 0.07, true).0);
        apply_motion(&slow, 0.0);
        assert_eq!(positions(&slow, 0.08, true), (100.0, 0.0));
        let mut full = slow.run_ui(egui::RawInput { time: Some(0.09), ..Default::default() }, |ui| {
            let id = egui::Id::unique("disabled_hover");
            assert_eq!(transition(ui.ctx(), id, false), 0.0);
            assert_eq!(transition(ui.ctx(), id, true), 1.0);
        });
        full.textures_delta.clear();
    }

    #[test]
    fn stable_id_motion_interpolates_and_retargets_without_jumping() {
        let ctx = egui::Context::default();
        apply_motion(&ctx, 1.0);
        assert_eq!(positions(&ctx, 0.0, false), (0.0, 100.0));
        positions(&ctx, 0.016, true);
        let middle = positions(&ctx, 0.08, true);
        assert!(middle.0 > 0.0 && middle.0 < 100.0);
        assert!(middle.1 > 0.0 && middle.1 < 100.0);
        let retargeted = positions(&ctx, 0.09, false);
        assert!(retargeted.0 > 0.0 && retargeted.0 < 100.0);
        assert_eq!(positions(&ctx, 0.28, false), (0.0, 100.0));
        // A surface absent for a while opens at the correct current layout.
        assert_eq!(positions(&ctx, 1.0, true), (100.0, 0.0));
    }
}
