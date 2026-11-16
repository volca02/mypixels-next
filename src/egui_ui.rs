//! egui integration for the tab strip, sidebars, switcher and transient UI.
//! The terminal uses its own pipeline; egui is composited over it in the same
//! render pass. Shared styling lives in `ui_style`, and tab layout in `tab_bar`.
//! Sidebars stay passive until an editor is explicitly focused; modal pickers
//! retain their own keyboard behavior. The app routes terminal keys directly.

use egui::{Context, FullOutput, ViewportId};
use crate::ui_style::{TermStyle, Icon, text, text_right, text_width, icon as draw_icon, icon_button, icon_button_faded, actions, section, form_grid, rgb_c32, preview_image, action_row};

/// The egui side of the app: context, winit event state and the wgpu
/// renderer that paints egui's tessellation.
pub struct Egui {
    pub ctx: Context,
    pub state: egui_winit::State,
    pub renderer: egui_wgpu::Renderer,
    /// One-shot flags set by the UI, consumed by `App::draw`.
    pub config_changed: bool,
    pub save_requested: bool,
    pub revert_requested: bool,
    pub ui_state: UiState,
    /// The card the pointer clicked on the previous frame (one-shot).
    pub switcher_pick: Option<usize>,
    /// The installed fonts the font popup can preview, from the app's own
    /// font database (see `fonts::font_choices`).
    pub font_choices: Vec<crate::fonts::FontChoice>,
    /// Last frame's (panel open, popup open), for close-edge focus release.
    pub open_prev: (bool, bool),
}

/// One frame of egui output, ready for `Gpu::render_frame`: tessellated
/// paint jobs plus the texture deltas that must be uploaded first.
pub struct EguiFrame<'a> {
    pub renderer: &'a mut egui_wgpu::Renderer,
    /// Mutable: `Gpu::render_frame` uploads the texture deltas and then
    /// clears the container -- epaint 0.37 panics (debug builds) if a
    /// TexturesDelta is dropped with unapplied deltas still in it.
    pub full: &'a mut FullOutput,
    pub paint_jobs: Vec<egui::epaint::ClippedPrimitive>,
    pub screen: egui_wgpu::ScreenDescriptor,
}

/// What the configuration UI asked for this frame.
pub struct ConfigEvents {
    pub apply: bool,
    pub save: bool,
    pub revert: bool,
}

/// Ephemeral UI state that must survive across frames.
#[derive(Default)]
pub struct UiState {
    /// Frames the context menu has been on screen. The right-press that
    /// *opened* the menu is still in the input state of the first frame
    /// that draws it - dismissal must not see its own opening.
    pub menu_age: u32,
    /// The open font popup, if any (at most one at a time).
    pub font_popup: Option<FontPopup>,
    /// Theme names for the picker. Fetched lazily and re-fetched after
    /// save/revert -- dropping a new file into a themes dir is exactly when
    /// the user expects the list to have noticed.
    pub theme_choice: Vec<String>,
    pub theme_fetched: bool,
    pub settings_page: SettingsPage,
    pub workspace_edit: usize,
    /// Last geometry logged for the category selector, so the line is written
    /// when the layout moves rather than on every frame. The smoke tests click
    /// a category by name from this line instead of guessing an offset.
    settings_nav_log: String,
    /// The keybinding row waiting for a chord. The app owns the next key
    /// while this is set; Escape clears it. Held here (not in the app) so the
    /// row can show its own waiting state and the keyboard arbiter can see it.
    pub key_capture: Option<String>,
    /// A chord was recorded this session, so saving must rewrite the
    /// `bindings:` block. Until then a hand-written block is left untouched.
    pub bindings_dirty: bool,
    /// A single slot keeps the right-side surfaces mutually exclusive.
    pub right_panel: RightPanel,
    /// Focus from the last completed UI frame, for editors opened by a click.
    sidebar_focus: Option<egui::Id>,
    /// Top of the terminal below the tab strip, in logical points.
    pub panel_top: f32,
    /// The tab-rename box, egui's version of the original's GTK entry. The
    /// app mirrors this to decide whether to swallow keys.
    pub rename_open: bool,
    pub rename_draft: String,
    /// Screen position (logical points) chosen by the app when it opens the
    /// box -- just under the tab strip, where row 0 used to host the prompt.
    pub rename_pos: [f32; 2],
    /// True for the frame the box opens: focus the entry and pre-select the
    /// draft, so the first typed key replaces it exactly like selected text
    /// in the GTK original (Enter with nothing typed keeps it).
    pub rename_seed: bool,
    /// Set by `rename_box` when the user commits (Some(Some(_))) or cancels
    /// (Some(None)); the app applies and clears it after the frame.
    pub rename_out: Option<Option<String>>,
    /// Window-close modal. Cancel receives initial focus; permanent closure
    /// requires a separate, timed danger step. Outcomes are applied by the app.
    pub confirm_open: bool,
    pub confirm_seed: bool,
    pub confirm: CloseDialog,
    pub confirm_out: Option<CloseChoice>,
    /// A font row toggled its popup closed on button-press; the matching
    /// release must not count as a fresh open click (see `font_row`).
    font_suppress: bool,
    /// The send-tab-to-window picker (Alt+Shift+w) and the move-tab-to-
    /// workspace picker (Alt+Shift+m). Items and targets are filled by the
    /// app when it opens the box; a click reports the target window index,
    /// usize::MAX = "detach into a new window". `move_title` is the box's
    /// heading; empty means the original "Send tab to:". `move_workspace`
    /// flips the pick's meaning: the target is a workspace index, not a
    /// window index.
    pub move_open: bool,
    pub move_title: String,
    pub move_workspace: bool,
    pub move_items: Vec<String>,
    pub move_targets: Vec<usize>,
    pub move_pos: [f32; 2],
    /// Some(Some(t)) = move to target t, Some(None) = cancelled, None =
    /// still open. Same shape as `rename_out` for the same reason: the
    /// app must learn about dismissal, not only about choice.
    pub move_out: Option<Option<usize>>,
    /// The context or application menu. Rows are (label, action, enabled);
    /// `menu_seps` are the indices a separator goes BEFORE. Actions are the
    /// app's own `do_action` names, so the menu adds no new machinery.
    pub menu_open: bool,
    pub menu_app: bool,
    pub info_page: Option<InfoPage>,
    pub menu_items: Vec<(String, String, bool)>,
    pub menu_seps: Vec<usize>,
    pub menu_pos: [f32; 2],
    /// Some(Some(action)) = run this, Some(None) = dismissed, None = open.
    pub menu_out: Option<Option<String>>,
    /// The command-history panel (Alt+Shift+H): the active tab's OSC 133
    /// pool as a live right-side list with filters, search, and actions.
    /// `history_rows` is the app's snapshot (rebuilt every frame while
    /// open); `history_out` is a one-shot (seq, action) the app applies
    /// after the frame, the same contract as the other `*_out` flags.
    pub history_rows: Vec<HistoryRow>,
    pub history_filter: HistoryFilter,
    pub history_query: String,
    pub history_search_open: bool,
    pub history_oldest_first: bool,
    pub history_sel: Option<u64>,
    /// Only the selected, completed command is decoded for preview.
    pub history_preview: Option<HistoryPreview>,
    pub history_out: Option<(u64, String)>,
    /// The graveyard panel (Ctrl+Shift+G): closed tabs that keep running.
    /// `grave_rows` is the app's snapshot (rebuilt every frame while
    /// open); `grave_sel` indexes it newest-first; `grave_out` is a
    /// one-shot (index, action) the app applies after the frame, the same
    /// contract as `history_out`.
    /// `ws_rows` is the app's snapshot of the ten workspaces (rebuilt every
    /// frame while the panel is open); `ws_sel` indexes it and starts on the
    /// current workspace; `ws_out` is the index the app switches to after the
    /// frame. Same contract as `grave_rows`.
    pub ws_rows: Vec<WsRow>,
    pub ws_sel: Option<usize>,
    pub ws_out: Option<usize>,
    pub ws_tab_out: Option<(usize, usize)>,
    pub ws_query: String,
    pub ws_search_open: bool,
    pub ws_populated_only: bool,
    pub ws_collapsed: Option<usize>,
    pub ws_visible_tabs: Vec<usize>,
    pub grave_rows: Vec<GraveRow>,
    /// Stable window id, active title, and total live tab count, newest first.
    pub grave_windows: Vec<(usize, String, usize)>,
    pub grave_window_out: Option<(usize, bool)>,
    pub grave_sel: Option<usize>,
    pub grave_out: Option<(usize, String)>,
}

pub struct HistoryPreview {
    pub seq: u64,
    pub lines: Vec<String>,
    pub columns: usize,
}

impl HistoryPreview {
    pub fn new(seq: u64, output: &str) -> Self {
        let lines: Vec<String> = output.lines().map(str::to_owned).collect();
        let columns = lines.iter().map(|line| line.chars().count()).max().unwrap_or(0);
        Self { seq, lines, columns }
    }
}

/// A window has one sidebar slot. Adding a panel here keeps visibility exclusive.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RightPanel {
    #[default]
    Closed,
    History,
    Graveyard,
    Workspaces,
    Settings,
}

/// One workspace, as the panel displays it. The app precomputes every field
/// each frame the panel is open, so the panel never walks a window's tab
/// lists and never touches a terminal.
#[derive(Clone)]
pub struct WsRow {
    pub index: usize,
    /// Tabs the workspace holds; zero renders as an empty slot.
    pub count: usize,
    /// Caption of the workspace's selected tab, empty when it holds none.
    pub title: String,
    pub genre: String,
    pub icon: crate::config::WorkspaceIcon,
    pub tabs: Vec<WsTab>,
    /// The workspace this window is looking at right now.
    pub current: bool,
    /// The chord that jumps here, read from the live config so the panel can
    /// never show a shortcut the app does not answer to.
    pub shortcut: String,
}

#[derive(Clone)]
pub struct WsTab {
    pub id: usize,
    pub title: String,
    pub selected: bool,
    pub preview: Option<(egui::TextureId, (u32, u32))>,
}

/// One buried tab, precomputed by the app while the graveyard panel is open.
#[derive(Clone)]
pub struct GraveRow {
    pub id: usize,
    pub title: String,
    pub cwd: String,
    /// "42 s" / "3 m" / "1 h 04 m" since the tab was buried.
    pub buried: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InfoPage {
    Shortcuts,
    About,
}

impl UiState {
    /// Sidebars are passive until an editor is explicitly focused. Merely
    /// opening a panel or clicking one of its buttons never claims typing.
    pub fn owns_keyboard(&self, ctx: &egui::Context) -> bool {
        // The list panels own navigation like menus; the app also routes
        // typing to the workspace search while its editor has focus.
        self.rename_open || self.confirm_open || self.menu_open || self.move_open || self.info_page.is_some()
            || self.key_capture.is_some()
            || matches!(self.right_panel, RightPanel::Graveyard | RightPanel::Workspaces)
            || egui::Popup::is_any_open(ctx)
            || (self.right_panel != RightPanel::Closed && ctx.memory(|m| m.focused())
                .is_some_and(|id| egui::text_edit::TextEditState::load(ctx, id).is_some()))
    }

    /// Button focus must not turn a passive sidebar into a keyboard surface.
    /// IME output identifies a currently rendered editor, including DragValue.
    pub fn release_passive_focus(&mut self, ctx: &egui::Context, output: &egui::PlatformOutput) {
        let focus = ctx.memory(|m| m.focused());
        let pending_editor = focus != self.sidebar_focus
            && focus.is_some_and(|id| egui::text_edit::TextEditState::load(ctx, id).is_some());
        self.sidebar_focus = focus;
        if self.right_panel != RightPanel::Closed && !self.rename_open && !self.confirm_open && !self.menu_open && !self.move_open && self.info_page.is_none() && !egui::Popup::is_any_open(ctx)
            && output.ime.is_none() {
            // DragValue and history search request edit focus after layout;
            // allow the next frame to render their editor before deciding.
            if pending_editor {
                ctx.request_repaint();
            } else {
                release_focus(ctx);
                self.sidebar_focus = None;
            }
        }
    }

    pub fn panel_is(&self, panel: RightPanel) -> bool {
        self.right_panel == panel
    }

    pub fn toggle_panel(&mut self, panel: RightPanel) {
        self.right_panel = if self.panel_is(panel) { RightPanel::Closed } else { panel };
        self.font_popup = None;
        if self.right_panel != RightPanel::History {
            self.history_preview = None;
        }
    }

    pub fn close_panel(&mut self, panel: RightPanel) {
        if self.panel_is(panel) {
            self.right_panel = RightPanel::Closed;
            self.font_popup = None;
            self.history_preview = None;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsPage {
    #[default]
    Appearance,
    Window,
    Tabs,
    /// Ways to move between terminals: the Alt-held switcher and workspaces.
    /// Neither is a tab-strip setting, whatever they happen to draw.
    Navigation,
    Terminal,
    Keys,
}

/// The categories, in the order the selector shows them. Both the selector and
/// the tests read this, so adding a page means adding a line here and an arm to
/// the dispatch - not hunting for a literal list in three places.
const SETTINGS_PAGES: &[(SettingsPage, &str)] = &[
    (SettingsPage::Appearance, "Appearance"),
    (SettingsPage::Window, "Window"),
    (SettingsPage::Tabs, "Tabs"),
    (SettingsPage::Navigation, "Navigation"),
    (SettingsPage::Terminal, "Terminal"),
    (SettingsPage::Keys, "Keys"),
];

/// The history panel's list filter, in chip order.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum HistoryFilter {
    #[default]
    All,
    Success,
    Error,
}

/// One command in the history panel. Every field is precomputed by the
/// app (at pool close, refreshed every frame while the panel is open),
/// so the panel - which is drawn on every frame of the app - never
/// touches the pool, the scrollback grid, or a codec.
#[derive(Clone)]
pub struct HistoryRow {
    pub seq: u64,
    /// Input captured at C, without the prompt. Can be missing when the
    /// shell does not mark its input or that input has already been lost.
    pub cmd: String,
    pub exit: Option<i32>,
    pub done: bool,
    pub alt_seen: bool,
    pub lines: usize,
    /// Local clock of the D mark (the C mark's while running): 11:21:03.
    pub time: String,
    /// 412 ms / 1.8 s / 2 m 05 s - empty while running.
    pub duration: String,
}

impl HistoryRow {
    fn command_label(&self) -> &str {
        if !self.cmd.is_empty() {
            &self.cmd
        } else if self.done {
            "(command unavailable)"
        } else {
            "(running)"
        }
    }
}

/// What the filter chip and the search box keep visible. Pure, so the
/// panel's behaviour is testable without a window.
pub fn history_visible(row: &HistoryRow, filter: HistoryFilter, query: &str) -> bool {
    if row.cmd.trim().is_empty() { return false; }
    match filter {
        HistoryFilter::All => {}
        HistoryFilter::Success => {
            if row.exit.is_some_and(|e| e != 0) {
                return false;
            }
        }
        HistoryFilter::Error => {
            if !row.exit.is_some_and(|e| e != 0) {
                return false;
            }
        }
    }
    if query.is_empty() {
        return true;
    }
    row.cmd.to_lowercase().contains(&query.to_lowercase())
}

/// The context menu: a borderless popup at the pointer, one button per row.
/// Same open/click-away/Escape contract as the rename box and the window
/// picker -- the three small popups should behave identically or they lie
/// about being the same kind of thing.
pub fn ctx_menu(ui: &egui::Ui, st: &mut UiState, cfg: &crate::config::Config) {
    if !st.menu_open {
        st.menu_out = None;
        return;
    }
    let ctx = ui.ctx();
    let pos = egui::pos2(st.menu_pos[0], st.menu_pos[1]);
    let colors = TermStyle::new(ui, ui.visuals().text_color(), ui.visuals().error_fg_color);
    let mut out: Option<Option<String>> = None;
    let mut area = egui::Rect::NOTHING;
    egui::Window::new("ctx_menu")
        .id(egui::Id::unique("ctx_menu_box"))
        .frame(colors.menu_frame())
        .fade_in(false)
        .fade_out(false)
        .title_bar(false)
        .movable(false)
        .resizable(false)
        .fixed_pos(pos)
        .constrain(true)
        .show(&ctx, |ui| {
            ui.set_width(300.0_f32.min((ctx.content_rect().width() - 24.0).max(80.0)));
            colors.menu_contents(ui, |ui| {
                egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 28.0).max(32.0)).show(ui, |ui| {
                    for (i, (label, action, enabled)) in st.menu_items.iter().enumerate() {
                        if st.menu_seps.contains(&i) { crate::ui_style::menu_separator(ui, &colors); }
                        let shortcut = cfg.bindings.iter().find(|(name, _)| name == action)
                            .map(|(_, keys)| keys.iter().map(|b| b.label()).collect::<Vec<_>>().join(" / "))
                            .unwrap_or_default();
                        if crate::ui_style::menu_item(ui, label, &shortcut, None, *enabled, &colors).clicked() {
                            out = Some(Some(action.clone()));
                        }
                    }
                });
            });
            area = ui.response().rect;
        });
    // Ignore the opening pointer press, including repeated sizing passes.
    // Escape is never the opening gesture and must dismiss immediately.
    if out.is_none()
        && ctx.input(|i| {
            i.key_pressed(egui::Key::Escape)
                || (st.menu_age >= 2 && i.pointer.any_pressed()
                    && i.pointer.interact_pos().is_some_and(|p| !area.expand(4.0).contains(p)
                        // The anchor toggles on release; closing on press
                        // would make that release reopen the application menu.
                        && !(st.menu_app && crate::tab_bar::menu_button_rect(st.panel_top).contains(p))))
        })
    {
        out = Some(None);
    }
    if st.menu_age < 2 { ctx.request_repaint(); }
    st.menu_age += 1;
    st.menu_out = out;
}

/// Application information stays in the window and never sends keys to the PTY.
pub fn app_info(ui: &egui::Ui, st: &mut UiState, cfg: &crate::config::Config) {
    let Some(page) = st.info_page else { return; };
    let colors = TermStyle::new(ui, ui.visuals().text_color(), ui.visuals().error_fg_color);
    let response = egui::Modal::new(egui::Id::unique("app_info")).frame(colors.menu_frame()).show(ui.ctx(), |ui| {
        colors.apply_controls(ui);
        ui.set_width((ui.ctx().content_rect().width() - 48.0).clamp(100.0, 560.0));
        ui.heading(if page == InfoPage::Shortcuts { "Keyboard shortcuts" } else { "About mypixels" });
        ui.separator();
        if page == InfoPage::Shortcuts {
            ui.label("Your current keybindings");
            egui::ScrollArea::both().max_height((ui.ctx().content_rect().height() - 160.0).max(60.0)).show(ui, |ui| {
                egui::Grid::new("shortcut_reference").striped(true).spacing([24.0, 8.0]).show(ui, |ui| {
                    for (action, bindings) in &cfg.bindings {
                        if bindings.is_empty() { continue; }
                        ui.label(action_label(action));
                        ui.monospace(bindings.iter().map(|b| b.label()).collect::<Vec<_>>().join(" / "));
                        ui.end_row();
                    }
                });
            });
        } else {
            ui.label(format!("mypixels {}", env!("CARGO_PKG_VERSION")));
            ui.label("A compact, GPU-rendered terminal emulator.");
        }
        ui.separator();
        ui.button("Close").clicked()
    });
    if response.inner || response.should_close() {
        st.info_page = None;
        release_focus(ui.ctx());
        ui.ctx().request_repaint();
    }
}

/// The window picker for "send tab to window": one small frameless list,
/// every live window plus an escape hatch into a new one. Click elsewhere
/// or Escape and nothing happens, exactly like the rename box.
pub fn move_pick_box(ui: &egui::Ui, st: &mut UiState) {
    if !st.move_open {
        st.move_out = None;
        return;
    }
    let ctx = ui.ctx();
    let pos = egui::pos2(st.move_pos[0], st.move_pos[1]);
    let mut out: Option<Option<usize>> = None;
    let mut area = egui::Rect::NOTHING;
    egui::Window::new("move_tab_pick")
        .id(egui::Id::unique("move_tab_pick_box"))
        .title_bar(false)
        .movable(false)
        .resizable(false)
        .fixed_pos(pos)
        .constrain(false)
        .show(&ctx, |ui| {
            let title = if st.move_title.is_empty() { "Send tab to:" } else { st.move_title.as_str() };
            ui.label(title);
            for (i, label) in st.move_items.iter().enumerate() {
                if ui.button(label).clicked() {
                    out = Some(Some(st.move_targets[i]));
                }
            }
            if ui.button("Cancel").clicked() {
                out = Some(None);
            }
            area = ui.response().rect;
        });
    if out.is_none()
        && ctx.input(|i| {
            i.key_pressed(egui::Key::Escape)
                || (i.pointer.any_pressed()
                    && i.pointer.interact_pos().is_some_and(|p| !area.expand(8.0).contains(p)))
        })
    {
        out = Some(None);
    }
    st.move_out = out;
}

/// egui-winit's consumption flag uses the pointer from the last UI pass.
/// Hit-test the event's current position when a sidebar is the only surface.
pub fn panel_pointer_input(ctx: &egui::Context, at: egui::Pos2) -> bool {
    ctx.egui_is_using_pointer()
        || ctx.layer_id_at(at).is_some_and(|layer| layer.order != egui::Order::Background)
}

/// winit pointer coordinates are physical pixels; egui popup anchors are points.
pub fn popup_position(pointer: (f32, f32), pixels_per_point: f32) -> [f32; 2] {
    [pointer.0 / pixels_per_point, pointer.1 / pixels_per_point]
}

/// The history sidebar uses its own compact layout, while retaining the
/// terminal theme's background, foreground, accent, and status colours.
pub fn history_panel(ui: &mut egui::Ui, st: &mut UiState, ok: egui::Color32, err: egui::Color32, top: f32) {
    if !st.panel_is(RightPanel::History) {
        st.history_out = None;
        return;
    }
    st.history_out = None;
    if !st.menu_open && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        release_focus(ui.ctx());
    }
    let colors = TermStyle::new(ui, ok, err);
    let TermStyle { bg, fg, muted, border, accent, card, .. } = colors;
    let mut host = ui.max_rect();
    host.min.y = host.min.y.max(top);
    host.min.x = (host.max.x - 380.0).max(host.min.x);
    // Drawing a Panel in a root-ui scope advances the root cursor past
    // the full-height sidebar. egui then claims the entire background as
    // used, including the terminal. A separate Area leaves it available.
    egui::Area::new(egui::Id::unique("mypixels_history_area"))
        .fixed_pos(host.min)
        .order(egui::Order::Middle)
        .fade_in(false)
        .movable(false)
        .constrain(false)
        .show(ui.ctx(), |ui| {
            ui.set_max_size(host.size());
            egui::Frame::new().fill(bg).inner_margin(18.0).show(ui, |ui| {
                ui.set_width((host.width() - 36.0).max(0.0));
                ui.set_min_height((host.height() - 36.0).max(0.0));
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                let edge = ui.max_rect().left() - 18.0;
                ui.painter().vline(edge, host.y_range(), egui::Stroke::new(1.0, border));
                let (header, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), egui::Sense::hover());
                draw_icon(
                    ui.painter(),
                    Icon::Layers,
                    egui::pos2(header.left() + 14.0, header.center().y),
                    fg,
                );
                text(
                    ui,
                    egui::Rect::from_min_max(
                        egui::pos2(header.left() + 36.0, header.top()),
                        egui::pos2(header.right() - 68.0, header.bottom()),
                    ),
                    "Command History",
                    fg,
                    16.0,
                    true,
                );
                let search_rect = egui::Rect::from_center_size(
                    egui::pos2(header.right() - 46.0, header.center().y),
                    egui::vec2(30.0, 30.0),
                );
                let search = icon_button(ui, search_rect, Icon::Search, "Search commands", &colors);
                let focus_search = search.clicked();
                if focus_search {
                    st.history_search_open = !st.history_search_open;
                    if !st.history_search_open {
                        st.history_query.clear();
                    }
                }
                let close_rect = egui::Rect::from_center_size(
                    egui::pos2(header.right() - 10.0, header.center().y),
                    egui::vec2(30.0, 30.0),
                );
                if icon_button(ui, close_rect, Icon::Dismiss, "Close history", &colors).clicked() {
                    st.close_panel(RightPanel::History);
                }
                ui.add_space(16.0);
                if st.history_search_open {
                    let search = ui.add_sized(
                        [ui.available_width(), 32.0],
                        egui::TextEdit::singleline(&mut st.history_query)
                            .hint_text("Search commands…")
                            .font(egui::FontId::monospace(14.0))
                            .margin(egui::vec2(10.0, 8.0)),
                    );
                    if focus_search {
                        search.request_focus();
                    }
                    ui.add_space(12.0);
                }
                let (filters, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), egui::Sense::hover());
                let mut x = filters.left() + 2.0;
                for (filter, label, icon, width) in [
                    (HistoryFilter::All, "All", None, 54.0),
                    (HistoryFilter::Success, "Success", Some(Icon::Check), 102.0),
                    (HistoryFilter::Error, "Error", Some(Icon::Error), 82.0),
                ] {
                    let rect = egui::Rect::from_min_size(egui::pos2(x, filters.top()), egui::vec2(width, 34.0));
                    let response = ui.interact(rect, ui.scope_id().with(label), egui::Sense::click());
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(egui::Role::RadioButton, true, st.history_filter == filter, label)
                    });
                    if response.clicked() {
                        st.history_filter = filter;
                    }
                    let selected = st.history_filter == filter;
                    let fill = if selected {
                        bg.lerp_to_gamma(accent, 0.16)
                    } else if response.hovered() {
                        bg.lerp_to_gamma(fg, 0.07)
                    } else {
                        card
                    };
                    ui.painter().rect(
                        rect,
                        17.0,
                        fill,
                        egui::Stroke::new(1.0, if selected || response.has_focus() { accent } else { border }),
                        egui::StrokeKind::Inside,
                    );
                    if let Some(icon) = icon {
                        draw_icon(
                            ui.painter(),
                            icon,
                            egui::pos2(x + 19.0, rect.center().y),
                            if selected {
                                accent
                            } else if filter == HistoryFilter::Success {
                                ok
                            } else {
                                muted
                            },
                        );
                    }
                    let label_rect =
                        if icon.is_some() { rect.with_min_x(x + 31.0) } else { rect.shrink2(egui::vec2(14.0, 0.0)) };
                    text(ui, label_rect, label, if selected { accent } else { muted }, 14.0, false);
                    x += width + 7.0;
                }
                let clock_rect = egui::Rect::from_min_size(egui::pos2(x, filters.top()), egui::vec2(42.0, 34.0));
                ui.painter().rect(
                    clock_rect,
                    17.0,
                    card,
                    egui::Stroke::new(1.0, if st.history_oldest_first { accent } else { border }),
                    egui::StrokeKind::Inside,
                );
                let clock = icon_button(
                    ui,
                    clock_rect,
                    Icon::Clock,
                    if st.history_oldest_first { "Newest first" } else { "Oldest first" },
                    &colors,
                );
                if clock.clicked() {
                    st.history_oldest_first = !st.history_oldest_first;
                }
                ui.add_space(18.0);
                // The list scrolls independently; on short windows the outer
                // scroll area keeps every details action reachable as well.
                let list_height = (ui.available_height() - 550.0).clamp(76.0, 274.0);
                let body_bottom = ui.max_rect().bottom();
                egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("history_body").show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("history_commands")
                        .auto_shrink([false, true])
                        .max_height(list_height)
                        .show(ui, |ui| {
                            let rows: Vec<_> = st
                                .history_rows
                                .iter()
                                .filter(|r| history_visible(r, st.history_filter, &st.history_query))
                                .collect();
                            // Keep the details card tied to a visible row after
                            // filtering, searching, tab changes, or pool eviction.
                            if !rows.iter().any(|r| Some(r.seq) == st.history_sel) {
                                st.history_sel = rows.iter().find(|r| r.done).or(rows.first()).map(|r| r.seq);
                            }
                            if rows.is_empty() {
                                ui.add_space(14.0);
                                ui.label(
                                    egui::RichText::new(if st.history_rows.is_empty() {
                                        "No command history yet"
                                    } else {
                                        "No commands match"
                                    })
                                    .color(muted),
                                );
                                ui.add_space(14.0);
                            }
                            let seq_width =
                                rows.iter().map(|r| r.seq.to_string().len()).max().unwrap_or(2).max(2) as f32 * 8.5
                                    + 12.0;
                            for index in 0..rows.len() {
                                let row = rows[if st.history_oldest_first { rows.len() - 1 - index } else { index }];
                                let (rect, _) = ui
                                    .allocate_exact_size(egui::vec2(ui.available_width(), 38.0), egui::Sense::hover());
                                let response = ui.interact(
                                    rect,
                                    ui.scope_id().with(("history_row", row.seq)),
                                    egui::Sense::click(),
                                );
                                response.widget_info(|| {
                                    egui::WidgetInfo::selected(
                                        egui::Role::RadioButton,
                                        true,
                                        st.history_sel == Some(row.seq),
                                        &row.cmd,
                                    )
                                });
                                if response.clicked() {
                                    st.history_sel = Some(row.seq);
                                }
                                let dot = egui::pos2(rect.left() + 15.0, rect.center().y);
                                ui.painter().vline(
                                    dot.x,
                                    rect.y_range(),
                                    egui::Stroke::new(1.0, bg.lerp_to_gamma(ok, 0.55)),
                                );
                                if st.history_sel == Some(row.seq) || response.hovered() || response.has_focus() {
                                    let selected = st.history_sel == Some(row.seq) || response.has_focus();
                                    ui.painter().rect(
                                        rect.shrink2(egui::vec2(0.0, 1.0)),
                                        7.0,
                                        bg.lerp_to_gamma(accent, if selected { 0.13 } else { 0.045 }),
                                        egui::Stroke::new(1.0, if selected { accent } else { border }),
                                        egui::StrokeKind::Inside,
                                    );
                                }
                                let status = colors.status(row.done, row.exit);
                                ui.painter().circle_filled(dot, 7.5, bg);
                                ui.painter().circle_filled(dot, 5.5, status);
                                if row.exit.is_some_and(|e| e != 0) {
                                    draw_icon(ui.painter(), Icon::Close, dot, bg);
                                } else if !row.done {
                                    ui.painter().circle_stroke(dot, 3.0, egui::Stroke::new(1.0, bg));
                                }
                                text(
                                    ui,
                                    rect.with_min_x(rect.left() + 38.0).with_max_x(rect.left() + 38.0 + seq_width),
                                    &format!("#{}", row.seq),
                                    muted,
                                    14.0,
                                    false,
                                );
                                let cmd_x = rect.left() + 38.0 + seq_width;
                                if row.done {
                                    draw_icon(
                                        ui.painter(),
                                        if row.exit.is_some_and(|e| e != 0) {
                                            Icon::Close
                                        } else {
                                            Icon::Check
                                        },
                                        egui::pos2(cmd_x + 4.0, rect.center().y),
                                        status,
                                    );
                                }
                                text(
                                    ui,
                                    rect.with_min_x(cmd_x + 15.0).with_max_x(rect.right() - 59.0),
                                    row.command_label(),
                                    fg,
                                    14.0,
                                    false,
                                );
                                text(
                                    ui,
                                    rect.with_min_x(rect.right() - 50.0),
                                    row.time.get(..5).unwrap_or(&row.time),
                                    muted,
                                    14.0,
                                    false,
                                );
                                response.on_hover_text(&row.cmd).on_hover_cursor(egui::CursorIcon::PointingHand);
                            }
                        });
                    if let Some(row) = st.history_rows.iter().find(|r| Some(r.seq) == st.history_sel) {
                        ui.add_space(18.0);
                        if let Some(action) = history_details(ui, row, &colors) {
                            st.history_out = Some((row.seq, action.to_string()));
                        }
                        ui.add_space(14.0);
                        history_output_preview(ui, row, st.history_preview.as_ref(), &colors,
                            (body_bottom - ui.cursor().top()).max(100.0));
                    }
                });
            });
    });
}

fn history_output_preview(ui: &mut egui::Ui, row: &HistoryRow, preview: Option<&HistoryPreview>, c: &TermStyle, height: f32) {
    egui::Frame::new()
        .fill(c.card)
        .stroke(egui::Stroke::new(1.0, c.border))
        .corner_radius(9.0)
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(egui::RichText::new("Output preview").size(14.0).strong().color(c.fg));
            ui.add_space(8.0);
            let height = (height - 50.0).max(40.0);
            let preview = preview.filter(|p| p.seq == row.seq);
            let message = if row.alt_seen {
                Some("Full-screen program; output was not captured.")
            } else if !row.done {
                Some("Output will appear when the command finishes.")
            } else if preview.is_none() {
                ui.ctx().request_repaint();
                Some("Loading output…")
            } else if preview.is_some_and(|p| p.lines.is_empty()) {
                Some("No output captured.")
            } else {
                None
            };
            if let Some(message) = message {
                ui.set_min_height(height);
                ui.label(egui::RichText::new(message).color(c.muted));
            } else if let Some(preview) = preview {
                let font = egui::FontId::monospace(12.0);
                let line_height = ui.fonts_mut(|f| f.row_height(&font));
                let column_width = ui.fonts_mut(|f| f.glyph_width(&font, 'M'));
                ui.spacing_mut().item_spacing.y = 0.0;
                egui::ScrollArea::both()
                    .id_salt(("history_output", row.seq))
                    .auto_shrink([false, false])
                    .max_height(height)
                    .min_scrolled_height(height)
                    .show_rows(ui, line_height, preview.lines.len(), |ui, range| {
                        ui.set_min_width(preview.columns as f32 * column_width);
                        for line in &preview.lines[range] {
                            ui.add(egui::Label::new(egui::RichText::new(if line.is_empty() { " " } else { line })
                                .font(font.clone()).color(c.fg)).wrap_mode(egui::TextWrapMode::Extend));
                        }
                    });
            }
        });
}

fn history_details(ui: &mut egui::Ui, row: &HistoryRow, c: &TermStyle) -> Option<&'static str> {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 368.0), egui::Sense::hover());
    ui.painter().rect(rect, 9.0, c.card, egui::Stroke::new(1.0, c.border), egui::StrokeKind::Inside);
    let left = rect.left() + 13.0;
    let title = egui::Rect::from_min_max(
        egui::pos2(left, rect.top() + 6.0),
        egui::pos2(rect.right() - 12.0, rect.top() + 46.0),
    );
    text(
        ui,
        title,
        &format!("#{}  {}", row.seq, row.command_label()),
        c.fg,
        15.0,
        true,
    );
    ui.interact(title, ui.scope_id().with("history_title"), egui::Sense::hover()).on_hover_text(&row.cmd);
    let status = if !row.done {
        "running".to_string()
    } else {
        match row.exit {
            Some(0) => "success (exit 0)".to_string(),
            Some(e) => format!("failed (exit {e})"),
            None => "finished (no status)".to_string(),
        }
    };
    let output = if row.alt_seen { "full-screen program".to_string() } else { format!("{} lines", row.lines) };
    for (i, (label, value)) in [
        ("Status", status.as_str()),
        ("Duration", if row.duration.is_empty() { "—" } else { &row.duration }),
        ("Output", output.as_str()),
        ("Time", row.time.as_str()),
    ]
    .into_iter()
    .enumerate()
    {
        let y = rect.top() + 56.0 + i as f32 * 26.0;
        let line = egui::Rect::from_min_size(egui::pos2(left, y), egui::vec2(rect.width() - 26.0, 26.0));
        text(ui, line.with_max_x(left + 100.0), label, c.muted, 14.0, false);
        let value_x = left + 103.0;
        if i == 0 {
            draw_icon(
                ui.painter(),
                if !row.done {
                    Icon::Clock
                } else if row.exit.is_some_and(|e| e != 0) {
                    Icon::Error
                } else {
                    Icon::Check
                },
                egui::pos2(value_x + 6.0, line.center().y),
                c.status(row.done, row.exit),
            );
        }
        text(ui, line.with_min_x(value_x + if i == 0 { 22.0 } else { 0.0 }), value, c.fg, 14.0, false);
    }
    let divider = rect.top() + 170.0;
    ui.painter().hline(rect.x_range(), divider, egui::Stroke::new(1.0, c.border));
    text(
        ui,
        egui::Rect::from_min_size(egui::pos2(left, divider + 8.0), egui::vec2(rect.width() - 26.0, 26.0)),
        "Actions",
        c.fg,
        15.0,
        true,
    );
    let mut action = None;
    for (i, (label, name, icon, enabled)) in [
        ("Copy output", "copy", Icon::Copy, row.done && !row.alt_seen),
        ("Pipe output…", "pipe", Icon::Pipe, false),
        ("Re-run", "rerun", Icon::Rerun, row.done && !row.cmd.is_empty()),
        ("View in scrollback", "view", Icon::View, true),
    ]
    .into_iter()
    .enumerate()
    {
        let button = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 7.0, divider + 42.0 + i as f32 * 37.0),
            egui::vec2(rect.width() - 14.0, 34.0),
        );
        let response = crate::ui_style::action_row(ui, button,
            ui.scope_id().with(("history_action", name)), label, "", Some(icon), enabled, c);
        if response.clicked() && enabled {
            action = Some(name);
        }
        if !enabled {
            response.on_hover_text(if name == "pipe" {
                "Piping output is not available yet"
            } else if !row.done {
                "Command is still running"
            } else if name == "rerun" {
                "No command line to re-run"
            } else {
                "No captured output available"
            });
        } else {
            response.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
    }
    action
}

/// The graveyard sidebar: closed tabs that keep running, with a live
/// preview of each. Same shape as the history panel -- header, a
/// scrollable list of cards, a details card with actions for the
/// selection. `thumbs` maps tab id to its current preview texture.
pub fn graveyard_panel(
    ui: &mut egui::Ui,
    st: &mut UiState,
    _ok: egui::Color32,
    _err: egui::Color32,
    top: f32,
    thumbs: &[(usize, egui::TextureId, (u32, u32))],
) {
    if !st.panel_is(RightPanel::Graveyard) {
        st.grave_out = None;
        return;
    }
    st.grave_out = None;
    st.grave_window_out = None;
    if !st.menu_open && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        release_focus(ui.ctx());
        st.close_panel(RightPanel::Graveyard);
        return;
    }
    let colors = TermStyle::new(ui, _ok, _err);
    let TermStyle { bg, fg, muted, border, accent, card, .. } = colors;
    // List navigation lives in the app's keyboard, which routes to egui
    // while this panel is the only surface.
    // Navigation is opt-in: without a selection (no click, no arrow key)
    // Enter is not the panel's, so a command typed with the panel open
    // still runs in the terminal.
    let keys = ui.input(|i| {
        (i.key_pressed(egui::Key::ArrowUp) || i.key_pressed(egui::Key::ArrowDown),
         i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::ArrowRight),
         i.key_pressed(egui::Key::Delete))
    });
    let (nav, commit, delete) = keys;
    if nav && !st.grave_rows.is_empty() {
        let len = st.grave_rows.len();
        let cur = st.grave_sel.map(|s| s.min(len - 1)).unwrap_or(if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) { len - 1 } else { 0 });
        st.grave_sel = Some(if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
            (cur + len - 1) % len
        } else {
            (cur + 1) % len
        });
    }
    if commit {
        if let Some(sel) = st.grave_sel.filter(|s| *s < st.grave_rows.len()) {
            st.grave_out = Some((sel, "raise".to_string()));
        }
    }
    if delete {
        if let Some(sel) = st.grave_sel.filter(|s| *s < st.grave_rows.len()) {
            st.grave_out = Some((sel, "kill".to_string()));
        }
    }
    let count = st.grave_rows.len();
    let mut host = ui.max_rect();
    host.min.y = host.min.y.max(top);
    host.min.x = (host.max.x - 380.0).max(host.min.x);
    egui::Area::new(egui::Id::unique("mypixels_graveyard_area"))
        .fixed_pos(host.min)
        .order(egui::Order::Middle)
        .fade_in(false)
        .movable(false)
        .constrain(false)
        .show(ui.ctx(), |ui| {
            ui.set_max_size(host.size());
            egui::Frame::new().fill(bg).inner_margin(18.0).show(ui, |ui| {
                ui.set_width((host.width() - 36.0).max(0.0));
                ui.set_min_height((host.height() - 36.0).max(0.0));
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                let edge = ui.max_rect().left() - 18.0;
                ui.painter().vline(edge, host.y_range(), egui::Stroke::new(1.0, border));
                let (header, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), egui::Sense::hover());
                draw_icon(
                    ui.painter(),
                    Icon::Tombstone,
                    egui::pos2(header.left() + 14.0, header.center().y),
                    fg,
                );
                text(
                    ui,
                    egui::Rect::from_min_max(
                        egui::pos2(header.left() + 36.0, header.top()),
                        egui::pos2(header.right() - 68.0, header.bottom()),
                    ),
                    &format!("Graveyard {}", count + st.grave_windows.len()).trim_end().to_string(),
                    fg,
                    16.0,
                    true,
                );
                let close_rect = egui::Rect::from_center_size(
                    egui::pos2(header.right() - 10.0, header.center().y),
                    egui::vec2(30.0, 30.0),
                );
                if icon_button(ui, close_rect, Icon::Dismiss, "Close graveyard", &colors).clicked() {
                    st.close_panel(RightPanel::Graveyard);
                }
                let hint = egui::Rect::from_min_size(
                    egui::pos2(ui.cursor().left(), ui.cursor().top()),
                    egui::vec2(ui.available_width(), 16.0),
                );
                text(ui, hint, "\u{2191}\u{2193} select \u{b7} Enter raise \u{b7} Del kill \u{b7} Esc close",
                    muted, 11.0, false);
                ui.add_space(14.0);
                // Keep an explicit selection on a visible row after eviction.
                if let Some(sel) = st.grave_sel.filter(|s| *s >= count) {
                    st.grave_sel = Some(sel.min(count.saturating_sub(1)));
                }
                egui::ScrollArea::vertical()
                    .id_salt("graveyard_list")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if !st.grave_windows.is_empty() {
                            ui.label(egui::RichText::new("Closed windows").color(fg));
                            for (id, title, tabs) in &st.grave_windows {
                                ui.add_space(6.0);
                                ui.label(format!("{title} ({tabs} tabs)"));
                                ui.horizontal(|ui| {
                                    if ui.button("Restore window").clicked() {
                                        st.grave_window_out = Some((*id, true));
                                    }
                                    if ui.button("Kill window").clicked() {
                                        st.grave_window_out = Some((*id, false));
                                    }
                                });
                            }
                            ui.add_space(12.0);
                            ui.separator();
                        }
                        if count == 0 {
                            ui.add_space(14.0);
                            ui.label(
                                egui::RichText::new("No buried tabs.\nClosed tabs keep running here.")
                                    .color(muted),
                            );
                            return;
                        }
                        // Newest first: the tab buried last is the one a
                        // mistaken Ctrl+Shift+x is most likely to mourn.
                        for index in (0..count).rev() {
                            let row = &st.grave_rows[index];
                            let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 92.0), egui::Sense::hover());
                            let response = ui.interact(
                                rect,
                                ui.scope_id().with(("grave_row", row.id)),
                                egui::Sense::click(),
                            );
                            response.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::Role::RadioButton,
                                    true,
                                    st.grave_sel == Some(index),
                                    &row.title,
                                )
                            });
                            if response.clicked() {
                                st.grave_sel = Some(index);
                            }
                            let selected = st.grave_sel == Some(index);
                            colors.card_surface(ui.painter(), rect.shrink2(egui::vec2(0.0, 3.0)), selected, response.hovered());
                            // Subtle per-row controls, revealed with the row.
                            // Registered after the row's own interaction, so a
                            // button click never selects the row as well.
                            let btn = 28.0;
                            let kill_rect = egui::Rect::from_min_size(
                                egui::pos2(rect.right() - 10.0 - btn, rect.top() + 13.0),
                                egui::vec2(btn, btn),
                            );
                            let raise_rect = egui::Rect::from_min_size(
                                egui::pos2(kill_rect.left() - 6.0 - btn, rect.top() + 13.0),
                                egui::vec2(btn, btn),
                            );
                            // Reveal is driven by a geometric pointer-in-row check,
                            // not response.hovered(): hovering a button puts that
                            // button on top, which de-hovers the row and would
                            // collapse reveal, dropping the button to hover-only
                            // sense so the row wins the click tie-break.
                            let pointer_in_row = ui.ctx().input(|i| {
                                i.pointer.interact_pos().is_some_and(|pos| rect.contains(pos))
                            });
                            let reveal = crate::ui_style::transition(
                                ui.ctx(),
                                ui.scope_id().with(("grave_btns", row.id)),
                                pointer_in_row || selected,
                            );
                            if icon_button_faded(ui, raise_rect, Icon::Rerun, "Raise tab", &colors, reveal).clicked() {
                                st.grave_out = Some((index, "raise".to_string()));
                            }
                            if icon_button_faded(ui, kill_rect, Icon::Close, "Kill for good", &colors, reveal).clicked() {
                                st.grave_out = Some((index, "kill".to_string()));
                            }
                            let preview = thumbs.iter().find(|(id, _, _)| *id == row.id)
                                .map(|(_, handle, size)| (*handle, *size));
                            let well = egui::Rect::from_min_size(
                                egui::pos2(rect.left() + 10.0, rect.top() + 10.0),
                                egui::vec2(128.0, 72.0),
                            );
                            preview_image(ui, well, preview, &colors);
                            let text_x = well.right() + 12.0;
                            text(ui, egui::Rect::from_min_max(
                                egui::pos2(text_x, rect.top() + 12.0),
                                egui::pos2(raise_rect.left() - 8.0, rect.top() + 32.0),
                            ), &row.title, fg, 14.0, true);
                            text(ui, egui::Rect::from_min_max(
                                egui::pos2(text_x, rect.top() + 36.0),
                                egui::pos2(rect.right() - 12.0, rect.top() + 56.0),
                            ), if row.cwd.is_empty() { "\u{2014}" } else { &row.cwd }, muted, 12.0, false);
                            let when = egui::Rect::from_min_max(
                                egui::pos2(text_x, rect.bottom() - 34.0),
                                egui::pos2(rect.right() - 12.0, rect.bottom() - 12.0),
                            );
                            draw_icon(ui.painter(), Icon::Clock, egui::pos2(when.left() + 6.0, when.center().y), muted);
                            text(ui, when.with_min_x(when.left() + 18.0), &format!("buried {}", row.buried), muted, 12.0, false);
                            response.on_hover_text(format!("{}\n{}\nburied {}", row.title, row.cwd, row.buried))
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                            ui.add_space(8.0);
                        }
                    });
                if let Some(index) = st.grave_sel.filter(|i| *i < count) {
                    let row = &st.grave_rows[index];
                    ui.add_space(14.0);
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 210.0), egui::Sense::hover());
                    ui.painter().rect(rect, 9.0, card, egui::Stroke::new(1.0, border), egui::StrokeKind::Inside);
                    let left = rect.left() + 13.0;
                    text(ui, egui::Rect::from_min_max(
                        egui::pos2(left, rect.top() + 8.0),
                        egui::pos2(rect.right() - 12.0, rect.top() + 30.0),
                    ), &row.title, fg, 15.0, true);
                    for (i, (label, value)) in [
                        ("Buried", row.buried.as_str()),
                        ("Path", if row.cwd.is_empty() { "\u{2014}" } else { &row.cwd }),
                    ].into_iter().enumerate()
                    {
                        let y = rect.top() + 42.0 + i as f32 * 26.0;
                        let line = egui::Rect::from_min_size(egui::pos2(left, y), egui::vec2(rect.width() - 26.0, 26.0));
                        text(ui, line.with_max_x(left + 80.0), label, muted, 14.0, false);
                        text(ui, line.with_min_x(left + 84.0), value, fg, 14.0, false);
                    }
                    ui.painter().hline(rect.x_range(), rect.top() + 100.0, egui::Stroke::new(1.0, border));
                    for (i, (label, name, icon, color)) in [
                        ("Raise tab", "raise", Icon::Rerun, accent),
                        ("Kill for good", "kill", Icon::Close, muted),
                    ].into_iter().enumerate()
                    {
                        let button = egui::Rect::from_min_size(
                            egui::pos2(rect.left() + 7.0, rect.top() + 112.0 + i as f32 * 46.0),
                            egui::vec2(rect.width() - 14.0, 40.0),
                        );
                        let response = action_row(ui, button, ui.scope_id().with(("grave_action", name)), label, "", Some(icon), true, &colors);
                        if response.clicked() {
                            st.grave_out = Some((index, name.to_string()));
                        }
                        let _ = color;
                        response.on_hover_cursor(egui::CursorIcon::PointingHand);
                    }
                }
            });
        });
}

/// Filter once for both keyboard navigation and the visible cards.
fn visible_workspaces(st: &UiState) -> Vec<usize> {
    let query = st.ws_query.trim().to_lowercase();
    st.ws_rows.iter().filter(|row| {
        (!st.ws_populated_only || row.count > 0)
            && (query.is_empty()
                || format!("Workspace {} {} {}", row.index + 1, row.genre, row.title).to_lowercase().contains(&query)
                || row.tabs.iter().any(|tab| tab.title.to_lowercase().contains(&query)))
    }).map(|row| row.index).collect()
}

/// Workspace cards share the terminal UI's surfaces and identity colors. The
/// keyboard selection expands in place; terminal wells are real GPU previews.
pub fn workspaces_panel(
    ui: &mut egui::Ui,
    st: &mut UiState,
    ok: egui::Color32,
    err: egui::Color32,
    top: f32,
    colors_ws: &[egui::Color32],
) {
    st.ws_out = None;
    st.ws_tab_out = None;
    st.ws_visible_tabs.clear();
    if !st.panel_is(RightPanel::Workspaces) { return; }
    let ctx = ui.ctx().clone();
    if !st.menu_open && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        release_focus(&ctx);
        if st.ws_search_open {
            st.ws_search_open = false;
            st.ws_query.clear();
        } else {
            st.close_panel(RightPanel::Workspaces);
        }
        return;
    }
    let colors = TermStyle::new(ui, ok, err);
    let TermStyle { bg, fg, muted, border, accent, .. } = colors;
    let mut host = ui.max_rect();
    host.min.y = host.min.y.max(top);
    let width = (host.width() * 0.44).clamp(360.0, 600.0).min(host.width());
    host.min.x = host.max.x - width;
    egui::Area::new(egui::Id::unique("mypixels_workspaces_area"))
        .fixed_pos(host.min).order(egui::Order::Middle).fade_in(false)
        .movable(false).constrain(false)
        .show(&ctx, |ui| {
            ui.set_max_size(host.size());
            egui::Frame::new().fill(bg).inner_margin(14.0).show(ui, |ui| {
                ui.set_width((host.width() - 28.0).max(0.0));
                ui.set_min_height((host.height() - 28.0).max(0.0));
                colors.apply_controls(ui);
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                ui.painter().vline(host.left(), host.y_range(), egui::Stroke::new(1.0, border));
                let (header, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 36.0), egui::Sense::hover());
                draw_icon(ui.painter(), Icon::Layers, egui::pos2(header.left() + 12.0, header.center().y), fg);
                text(ui, header.with_min_x(header.left() + 32.0).with_max_x(header.left() + 140.0), "Workspaces", fg, 16.0, true);
                let badge = egui::Rect::from_center_size(egui::pos2(header.left() + 150.0, header.center().y), egui::vec2(24.0, 23.0));
                ui.painter().rect_filled(badge, 7.0, colors.card.lerp_to_gamma(fg, 0.07));
                crate::ui_style::text_centered(ui, badge, &st.ws_rows.iter().filter(|row| row.count > 0).count().to_string(), fg, 12.0, true);
                let control = |slot: f32| egui::Rect::from_center_size(
                    egui::pos2(header.right() - 15.0 - slot * 31.0, header.center().y), egui::vec2(28.0, 28.0));
                let search_key = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F));
                let search_clicked = icon_button(ui, control(3.0), Icon::Search, "Search workspaces", &colors).clicked();
                let focus_search = search_key || (search_clicked && !st.ws_search_open);
                if search_clicked || search_key {
                    st.ws_search_open = search_key || !st.ws_search_open;
                    if !st.ws_search_open { st.ws_query.clear(); release_focus(&ctx); }
                }
                let mut filter_colors = colors;
                if st.ws_populated_only { filter_colors.fg = accent; }
                if icon_button(ui, control(2.0), Icon::Filter, "Show only populated workspaces", &filter_colors).clicked() {
                    st.ws_populated_only = !st.ws_populated_only;
                }
                let empty = st.ws_rows.iter().find(|row| row.count == 0).map(|row| row.index);
                let mut add_colors = colors;
                if empty.is_none() { add_colors.fg = muted; }
                if icon_button(ui, control(1.0), Icon::Plus,
                    if empty.is_some() { "Open an empty workspace" } else { "All ten workspaces are in use" }, &add_colors).clicked()
                    && let Some(index) = empty {
                    st.ws_populated_only = false;
                    st.ws_query.clear();
                    st.ws_sel = Some(index);
                    st.ws_collapsed = None;
                    st.ws_out = Some(index);
                }
                if icon_button(ui, control(0.0), Icon::Dismiss, "Close workspaces", &colors).clicked() {
                    st.close_panel(RightPanel::Workspaces);
                }
                if st.ws_search_open {
                    ui.add_space(8.0);
                    let response = ui.add(egui::TextEdit::singleline(&mut st.ws_query)
                        .id_salt("workspace_search").hint_text("Find a workspace, genre or tab…")
                        .desired_width(ui.available_width()));
                    if focus_search { response.request_focus(); }
                    ui.add_space(6.0);
                }
                ui.add_space(8.0);
                let visible = visible_workspaces(st);
                let nav = ui.input(|i| i.key_pressed(egui::Key::ArrowUp) || i.key_pressed(egui::Key::ArrowDown));
                if !visible.contains(&st.ws_sel.unwrap_or(usize::MAX)) { st.ws_sel = visible.first().copied(); }
                if nav && !visible.is_empty() {
                    let cur = visible.iter().position(|index| Some(*index) == st.ws_sel).unwrap_or(0);
                    let up = ui.input(|i| i.key_pressed(egui::Key::ArrowUp));
                    st.ws_sel = Some(visible[if up { (cur + visible.len() - 1) % visible.len() } else { (cur + 1) % visible.len() }]);
                    st.ws_collapsed = None;
                }
                let editing = ui.memory(|m| m.focused()).is_some_and(|id| egui::text_edit::TextEditState::load(&ctx, id).is_some());
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    st.ws_out = st.ws_sel;
                }
                if !editing {
                    let left = ui.input(|i| i.key_pressed(egui::Key::ArrowLeft));
                    let right = ui.input(|i| i.key_pressed(egui::Key::ArrowRight));
                    if left != right
                        && let Some(row) = st.ws_sel.and_then(|index| st.ws_rows.get(index))
                        && let Some(id) = neighboring_workspace_tab(row, left) {
                        st.ws_tab_out = Some((row.index, id));
                        st.ws_collapsed = None;
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::Home)) { st.ws_sel = visible.first().copied(); }
                    if ui.input(|i| i.key_pressed(egui::Key::End)) { st.ws_sel = visible.last().copied(); }
                }
                let scroll_selected = ctx.data_mut(|data| {
                    let id = egui::Id::unique("workspace_selection_viewport");
                    let value = (st.ws_sel, host.size());
                    let changed = data.get_temp::<(Option<usize>, egui::Vec2)>(id) != Some(value);
                    data.insert_temp(id, value);
                    nav || changed
                });
                let footer_h = 25.0;
                egui::ScrollArea::vertical().id_salt("workspaces_list")
                    .auto_shrink([false, false]).max_height((ui.available_height() - footer_h).max(0.0))
                    .show(ui, |ui| {
                        if visible.is_empty() {
                            ui.add_space(32.0);
                            ui.label(egui::RichText::new("No matching workspaces").color(muted));
                        }
                        for index in visible {
                            let row = &st.ws_rows[index];
                            let selected = st.ws_sel == Some(index);
                            let color = colors_ws.get(index).copied().unwrap_or(accent);
                            let expanded = selected && row.count > 0 && st.ws_collapsed != Some(index);
                            let tile_w = ((ui.available_width() - 40.0) / 3.0).clamp(100.0, 174.0);
                            let tile_h = (tile_w * 0.60).clamp(64.0, 104.0);
                            let height = if expanded { 68.0 + tile_h + 31.0 } else { 68.0 };
                            let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::hover());
                            let heading = rect.with_max_y(rect.top() + 66.0);
                            let response = ui.interact(heading.with_max_x(rect.right() - 29.0), ui.scope_id().with(("ws_row", index)), egui::Sense::click());
                            let disclosure = ui.interact(heading.with_min_x(rect.right() - 29.0),
                                ui.scope_id().with(("ws_disclosure", index)), egui::Sense::click());
                            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Workspace {}, {} tabs, {}", index + 1, row.count, row.genre)));
                            let hover = response.hovered() || disclosure.hovered();
                            let fill = bg.lerp_to_gamma(color, if selected { 0.085 } else if hover { 0.055 } else { 0.015 });
                            ui.painter().rect(rect, 11.0, fill, egui::Stroke::new(if selected { 1.5 } else { 1.0 },
                                if selected { color } else if hover { border.lerp_to_gamma(color, 0.45) } else { border }), egui::StrokeKind::Inside);
                            let mark = egui::Rect::from_center_size(egui::pos2(rect.left() + 27.0, heading.center().y), egui::vec2(30.0, 32.0));
                            crate::ui_style::workspace_icon(ui.painter(), row.icon, mark.center(), color);
                            let name_x = rect.left() + 53.0;
                            let badge = egui::Rect::from_center_size(egui::pos2(rect.right() - 60.0, heading.center().y), egui::vec2(62.0, 24.0));
                            ui.painter().rect_filled(badge, 12.0, fill.lerp_to_gamma(fg, 0.07));
                            let count = if row.count == 0 { "Empty".into() } else { format!("{} tab{}", row.count, if row.count == 1 { "" } else { "s" }) };
                            crate::ui_style::text_centered(ui, badge, &count, if row.count == 0 { muted } else { fg }, 11.0, false);
                            let compact_start = (name_x + 142.0).min(badge.left() - 8.0);
                            let name_right = if expanded { badge.left() - 8.0 } else { compact_start };
                            text(ui, egui::Rect::from_min_max(egui::pos2(name_x, heading.top() + 10.0), egui::pos2(name_right, heading.top() + 31.0)),
                                &format!("Workspace {}", index + 1), fg, 14.0, true);
                            text(ui, egui::Rect::from_min_max(egui::pos2(name_x, heading.top() + 33.0), egui::pos2(name_right, heading.top() + 52.0)),
                                if row.genre.is_empty() { &row.shortcut } else { &row.genre }, muted, 11.0, false);
                            let chevron = egui::pos2(rect.right() - 14.0, heading.center().y);
                            let points = if expanded { vec![chevron + egui::vec2(-4.0, -2.0), chevron + egui::vec2(0.0, 2.0), chevron + egui::vec2(4.0, -2.0)] }
                                else { vec![chevron + egui::vec2(-2.0, -4.0), chevron + egui::vec2(2.0, 0.0), chevron + egui::vec2(-2.0, 4.0)] };
                            ui.painter().add(egui::Shape::line(points, egui::Stroke::new(1.5, if selected { color } else { muted })));
                            if row.current {
                                ui.painter().circle_filled(egui::pos2(mark.right() - 1.0, mark.bottom() - 1.0), 2.5, color);
                            }
                            if !expanded {
                                let space = badge.left() - compact_start - 10.0;
                                if space >= 54.0 {
                                    let w = ((space - 34.0) / 2.0).clamp(32.0, 86.0);
                                    let strip = egui::Rect::from_min_size(egui::pos2(compact_start, heading.center().y - 20.0), egui::vec2(space, 40.0));
                                    if let Some(id) = workspace_tab_strip(ui, strip, row, egui::vec2(w, 40.0), false, color, &colors, &mut st.ws_visible_tabs) {
                                        st.ws_tab_out = Some((index, id));
                                        release_focus(&ctx);
                                    }
                                }
                            }
                            if response.clicked() {
                                st.ws_sel = Some(index);
                                st.ws_collapsed = None;
                                st.ws_out = Some(index);
                                release_focus(&ctx);
                            }
                            if disclosure.clicked() {
                                st.ws_sel = Some(index);
                                st.ws_collapsed = if expanded { Some(index) } else { None };
                                if row.count == 0 { st.ws_out = Some(index); }
                                release_focus(&ctx);
                            }
                            disclosure.on_hover_text(if row.count == 0 { "Open workspace" } else if expanded { "Collapse workspace" } else { "Expand workspace" })
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                            response.on_hover_text(format!("Workspace {} · {}\n{}\n{}", index + 1,
                                if row.genre.is_empty() { "No genre" } else { &row.genre }, row.title, row.shortcut))
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                            if expanded {
                                let preview_rect = egui::Rect::from_min_max(rect.min + egui::vec2(12.0, 64.0), rect.max - egui::vec2(12.0, 8.0));
                                if let Some(id) = workspace_tab_strip(ui, preview_rect, row, egui::vec2(tile_w, tile_h), true, color, &colors, &mut st.ws_visible_tabs) {
                                    st.ws_tab_out = Some((index, id));
                                    release_focus(&ctx);
                                }
                            }
                            if scroll_selected && selected { ui.scroll_to_rect(rect, Some(egui::Align::Center)); }
                            ui.add_space(8.0);
                        }
                    });
                ui.add_space(7.0);
                let (hint, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 18.0), egui::Sense::hover());
                text(ui, hint, "↑↓ workspace · ←→ tab · Enter go · Esc close", muted, 10.0, false);
            });
        });
    // The panel is a live view, including terminals parked in other workspaces.
    ctx.request_repaint_after(std::time::Duration::from_millis(200));
}

#[derive(Clone, Default)]
struct WorkspaceStripState {
    focus: Option<(usize, usize)>,
    count: usize,
    width: f32,
    last_frame: Option<u64>,
}

/// A workspace keeps its own scroll position in each presentation. Follow its
/// focused terminal when focus/layout changes or the view opens; otherwise let
/// the user browse freely with the wheel or trackpad. Focus changes only move
/// the viewport when the newly focused card would otherwise be clipped.
fn workspace_tab_strip(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    row: &WsRow,
    tile: egui::Vec2,
    expanded: bool,
    color: egui::Color32,
    colors: &TermStyle,
    visible_tabs: &mut Vec<usize>,
) -> Option<usize> {
    let gap = if expanded { 8.0 } else { 6.0 };
    let pitch = tile.x + gap;
    let content_width = row.tabs.len() as f32 * pitch - gap;
    let overflow = content_width > rect.width() + 0.5;
    let gutter = if !overflow { 0.0 } else if expanded { 24.0 } else if rect.width() < 70.0 { 10.0 } else { 14.0 };
    let viewport = rect.shrink2(egui::vec2(gutter, 0.0));
    let id = egui::Id::unique("workspace_tab_strip").with((row.index, expanded));
    let focus = row.tabs.iter().enumerate().find(|(_, tab)| tab.selected).map(|(index, tab)| (tab.id, index));
    let frame = ui.ctx().cumulative_frame_nr();
    let mut state = ui.ctx().data_mut(|data| data.get_temp::<WorkspaceStripState>(id).unwrap_or_default());
    let reset = state.count != row.tabs.len() || state.width != viewport.width()
        || state.last_frame.is_none_or(|last| frame > last + 1);
    let follow_focus = state.focus != focus && !reset;
    let offset = if reset {
        focus.map(|(_, index)| (index as f32 * pitch + tile.x * 0.5 - viewport.width() * 0.5)
            .clamp(0.0, (content_width - viewport.width()).max(0.0)))
    } else { None };
    state.focus = focus;
    state.count = row.tabs.len();
    state.width = viewport.width();
    state.last_frame = Some(frame);
    let mut child = ui.new_child(egui::UiBuilder::new().id_salt(("workspace_tabs", row.index, expanded)).max_rect(viewport));
    child.set_clip_rect(ui.clip_rect().intersect(viewport));
    let mut scroll = egui::ScrollArea::horizontal().id_salt("previews")
        .animated(ui.ctx().global_style().animation_time > 0.0)
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .auto_shrink([false, false]).max_width(viewport.width()).max_height(viewport.height());
    if let Some(offset) = offset { scroll = scroll.horizontal_scroll_offset(offset); }
    let output = scroll.show(&mut child, |ui| {
        ui.spacing_mut().item_spacing.x = gap;
        ui.horizontal(|ui| {
            let mut pick = None;
            for (number, tab) in row.tabs.iter().enumerate() {
                let (tile_rect, response) = ui.allocate_exact_size(
                    egui::vec2(tile.x, tile.y + if expanded { 23.0 } else { 0.0 }), egui::Sense::click());
                let well = tile_rect.with_max_y(tile_rect.top() + tile.y);
                if follow_focus && tab.selected
                    && (well.left() < ui.clip_rect().left() || well.right() > ui.clip_rect().right()) {
                    ui.scroll_to_rect_animation(well, Some(egui::Align::Center),
                        egui::style::ScrollAnimation::duration(ui.ctx().global_style().animation_time));
                }
                if ui.is_rect_visible(well) { visible_tabs.push(tab.id); }
                workspace_preview(ui, well, tab, colors);
                if tab.selected || response.hovered() {
                    ui.painter().rect_stroke(well, 7.0, egui::Stroke::new(1.5, color), egui::StrokeKind::Inside);
                }
                if expanded {
                    text(ui, tile_rect.with_min_y(well.bottom() + 2.0).shrink2(egui::vec2(4.0, 0.0)),
                        &format!("{}  {}", number + 1, tab.title), if tab.selected { colors.fg } else { colors.muted }, 10.0, false);
                }
                if response.clicked() { pick = Some(tab.id); }
                response.on_hover_text(&tab.title).on_hover_cursor(egui::CursorIcon::PointingHand);
            }
            pick
        }).inner
    });
    let offset = output.state.clamped_offset().x;
    let max_offset = (output.content_size.x - output.inner_rect.width()).max(0.0);
    let mut pick = output.inner;
    if overflow {
        for (left, available, x) in [(true, offset > 0.5, rect.left() + gutter * 0.5),
            (false, offset < max_offset - 0.5, rect.right() - gutter * 0.5)] {
            let Some(neighbor) = neighboring_workspace_tab(row, left).filter(|_| available) else { continue; };
            let button = egui::Rect::from_center_size(egui::pos2(x, rect.top() + tile.y * 0.5), egui::vec2(gutter, tile.y.min(30.0)));
            let response = ui.interact(button, id.with(left), egui::Sense::click());
            let label = format!("Focus {} tab in workspace {}", if left { "Previous" } else { "Next" }, row.index + 1);
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
            if response.hovered() { ui.painter().rect_filled(button, 5.0, colors.card); }
            draw_icon(ui.painter(), if left { Icon::ChevronLeft } else { Icon::ChevronRight }, button.center(),
                if response.hovered() { color } else { colors.muted });
            if response.clicked() {
                pick = Some(neighbor);
            }
            response.on_hover_text(label).on_hover_cursor(egui::CursorIcon::PointingHand);
        }
    }
    ui.ctx().data_mut(|data| data.insert_temp(id, state));
    pick
}

fn neighboring_workspace_tab(row: &WsRow, left: bool) -> Option<usize> {
    let current = row.tabs.iter().position(|tab| tab.selected)?;
    let next = if left { current.checked_sub(1)? } else { current.checked_add(1)? };
    row.tabs.get(next).map(|tab| tab.id)
}

fn workspace_preview(ui: &egui::Ui, rect: egui::Rect, tab: &WsTab, colors: &TermStyle) {
    if tab.preview.is_some() {
        preview_image(ui, rect, tab.preview, colors);
    } else {
        ui.painter().rect(rect, 7.0, colors.bg, egui::Stroke::new(1.0, colors.border), egui::StrokeKind::Inside);
        draw_icon(ui.painter(), Icon::Terminal, rect.center(), colors.muted);
    }
}

/// The rename box: a small frameless window with one text field. Enter
/// commits, anything that drops the focus (Escape, a click elsewhere)
/// cancels. Returns whatever `UiState::rename_out` received this frame.
pub fn rename_box(ui: &egui::Ui, st: &mut UiState) {
    if !st.rename_open {
        st.rename_out = None;
        return;
    }
    let ctx = ui.ctx();
    let pos = egui::pos2(st.rename_pos[0], st.rename_pos[1]);
    let seed = st.rename_seed;
    let mut out: Option<Option<String>> = None;
    egui::Window::new("rename_tab")
        .id(egui::Id::unique("rename_tab_box"))
        .title_bar(false)
        .movable(false)
        .resizable(false)
        .fixed_pos(pos)
        .constrain(false)
        .show(&ctx, |ui| {
            // Deliberately no lock_focus: losing the focus *is* the cancel
            // gesture (Escape, or a click anywhere else in the window).
            let te = egui::TextEdit::singleline(&mut st.rename_draft).desired_width(280.0);
            let resp = ui.add(te);
            if seed {
                resp.request_focus();
                st.rename_seed = false;
                // egui 0.37 dropped select_all_on, so pre-select through the
                // edit's stored cursor state: the whole draft reads as
                // selected text, and the first typed key replaces it.
                let id = resp.id;
                let len = st.rename_draft.chars().count();
                let mut state =
                    egui::text_edit::TextEditState::load(&ctx, id).unwrap_or_default();
                state.cursor.set_char_range(Some(egui::text_selection::CCursorRange::two(
                    egui::epaint::text::cursor::CCursor::new(0),
                    egui::epaint::text::cursor::CCursor::new(len),
                )));
                state.store(&ctx, id);
            }
            if resp.lost_focus() {
                // egui drops the edit's focus on Enter and on Escape alike;
                // only Enter commits. A click on another widget lands here.
                out = Some(if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    Some(st.rename_draft.clone())
                } else {
                    None
                });
            } else if ctx.input(|i| {
                // A press on empty background (the terminal, mostly) does
                // not steal egui focus, so treat it as cancel by hand. The
                // app also surrenders egui focus on its own mouse presses;
                // whichever fires first settles it.
                i.pointer.any_pressed()
                    && i
                        .pointer
                        .interact_pos()
                        .is_some_and(|p| !resp.rect.expand(8.0).contains(p))
            }) {
                out = Some(None);
            }
        });
    st.rename_out = out;
}

/// Only this session's daemon or another native window can retain terminals.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum WindowRetainer {
    Daemon,
    OtherWindow,
    #[default]
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseChoice { Cancel, Graveyard, Permanent }

#[derive(Default)]
pub struct CloseDialog {
    pub retainer: WindowRetainer,
    pub tabs: usize,
    pub workspaces: usize,
    pub limit: usize,
    pub retained_tabs: usize,
    permanent: bool,
    unlock_at: Option<f64>,
}

impl CloseDialog {
    pub const DELAY: f64 = 5.0;

    pub fn new(retainer: WindowRetainer, tabs: usize, workspaces: usize, limit: usize) -> Self {
        Self { retainer, tabs, workspaces, limit, ..Default::default() }
    }

    /// A lost recovery host requires a fresh warning and countdown. Never
    /// reinterpret an already displayed recovery action as permanent closure.
    pub fn set_retainer(&mut self, retainer: WindowRetainer) -> bool {
        if self.retainer == retainer { return false; }
        self.retainer = retainer;
        self.permanent = false;
        self.unlock_at = None;
        true
    }

    fn danger(&self) -> bool {
        self.permanent || self.retainer == WindowRetainer::None
    }

    pub fn permanent_ready(&self, now: f64) -> bool {
        self.danger() && self.unlock_at.is_some_and(|at| now >= at)
    }
}

/// A centered, input-blocking close card. Cancel is always the default;
/// recovery is explicit, and no timer or bare Enter can destroy terminals.
pub fn confirm_box(ui: &egui::Ui, st: &mut UiState) {
    st.confirm_out = None;
    if !st.confirm_open { return; }
    let ctx = ui.ctx();
    let now = ctx.input(|i| i.time);
    let danger = st.confirm.danger();
    let remaining = if danger {
        let at = *st.confirm.unlock_at.get_or_insert(now + CloseDialog::DELAY);
        (at - now).max(0.0)
    } else { 0.0 };
    if remaining > 0.0 {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    let c = TermStyle::new(ui, egui::Color32::from_rgb(59, 193, 126),
        egui::Color32::from_rgb(239, 97, 109));
    let seed = std::mem::take(&mut st.confirm_seed);
    let width = (ctx.content_rect().width() - 48.0).clamp(220.0, 640.0);
    let mut out = None;
    let response = egui::Modal::new(egui::Id::unique("confirm_box"))
        .backdrop_color(egui::Color32::from_black_alpha(115))
        .frame(egui::Frame::new().fill(c.bg)
            .stroke(egui::Stroke::new(1.0, c.bg.lerp_to_gamma(c.fg, 0.32)))
            .corner_radius(12.0).inner_margin(24.0))
        .show(ctx, |ui| {
            ui.set_width(width);
            c.apply_controls(ui);
            // Proportional typography matches the reference's dialog, while
            // the terminal and the rest of the UI retain their own font.
            for style in [egui::TextStyle::Body, egui::TextStyle::Button] {
                ui.style_mut().text_styles.insert(style, egui::FontId::proportional(15.0));
            }
            let title = if danger { "Close window permanently?" } else { "Close window?" };
            let title_galley = ui.painter().layout(title.into(), egui::FontId::proportional(22.0), c.fg, (width - 95.0).max(100.0));
            let title_height = title_galley.size().y;
            let (header, _) = ui.allocate_exact_size(egui::vec2(width, (title_height + 30.0).max(54.0)), egui::Sense::hover());
            draw_icon(ui.painter(), if danger { Icon::Error } else { Icon::Archive },
                egui::pos2(header.left() + 14.0, header.top() + 16.0), if danger { c.err } else { c.fg });
            ui.painter().galley(header.min + egui::vec2(48.0, 0.0), title_galley, c.fg);
            let counts = format!("{} workspace{}  ·  {} tab{}", st.confirm.workspaces,
                if st.confirm.workspaces == 1 { "" } else { "s" }, st.confirm.tabs,
                if st.confirm.tabs == 1 { "" } else { "s" });
            ui.painter().text(header.min + egui::vec2(48.0, title_height + 6.0), egui::Align2::LEFT_TOP,
                counts, egui::FontId::proportional(14.0), c.muted);
            let dismiss = egui::Rect::from_center_size(
                egui::pos2(header.right() - 7.0, header.top() + 9.0), egui::vec2(30.0, 30.0));
            if icon_button(ui, dismiss, Icon::Dismiss, "Cancel window close", &c).clicked() {
                out = Some(CloseChoice::Cancel);
            }
            ui.add_space(10.0);
            let footer_height = if width < 500.0 {
                if st.confirm.retainer == WindowRetainer::None { 88.0 } else { 136.0 }
            } else { 40.0 };
            egui::ScrollArea::vertical().id_salt("close_dialog_details")
                .max_height((ctx.content_rect().height() - header.height() - footer_height - 110.0).max(64.0))
                .auto_shrink([false, true]).show(ui, |ui| {
                    ui.label(egui::RichText::new(if danger && st.confirm.retainer == WindowRetainer::None && st.confirm.retained_tabs > 0 {
                        format!("This will end the session, terminating this window and {} previously closed terminal(s). Workspaces, running programs and terminal contents cannot be restored.", st.confirm.retained_tabs)
                    } else if danger {
                        "This will terminate every terminal in this window. Its workspaces, running programs and terminal contents cannot be restored.".into()
                    } else {
                        "Closing this window will move it to the Graveyard. Its workspaces and tabs can be restored while the session keeps them alive.".into()
                    }).color(c.fg).size(16.0));
                    ui.add_space(16.0);
                    let mut detail = |glyph, label: String, color| {
                        ui.horizontal(|ui| {
                            let (rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 22.0), egui::Sense::hover());
                            draw_icon(ui.painter(), glyph, rect.center(), color);
                            ui.add(egui::Label::new(egui::RichText::new(label).color(color).size(14.0)).wrap());
                        });
                    };
                    if danger {
                        detail(Icon::Layers, if st.confirm.retainer == WindowRetainer::None {
                            "No background daemon or other open window can retain these terminals.".into()
                        } else { "Permanent close bypasses the Graveyard.".into() }, c.muted);
                        detail(Icon::Clock, if remaining > 0.0 {
                            format!("Take a moment to check — permanent close unlocks in {} s.", remaining.ceil() as u64)
                        } else { "This cannot be undone.".into() }, c.err);
                    } else {
                        detail(Icon::Clock, format!("Keeps up to {} closed window{}; the oldest is evicted when full.", st.confirm.limit, if st.confirm.limit == 1 { "" } else { "s" }), c.muted);
                        detail(Icon::Layers, match st.confirm.retainer {
                            WindowRetainer::Daemon => "Window retained by the background daemon",
                            _ => "Window retained by another open window",
                        }.into(), c.muted);
                    }
                });
            ui.add_space(18.0);
            ui.separator();
            ui.add_space(10.0);
            // Explicit footer geometry keeps the primary action at the right
            // edge without allowing button widths to expand the centered card.
            let compact = width < 500.0;
            let has_back = st.confirm.retainer != WindowRetainer::None;
            let primary_w: f32 = if danger { 220.0 } else { 185.0 };
            let height = if compact { if has_back { 136.0 } else { 88.0 } } else { 40.0 };
            let (footer, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
            let primary = egui::Rect::from_min_size(
                if compact { footer.min + egui::vec2(0.0, 48.0) }
                else { egui::pos2(footer.right() - primary_w, footer.top()) },
                egui::vec2(if compact { width } else { primary_w }, 40.0));
            let cancel_rect = egui::Rect::from_min_size(
                if compact { footer.min } else { egui::pos2(primary.left() - 102.0, footer.top()) },
                egui::vec2(if compact { width } else { 90.0 }, 40.0));
            let cancel = ui.put(cancel_rect, egui::Button::new("Cancel"));
            if seed { cancel.request_focus(); }
            if cancel.clicked() { out = Some(CloseChoice::Cancel); }
            if danger {
                let label = if remaining > 0.0 { format!("Close permanently ({} s)", remaining.ceil() as u64) }
                    else { "Close permanently".into() };
                let red = if remaining > 0.0 { c.bg.lerp_to_gamma(c.err, 0.25) } else { c.err };
                let clicked = ui.push_id("permanent_close", |ui| {
                    ui.add_enabled_ui(remaining == 0.0, |ui| {
                        ui.put(primary, egui::Button::new(egui::RichText::new(label)
                            .color(if remaining > 0.0 { c.muted } else { egui::Color32::BLACK }))
                            .fill(red).stroke(egui::Stroke::new(1.0, c.err))).clicked()
                    }).inner
                }).inner;
                if clicked && st.confirm.permanent_ready(now) { out = Some(CloseChoice::Permanent); }
            } else if ui.push_id("graveyard_close", |ui| ui.put(primary,
                egui::Button::new(egui::RichText::new("Move to Graveyard").color(egui::Color32::BLACK))
                    .fill(c.accent))).inner.clicked() {
                out = Some(CloseChoice::Graveyard);
            }
            if has_back {
                let rect = if compact {
                    egui::Rect::from_min_size(footer.min + egui::vec2(0.0, 96.0), egui::vec2(width, 40.0))
                } else { egui::Rect::from_min_size(footer.min, egui::vec2(165.0, 40.0)) };
                let label = if danger { "Back to recovery" } else { "Close permanently…" };
                if ui.put(rect, egui::Button::new(egui::RichText::new(label).color(c.muted)).frame(false)).clicked() {
                    st.confirm.permanent = !danger;
                    st.confirm.unlock_at = if danger { None } else { Some(now + CloseDialog::DELAY) };
                    st.confirm_seed = true;
                    ctx.request_repaint();
                }
            }
        });
    if response.should_close() || (out.is_none() && ctx.memory(|m| m.focused()).is_none()
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))) {
        out = Some(CloseChoice::Cancel);
    }
    st.confirm_out = out;
}

/// The font selector popup, opened from one of the two font rows. It works
/// like the classic font-selection dialog: family, style, size, and a live
/// preview; nothing reaches the config until OK.
pub struct FontPopup {
    /// The row it belongs to; the chosen description is written there.
    pub which: FontWhich,
    /// egui's open flag; click-outside (or OK/Cancel) flips it to false.
    pub open: bool,
    /// The filter box contents, persisted per frame while open.
    pub search: String,
    // ---- Draft (the popup's own selection, not yet the config's) ----
    /// The drafted family name.
    pub family: String,
    /// Index of `family` in the choices slice ([`usize::MAX`] when the
    /// drafted family is not installed -- the config can name anything).
    /// Carrying the index keeps the per-frame path free of name lookups.
    pub family_idx: usize,
    /// Index into the family's face list (style).
    pub style: usize,
    /// Drafted size in points.
    pub size_pt: f32,
    /// The rendered family list: `(filter, drafted family, choice indices)`.
    /// Rebuilt only when one of its two inputs changes -- a keystroke or a
    /// family click -- never per frame, over what can be hundreds of
    /// families. (Filtering an already-sorted list stays sorted; the
    /// drafted family is rotated to the front.)
    pub list_cache: Option<(String, String, Vec<usize>)>,
}

/// Which font row a [`FontPopup`] belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FontWhich {
    Term,
    Tablist,
}

/// egui family name for one face's preview. Faces live under a name built
/// from the family, weight and slant so a family's styles cannot overwrite
/// each other in egui's face store (which dedupes installs by name).
pub fn preview_family_name(family: &str, weight: u16, slant: crate::fonts::DbStyle) -> String {
    let slope = match slant {
        crate::fonts::DbStyle::Normal => 'n',
        crate::fonts::DbStyle::Italic => 'i',
        crate::fonts::DbStyle::Oblique => 'o',
    };
    format!("{family}\u{1}{weight}{slope}")
}

/// Serves the installed font files to egui's text layout, on first use.
///
/// The font popup previews families and styles egui does not ship; this
/// provider is where their bytes come from. `fonts_for_family` is asked once
/// per family name at layout time, so each previewed file is read exactly
/// once per session and only when it is actually shown.
struct FontPreviewProvider {
    fonts: std::sync::Mutex<std::collections::HashMap<String, (std::path::PathBuf, u32)>>,
}

impl egui::FontProvider for FontPreviewProvider {
    fn fonts_for_family(&self, family: &egui::FontFamily) -> Vec<egui::FontInsert> {
        // Match ergonomics: `name` is a `&Arc<str>`.
        let egui::FontFamily::Name(name) = family else {
            return Vec::new();
        };
        let Some((path, face)) = self.fonts.lock().unwrap().remove(name.as_ref()) else {
            return Vec::new();
        };
        match std::fs::read(&path) {
            Ok(bytes) => vec![egui::FontInsert {
                name: name.to_string(),
                data: egui::FontData::from_blob(std::sync::Arc::new(bytes), face),
                families: vec![egui::InsertFontFamily {
                    family: egui::FontFamily::Name(std::sync::Arc::from(&**name)),
                    priority: egui::FontPriority::Highest,
                }],
            }],
            Err(err) => {
                log::warn!("font preview: cannot read {path:?}: {err}");
                Vec::new()
            }
        }
    }
}

impl Egui {
    pub fn init(
        event_loop: &winit::event_loop::ActiveEventLoop,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        scale: f32,
        font_choices: Vec<crate::fonts::FontChoice>,
    ) -> Self {
        let ctx = Context::default();
        ctx.set_visuals(egui::Visuals::dark());
        // The font popup previews any installed family/style; this provider
        // hands the bytes over to egui's layout when a preview is first
        // drawn. One entry per face, under its preview family name.
        ctx.add_font_provider(std::sync::Arc::new(FontPreviewProvider {
            fonts: std::sync::Mutex::new(
                font_choices
                    .iter()
                    .flat_map(|c| {
                        c.faces.iter().map(move |f| {
                            (
                                preview_family_name(&c.name, f.weight, f.slant),
                                (f.path.clone(), f.index),
                            )
                        })
                    })
                    .collect(),
            ),
        }));
        let state = egui_winit::State::new(
            ctx.clone(),
            ViewportId::ROOT,
            event_loop,
            Some(scale),
            None,
            None,
        );
        // No MSAA, no depth: egui feathers its own edges, and the frame is
        // 2D. No dithering: it assumes sRGB output and would shift tones
        // against the app's hand-drawn colours.
        let mut renderer = egui_wgpu::Renderer::new(
            device,
            format,
            egui_wgpu::RendererOptions {
                msaa_samples: 1,
                depth_stencil_format: None,
                dithering: false,
                predictable_texture_filtering: false,
            },
        );
        // Warm the font atlas now. egui loads and rasterises its bundled
        // fonts lazily on the first pass that lays out text, which would
        // hitch the first config open by well over a frame. Lay the whole panel
        // window out once against a default config and upload the font
        // textures, so the first real open is ordinary UI work.
        {
            let mut warm_cfg = crate::config::Config::default();
            let mut warm_st = UiState::default();
            let mut open = true;
            let mut full = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_max(
                        egui::pos2(0.0, 0.0),
                        egui::pos2(1200.0, 800.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    config_window(ui, &mut open, &mut warm_cfg, &mut warm_st, &[]);
                },
            );
            apply_textures(device, queue, &mut renderer, &mut full);
        }

        Self {
            ctx,
            state,
            renderer,
            config_changed: false,
            save_requested: false,
            revert_requested: false,
            ui_state: UiState::default(),
            switcher_pick: None,
            font_choices,
            open_prev: (false, false),
        }
    }

    /// Feed one winit event to egui. Returns true when egui claims the
    /// event (config window open and under the pointer / focused) and the
    /// app should not handle it.
    pub fn on_event(&mut self, window: &winit::window::Window, event: &winit::event::WindowEvent) -> bool {
        let response = self.state.on_window_event(window, event);
        // Unfocused menus still need a frame to process Escape. Repainting
        // and claiming terminal input are independent egui responses.
        // RedrawRequested is the frame we are about to draw, not a request
        // for another one. Echoing it would spin even with no terminal damage.
        if response.repaint && !matches!(event, winit::event::WindowEvent::RedrawRequested) {
            window.request_redraw();
        }
        response.consumed
    }

    /// The card the pointer clicked on the previous frame, if any.
    pub fn take_switcher_pick(&mut self) -> Option<usize> {
        self.switcher_pick.take()
    }
}

/// One tab card in the switcher overlay.
#[derive(Clone, Copy, Default)]
pub enum SwitcherStatus {
    #[default]
    Unknown,
    Running,
    Passed,
    Failed,
    Completed,
}

pub struct SwitcherCard {
    pub id: usize,
    pub status: SwitcherStatus,
    pub title: String,
    /// The tab that is actually active (marked, like the strip does).
    pub active: bool,
    /// The entangled-tab marker the strip shows.
    pub marker: bool,
    /// Working directory shown in the card tooltip.
    pub cwd: Option<String>,
    /// The cached GPU image plus its display size in logical points.
    /// The size follows the terminal area's own aspect, so the image needs
    /// no letterboxing or cropping.
    pub thumb: Option<(egui::TextureId, (u32, u32))>,
}

const CARD_GAP: f32 = 14.0;
const CARD_PAD: f32 = 8.0;
const CARD_HEADER: f32 = 40.0;

/// The switcher's usable panel width.
fn switcher_panel_w(screen_width: f32) -> f32 {
    (screen_width * 0.84).min(1400.0)
}

/// Preview dimensions: the terminal frame shrunk by `scale`, capped so one
/// card still fits the panel. The aspect is the terminal's own, always --
/// the preview is a scaled picture of the tab, so it can never differ in
/// shape. The grid reflows around the size (fewer columns, more rows) rather
/// than squeezing it.
pub fn thumbnail_size(screen_width: f32, aspect: f32, scale: f32) -> (f32, f32) {
    if aspect <= 0.0 {
        return (0.0, 0.0);
    }
    let one_card = switcher_panel_w(screen_width) - 36.0 - CARD_PAD * 2.0;
    let box_w = (screen_width * scale).min(one_card).max(48.0);
    (box_w, box_w / aspect)
}

/// The card the preview sits in: padding around, never narrower than 204 so
/// the header and index chip keep their room.
pub fn switcher_card_width(screen_width: f32, aspect: f32, scale: f32) -> f32 {
    let (tw, _) = thumbnail_size(screen_width, aspect, scale);
    (tw + CARD_PAD * 2.0).max(204.0)
}

/// Columns shared by drawing and arrow-key navigation. The cards pick the
/// column count, not the other way round: as many as fit the panel.
pub fn switcher_columns(screen_width: f32, count: usize, card_w: f32) -> usize {
    let fit = ((switcher_panel_w(screen_width) - 36.0 + CARD_GAP) / (card_w + CARD_GAP)).floor().max(1.0) as usize;
    fit.min(4).min(count.max(1))
}

/// Release egui keyboard focus. A TextEdit that vanishes with its panel or
/// popup does NOT release focus by itself, and while egui holds focus,
/// egui-winit reports every key event as consumed -- the app then drops all
/// keystrokes and the terminal silently stops receiving input ("letters
/// vanish"). Call this whenever a UI region that could own the keyboard
/// steps aside, or when the app claims a click for itself.
pub fn release_focus(ctx: &egui::Context) {
    if let Some(id) = ctx.memory(|m| m.focused()) {
        ctx.memory_mut(|m| m.surrender_focus(id));
    }
}

/// Edge-triggered release: fires when the config panel or the font popup
/// transitions open -> closed. `prev` is the previous frame's state, owned
/// by the caller (the app).
pub fn release_focus_if_closed(
    ctx: &egui::Context,
    open: bool,
    popup_open: bool,
    prev: &mut (bool, bool),
) {
    if (prev.0 && !open) || (prev.1 && !popup_open) {
        release_focus(ctx);
    }
    *prev = (open, popup_open);
}

/// egui's look, built from the terminal's own colours: an egui panel that
/// ignores the theme the user configured for the terminal would be a second,
/// uglier app sharing the window. Everything derived (weak text, panel mix,
/// borders) is a mix between the configured background and foreground, so
/// light *and* dark palettes both get a coherent UI.
/// The switcher backdrop: the theme's background pressed towards dark,
/// not flat black. Over a light theme a black dim reads as smoke; over a
/// dark one the difference is subtle - either way the overlay now belongs
/// to the palette the user chose.
pub fn dim_fill(bg: [f32; 3], alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        (bg[0] * 0.30 * 255.0) as u8,
        (bg[1] * 0.30 * 255.0) as u8,
        (bg[2] * 0.30 * 255.0) as u8,
        alpha,
    )
}

/// The Alt-held switcher keeps its live previews inside a centered surface.
/// The grid scrolls on small windows and follows keyboard selection.
pub fn switcher_screen(
    ui: &mut egui::Ui,
    open: bool,
    selected: usize,
    dim_fill: egui::Color32,
    accent: egui::Color32,
    cards: &[SwitcherCard],
    preview: egui::Vec2,
    // Which workspace these tiles came from, while workspaces are on. `None`
    // leaves the header as it was: with one set of tabs there is nothing to name.
    workspace: Option<usize>,
    pick: &mut Option<usize>,
) {
    if !open || cards.is_empty() {
        return;
    }
    let mut colors =
        TermStyle::new(ui, egui::Color32::from_rgb(104, 239, 145), egui::Color32::from_rgb(245, 115, 126));
    colors.accent = accent;
    let screen = ui.max_rect();
    let card_size = egui::vec2((preview.x + CARD_PAD * 2.0).max(204.0), preview.y + CARD_HEADER + CARD_PAD);
    let cols = switcher_columns(screen.width(), cards.len(), card_size.x);
    let grid_w = card_size.x * cols as f32 + CARD_GAP * (cols - 1) as f32;
    let rows = cards.len().div_ceil(cols);
    let grid_h = card_size.y * rows as f32 + CARD_GAP * (rows - 1) as f32;
    let width = (grid_w + 36.0).max(440.0).min(screen.width() - 24.0);
    let viewport_h = grid_h.min((screen.height() - 180.0).max(80.0));
    let height = viewport_h + 134.0;
    let origin = egui::pos2(screen.center().x - width * 0.5, screen.center().y - height * 0.5);
    let selection_id = egui::Id::unique("switcher_last_selection");
    let view = (selected, cols, [screen.width().to_bits(), screen.height().to_bits(), preview.x.to_bits(), preview.y.to_bits()]);
    let previous = ui.ctx().data_mut(|d| {
        let previous = d.get_temp::<(usize, usize, [u32; 4])>(selection_id);
        d.insert_temp(selection_id, view);
        previous
    });
    let follow_selection = previous != Some(view);
    egui::CentralPanel::no_frame().frame(egui::Frame::NONE.fill(dim_fill)).show(ui, |ui| {
        ui.interact(ui.max_rect(), egui::Id::unique("switcher_backdrop"), egui::Sense::click());
    });
    egui::Area::new(egui::Id::unique("switcher_surface"))
        .order(egui::Order::Foreground)
        .fixed_pos(origin)
        .movable(false)
        .fade_in(false)
        .show(ui.ctx(), |ui| {
            ui.set_width(width);
            egui::Frame::new()
                .fill(colors.bg)
                .stroke(egui::Stroke::new(1.0, colors.border))
                .corner_radius(18.0)
                .inner_margin(18.0)
                .show(ui, |ui| {
                    ui.set_width(width - 36.0);
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                    let (header, _) =
                        ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), egui::Sense::hover());
                    draw_icon(
                        ui.painter(),
                        Icon::Layers,
                        egui::pos2(header.left() + 15.0, header.center().y),
                        colors.fg,
                    );
                    let title =
                        ui.painter().layout_no_wrap("Tab Switcher".into(), egui::FontId::proportional(24.0), colors.fg);
                    let title_x = header.left() + 42.0;
                    ui.painter().galley(
                        egui::pos2(title_x, header.center().y - title.size().y * 0.5),
                        title.clone(),
                        colors.fg,
                    );
                    let divider = title_x + title.size().x + 22.0;
                    let count = format!("{} open terminal{}", cards.len(), if cards.len() == 1 { "" } else { "s" });
                    ui.painter().vline(
                        divider,
                        header.center().y - 11.0..=header.center().y + 11.0,
                        egui::Stroke::new(1.0, colors.border),
                    );
                    text(
                        ui,
                        header.with_min_x(divider + 20.0),
                        &count,
                        colors.muted,
                        14.0,
                        false,
                    );
                    // The header's other half: which workspace these previews are
                    // the tabs of. Ten grids of live terminals look alike, so the
                    // count alone leaves the user guessing which set Alt is about
                    // to commit. Tinted with the identity color - the same one the
                    // selected card is outlined in - to agree with the indicator
                    // the tab bar shows.
                    if let Some(workspace) = workspace {
                        let full = format!("Workspace {}", workspace + 1);
                        // A name is worth its room, and the narrowest surface does
                        // not have it. A bare number is the same fact, shorter, so
                        // the header never truncates or overruns the count.
                        let label = if header.right() - (divider + 20.0 + text_width(ui, &count, 14.0) + 24.0)
                            >= text_width(ui, &full, 14.0)
                        {
                            full
                        } else {
                            (workspace + 1).to_string()
                        };
                        let label_w = text_width(ui, &label, 14.0);
                        text_right(ui, header, &label, colors.accent, 14.0, false);
                        // One rule, on its left: nothing else shares the right end
                        // of the line, and the count is a different kind of fact.
                        ui.painter().vline(
                            header.right() - label_w - 14.0,
                            header.center().y - 11.0..=header.center().y + 11.0,
                            egui::Stroke::new(1.0, colors.border),
                        );
                    }
                    ui.add_space(12.0);
                    egui::ScrollArea::vertical()
                        .id_salt("switcher_grid")
                        .animated(false)
                        .auto_shrink([false, false])
                        .max_height(viewport_h)
                        .show(ui, |ui| {
                            let inset = ((ui.available_width() - grid_w) * 0.5).max(0.0);
                            let mut selection = None;
                            let mut scroll_origin_y = 0.0_f32;
                            for row in 0..rows {
                                let (row_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), card_size.y),
                                    egui::Sense::hover(),
                                );
                                if row == 0 { scroll_origin_y = row_rect.top(); }
                                for col in 0..cols {
                                    let i = row * cols + col;
                                    if i >= cards.len() {
                                        break;
                                    }
                                    let rect = egui::Rect::from_min_size(
                                        row_rect.min + egui::vec2(inset + col as f32 * (card_size.x + CARD_GAP), 0.0),
                                        card_size,
                                    );
                                    let logical_rect = rect;
                                    let local = egui::Rect::from_min_size(egui::pos2(
                                        col as f32 * (card_size.x + CARD_GAP), row as f32 * (card_size.y + CARD_GAP)), card_size);
                                    let motion = crate::ui_style::animated_rect(ui.ctx(),
                                        egui::Id::unique("switcher_card_motion").with((cards[i].id, cols, preview.x.to_bits())), local);
                                    let rect = rect.translate(motion.min - local.min);
                                    let response = switcher_card(ui, rect, i, &cards[i], i == selected, &colors);
                                    if i == selected { selection = Some(rect); }
                                    if response.clicked() {
                                        *pick = Some(i);
                                    }
                                    if i == selected && follow_selection {
                                        ui.scroll_to_rect(logical_rect, None);
                                    }
                                }
                                if row + 1 < rows {
                                    ui.add_space(CARD_GAP);
                                }
                            }
                            if let Some(rect) = selection {
                                // Scrolling follows selection immediately; only
                                // motion within the current viewport is tweened.
                                let id = egui::Id::unique("switcher_selection_motion").with((scroll_origin_y.to_bits(), screen.width().to_bits(), screen.height().to_bits()));
                                let rect = crate::ui_style::animated_rect_in(ui.ctx(), id, rect, ui.clip_rect().shrink(1.0));
                                ui.painter().rect_stroke(rect, 11.0,
                                    egui::Stroke::new(2.5, colors.accent), egui::StrokeKind::Inside);
                            }
                        });
                    ui.add_space(18.0);
                    if ui.available_width() < 650.0 {
                        let (help, _) =
                            ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), egui::Sense::hover());
                        text(
                            ui,
                            help,
                            "← →: move · release Alt: switch · Esc: cancel",
                            colors.muted,
                            11.0,
                            false,
                        );
                    } else {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 7.0;
                            let full = ui.available_width();
                            // Keep the help centered; its labels describe the actual Alt-held controls.
                            ui.add_space(((full - 650.0) * 0.5).max(0.0));
                            for (key, label) in
                                [("← / →", "move"), ("Enter / release Alt", "switch"), ("Esc", "cancel")]
                            {
                                egui::Frame::new()
                                    .fill(colors.card)
                                    .stroke(egui::Stroke::new(1.0, colors.border))
                                    .corner_radius(5.0)
                                    .inner_margin(egui::vec2(7.0, 4.0))
                                    .show(ui, |ui| {
                                        ui.label(egui::RichText::new(key).monospace().size(12.0).color(colors.fg));
                                    });
                                ui.label(egui::RichText::new(label).size(12.0).color(colors.muted));
                                ui.add_space(6.0);
                            }
                        });
                    }
                });
        });
}

fn switcher_card(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    index: usize,
    card: &SwitcherCard,
    selected: bool,
    c: &TermStyle,
) -> egui::Response {
    let response = ui.interact(rect, egui::Id::unique("switcher_card").with(card.id), egui::Sense::click());
    let p = ui.painter();
    c.card_surface(p, rect, false, response.hovered());
    let weight = crate::ui_style::transition(ui.ctx(), egui::Id::unique("switcher_selected_fill").with(card.id), selected);
    if weight > 0.0 { p.rect_filled(rect.shrink(1.0), 11.0, c.accent.gamma_multiply(weight * 0.08)); }
    let badge = egui::Rect::from_min_size(rect.min + egui::vec2(14.0, 9.0), egui::vec2(28.0, 24.0));
    c.number_badge(p, badge, index + 1, selected, 16.0);
    let (status, color) = match card.status {
        SwitcherStatus::Running => ("running", c.ok),
        SwitcherStatus::Passed => ("passed", c.ok),
        SwitcherStatus::Failed => ("failed", c.err),
        SwitcherStatus::Completed => ("done", c.muted),
        SwitcherStatus::Unknown => (if card.active { "active" } else { "terminal" }, c.muted),
    };
    let show_status = rect.width() >= 270.0;
    let status_width = if show_status { 78.0 } else { 20.0 };
    let dot = egui::pos2(rect.right() - status_width, badge.center().y);
    p.circle_filled(dot, 3.5, color);
    if show_status {
        text(
            ui,
            egui::Rect::from_min_max(dot + egui::vec2(9.0, -12.0), egui::pos2(rect.right() - 8.0, dot.y + 12.0)),
            status,
            color,
            12.0,
            false,
        );
    }
    let title = if card.marker { format!("* {}", card.title) } else { card.title.clone() };
    text(
        ui,
        egui::Rect::from_min_max(
            egui::pos2(badge.right() + 12.0, badge.top()),
            egui::pos2(dot.x - 9.0, badge.bottom()),
        ),
        &title,
        c.fg,
        16.0,
        true,
    );
    let well = egui::Rect::from_min_max(
        rect.min + egui::vec2(CARD_PAD, CARD_HEADER),
        rect.max - egui::vec2(CARD_PAD, CARD_PAD),
    );
    crate::ui_style::preview_image(ui, well, card.thumb, c);
    response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(format!(
        "{}\n{}\n{status}",
        card.title,
        card.cwd.as_deref().unwrap_or("")
    ))
}

/// Upload one pass's texture deltas, free the released textures, and clear
/// the container. epaint panics (debug builds) when a TexturesDelta is
/// dropped with unapplied deltas, so every FullOutput must go through here
/// -- `Gpu::render_frame` for live frames, `Egui::init` for the warm-up.
pub fn apply_textures(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut egui_wgpu::Renderer,
    full: &mut FullOutput,
) {
    for (id, deltas) in &full.textures_delta.set {
        for d in deltas {
            renderer.update_texture(device, queue, *id, d);
        }
    }
    for id in &full.textures_delta.free {
        renderer.free_texture(id);
    }
    full.textures_delta.clear();
}

/// Settings shares the history sidebar's palette and outlined controls.
/// Fonts and colours preview live; Save persists them and Cancel reloads
/// the saved configuration. The action row stays outside the scroll area.
pub fn config_window(
    ui: &mut egui::Ui,
    open: &mut bool,
    cfg: &mut crate::config::Config,
    st: &mut UiState,
    choices: &[crate::fonts::FontChoice],
) -> ConfigEvents {
    let mut ev = ConfigEvents { apply: false, save: false, revert: false };
    if !*open { return ev; }
    let colors = TermStyle::new(ui, rgb_c32(cfg.col[2]), rgb_c32(cfg.col[1]));
    let top = st.panel_top;
    let height = (ui.max_rect().bottom() - top).max(180.0);
    egui::Window::new("Settings")
        .id(egui::Id::unique("config_panel"))
        .title_bar(false)
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(0.0, top))
        .resizable([true, false])
        .default_width(448.0)
        .min_width(412.0)
        .max_width(640.0)
        .min_height(height).max_height(height).default_height(height)
        .fade_in(false)
        .fade_out(false)
        .frame(egui::Frame::new().fill(colors.bg).inner_margin(18.0))
        .show(ui.ctx(), |ui| {
            colors.apply_controls(ui);
            let edge = ui.max_rect().left() - 18.0;
            ui.painter().vline(edge, top..=ui.max_rect().bottom() + 18.0, egui::Stroke::new(1.0, colors.border));
            let (header, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), egui::Sense::hover());
            draw_icon(ui.painter(), Icon::Settings, egui::pos2(header.left() + 13.0, header.center().y), colors.fg);
            text(ui, header.with_min_x(header.left() + 36.0).with_max_x(header.right() - 40.0), "Settings", colors.fg, 16.0, true);
            let close = egui::Rect::from_center_size(egui::pos2(header.right() - 12.0, header.center().y), egui::vec2(30.0, 30.0));
            if icon_button(ui, close, Icon::Dismiss, "Close settings", &colors).clicked() { *open = false; }
            ui.label(egui::RichText::new("Make this terminal your own.").small().color(colors.muted));
            ui.add_space(8.0);
            // Two columns, three rows. Six labels cannot share one row at the
            // 412 px minimum without truncating one, and letting them wrap would
            // move a label every time the panel is resized. Six fills an even
            // grid, so nothing has to span or stretch.
            let cell = ((ui.available_width() - 8.0) / 2.0).max(120.0);
            let mut nav = String::new();
            for group in SETTINGS_PAGES.chunks(2) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    for &(page, name) in group {
                        let selected = st.settings_page == page;
                        let label = egui::RichText::new(name)
                            .color(if selected { colors.accent } else { colors.muted });
                        let response = ui.add(egui::Button::new(if selected { label.strong() } else { label })
                            .corner_radius(17.0).min_size(egui::vec2(cell, 34.0))
                            .fill(if selected { colors.bg.lerp_to_gamma(colors.accent, 0.16) } else { colors.card })
                            .stroke(egui::Stroke::new(1.0, if selected { colors.accent } else { colors.border })));
                        nav.push_str(&format!("{name}={:.0},{:.0} ", response.rect.center().x, response.rect.center().y));
                        if response.clicked() {
                            st.settings_page = page;
                            // A popup belongs to the page that opened it. Left
                            // open it floats over a page with no row to close
                            // it, and its widget id is gone next frame. An armed
                            // chord capture is the same hazard in the app.
                            st.font_popup = None;
                            st.key_capture = None;
                        }
                    }
                });
            }
            let nav = nav.trim_end().to_string();
            if st.settings_nav_log != nav {
                st.settings_nav_log = nav.clone();
                // The geometry of the selector, for the headless tests: they
                // click a category by name instead of guessing an x-offset that
                // a layout change would silently move.
                log::info!("settings nav: {nav}");
            }
            ui.add_space(12.0);
            egui::ScrollArea::vertical()
                .id_salt(("settings_body", st.settings_page as u8))
                .auto_shrink([false, false])
                .max_height((ui.available_height() - 86.0).max(40.0))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 12.0;
                    match st.settings_page {
                        SettingsPage::Appearance => appearance_page(ui, cfg, st, choices, &mut ev, &colors),
                        SettingsPage::Window => window_page(ui, cfg, &mut ev, &colors),
                        SettingsPage::Tabs => tabs_page(ui, cfg, &mut ev, &colors),
                        SettingsPage::Navigation => navigation_page(ui, cfg, &mut ev, &colors),
                        SettingsPage::Terminal => terminal_page(ui, cfg, &mut ev, &colors),
                        SettingsPage::Keys => keys_page(ui, cfg, st, &colors),
                    }
                });
            ui.add_space(12.0);
            ui.separator();
            ui.label(egui::RichText::new("Changes preview instantly.").small().color(colors.muted));
            let (save, cancel) = actions(ui, ui.available_width(), "Save", "Cancel");
            if save || cancel {
                ev.save = save;
                ev.revert = cancel;
                st.theme_fetched = false;
                *open = false;
            }
        });
    if !*open { st.font_popup = None; }
    ev
}

/// Every page renders through its own function, so a page's shape is readable
/// in one screen and the match above is only a dispatch. Grouping is by what the
/// user came here to change: a tab-strip background is an appearance setting even
/// though its field is named for tabs, and the switcher is navigation even though
/// it draws tabs.
///
/// Appearance: theme and colors, fonts, and every tab color.
fn appearance_page(
    ui: &mut egui::Ui,
    cfg: &mut crate::config::Config,
    st: &mut UiState,
    choices: &[crate::fonts::FontChoice],
    ev: &mut ConfigEvents,
    colors: &TermStyle,
) {
    section(ui, "Theme & colors", colors, |ui| {
        if !st.theme_fetched {
            st.theme_choice = crate::theme::available();
            st.theme_fetched = true;
        }
        form_grid("colors-terminal").show(ui, |ui| {
            row(ui, "Theme", |ui| {
                let written = if cfg.theme.is_empty() { "default".to_string() } else { cfg.theme.clone() };
                let mut names = st.theme_choice.clone();
                if !names.contains(&written) { names.push(written.clone()); }
                let mut chosen = written.clone();
                egui::ComboBox::from_id_salt("theme_preset").popup_style((**ui.style()).clone().into())
                    .width(170.0).selected_text(&written)
                    .show_ui(ui, |ui| {
                        for name in &names { ui.selectable_value(&mut chosen, name.clone(), name); }
                    });
                if chosen != written {
                    cfg.theme = chosen.clone();
                    // A preset rewrites the color fields, so the rows below
                    // change under the user. That is the point: the preset stays
                    // above them, and a manual override is applied after it.
                    if let Err(e) = crate::theme::apply_name(cfg, &chosen) { log::warn!("theme {chosen}: {e}"); }
                    ev.apply = true;
                }
            });
            color_row(ui, "Background", &mut cfg.background_color, ev);
            color_row(ui, "Foreground", &mut cfg.foreground_color, ev);
            color_row(ui, "Selection", &mut cfg.selection_color, ev);
            color_row(ui, "Cursor", &mut cfg.cursor_color, ev);
            color_row(ui, "Accent", &mut cfg.accent_color, ev);
            color_row(ui, "Background tint", &mut cfg.bg_tint_color, ev);
            row(ui, "Terminal brightness", |ui| {
                ev.apply |= ui.add(egui::DragValue::new(&mut cfg.terminal_brightness)
                    .speed(1.0).range(-100.0..=100.0).suffix("%").fixed_decimals(0))
                    .on_hover_text("0% keeps the original terminal colors; negative values darken them, positive values brighten them.")
                    .changed();
            });
        });
    });
    workspace_settings(ui, cfg, st, ev, colors);
    section(ui, "Fonts", colors, |ui| {
        form_grid("fonts").show(ui, |ui| {
            font_row(ui, "Terminal font", &mut cfg.term_font, FontWhich::Term, st, choices, ev);
            row(ui, "Line spacing", |ui| {
                ev.apply |= ui.add(egui::DragValue::new(&mut cfg.line_spacing)
                    .speed(0.05).range(-0.9..=3.0).fixed_decimals(2))
                    .on_hover_text("0 = current spacing; +0.1 adds 10% row height; -0.1 removes 10%.")
                    .changed();
            });
            font_row(ui, "Tab font", &mut cfg.tablist_font, FontWhich::Tablist, st, choices, ev);
        });
    });
    section(ui, "Motion", colors, |ui| {
        form_grid("appearance-motion").show(ui, |ui| {
            row(ui, "Animation speed", |ui| {
                ev.apply |= ui.add(egui::Slider::new(&mut cfg.animation_speed, 0.0..=4.0)
                    .step_by(0.25).custom_formatter(|speed, _| {
                        if speed == 0.0 { "Off".into() } else { format!("{speed:.2}×") }
                    })).changed();
            });
        });
        ui.label(egui::RichText::new("1× is normal. Lower is slower; 0 turns animations off.")
            .small().color(colors.muted));
    });
    section(ui, "Tab colors", colors, |ui| {
        form_grid("colors-tabs").show(ui, |ui| {
            color_row(ui, "Tab strip", &mut cfg.tablist_background, ev);
            color_row(ui, "Add button", &mut cfg.add_button_color, ev);
        });
        // Manual tab groups have a separate palette from workspace identities.
        ui.add_space(8.0);
        ui.label("Group colors");
        ui.label(egui::RichText::new("The palette a tab is tagged with.").small().color(colors.muted));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            for (i, color) in cfg.col.iter_mut().enumerate() {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(format!("{}", i + 1)).small().color(colors.muted));
                    ev.apply |= color_cell(ui, color);
                });
            }
        });

    });
}

/// A single workspace editor keeps identity choices together, as in the mockup.
fn workspace_settings(ui: &mut egui::Ui, cfg: &mut crate::config::Config, st: &mut UiState,
    ev: &mut ConfigEvents, colors: &TermStyle) {
    use crate::config::{WorkspaceButtonDisplay, WorkspaceButtonPosition, WorkspaceIcon};
    let mut card = *colors;
    card.border = colors.accent.gamma_multiply(0.65);
    section(ui, "Workspaces", &card, |ui| {
        ui.label(egui::RichText::new("Manage workspace names, colors, and icons.").small().color(colors.muted));
        st.workspace_edit = st.workspace_edit.min(crate::config::WORKSPACES - 1);
        let selected = st.workspace_edit;
        let caption = format!("    {} {}", selected + 1, cfg.workspace_genres[selected].trim());
        let picker = egui::ComboBox::from_id_salt("workspace_editor")
            .popup_style((**ui.style()).clone().into()).width(ui.available_width() - 8.0)
            .selected_text(caption).truncate().show_ui(ui, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                for index in 0..crate::config::WORKSPACES {
                    let label = format!("    {} {}", index + 1, cfg.workspace_genres[index].trim());
                    let response = ui.selectable_value(&mut st.workspace_edit, index, label);
                    crate::ui_style::workspace_icon(ui.painter(), cfg.workspace_icons[index],
                        egui::pos2(response.rect.left() + 14.0, response.rect.center().y), rgb_c32(cfg.workspace_colors[index]));
                    response.on_hover_text(format!("Workspace {} · {}", index + 1, cfg.workspace_genres[index]));
                }
            });
        crate::ui_style::workspace_icon(ui.painter(), cfg.workspace_icons[selected],
            egui::pos2(picker.response.rect.left() + 14.0, picker.response.rect.center().y), rgb_c32(cfg.workspace_colors[selected]));
        let index = st.workspace_edit;
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label("Number");
                let mut number = (index + 1).to_string();
                ui.add(egui::TextEdit::singleline(&mut number).interactive(false).desired_width(40.0));
            });
            ui.vertical(|ui| {
                ui.label("Name");
                ev.apply |= ui.add(egui::TextEdit::singleline(&mut cfg.workspace_genres[index])
                    .hint_text("e.g. Development").desired_width(ui.available_width() - 8.0)).changed();
            });
        });
        ui.label("Color");
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for hex in ["#f07b72", "#ec9756", "#e7b85a", "#bbc96a", "#71c58c", "#5fc7d0", "#5e9eff", "#b792ed", "#e998be", "#95a8c4"] {
                let color = crate::config::parse_hex(hex);
                let (rect, response) = ui.allocate_exact_size(egui::vec2(25.0, 25.0), egui::Sense::click());
                let selected = cfg.workspace_colors[index] == color;
                ui.painter().rect_filled(rect.shrink(3.0), 4.0, rgb_c32(color));
                if selected || response.hovered() || response.has_focus() {
                    ui.painter().rect_stroke(rect, 5.0, egui::Stroke::new(1.5, colors.accent), egui::StrokeKind::Inside);
                }
                if selected { crate::ui_style::icon(ui.painter(), Icon::Check, rect.center(), crate::ui_style::black_or_white_text(rgb_c32(color))); }
                response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Workspace color {hex}")));
                if response.on_hover_text(hex).clicked() { cfg.workspace_colors[index] = color; ev.apply = true; }
            }
        });
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Custom color").small().color(colors.muted));
            ev.apply |= color_cell(ui, &mut cfg.workspace_colors[index]);
        });
        ui.label("Icon");
        let cell_width = ((ui.available_width() - 32.0) / 5.0).max(30.0);
        egui::Grid::new("workspace_icons").spacing(egui::vec2(8.0, 8.0)).show(ui, |ui| {
            for (slot, icon) in WorkspaceIcon::ALL.into_iter().enumerate() {
                let (rect, response) = ui.allocate_exact_size(egui::vec2(cell_width, 36.0), egui::Sense::click());
                let selected = cfg.workspace_icons[index] == icon;
                let hot = response.hovered() || response.has_focus();
                ui.painter().rect(rect, 5.0, colors.bg.lerp_to_gamma(colors.accent, if selected { 0.12 } else { 0.02 }),
                    egui::Stroke::new(1.0, if selected || hot { colors.accent } else { colors.border }), egui::StrokeKind::Inside);
                crate::ui_style::workspace_icon(ui.painter(), icon, rect.center(), rgb_c32(cfg.workspace_colors[index]));
                response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Workspace icon {}", icon.label())));
                if response.on_hover_text(icon.label()).clicked() { cfg.workspace_icons[index] = icon; ev.apply = true; }
                if slot % 5 == 4 { ui.end_row(); }
            }
        });
        ui.add_space(4.0);
        form_grid("workspace_button_options").show(ui, |ui| {
            row(ui, "Button display", |ui| {
                egui::ComboBox::from_id_salt("workspace_button_display").width(140.0)
                    .popup_style((**ui.style()).clone().into()).selected_text(cfg.workspace_button_display.label())
                    .show_ui(ui, |ui| { for mode in WorkspaceButtonDisplay::ALL {
                        ev.apply |= ui.selectable_value(&mut cfg.workspace_button_display, mode, mode.label()).changed();
                    }});
            });
            row(ui, "Button position", |ui| {
                egui::ComboBox::from_id_salt("workspace_button_position").width(140.0)
                    .popup_style((**ui.style()).clone().into()).selected_text(cfg.workspace_button_position.label())
                    .show_ui(ui, |ui| { for position in [WorkspaceButtonPosition::Left, WorkspaceButtonPosition::Right] {
                        ev.apply |= ui.selectable_value(&mut cfg.workspace_button_position, position, position.label()).changed();
                    }}).response.on_hover_text("Left: next to the menu. Right: at the end of the tab strip.");
            });
        });
        ui.label(egui::RichText::new("Button display and position apply to all workspaces.").small().color(colors.muted));
    });
}

/// Window: how big it starts and how it behaves, split because a size and a
/// behavior are different decisions taken at different times.
fn window_page(ui: &mut egui::Ui, cfg: &mut crate::config::Config, ev: &mut ConfigEvents, colors: &TermStyle) {
    section(ui, "Window dimensions", colors, |ui| {
        form_grid("window-size").show(ui, |ui| {
            row(ui, "Default width", |ui| {
                ev.apply |= ui.add(egui::DragValue::new(&mut cfg.default_width).suffix(" px")).changed();
            });
            row(ui, "Default height", |ui| {
                ev.apply |= ui.add(egui::DragValue::new(&mut cfg.default_height).suffix(" px")).changed();
            });
        });
    });
    section(ui, "Window behavior", colors, |ui| {
        form_grid("window-behavior").show(ui, |ui| {
            checkbox_row(ui, "Start maximized", &mut cfg.default_maximize, ev);
            row(ui, "Close window dialog", |ui| {
                use crate::config::CloseWindowDialog;
                egui::ComboBox::from_id_salt("close_window_dialog")
                    .popup_style((**ui.style()).clone().into())
                    .width(210.0).selected_text(cfg.close_window_dialog.label())
                    .show_ui(ui, |ui| {
                        for policy in [CloseWindowDialog::Never, CloseWindowDialog::Loss, CloseWindowDialog::Always] {
                            ev.apply |= ui.selectable_value(&mut cfg.close_window_dialog, policy, policy.label()).changed();
                        }
                    });
            });
            row(ui, "Transparency", |ui| {
                ev.apply |= ui.add(egui::Slider::new(&mut cfg.term_transparency, 0.0..=1.0))
                    .on_hover_text("0 is opaque; 1 is fully transparent. Brightness is separate under Appearance.").changed();
            });
        });
    });
}

/// Tabs: the strip that lists them, then what happens when one opens or closes.
fn tabs_page(ui: &mut egui::Ui, cfg: &mut crate::config::Config, ev: &mut ConfigEvents, colors: &TermStyle) {
    section(ui, "Tab bar", colors, |ui| {
        form_grid("tabs").show(ui, |ui| {
            checkbox_row(ui, "Compact tabs", &mut cfg.compact_tabs, ev);
            checkbox_row(ui, "Show tab titles", &mut cfg.show_tab_titles, ev);
            checkbox_row(ui, "Strip on top", &mut cfg.tablist_top, ev);
            checkbox_row(ui, "Hide strip", &mut cfg.tablist_hide, ev);
            checkbox_row(ui, "Hide single tab", &mut cfg.hide_single_tab, ev);
            checkbox_row(ui, "Multiline tabs", &mut cfg.multiline_tabs, ev);
            row(ui, "Title length", |ui| {
                ev.apply |= ui.add(egui::DragValue::new(&mut cfg.tab_title_max_length)).changed();
            });
            row(ui, "Active title length", |ui| {
                ev.apply |= ui.add(egui::DragValue::new(&mut cfg.tab_title_short_current)).changed();
            });
            checkbox_row(ui, "Shorten from start", &mut cfg.tab_title_shorten_start, ev);
        });
    });
    section(ui, "Tab behavior", colors, |ui| {
        form_grid("tab-behavior").show(ui, |ui| {
            checkbox_row(ui, "Focus new tabs", &mut cfg.focus_new_tabs, ev);
            row(ui, "New tab position", |ui| {
                let mut chosen = cfg.new_tab_position.clone();
                egui::ComboBox::from_id_salt("new_tab_position")
                    .popup_style((**ui.style()).clone().into())
                    .width(140.0)
                    .selected_text(&cfg.new_tab_position)
                    .show_ui(ui, |ui| {
                        for name in ["next", "end", "first"] {
                            ui.selectable_value(&mut chosen, name.to_string(), name);
                        }
                    });
                if chosen != cfg.new_tab_position {
                    cfg.new_tab_position = chosen;
                    ev.apply = true;
                }
            });
            row(ui, "Graveyard keeps", |ui| {
                ev.apply |= ui.add(egui::DragValue::new(&mut cfg.graveyard_limit).range(1..=32))
                    .on_hover_text("Closed tabs stay running and raisable until this many are kept.")
                    .changed();
            });
        });
    });
}

/// Navigation: the ways to move between terminals. The switcher and workspaces
/// both draw tabs, which is exactly why they sat under Tabs before, and neither
/// is a tab-strip setting.
fn navigation_page(ui: &mut egui::Ui, cfg: &mut crate::config::Config, ev: &mut ConfigEvents, colors: &TermStyle) {
    section(ui, "Tab switcher", colors, |ui| {
        form_grid("tab_switcher").show(ui, |ui| {
            checkbox_row(ui, "Tab switcher enabled", &mut cfg.tab_overview_on_alt, ev);
            row(ui, "Delay (ms)", |ui| {
                ev.apply |= ui.add(egui::DragValue::new(&mut cfg.tab_overview_delay_ms)).changed();
            });
            row(ui, "Preview scale", |ui| {
                ev.apply |= ui.add(egui::Slider::new(&mut cfg.tab_overview_scale, 0.05..=1.0)).changed();
            });
            row(ui, "Backdrop dim", |ui| {
                ev.apply |= ui.add(egui::Slider::new(&mut cfg.tab_overview_opacity, 0.0..=1.0)).changed();
            });
        });
    });
    section(ui, "Workspaces", colors, |ui| {
        form_grid("workspaces").show(ui, |ui| {
            checkbox_row(ui, "Enable workspaces", &mut cfg.workspaces_enabled, ev);
        });
        ui.add_space(6.0);
        // The shortcuts are not repeated here. A label baked into this page
        // would keep advertising a chord the user has since rebound or unbound;
        // Keys reads the real bindings.
        ui.label(egui::RichText::new(
            "Ten slots, each holding its own tabs. Their shortcuts are on the Keys page.",
        )
        .small()
        .color(colors.muted));
        ui.add_space(10.0);
        ui.label(egui::RichText::new("Manage names, colors, icons, and button display in Appearance → Workspaces.")
            .small().color(colors.muted));
    });
}

/// Terminal: deliberately sparse. Unrelated controls do not belong here just
/// because this page has room.
fn terminal_page(ui: &mut egui::Ui, cfg: &mut crate::config::Config, ev: &mut ConfigEvents, colors: &TermStyle) {
    section(ui, "Scrollback", colors, |ui| {
        form_grid("terminal").show(ui, |ui| {
            row(ui, "Scrollback lines", |ui| {
                ev.apply |= ui.add(egui::DragValue::new(&mut cfg.scrollback_lines).range(0..=1_000_000)).changed();
            });
        });
    });
}

/// Keys: every action, its current chord, and a reset or clear per row.
fn keys_page(ui: &mut egui::Ui, cfg: &mut crate::config::Config, st: &mut UiState, colors: &TermStyle) {
    if let Some(action) = st.key_capture.clone() {
        section(ui, "Waiting for a shortcut", colors, |ui| {
            ui.label(egui::RichText::new(format!(
                "Press the keys for \u{201c}{}\u{201d}. Esc cancels.", action_label(&action)))
                .strong().color(colors.accent));
        });
    }
    for (group, rows) in key_groups(cfg) {
        section(ui, group, colors, |ui| {
            for (action, label) in rows {
                binding_row(ui, &action, &label, cfg, st, colors);
            }
        });
    }
}

fn row(ui: &mut egui::Ui, label: &str, add_control: impl FnOnce(&mut egui::Ui)) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(158.0, 32.0), egui::Sense::hover());
    text(ui, rect, label, ui.visuals().text_color(), 13.0, false);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), label));
    response.on_hover_text(label);
    add_control(ui);
    ui.end_row();
}

/// The actions the Keys page names, grouped. Numbered tabs and workspaces are
/// generated (they are ranges), so they are not here; `action_label` knows
/// both prefixes, so a generated action still reads well everywhere.
const KEY_GROUPS: &[(&str, &[(&str, &str)])] = &[
    ("Tabs", &[
        ("new_tab", "New tab"), ("new_window", "New window"), ("close_tab", "Close tab"),
        ("reopen_tab", "Raise last closed tab"), ("reopen_window", "Restore last closed window"), ("rename_tab", "Rename tab"),
        ("toggle_hide_tabs", "Hide the tab strip"), ("toggle_entangle", "Type in all tabs"),
        ("prev_color", "Previous tab color"), ("next_color", "Next tab color"),
    ]),
    ("Move between tabs", &[
        ("prev_tab", "Previous tab"), ("next_tab", "Next tab"),
        ("prev_tab_row", "Previous row of tabs"), ("next_tab_row", "Next row of tabs"),
        ("first_tab", "First tab"), ("last_tab", "Last tab"),
        ("move_prev_tab", "Move tab left"), ("move_next_tab", "Move tab right"),
        ("move_first_tab", "Move tab to the start"), ("move_last_tab", "Move tab to the end"),
        ("send_tab_to_window", "Move tab to another window"),
        ("move_tab_to_workspace", "Move tab to a workspace"),
    ]),
    ("Workspaces", &[
        ("show_workspaces", "Workspaces panel"),
        ("prev_workspace", "Previous workspace"), ("next_workspace", "Next workspace"),
    ]),
    ("Copy and paste", &[
        ("copy", "Copy"), ("paste", "Paste"), ("paste_primary", "Paste the selection"),
        ("select_all", "Select all"), ("copy_last_output", "Copy last command output"),
    ]),
    ("Search and history", &[
        ("show_search", "Search this tab"), ("show_search_all", "Search every tab"),
        ("search_prev", "Previous match"), ("search_next", "Next match"),
        ("show_history", "Command history"), ("graveyard", "Graveyard panel"),
    ]),
    ("Scrolling", &[
        ("scroll_page_up", "Page up"), ("scroll_page_down", "Page down"),
        ("scroll_bottom", "Jump to the prompt"),
        ("prev_command", "Previous prompt"), ("next_command", "Next prompt"),
    ]),
    ("Text size", &[
        ("enlarge_font", "Text bigger"), ("shrink_font", "Text smaller"),
    ]),
    ("Application", &[
        ("config_reload", "Reload the configuration"), ("show_config", "Open settings"),
    ]),
];

/// A human name for an action, shared by the Keys page and the shortcut list.
/// Must not consult `key_groups`: that builds its "Other" group from here.
pub fn action_label(action: &str) -> String {
    for (_, rows) in KEY_GROUPS {
        if let Some((_, label)) = rows.iter().find(|(a, _)| *a == action) {
            return (*label).to_string();
        }
    }
    if let Some(i) = action.strip_prefix("goto_tab_").and_then(|n| n.parse::<usize>().ok()) {
        return format!("Go to tab {}", if i == 9 { 10 } else { i + 1 });
    }
    if let Some(i) = action.strip_prefix("goto_workspace_").and_then(|n| n.parse::<usize>().ok()) {
        return format!("Go to workspace {}", i + 1);
    }
    let words = action.replace('_', " ");
    words.get(..1).map_or_else(|| words.clone(), |first| format!("{}{}", first.to_uppercase(), &words[1..]))
}

/// Actions the Keys page offers, grouped. A chord the user bound by hand for
/// an action this list does not name still shows up, under "Other".
fn key_groups(cfg: &crate::config::Config) -> Vec<(&'static str, Vec<(String, String)>)> {
    let mut groups: Vec<(&'static str, Vec<(String, String)>)> = KEY_GROUPS.iter()
        .map(|(name, rows)| (*name, rows.iter()
            .map(|(a, l)| (a.to_string(), l.to_string())).collect()))
        .collect();
    groups.insert(2, ("Numbered tabs", (0..10)
        .map(|i| (format!("goto_tab_{i}"), format!("Go to tab {}", if i == 9 { 10 } else { i + 1 })))
        .collect()));
    // Listed whether or not the feature is on: the chords are real either way,
    // and hiding them here would drop ten rows into "Other".
    let ws_rows = &mut groups.iter_mut().find(|(name, _)| *name == "Workspaces").unwrap().1;
    ws_rows.extend((0..crate::config::WORKSPACES)
        .map(|i| (format!("goto_workspace_{i}"), format!("Go to workspace {}", i + 1))));
    let named: std::collections::HashSet<&str> = groups.iter()
        .flat_map(|(_, rows)| rows.iter().map(|a| a.0.as_str())).collect();
    let other: Vec<(String, String)> = cfg.bindings.iter()
        .filter(|(a, _)| !named.contains(a.as_str()))
        .map(|(a, _)| (a.clone(), action_label(a)))
        .collect();
    if !other.is_empty() {
        groups.push(("Other", other));
    }
    groups
}

/// One keybinding row: the chord as a button, then reset and unbind. Clicking
/// the chord arms the recorder; the app feeds the next key back in.
fn binding_row(
    ui: &mut egui::Ui,
    action: &str,
    label: &str,
    cfg: &mut crate::config::Config,
    st: &mut UiState,
    colors: &TermStyle,
) {
    let current: Vec<crate::config::Binding> = cfg.bindings_for(action).to_vec();
    let recording = st.key_capture.as_deref() == Some(action);
    let shown = if recording { "press a key".to_string() }
        else if current.is_empty() { "unbound".to_string() }
        else { current.iter().map(|b| b.label()).collect::<Vec<_>>().join("  /  ") };
    let restore = crate::config::Config::default_binding(action);
    let changed = current != restore;
    ui.horizontal(|ui| {
        // A row of its own rather than a form_grid cell: a chord row holds
        // three widgets (chord, reset, unbind) and the settings grids are
        // two columns.
        ui.spacing_mut().item_spacing.x = 6.0;
        let (label_rect, label_response) =
            ui.allocate_exact_size(egui::vec2(158.0, 28.0), egui::Sense::hover());
        text(ui, label_rect, label, ui.visuals().text_color(), 13.0, false);
        label_response.on_hover_text(label);
        let fill = if recording { colors.bg.lerp_to_gamma(colors.accent, 0.18) } else { colors.card };
        let edge = if recording { colors.accent } else { colors.border };
        let ink = if recording { colors.accent } else if current.is_empty() { colors.muted } else { colors.fg };
        let button = egui::Button::new(egui::RichText::new(shown).monospace().color(ink))
            .corner_radius(6.0)
            .fill(fill)
            .stroke(egui::Stroke::new(1.0, edge))
            .truncate();
        let response = ui.add_sized([168.0, 28.0], button);
        response.clone().on_hover_text(if recording {
            format!("Waiting for the next shortcut. Esc cancels {label}.")
        } else {
            format!("Click, then press the shortcut for {label}.")
        });
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
        if response.clicked() {
            st.key_capture = if recording { None } else { Some(action.to_string()) };
            release_focus(ui.ctx());
            ui.ctx().request_repaint();
        }
        if recording {
            return;
        }
        if changed {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(30.0, 28.0), egui::Sense::hover());
            if crate::ui_style::icon_button(ui, rect, Icon::Rerun, &format!("Restore the default for {label}"), colors).clicked() {
                cfg.set_binding(action, restore);
                st.bindings_dirty = true;
            }
        }
        if !current.is_empty() {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(30.0, 28.0), egui::Sense::hover());
            if crate::ui_style::icon_button(ui, rect, Icon::Dismiss, &format!("Unbind {label}"), colors).clicked() {
                cfg.set_binding(action, Vec::new());
                st.bindings_dirty = true;
            }
        }
    });
}

/// The sample text a font popup previews with.
const FONT_PREVIEW_TEXT: &str = "AaBbGgRr 0123 The quick brown fox";
/// A popup lists at most this many families per pass. Each previewed face
/// reads one font file into the atlas, and a full system can hold hundreds
/// of families; the filter box covers the tail.
const FONT_POPUP_MAX: usize = 300;

/// A font row: a button showing the current description that opens the
/// selection popup. The popup drafts family, style and size against a live
/// preview; only OK writes the Pango-style description ("Family Bold Italic
/// 12") into the config -- a font change rebuilds the whole font system, so
/// it must never fire per keystroke or per list click.
fn font_row(
    ui: &mut egui::Ui,
    label: &str,
    cfg_value: &mut String,
    which: FontWhich,
    st: &mut UiState,
    choices: &[crate::fonts::FontChoice],
    ev: &mut ConfigEvents,
) {
    row(ui, label, |ui| {
        let shown: String = cfg_value.chars().take(30).collect();
        let r = ui.add_sized([174.0, 30.0], egui::Button::new(shown).truncate())
            .on_hover_text(cfg_value.as_str());
        // One click does the whole job: closed opens, open (this row)
        // closes, the *other* row's popup is swapped for this one. The
        // toggle closes on PRESS, not on the click: a release-triggered
        // close would run before this frame's `clicked()` and the same
        // click would immediately re-open the popup (visible as "nothing
        // happens"). The suppress flag swallows the matching release.
        let same = st.font_popup.as_ref().is_some_and(|p| p.which == which);
        let pressed = ui.input(|i| i.pointer.any_pressed());
        if r.hovered() && pressed && same {
            st.font_popup = None;
            st.font_suppress = true;
        }
        if r.clicked() && !st.font_suppress && !same {
            st.font_popup = Some(seed_popup(which, cfg_value, choices));
        }
        if ui.input(|i| i.pointer.any_released()) {
            st.font_suppress = false;
        }
        if !st.font_popup.as_ref().is_some_and(|p| p.which == which) {
            return;
        }
        // The list is rebuilt only when one of its inputs changes -- filter
        // text or drafted family -- not every frame.
        {
            let popup = st.font_popup.as_mut().unwrap();
            let needle = popup.search.to_lowercase();
            let stale = !popup
                .list_cache
                .as_ref()
                .is_some_and(|(n, f, _)| *n == needle && *f == popup.family);
            if stale {
                let mut idx: Vec<usize> = choices
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| needle.is_empty() || c.name_lower.contains(&needle))
                    .map(|(i, _)| i)
                    .collect();
                // Drafted family first, then plain alphabetical (a rotate
                // would leave the alphabet split at the draft instead).
                if let Some(p) = idx.iter().position(|&i| choices[i].name == popup.family) {
                    let d = idx.remove(p);
                    idx.insert(0, d);
                }
                idx.truncate(FONT_POPUP_MAX);
                popup.list_cache = Some((needle, popup.family.clone(), idx));
            }
        }
        let mut search = std::mem::take(&mut st.font_popup.as_mut().unwrap().search);
        // Copied out so the popup closure can hold &mut popup while reading
        // it. A few hundred usize once per frame, versus a full
        // filter + sort + String churn before.
        let order: Vec<usize> = st
            .font_popup
            .as_ref()
            .and_then(|p| p.list_cache.as_ref().map(|(_, _, v)| v.clone()))
            .unwrap_or_default();

        let mut commit: Option<String> = None;
        let mut close = false;
        {
            let popup = st.font_popup.as_mut().unwrap();
            let inner = egui::Popup::from_response(&r)
                .style(crate::ui_style::picker_style(ui))
                .open_bool(&mut popup.open)
                .width(420.0)
                // from_response leaves the default left-to-right layout;
                // without this the whole dialog renders as one horizontal
                // strip (found by the layout test below).
                .layout(egui::Layout::top_down_justified(egui::Align::Min))
                // ...and its DEFAULT close behavior is CloseOnClick: ANY
                // click -- including the ones on this popup's own widgets --
                // closed it, so clicking the filter box dismissed the dialog
                // (found by the same test). A dialog closes on outside
                // clicks only.
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| -> Option<String> {
                    // The popup lives in an auto-sizing Area whose available
                    // size is not trustworthy (an earlier version flowed the
                    // whole dialog as one horizontal strip across the
                    // screen). Lay the dialog out in an exactly-sized box
                    // instead: every region gets an explicit rect.
                    ui.allocate_ui_with_layout(
                        egui::vec2(404.0, 452.0),
                        egui::Layout::top_down_justified(egui::Align::Min),
                        |ui| {
                            // ---- Preview (draft face, draft size) ----
                            let face = face_of_idx(choices, popup.family_idx, popup.style);
                            let rt = match face {
                                Some(f) => {
                                    // The terminal draws `size` pt at 96 dpi,
                                    // i.e. size*4/3 px; preview the same
                                    // physical size.
                                    let px = (popup.size_pt * 96.0 / 72.0).clamp(9.0, 48.0);
                                    let key = preview_family_name(&popup.family, f.weight, f.slant);
                                    egui::RichText::new(FONT_PREVIEW_TEXT).font(egui::FontId::new(
                                        px,
                                        egui::FontFamily::Name(std::sync::Arc::from(key.as_str())),
                                    ))
                                }
                                None => egui::RichText::new(FONT_PREVIEW_TEXT)
                                    .weak()
                                    .italics(),
                            };
                            ui.add_sized([404.0, 64.0], egui::Label::new(rt).selectable(false));
                            ui.separator();
                            // ---- Filter ----
                            ui.add_sized(
                                [404.0, 24.0],
                                egui::TextEdit::singleline(&mut search).hint_text("Filter fonts\u{2026}"),
                            );
                            // ---- Family list (neutral labels) | style list ----
                            let mut picked_family: Option<usize> = None;
                            let mut dbl = false;
                            ui.horizontal(|ui| {
                                ui.allocate_ui_with_layout(
                                    egui::vec2(250.0, 240.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        egui::ScrollArea::vertical()
                                            .id_salt("font-families")
                                            .auto_shrink([false, false])
                                            .show(ui, |ui| {
                                                for &i in &order {
                                                    let c = &choices[i];
                                                    let resp = ui.selectable_label(
                                                        i == popup.family_idx,
                                                        // The NAME in the neutral UI
                                                        // font: the face belongs to
                                                        // the preview, not the list.
                                                        &c.name,
                                                    );
                                                    if resp.double_clicked() {
                                                        picked_family = Some(i);
                                                        dbl = true;
                                                        break;
                                                    } else if resp.clicked() {
                                                        picked_family = Some(i);
                                                        break;
                                                    }
                                                }
                                            });
                                    },
                                );
                                ui.separator();
                                ui.allocate_ui_with_layout(
                                    egui::vec2(140.0, 240.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        egui::ScrollArea::vertical()
                                            .id_salt("font-styles")
                                            .auto_shrink([false, false])
                                            .show(ui, |ui| {
                                                let Some(c) = choices.get(popup.family_idx)
                                                else {
                                                    return;
                                                };
                                                if c.name != popup.family {
                                                    return; // draft names nothing installed
                                                }
                                                for (i, f) in c.faces.iter().enumerate() {
                                                    let words =
                                                        crate::fonts::style_words(f.weight, f.slant);
                                                    let name = if words.is_empty() {
                                                        "Regular".to_string()
                                                    } else {
                                                        words
                                                    };
                                                    if ui
                                                        .selectable_value(&mut popup.style, i, name)
                                                        .double_clicked()
                                                    {
                                                        dbl = true;
                                                        break;
                                                    }
                                                }
                                            });
                                    },
                                );
                            });
                            if let Some(i) = picked_family {
                                // Snap the style to the same weight/slant the
                                // previous family had selected, when offered.
                                let want = face_of_idx(choices, popup.family_idx, popup.style)
                                    .map(|f| (f.weight, f.slant));
                                popup.family = choices[i].name.clone();
                                popup.family_idx = i;
                                popup.style = nearest_style(&choices[i], want);
                            }
                            // ---- Size, OK, Cancel ----
                            // Type precise sizes directly; the buttons still step by one point.
                            ui.horizontal(|ui| {
                                ui.label("Size");
                                let step = egui::Vec2::new(26.0, 20.0);
                                if ui
                                    .add(
                                        egui::Button::new(
                                            egui::RichText::new("\u{2212}").strong(),
                                        )
                                        .min_size(step),
                                    )
                                    .on_hover_text("one point smaller")
                                    .clicked()
                                {
                                    popup.size_pt = (popup.size_pt - 1.0).max(4.0);
                                }
                                ui.add(
                                    egui::DragValue::new(&mut popup.size_pt)
                                        .range(4.0..=96.0)
                                        .speed(0.0)
                                        .min_decimals(0)
                                        .max_decimals(2)
                                        .suffix(" pt"),
                                );
                                if ui
                                    .add(
                                        egui::Button::new(
                                            egui::RichText::new("+").strong(),
                                        )
                                        .min_size(step),
                                    )
                                    .on_hover_text("one point bigger")
                                    .clicked()
                                {
                                    popup.size_pt = (popup.size_pt + 1.0).min(96.0);
                                }
                                ui.add_space(2.0);
                                let (ok, canc) = actions(ui, 250.0, "OK", "Cancel");
                                if ok || dbl {
                                    commit = compose_value(
                                        choices,
                                        &popup.family,
                                        popup.style,
                                        popup.size_pt,
                                    );
                                    close = true;
                                }
                                if canc {
                                    close = true;
                                }
                            });
                        },
                    );
                    commit.clone()
                });
            if commit.is_none() {
                commit = inner.and_then(|i| i.inner);
            }
            if close {
                popup.open = false;
            }
        }
        if let Some(val) = commit {
            if *cfg_value != val {
                *cfg_value = val;
                ev.apply = true;
            }
        }
        st.font_popup.as_mut().unwrap().search = search;
        if !st.font_popup.as_ref().is_some_and(|p| p.open) {
            st.font_popup = None;
        }
    });
}

/// The face a popup draft selects, by cached family index. This is the
/// per-frame accessor; [`face_of`] (by name) is for one-shot paths.
fn face_of_idx<'a>(
    choices: &'a [crate::fonts::FontChoice],
    family_idx: usize,
    style: usize,
) -> Option<&'a crate::fonts::FaceChoice> {
    choices.get(family_idx)?.faces.get(style)
}

/// The drafted face, if the family exists and has one at that index.
fn face_of<'a>(
    choices: &'a [crate::fonts::FontChoice],
    family: &str,
    style: usize,
) -> Option<&'a crate::fonts::FaceChoice> {
    choices.iter().find(|c| c.name == family)?.faces.get(style)
}

/// The style index of the face closest to `want` (weight, slant); defaults
/// to the family's Regular-ish face.
fn nearest_style(
    c: &crate::fonts::FontChoice,
    want: Option<(u16, crate::fonts::DbStyle)>,
) -> usize {
    let (w, s) = want.unwrap_or((400, crate::fonts::DbStyle::Normal));
    c.faces
        .iter()
        .enumerate()
        .min_by_key(|(_, f)| {
            (f.weight as i32 - w as i32).unsigned_abs() + if f.slant == s { 0 } else { 1000 }
        })
        .map_or(0, |(i, _)| i)
}

/// Seed a popup's draft from the current config description: the spec names
/// a family exactly, or it splits into family + style, or the draft starts
/// from its first word so the list at least highlights near it.
fn seed_popup(
    which: FontWhich,
    cfg_value: &str,
    choices: &[crate::fonts::FontChoice],
) -> FontPopup {
    let (spec, size) = crate::config::parse_font(cfg_value);
    let (family, want) = if choices.iter().any(|c| c.name == spec) {
        (spec, None)
    } else if let Some((base, w, s)) = crate::fonts::split_style_suffix(&spec) {
            if choices.iter().any(|c| c.name == base) {
                (base, Some((w, s)))
            } else {
                (spec, None)
            }
        } else {
            (spec, None)
        };
    let fam_idx = choices.iter().position(|c| c.name == family);
    let style = fam_idx.map(|i| nearest_style(&choices[i], want)).unwrap_or(0);
    FontPopup {
        which,
        open: true,
        search: String::new(),
        family,
        family_idx: fam_idx.unwrap_or(usize::MAX),
        style,
        size_pt: size,
        list_cache: None,
    }
}

/// Compose the config value the popup commits: "Family Bold Italic 12",
/// with the style words omitted for the plain upright face. `None` when the
/// draft names no real family (OK on a family-less draft must not write
/// garbage).
fn compose_value(
    choices: &[crate::fonts::FontChoice],
    family: &str,
    style: usize,
    size_pt: f32,
) -> Option<String> {
    let face = face_of(choices, family, style)?;
    let words = crate::fonts::style_words(face.weight, face.slant);
    let size = format!("{size_pt:.2}");
    let size = size.trim_end_matches('0').trim_end_matches('.');
    Some(if words.is_empty() {
        format!("{family} {size}")
    } else {
        format!("{family} {words} {size}")
    })
}

fn color_row(ui: &mut egui::Ui, label: &str, c: &mut crate::config::Rgb, ev: &mut ConfigEvents) {
    row(ui, label, |ui| {
        ui.horizontal(|ui| {
            if color_cell(ui, c) { ev.apply = true; }
            ui.label(egui::RichText::new(format!("#{:02X}{:02X}{:02X}", c.0, c.1, c.2))
                .monospace().color(ui.visuals().weak_text_color()));
        });
    });
}

/// The color button alone (the control side of an aligned row).
fn color_cell(ui: &mut egui::Ui, c: &mut crate::config::Rgb) -> bool {
    let mut color = rgb_c32(*c);
    let response = ui.add_sized([30.0, 28.0], egui::Button::new("").fill(color)
        .stroke(ui.visuals().widgets.inactive.bg_stroke).corner_radius(5.0))
        .on_hover_text(format!("Edit #{:02X}{:02X}{:02X}", c.0, c.1, c.2));
    let mut changed = false;
    egui::Popup::menu(&response)
        .style((**ui.style()).clone())
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.spacing_mut().slider_width = 275.0;
            changed = egui::color_picker::color_picker_color32(ui, &mut color, egui::color_picker::Alpha::Opaque);
        });
    if changed { *c = crate::config::Rgb(color.r(), color.g(), color.b()); }
    changed
}

fn checkbox_row(ui: &mut egui::Ui, label: &str, v: &mut bool, ev: &mut ConfigEvents) {
    row(ui, label, |ui| ev.apply |= crate::ui_style::toggle(ui, label, v));
}




#[cfg(test)]
mod regression {
    use super::*;

    #[test]
    fn every_shipped_keybinding_is_listed_and_named() {
        // An action missing from the table would land in "Other" with a
        // snake_case name, and a label that calls key_groups would recurse.
        let cfg = crate::config::Config::default();
        let listed: std::collections::HashSet<String> = key_groups(&cfg)
            .iter().flat_map(|(_, rows)| rows.iter().map(|(a, _)| a.clone())).collect();
        for (action, _) in &cfg.bindings {
            assert!(listed.contains(action), "{action} is not on the Keys page");
            let label = action_label(action);
            assert!(!label.is_empty() && !label.contains('_'), "{action} labels as {label:?}");
        }
        assert!(!key_groups(&cfg).iter().any(|(name, _)| *name == "Other"),
            "a default config should need no Other group");
    }

    #[test]
    fn a_hand_bound_action_still_reaches_the_keys_page() {
        let cfg = crate::config::Config::from_text(
            "wal_palette: ''\nbindings:\n  my_odd_action: MOD1+SHIFT+y\n", std::path::Path::new(".")).unwrap();
        let groups = key_groups(&cfg);
        let other = groups.iter().find(|(name, _)| *name == "Other")
            .expect("a custom action belongs under Other");
        assert!(other.1.iter().any(|(a, l)| a == "my_odd_action" && l == "My odd action"));
    }

    #[test]
    fn popup_anchors_convert_physical_pixels_once() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            assert_eq!(popup_position((120.0 * scale, 80.0 * scale), scale), [120.0, 80.0]);
        }
    }

    #[test]
    fn escape_dismisses_a_menu_even_before_its_second_frame() {
        let ctx = Context::default();
        let cfg = crate::config::Config::default();
        let mut st = UiState { menu_open: true, menu_age: 0,
            menu_items: vec![("New window".into(), "new_window".into(), true)], ..Default::default() };
        let mut full = ctx.run_ui(egui::RawInput {
            events: vec![egui::Event::Key { key: egui::Key::Escape, physical_key: None,
                pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }],
            ..Default::default()
        }, |ui| ctx_menu(ui, &mut st, &cfg));
        full.textures_delta.clear();
        assert_eq!(st.menu_out, Some(None));
    }

    #[test]
    fn context_and_app_menus_use_shared_surface_at_each_scale() {
        for dark in [false, true] {
            for scale in [1.0, 2.0] {
                for app in [false, true] {
                    let ctx = Context::default();
                    ctx.set_pixels_per_point(scale);
                    ctx.set_visuals(if dark { egui::Visuals::dark() } else { egui::Visuals::light() });
                    let cfg = crate::config::Config::default();
                    let mut st = UiState { menu_open: true, menu_app: app,
                        menu_pos: popup_position((120.0 * scale, 80.0 * scale), scale),
                        menu_items: vec![("New window".into(), "new_window".into(), true)],
                        ..Default::default() };
                    for frame in 0..3 {
                        let mut colors = None;
                        let mut full = ctx.run_ui(egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 900.0))),
                            ..Default::default()
                        }, |ui| {
                            colors = Some(TermStyle::new(ui, ui.visuals().text_color(), ui.visuals().error_fg_color));
                            ctx_menu(ui, &mut st, &cfg);
                        });
                        full.textures_delta.clear();
                        if frame == 0 { continue; } // egui's initial sizing pass is invisible.
                        let c = colors.unwrap();
                        let surface = full.shapes.iter().find_map(|shape| match &shape.shape {
                            egui::Shape::Rect(r) if r.fill == c.bg && r.stroke.color == c.border => Some(r),
                            _ => None,
                        }).expect("shared popup surface missing");
                        assert!((surface.rect.left() - 120.0).abs() <= 1.0);
                        assert!((surface.rect.top() - 80.0).abs() <= 1.0);
                        assert_eq!(surface.corner_radius, egui::CornerRadius::same(9));
                        full.textures_delta.clear();
                    }
                }
            }
        }
    }

    #[test]
    fn history_filters_reject_blank_commands() {
        for cmd in ["", "  ", "\t\n", "\u{2003}"] {
            let row = HistoryRow { seq: 1, cmd: cmd.into(), exit: Some(0), done: true,
                alt_seen: false, lines: 0, time: String::new(), duration: String::new() };
            for filter in [HistoryFilter::All, HistoryFilter::Success, HistoryFilter::Error] {
                assert!(!history_visible(&row, filter, ""));
            }
        }
    }

    #[test]
    fn application_info_owns_keyboard_and_escape_restores_terminal_input() {
        for page in [InfoPage::Shortcuts, InfoPage::About] {
            let ctx = Context::default();
            let cfg = crate::config::Config::default();
            let mut st = UiState { info_page: Some(page), ..Default::default() };
            assert!(st.owns_keyboard(&ctx));
            for escape in [false, false, true] {
                let mut full = ctx.run_ui(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(360.0, 300.0))),
                    events: if escape { vec![egui::Event::Key {
                        key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }] } else { vec![] },
                    ..Default::default()
                }, |ui| app_info(ui, &mut st, &cfg));
                full.textures_delta.clear();
            }
            assert!(st.info_page.is_none());
            assert!(!st.owns_keyboard(&ctx));
        }
    }

    /// One layout pass of the config panel at a given screen size.
    fn pass(ctx: &Context, w: f32, h: f32, open: bool) -> egui::FullOutput {
        let mut st = UiState::default();
        let mut cfg = crate::config::Config::default();
        let mut open_local = open;
        let mut full = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(w, h))),
                ..Default::default()
            },
            |ui| { config_window(ui, &mut open_local, &mut cfg, &mut st, &[]); },
        );
        full.textures_delta.clear();
        full
    }

    // Regression: a row that requested an infinite size blew the layout up
    // to NaN (INF - INF) and tripped egui's `frame_rect is NaN` assert. The
    // sequence below mirrors the real one: init warm-up, live frames at the
    // window's real size, close, reopen.
    #[test]
    fn config_window_layout_sequence() {
        let ctx = Context::default();
        ctx.set_visuals(egui::Visuals::dark());
        pass(&ctx, 1200.0, 800.0, true); // warm-up
        // Open pass: the panel is a full-height frame docked at the right
        // edge (this is what catches a layout that slides it off-screen).
        let full = pass(&ctx, 1280.0, 720.0, true);
        let docked = full.shapes.iter().any(|cs| {
            matches!(
                cs,
                egui::epaint::ClippedShape {
                    shape: egui::Shape::Rect(r),
                    ..
                } if r.rect.size().y > 720.0 * 0.9
                    && (r.rect.max.x - 1280.0).abs() < 1.0
                    && r.rect.size().x > 200.0
            )
        });
        assert!(docked, "panel is not full-height and docked right");
        pass(&ctx, 1280.0, 720.0, true);
        pass(&ctx, 900.0, 600.0, true); // resize
        pass(&ctx, 900.0, 600.0, false); // close
        pass(&ctx, 900.0, 600.0, true); // reopen
    }

    #[test]
    fn workspace_editor_changes_only_the_selected_workspace() {
        use crate::config::{WorkspaceIcon, parse_hex};
        let ctx = Context::default();
        let mut cfg = crate::config::Config::default();
        let original = cfg.clone();
        let mut st = UiState { workspace_edit: 9, ..Default::default() };
        fn frame(ctx: &Context, cfg: &mut crate::config::Config, st: &mut UiState, events: Vec<egui::Event>) -> Vec<egui::epaint::ClippedShape> {
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(420.0, 700.0))),
                events, ..Default::default()
            }, |ui| {
                let colors = TermStyle::new(ui, egui::Color32::GREEN, egui::Color32::RED);
                colors.apply_controls(ui);
                workspace_settings(ui, cfg, st, &mut ConfigEvents { apply: false, save: false, revert: false }, &colors);
            });
            output.textures_delta.clear();
            output.shapes
        }
        fn click(ctx: &Context, cfg: &mut crate::config::Config, st: &mut UiState, pos: egui::Pos2) {
            frame(ctx, cfg, st, vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton {
                pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE }]);
            frame(ctx, cfg, st, vec![egui::Event::PointerButton {
                pos, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE }]);
        }
        let mut shapes = Vec::new();
        for _ in 0..3 { shapes = frame(&ctx, &mut cfg, &mut st, vec![]); }
        let icons: Vec<_> = shapes.iter().filter_map(|s| match &s.shape {
            egui::Shape::Rect(rect) if (rect.rect.height() - 36.0).abs() < 0.1 => Some(rect.rect), _ => None,
        }).collect();
        assert_eq!(icons.len(), 10);
        click(&ctx, &mut cfg, &mut st, icons[4].center());
        assert_eq!(cfg.workspace_icons[9], WorkspaceIcon::Cube);
        let red = shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Rect(rect) if rect.fill == rgb_c32(parse_hex("#f07b72")) && rect.rect.width() == 19.0 => Some(rect.rect.center()), _ => None,
        }).unwrap();
        click(&ctx, &mut cfg, &mut st, red);
        assert_eq!(cfg.workspace_colors[9], parse_hex("#f07b72"));
        let name_label = shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Text(text) if text.galley.text() == "Name" => Some(text.pos), _ => None,
        }).unwrap();
        click(&ctx, &mut cfg, &mut st, name_label + egui::vec2(35.0, 35.0));
        frame(&ctx, &mut cfg, &mut st, vec![egui::Event::Text("Research".into())]);
        assert_eq!(cfg.workspace_genres[9], "Research");
        assert_eq!(cfg.workspace_icons[..9], original.workspace_icons[..9]);
        assert_eq!(cfg.workspace_colors[..9], original.workspace_colors[..9]);
        assert_eq!(cfg.workspace_genres[..9], original.workspace_genres[..9]);
        st.workspace_edit = 0;
        frame(&ctx, &mut cfg, &mut st, vec![]);
        assert_eq!(cfg.workspace_icons[9], WorkspaceIcon::Cube);
    }

    #[test]
    fn appearance_exposes_workspace_identity_controls() {
        let ctx = Context::default();
        let mut st = UiState { settings_page: SettingsPage::Appearance, ..Default::default() };
        let mut cfg = crate::config::Config::default();
        let mut labels = Vec::new();
        for _ in 0..3 {
            let mut full = ctx.run_ui(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 2200.0))),
                ..Default::default()
            }, |ui| { config_window(ui, &mut true, &mut cfg, &mut st, &[]); });
            labels = full.shapes.iter().filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            }).collect();
            full.textures_delta.clear();
        }
        for label in ["Manage workspace names, colors, and icons.", "Number", "Name", "Color", "Icon", "Button display"] {
            assert!(labels.iter().any(|text| text == label), "missing {label}");
        }
        for retired in ["Tab", "Tab (active)", "Tab text", "Tab text (active)"] {
            assert!(!labels.iter().any(|label| label == retired));
        }
    }

    /// A control belongs to exactly one category. These labels are unique across
    /// the whole settings UI (unlike section titles - the Keys page has a
    /// "Workspaces" group of chords and Navigation has a "Workspaces" section of
    /// settings), so a control that drifts to another page fails this test
    /// instead of quietly rendering somewhere.
    const PAGE_MARKS: [(SettingsPage, &[&str]); 6] = [
        (SettingsPage::Appearance, &["Theme", "Terminal brightness", "Terminal font", "Animation speed", "Button position", "Tab strip", "Group colors", "Button display"]),
        (SettingsPage::Window, &["Default width", "Default height", "Transparency", "Start maximized", "Close window dialog"]),
        (SettingsPage::Tabs, &["Compact tabs", "Multiline tabs", "Title length", "New tab position", "Graveyard keeps"]),
        (SettingsPage::Navigation, &["Tab switcher enabled", "Delay (ms)", "Preview scale", "Backdrop dim"]),
        (SettingsPage::Terminal, &["Scrollback lines"]),
        (SettingsPage::Keys, &["Copy", "Search this tab", "Text bigger"]),
    ];

    fn page_labels(ctx: &Context, page: SettingsPage, size: (f32, f32)) -> Vec<String> {
        let mut st = UiState { settings_page: page, ..Default::default() };
        let mut cfg = crate::config::Config::default();
        let mut labels = Vec::new();
        // Two passes: a Window's first pass only measures.
        for _ in 0..2 {
            let mut full = ctx.run_ui(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size.0, size.1))),
                ..Default::default()
            }, |ui| { config_window(ui, &mut true, &mut cfg, &mut st, &[]); });
            labels = full.shapes.iter().filter_map(|shape| match &shape.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            }).collect();
            full.textures_delta.clear();
        }
        labels
    }

    #[test]
    fn every_category_is_selectable_and_owns_its_controls() {
        let ctx = Context::default();
        for (page, marks) in PAGE_MARKS {
            let labels = page_labels(&ctx, page, (1100.0, 2200.0));
            for mark in marks {
                assert!(labels.iter().any(|l| l == *mark), "{page:?} lost its {mark:?} control");
            }
            for (other, other_marks) in PAGE_MARKS {
                if other == page { continue; }
                for mark in other_marks {
                    assert!(!labels.iter().any(|l| l == *mark),
                        "{mark:?} belongs to {other:?} but appeared on {page:?}");
                }
            }
            // The selector is the same on every page, or a category becomes
            // unreachable once the user is on another one.
            for (_, name) in SETTINGS_PAGES {
                assert!(labels.iter().any(|l| l == *name), "selector lost {name} on {page:?}");
            }
        }
    }

    #[test]
    fn category_labels_survive_the_minimum_panel_width() {
        // 448 is the default panel width and 412 the minimum; a long name must
        // not be truncated at either, and nothing may reach outside the window.
        for width in [448.0, 412.0, 640.0] {
            let ctx = Context::default();
            let mut st = UiState { panel_top: 24.0, ..Default::default() };
            let mut cfg = crate::config::Config::default();
            for _ in 0..3 {
                let mut full = ctx.run_ui(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 850.0))),
                    ..Default::default()
                }, |ui| { config_window(ui, &mut true, &mut cfg, &mut st, &[]); });
                for (_, name) in SETTINGS_PAGES {
                    let text = full.shapes.iter().find_map(|s| match &s.shape {
                        egui::Shape::Text(t) if t.galley.text() == *name => Some((s.clip_rect, t)),
                        _ => None,
                    });
                    // The first sizing pass of a new window doesn't paint.
                    if let Some((clip, t)) = text {
                        let rect = egui::Rect::from_min_size(t.pos, t.galley.size());
                        assert!(clip.contains_rect(rect), "{name} is clipped at {width}px: {rect:?} by {clip:?}");
                        assert!(rect.max.x <= width, "{name} runs past the window at {width}px: {rect:?}");
                    }
                }
                full.textures_delta.clear();
            }
        }
    }

    #[test]
    fn settings_pages_keep_save_and_cancel_visible_after_resize() {
        let ctx = Context::default();
        let mut st = UiState { panel_top: 24.0, ..Default::default() };
        let mut cfg = crate::config::Config::default();
        let mut open = true;
        for (w, h) in [(1280.0, 850.0), (900.0, 540.0), (640.0, 420.0)] {
            for page in SETTINGS_PAGES.iter().map(|(page, _)| *page) {
                st.settings_page = page;
                for _ in 0..3 {
                    let mut full = ctx.run_ui(egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h))),
                        ..Default::default()
                    }, |ui| { config_window(ui, &mut open, &mut cfg, &mut st, &[]); });
                    full.textures_delta.clear();
                    for label in ["Save", "Cancel"] {
                        let text = full.shapes.iter().find_map(|s| match &s.shape {
                            egui::Shape::Text(t) if t.galley.text() == label => Some((s.clip_rect, t)),
                            _ => None,
                        });
                        // The first sizing pass of a new window doesn't paint.
                        if let Some((clip, text)) = text {
                            let rect = egui::Rect::from_min_size(text.pos, text.galley.size());
                            assert!(rect.min.y >= 24.0 && rect.max.y < h, "{label} is outside the window: {rect:?}");
                            assert!(clip.contains_rect(rect), "{label} is clipped: {rect:?} by {clip:?}");
                        }
                    }
                }
                let mut full = ctx.run_ui(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h))),
                    ..Default::default()
                }, |ui| { config_window(ui, &mut open, &mut cfg, &mut st, &[]); });
                for label in ["Save", "Cancel"] {
                    assert!(full.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == label)), "{label} missing");
                }
                full.textures_delta.clear();
            }
        }
    }

    // The font popup: opens from a row, previews installed fonts through the
    // FontPreviewProvider (which reads a real font file into egui's atlas on
    // first use), and stays on screen across passes.
    #[test]
    fn font_popup_layout() {
        let ctx = Context::default();
        ctx.set_visuals(egui::Visuals::dark());
        // A real database so the provider has real files to read (the same
        // construction the app's FontSystem uses).
        let mut db = glyphon::fontdb::Database::new();
        db.load_system_fonts();
        let choices = crate::fonts::font_choices(&db);
        assert!(!choices.is_empty(), "no installed fonts on the test machine");
        // Register the same provider Egui::init does: one entry per face.
        ctx.add_font_provider(std::sync::Arc::new(FontPreviewProvider {
            fonts: std::sync::Mutex::new(
                choices
                    .iter()
                    .flat_map(|c| {
                        c.faces.iter().map(move |f| {
                            (
                                preview_family_name(&c.name, f.weight, f.slant),
                                (f.path.clone(), f.index),
                            )
                        })
                    })
                    .collect(),
            ),
        }));

        let mut st = UiState::default();
        let mut cfg = crate::config::Config::default();
        // Seed from a family in the middle of the list: the popup puts the
        // current family first, so the next row down is the first family
        // alphabetically -- two distinct names to measure the list with.
        let mid = &choices[choices.len() / 2];
        cfg.term_font = format!("{} 13", mid.name);
        let mut open = true;
        // The popup, seeded from the config value exactly as the button does.
        st.font_popup = Some(seed_popup(FontWhich::Term, &cfg.term_font, &choices));
        let family = st.font_popup.as_ref().unwrap().family.clone();
        assert_eq!(family, mid.name, "seed should name the family");
        assert_eq!(st.font_popup.as_ref().unwrap().size_pt, 13.0);
        for _ in 0..3 {
            let mut full = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_max(
                        egui::pos2(0.0, 0.0),
                        egui::pos2(1280.0, 720.0),
                    )),
                    ..Default::default()
                },
                |ui| { config_window(ui, &mut open, &mut cfg, &mut st, &choices); },
            );
            full.textures_delta.clear();
        }

        // The drafted face made it into egui's font system: the provider
        // read the file and epaint installed it under the face's preview
        // family name when the preview laid out (this is the whole point of
        // the popup). The chain is observable through the family's
        // character map.
        let style = st.font_popup.as_ref().unwrap().style;
        let face = face_of(&choices, &family, style).expect("drafted face exists");
        let key = preview_family_name(&family, face.weight, face.slant);
        let chars = ctx.fonts_mut(|f| {
            f.characters(&egui::FontFamily::Name(std::sync::Arc::from(key.as_str()))).clone()
        });
        assert!(!chars.is_empty(), "preview face was not installed into egui");

        // Layout: the popup must stack VERTICALLY (its default layout is
        // left-to-right, which once rendered the whole dialog as one
        // horizontal strip). Measure: the current family label first, the
        // alphabetically-first family below it, and the OK button under the
        // lists -- all compared by galley position.
        let full = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_max(
                    egui::pos2(0.0, 0.0),
                    egui::pos2(1280.0, 720.0),
                )),
                ..Default::default()
            },
            |ui| { config_window(ui, &mut open, &mut cfg, &mut st, &choices); },
        );
        let label_pos = |want: &str| -> Option<egui::Pos2> {
            full.shapes.iter().find_map(|cs| match cs {
                egui::epaint::ClippedShape { shape: egui::Shape::Text(t), .. }
                    if t.galley.text() == want =>
                {
                    Some(t.pos)
                }
                _ => None,
            })
        };
        let cur = label_pos(&mid.name).expect("current family name in the list");
        let next = label_pos(&choices[0].name).expect("second family name in the list");
        assert!(
            next.y > cur.y + 8.0 && (next.x - cur.x).abs() < 8.0,
            "family list is not a vertical column: {:?} at {cur:?}, {:?} at {next:?}",
            mid.name,
            choices[0].name
        );
        let ok = label_pos("OK").expect("OK button");
        assert!(
            ok.y > cur.y + 100.0,
            "OK button is not below the lists: OK at {ok:?}, list starts at {cur:?}"
        );
        assert!(label_pos("Size").is_some(), "size control missing");

        // Commit round-trip: the composed description parses back to the
        // same family and lands on the same face.
        let composed =
            compose_value(&choices, &family, style, 13.0).expect("draft composes");
        let (spec, size) = crate::config::parse_font(&composed);
        assert_eq!(size, 13.0, "size must round-trip through {composed:?}");
        let r = crate::fonts::resolve_font(&db, &spec);
        assert_eq!(r.family, family, "family must round-trip through {composed:?}");
        assert_eq!(
            (r.weight, r.slant),
            (face.weight, face.slant),
            "style must round-trip through {composed:?}"
        );
    }

    /// The switcher's first line names the workspace on its right, and only while
    /// workspaces are on. Checked at both the widest and the narrowest surface, so
    /// the label cannot run into the open-terminal count when the room is gone.
    #[test]
    fn switcher_header_names_the_workspace_without_reaching_the_count() {
        let cards: Vec<SwitcherCard> = (0..8).map(|i| SwitcherCard {
            id: i,
            title: format!("tab {i}"),
            status: SwitcherStatus::Unknown,
            active: false,
            marker: false,
            cwd: None,
            thumb: None,
        }).collect();
        for workspace in [Some(9), None] {
            for (w, h) in [(1920.0, 1080.0), (560.0, 480.0)] {
                let ctx = Context::default();
                let (tw, th) = thumbnail_size(w, 1.78, 0.25);
                let mut output = None;
                for _ in 0..3 {
                    let mut full = ctx.run_ui(egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h))),
                        ..Default::default()
                    }, |ui| switcher_screen(ui, true, 0, egui::Color32::from_black_alpha(150),
                        egui::Color32::CYAN, &cards, egui::vec2(tw.round(), th.round()), workspace, &mut None));
                    full.textures_delta.clear();
                    output = Some(full);
                }
                let full = output.unwrap();
                let placed = |wanted: &dyn Fn(&str) -> bool| -> Option<(egui::Rect, egui::Rect)> {
                    full.shapes.iter().find_map(|cs| match &cs.shape {
                        egui::Shape::Text(t) if wanted(t.galley.text()) => Some((
                            egui::Rect::from_min_size(t.pos, t.galley.size()), cs.clip_rect)),
                        _ => None,
                    })
                };
                let count = placed(&|t: &str| t.starts_with("8 open terminal"));
                let name = placed(&|t: &str| t == "Workspace 10" || t == "10");
                assert!(count.is_some(), "the count vanished at {w}x{h}");
                assert_eq!(name.is_some(), workspace.is_some(), "workspace label wrong for {workspace:?} at {w}x{h}");
                if let (Some((count, _)), Some((label, clip))) = (count, name) {
                    assert!(clip.contains_rect(label), "workspace label clipped at {w}x{h}: {label:?} in {clip:?}");
                    assert!(label.min.x >= count.max.x, "workspace label reached the count at {w}x{h}: {label:?} vs {count:?}");
                    assert!(label.max.x <= clip.max.x, "workspace label overflowed at {w}x{h}");
                }
            }
        }
    }

    #[test]
    fn switcher_keeps_selection_and_previews_inside_the_surface() {
        let ctx = Context::default();
        for (w, h) in [(1920.0, 1080.0), (560.0, 480.0)] {
            let (tw, th) = thumbnail_size(w, 1.78, 0.25);
            let (tw, th) = (tw.round() as u32, th.round() as u32);
            let tex = ctx.load_texture("preview", egui::ColorImage::new(
                [tw as usize, th as usize], vec![egui::Color32::BLACK; (tw * th) as usize]), egui::TextureOptions::LINEAR);
            let cards: Vec<SwitcherCard> = (0..8).map(|i| SwitcherCard {
                id: i,
                title: format!("a much longer tab title number {i}"),
                status: SwitcherStatus::Unknown,
                active: i == 1,
                marker: false,
                cwd: Some("~/projects".into()),
                thumb: Some((tex.id(), (tw, th))),
            }).collect();
            for selected in [7, 0, 7] {
                let mut output = None;
                // Check final card containment after the shared transition settles.
                for _ in 0..20 {
                    let mut full = ctx.run_ui(egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h))),
                        ..Default::default()
                    }, |ui| switcher_screen(ui, true, selected, egui::Color32::from_black_alpha(150), egui::Color32::CYAN, &cards, egui::vec2(tw as f32, th as f32), None, &mut None));
                    full.textures_delta.clear();
                    output = Some(full);
                }
                let full = output.unwrap();
                let (clip, selection) = full.shapes.iter().find_map(|cs| match &cs.shape {
                    egui::Shape::Rect(r) if r.stroke.width == 2.5 => Some((cs.clip_rect, r.rect)),
                    _ => None,
                }).expect("selected card missing");
                assert!(clip.expand(1.0).contains_rect(selection), "selected card is clipped: {selection:?} by {clip:?}");
                assert!(selection.min.x >= 0.0 && selection.max.x <= w && selection.min.y >= 0.0 && selection.max.y <= h);
                let images: Vec<_> = full.shapes.iter().filter_map(|cs| match &cs.shape {
                    egui::Shape::Rect(r) if r.fill_texture_id() == tex.id() => Some(r.rect),
                    _ => None,
                }).collect();
                assert!(!images.is_empty(), "live previews missing");
                for image in images {
                    assert!((image.width() - tw as f32).abs() < 1.0 && (image.height() - th as f32).abs() < 1.0, "preview was distorted");
                    if image.intersects(selection) {
                        assert!(selection.contains_rect(image), "preview outside card");
                        assert!(image.top() >= selection.top() + 32.0, "preview overlaps header");
                    }
                }
            }
        }
    }

}

#[cfg(test)]
mod input_regression {
    use super::*;

    struct Sim<'a> {
        ctx: Context,
        st: &'a mut UiState,
        cfg: &'a mut crate::config::Config,
        open: bool,
        choices: &'a [crate::fonts::FontChoice],
        last_popup_rect: Option<egui::Rect>,
    }

    impl<'a> Sim<'a> {
        fn frame(&mut self, events: Vec<egui::Event>) -> (Vec<String>, bool, Option<String>) {
            let Self { ctx, st, cfg, open, choices, .. } = self;
            let mut full = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1280.0, 720.0))),
                    events,
                    ..Default::default()
                },
                |ui| { config_window(ui, open, cfg, st, choices); },
            );
            full.textures_delta.clear();
            let labels: Vec<String> = full
                .shapes
                .iter()
                .filter_map(|cs| match cs {
                    egui::epaint::ClippedShape { shape: egui::Shape::Text(t), .. } => {
                        let s = t.galley.text().to_string();
                        (self.choices.iter().any(|c| c.name == s)).then_some(s)
                    }
                    _ => None,
                })
                .collect();
            if let Some(r) = full.shapes.iter().find_map(|cs| match cs {
                egui::epaint::ClippedShape { shape: egui::Shape::Rect(r), .. } => {
                    let w = r.rect.width();
                    (w > 380.0 && w < 460.0 && r.rect.height() > 18.0 && r.rect.height() < 40.0)
                        .then_some(r.rect)
                }
                _ => None,
            }) {
                self.last_popup_rect = Some(r);
            }
            let popup_open = self.st.font_popup.as_ref().is_some_and(|p| p.open);
            let search = self.st.font_popup.as_ref().map(|p| p.search.clone());
            (labels, popup_open, search)
        }
    }

    /// Clicking and typing in the popup's filter must not dismiss it: the
    /// popup's DEFAULT close behavior (CloseOnClick) closed it on its own
    /// filter click, and the app then tore the state down. Regression test
    /// for the reported "typing into the font filter makes the popup vanish".
    #[test]
    fn filter_click_and_typing_keep_popup_open() {
        let ctx = Context::default();
        ctx.set_visuals(egui::Visuals::dark());
        let mut db = glyphon::fontdb::Database::new();
        db.load_system_fonts();
        let choices = crate::fonts::font_choices(&db);
        let mut st = UiState::default();
        let mut cfg = crate::config::Config::default();
        let seed_spec = choices[choices.len() / 2].name.clone();
        cfg.term_font = format!("{seed_spec} 13");
        let mut sim = Sim {
            ctx,
            st: &mut st,
            cfg: &mut cfg,
            open: true,
            choices: &choices,
            last_popup_rect: None,
        };

        // pass 0: popup seeded, as the button click does
        sim.st.font_popup = Some(seed_popup(FontWhich::Term, &format!("{seed_spec} 13"), &choices));
        let (l, pop, se) = sim.frame(vec![]);
        eprintln!("P0 popup={pop} labels={} search={se:?}", l.len());
        let (l, pop, se) = sim.frame(vec![]);
        eprintln!("P1 popup={pop} labels={} search={se:?}", l.len());
        let pr = sim.last_popup_rect.expect("filter box laid out in the popup");
        let filter_pos = pr.center();
        let mods = egui::Modifiers::default();
        let (_l, pop, _se) = sim.frame(
            vec![
                egui::Event::PointerMoved(filter_pos),
                egui::Event::PointerButton { pos: filter_pos, button: egui::PointerButton::Primary, pressed: true, modifiers: mods },
                egui::Event::PointerButton { pos: filter_pos, button: egui::PointerButton::Primary, pressed: false, modifiers: mods },
            ],
        );
        let focused = sim.ctx.memory(|m| m.focused().is_some());
        assert!(pop, "click on the popup's own filter dismissed the popup");
        assert!(focused, "the click did not focus the filter box");
        let (l, pop, se) = sim.frame(vec![egui::Event::Text("o".into())]);
        assert!(pop, "typing dismissed the popup");
        assert_eq!(se.as_deref(), Some("o"), "keystroke did not reach the filter");
        let (l2, pop, se) = sim.frame(vec![egui::Event::Text("n".into())]);
        assert!(pop, "typing dismissed the popup");
        assert_eq!(se.as_deref(), Some("on"), "keystroke did not reach the filter");
        assert!(l2.len() <= l.len(), "the filter did not narrow the family list");
    }

    /// The app gates real keystrokes on `egui_winit::consumed`, which is
    /// true whenever egui holds keyboard focus. If focus survives closing
    /// the panel/popup or a click back onto the terminal, every keystroke
    /// afterwards is eaten by egui -- no widget visible, nothing typed:
    /// the reported "letters vanish when I start typing". These cases pin
    /// that focus is actually surrendered.
    fn fresh_sim<'a>(st: &'a mut UiState, cfg: &'a mut crate::config::Config, open: bool, choices: &'a [crate::fonts::FontChoice]) -> Sim<'a> {
        Sim {
            ctx: Context::default(),
            st,
            cfg,
            open,
            choices,
            last_popup_rect: None,
        }
    }

    #[test]
    fn size_selector_preserves_decimal_points_and_is_easy_to_step() {
        let ctx = Context::default();
        ctx.set_visuals(egui::Visuals::dark());
        let mut db = glyphon::fontdb::Database::new();
        db.load_system_fonts();
        let choices = crate::fonts::font_choices(&db);
        let mut st = UiState::default();
        let mut cfg = crate::config::Config::default();
        let mid = choices[choices.len() / 2].name.clone();
        for size in [11.2, 11.25, 12.5, 13.0] {
            let value = format!("{mid} {size}");
            let popup = seed_popup(FontWhich::Term, &value, &choices);
            assert_eq!(popup.size_pt, size, "opening the picker rounded {size}");
            let committed = compose_value(&choices, &mid, popup.style, popup.size_pt).expect("commits");
            assert_eq!(crate::config::parse_font(&committed).1, size, "commit rounded {value}");
        }
        st.font_popup = Some(seed_popup(FontWhich::Term, &format!("{mid} 11.25"), &choices));
        let mut open = true;
        cfg.term_font = format!("{mid} 11.25");
        let mut texts = |ctx: &Context| -> Vec<String> {
            let mut full = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_max(
                        egui::pos2(0.0, 0.0),
                        egui::pos2(1280.0, 720.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    config_window(ui, &mut open, &mut cfg, &mut st, &choices);
                },
            );
            full.textures_delta.clear();
            full.shapes
                .iter()
                .filter_map(|cs| match cs {
                    egui::epaint::ClippedShape { shape: egui::Shape::Text(t), .. } => {
                        Some(t.galley.text().to_string())
                    }
                    _ => None,
                })
                .collect()
        };
        let g = {
            texts(&ctx);
            texts(&ctx)
        };
        // egui renders a DragValue's number and suffix as separate galleys.
        assert!(g.iter().any(|t| t.trim() == "11.25"), "decimal size is not displayed: {g:?}");
        assert!(g.iter().any(|t| t.trim() == "pt"), "size label lost its unit");
        // One-click steppers flank the number.
        assert!(g.iter().any(|t| t.trim() == "\u{2212}"), "no minus stepper");
        assert!(g.iter().any(|t| t.trim() == "+"), "no plus stepper");
    }

    /// The egui rename box: seeded draft arrives selected (first key
    /// replaces it), Enter commits, and any other focus loss -- Escape or a
    /// click away -- cancels. The hand-drawn prompt had this logic in the
    /// app's key handler; egui owns it now.
    #[test]
    fn rename_box_selects_seeds_commits_and_cancels() {
        let ctx = Context::default();
        let mut st = UiState::default();
        let step = |st: &mut UiState, events: Vec<egui::Event>| {
            let mut full = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_max(
                        egui::pos2(0.0, 0.0),
                        egui::pos2(900.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| rename_box(ui, st),
            );
            full.textures_delta.clear();
        };
        let key = |k: egui::Key| egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let click_far = egui::Event::PointerButton {
            pos: egui::pos2(800.0, 500.0),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        };

        st.rename_open = true;
        st.rename_seed = true;
        st.rename_draft = "worktree".to_string();
        step(&mut st, vec![]);
        // Focused, and the whole draft reads as selected text.
        let focused = ctx.memory(|m| m.focused()).expect("rename entry took focus");
        let sel = egui::text_edit::TextEditState::load(&ctx, focused)
            .and_then(|s| s.cursor.char_range())
            .expect("cursor state");
        assert_ne!(sel.primary, sel.secondary, "seeded draft is not selected");
        // The seed is a one-frame gesture.
        assert!(!st.rename_seed);

        // Typing over a selection replaces it -- the GTK behaviour the
        // hand-drawn draft used to emulate.
        step(
            &mut st,
            vec![egui::Event::Text("w".into())],
        );
        assert_eq!(st.rename_draft, "w", "typing did not replace the selection");

        // Enter commits exactly what is on screen.
        step(&mut st, vec![key(egui::Key::Enter)]);
        assert_eq!(
            st.rename_out.as_ref().map(Option::as_deref),
            Some(Some("w")),
            "Enter did not commit"
        );

        // A click elsewhere cancels: nothing is renamed.
        let mut st2 = UiState::default();
        st2.rename_open = true;
        st2.rename_seed = true;
        st2.rename_draft = "keep".to_string();
        step(&mut st2, vec![]);
        assert!(ctx.memory(|m| m.focused()).is_some());
        step(&mut st2, vec![click_far]);
        assert_eq!(
            st2.rename_out.as_ref().map(Option::as_deref),
            Some(None),
            "clicking away did not cancel"
        );
    }

    fn close_frame(ctx: &Context, st: &mut UiState, time: f64, events: Vec<egui::Event>) -> Vec<egui::epaint::Shape> {
        let mut full = ctx.run_ui(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0))),
            time: Some(time), events, ..Default::default()
        }, |ui| confirm_box(ui, st));
        full.textures_delta.clear();
        full.shapes.into_iter().map(|clipped| clipped.shape).collect()
    }

    fn close_key(key: egui::Key) -> Vec<egui::Event> {
        [true, false].into_iter().map(|pressed| egui::Event::Key {
            key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE,
        }).collect()
    }

    fn close_button(shapes: &[egui::epaint::Shape], label: &str) -> egui::Pos2 {
        shapes.iter().find_map(|shape| match shape {
            egui::epaint::Shape::Text(t) if t.galley.job.text == label => Some(t.pos + t.galley.size() * 0.5),
            egui::epaint::Shape::Vec(shapes) => {
                // Normally text is flattened; nested shapes are legal too.
                shapes.iter().find_map(|shape| match shape {
                    egui::epaint::Shape::Text(t) if t.galley.job.text == label => Some(t.pos + t.galley.size() * 0.5),
                    _ => None,
                })
            }
            _ => None,
        }).unwrap_or_else(|| panic!("button {label} not drawn"))
    }

    fn close_click(pos: egui::Pos2) -> Vec<egui::Event> {
        let mut events = vec![egui::Event::PointerMoved(pos)];
        events.extend([true, false].into_iter().map(|pressed| egui::Event::PointerButton {
            pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
        }));
        events
    }

    fn close_state(retainer: WindowRetainer) -> UiState {
        UiState { confirm_open: true, confirm_seed: true,
            confirm: CloseDialog::new(retainer, 8, 3, 5), ..Default::default() }
    }

    #[test]
    fn close_modal_defaults_to_cancel_and_recovery_is_explicit() {
        let ctx = Context::default();
        let mut st = close_state(WindowRetainer::Daemon);
        close_frame(&ctx, &mut st, 0.0, vec![]);
        let shapes = close_frame(&ctx, &mut st, 0.01, vec![]);
        assert!(st.owns_keyboard(&ctx));
        assert!(st.confirm_out.is_none());
        assert!(ctx.memory(|m| m.focused()).is_some());
        close_frame(&ctx, &mut st, 0.02, close_key(egui::Key::Enter));
        assert_eq!(st.confirm_out, Some(CloseChoice::Cancel), "bare Enter must select Cancel");
        close_frame(&ctx, &mut st, 0.03, close_click(close_button(&shapes, "Move to Graveyard")));
        assert_eq!(st.confirm_out, Some(CloseChoice::Graveyard));
        close_frame(&ctx, &mut st, 0.04, close_key(egui::Key::Escape));
        assert_eq!(st.confirm_out, Some(CloseChoice::Cancel));
        close_frame(&ctx, &mut st, 0.05, close_click(egui::pos2(5.0, 5.0)));
        assert_eq!(st.confirm_out, Some(CloseChoice::Cancel), "backdrop must dismiss safely");
    }

    #[test]
    fn destructive_close_waits_five_seconds_and_never_auto_confirms() {
        let ctx = Context::default();
        let mut st = close_state(WindowRetainer::None);
        close_frame(&ctx, &mut st, 10.0, vec![]);
        let shapes = close_frame(&ctx, &mut st, 10.01, vec![]);
        let button = close_button(&shapes, "Close permanently (5 s)");
        close_frame(&ctx, &mut st, 10.1, close_click(button));
        assert_eq!(st.confirm_out, None, "disabled destructive button fired");
        assert!(!st.confirm.permanent_ready(14.999));
        close_frame(&ctx, &mut st, 15.0, vec![]);
        assert_eq!(st.confirm_out, None, "countdown must never close the window");
        assert!(st.confirm.permanent_ready(15.0));
        close_frame(&ctx, &mut st, 15.01, close_key(egui::Key::Enter));
        assert_eq!(st.confirm_out, Some(CloseChoice::Cancel), "Enter still defaults to Cancel after unlocking");
        let shapes = close_frame(&ctx, &mut st, 15.02, vec![]);
        close_frame(&ctx, &mut st, 15.03, close_click(close_button(&shapes, "Close permanently")));
        assert_eq!(st.confirm_out, Some(CloseChoice::Permanent));
    }

    #[test]
    fn permanent_close_from_recoverable_window_starts_a_new_countdown() {
        let ctx = Context::default();
        let mut st = close_state(WindowRetainer::OtherWindow);
        close_frame(&ctx, &mut st, 0.0, vec![]);
        let shapes = close_frame(&ctx, &mut st, 100.0, vec![]);
        close_frame(&ctx, &mut st, 100.01, close_click(close_button(&shapes, "Close permanently…")));
        assert!(st.confirm_out.is_none());
        assert!(!st.confirm.permanent_ready(105.0));
        assert!(st.confirm.permanent_ready(105.01));
        assert!(st.confirm.set_retainer(WindowRetainer::None));
        close_frame(&ctx, &mut st, 110.0, vec![]);
        assert!(!st.confirm.permanent_ready(114.999), "losing the recovery host must restart the warning");
        assert!(st.confirm.permanent_ready(115.0));
    }

    #[test]
    fn close_modal_keeps_cancel_visible_in_small_windows() {
        for retainer in [WindowRetainer::Daemon, WindowRetainer::None] {
            let ctx = Context::default();
            let mut st = close_state(retainer);
            let mut shapes = Vec::new();
            for frame in 0..2 {
                let mut full = ctx.run_ui(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 420.0))),
                    time: Some(frame as f64 * 0.01), ..Default::default()
                }, |ui| confirm_box(ui, &mut st));
                full.textures_delta.clear();
                shapes = full.shapes.into_iter().map(|s| s.shape).collect::<Vec<_>>();
            }
            let cancel = close_button(&shapes, "Cancel");
            assert!(cancel.x > 0.0 && cancel.x < 320.0 && cancel.y > 0.0 && cancel.y < 420.0,
                "Cancel fell outside the window: {cancel:?}");
        }
    }

    #[test]
    fn keyboard_focus_released_when_ui_steps_aside() {
        let mut db = glyphon::fontdb::Database::new();
        db.load_system_fonts();
        let choices = crate::fonts::font_choices(&db);
        let seed_spec = choices[choices.len() / 2].name.clone();
        let mods = egui::Modifiers::default();

        // Focus the font filter: click it, verify focus actually landed.
        let focus_filter = |sim: &mut Sim| {
            let p = seed_popup(FontWhich::Term, &format!("{seed_spec} 13"), sim.choices);
            sim.st.font_popup = Some(p);
            // egui's first pass sizes the popup; the second paints its controls.
            sim.frame(vec![]);
            sim.frame(vec![]);
            let r = sim.last_popup_rect.expect("filter laid out");
            let pos = r.center();
            sim.frame(vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: mods },
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: mods },
            ]);
            assert!(sim.ctx.memory(|m| m.focused().is_some()), "filter never got focus");
        };

        // A: panel closed (Save / x) while the filter had focus. Closing the
        // panel also clears the popup, exactly as config_window does, so the
        // app's edge helper sees (true,true) -> (false,false).
        let mut st = UiState::default();
        let mut cfg = crate::config::Config::default();
        let mut sim = fresh_sim(&mut st, &mut cfg, true, &choices);
        focus_filter(&mut sim);
        let mut prev = (true, true);
        eprintln!("DBG pre-close: mem={:?} i.focused={}", sim.ctx.memory(|m| m.focused()), sim.ctx.input(|i| i.focused));
        sim.open = false;
        sim.st.font_popup = None;
        sim.frame(vec![]);
        eprintln!("DBG post-close-frame: mem={:?} i.focused={}", sim.ctx.memory(|m| m.focused()), sim.ctx.input(|i| i.focused));
        release_focus_if_closed(&sim.ctx, false, false, &mut prev);
        sim.frame(vec![egui::Event::Text("z".into())]);
        eprintln!("DBG post-z: mem={:?} i.focused={}", sim.ctx.memory(|m| m.focused()), sim.ctx.input(|i| i.focused));
        assert!(sim.ctx.memory(|m| m.focused()).is_none(), "egui still holds a widget focus after the panel closed");

        // B: popup closed (OK/Cancel) while the filter had focus, panel open.
        let mut st = UiState::default();
        let mut cfg = crate::config::Config::default();
        let mut sim = fresh_sim(&mut st, &mut cfg, true, &choices);
        focus_filter(&mut sim);
        let mut prev = (true, true);
        sim.st.font_popup = None;
        sim.frame(vec![]);
        release_focus_if_closed(&sim.ctx, true, false, &mut prev);
        sim.frame(vec![egui::Event::Text("z".into())]);
        assert!(sim.ctx.memory(|m| m.focused()).is_none(), "egui still holds a widget focus after the popup closed");

        // C: filter focused, user clicks the terminal. Such a click is NOT
        // egui-consumed, and the app releases focus for every click it
        // claims itself; simulate that call.
        let mut st = UiState::default();
        let mut cfg = crate::config::Config::default();
        let mut sim = fresh_sim(&mut st, &mut cfg, true, &choices);
        focus_filter(&mut sim);
        let term = egui::pos2(400.0, 600.0); // left of the right-docked panel
        sim.frame(vec![
            egui::Event::PointerMoved(term),
            egui::Event::PointerButton { pos: term, button: egui::PointerButton::Primary, pressed: true, modifiers: mods },
            egui::Event::PointerButton { pos: term, button: egui::PointerButton::Primary, pressed: false, modifiers: mods },
        ]);
        eprintln!("DBG C pre-release: mem={:?}", sim.ctx.memory(|m| m.focused()));
        release_focus(&sim.ctx);
        eprintln!("DBG C post-release: mem={:?}", sim.ctx.memory(|m| m.focused()));
        sim.frame(vec![egui::Event::Text("z".into())]);
        eprintln!("DBG C post-z: mem={:?}", sim.ctx.memory(|m| m.focused()));
        assert!(sim.ctx.memory(|m| m.focused()).is_none(), "egui kept keyboard focus after a click on the terminal");
    }

    fn row(seq: u64, cmd: &str, exit: Option<i32>) -> HistoryRow {
        HistoryRow {
            seq,
            cmd: cmd.to_string(),
            exit,
            done: true,
            alt_seen: false,
            lines: 1,
            time: "00:00:00".into(),
            duration: String::new(),
        }
    }

    #[test]
    fn history_filter_and_search_narrow_the_list() {
        let ok = row(1, "cargo test", Some(0));
        let fail = row(2, "cargo build", Some(2));
        let nos = row(3, "ls", None);
        // All keeps everything; Success keeps zero-exit and no-status;
        // Error keeps only nonzero.
        assert!(history_visible(&ok, HistoryFilter::All, ""));
        assert!(history_visible(&ok, HistoryFilter::Success, ""));
        assert!(!history_visible(&fail, HistoryFilter::Success, ""));
        assert!(!history_visible(&ok, HistoryFilter::Error, ""));
        assert!(history_visible(&fail, HistoryFilter::Error, ""));
        assert!(history_visible(&nos, HistoryFilter::Success, ""), "no status is not an error");
        // Search is case-insensitive on the command line.
        assert!(history_visible(&ok, HistoryFilter::All, "CARGO"));
        assert!(!history_visible(&ok, HistoryFilter::All, "build"));
        assert!(history_visible(&ok, HistoryFilter::All, ""), "empty query keeps everything");
    }
    #[test]
    fn sidebars_claim_keyboard_only_for_active_editors() {
        for panel in [RightPanel::History, RightPanel::Settings] {
            let ctx = Context::default();
            let mut st = UiState { right_panel: panel, ..Default::default() };
            let id = egui::Id::unique("editor_or_button");
            let mut text = String::new();
            // Reuse the editor's id for a button to exercise stale TextEditState.
            for editor in [false, true, false] {
                ctx.memory_mut(|m| m.request_focus(id));
                let mut full = ctx.run_ui(egui::RawInput::default(), |ui| {
                    if editor {
                        ui.add(egui::TextEdit::singleline(&mut text).id(id));
                    } else {
                        ui.interact(ui.max_rect(), id, egui::Sense::click());
                    }
                });
                st.release_passive_focus(&ctx, &full.platform_output);
                full.textures_delta.clear();
                assert_eq!(st.owns_keyboard(&ctx), editor, "{panel:?}: editor={editor}");
            }
        }
    }

    #[test]
    fn history_only_claims_pointer_input_inside_the_sidebar() {
        let ctx = Context::default();
        let mut st = UiState { right_panel: RightPanel::History, ..Default::default() };
        for (point, claims) in [(egui::pos2(100.0, 200.0), false), (egui::pos2(1100.0, 200.0), true)] {
            for _ in 0..3 {
                let mut full = ctx.run_ui(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 720.0))),
                    events: vec![egui::Event::PointerMoved(point)],
                    ..Default::default()
                }, |ui| history_panel(ui, &mut st, egui::Color32::GREEN, egui::Color32::RED, 24.0));
                full.textures_delta.clear();
            }
            assert_eq!(ctx.egui_wants_pointer_input(), claims, "pointer at {point:?}");
            // A subsequent event can carry a new position before the next
            // UI pass has updated egui's pointer state.
            assert!(!panel_pointer_input(&ctx, egui::pos2(100.0, 200.0)));
            assert!(panel_pointer_input(&ctx, egui::pos2(1100.0, 200.0)));
        }
    }

    #[test]
    fn history_preview_uses_remaining_space_and_only_draws_visible_lines() {
        for height in [700.0, 1000.0] {
            let ctx = Context::default();
            let mut st = UiState {
                right_panel: RightPanel::History,
                history_rows: vec![row(1, "build", Some(0))],
                history_sel: Some(1),
                history_preview: Some(HistoryPreview::new(1, &"build output\n".repeat(50_000))),
                ..Default::default()
            };
            for frame in 0..3 {
                let mut full = ctx.run_ui(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, height))),
                    ..Default::default()
                }, |ui| history_panel(ui, &mut st, egui::Color32::GREEN, egui::Color32::RED, 28.0));
                full.textures_delta.clear();
                if frame == 0 { continue; }
                let lines: Vec<_> = full.shapes.iter().filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text() == "build output" => Some((s.clip_rect, t)),
                    _ => None,
                }).collect();
                assert!(!lines.is_empty(), "preview missing at height {height}");
                assert!(lines.len() < 100, "offscreen output was laid out");
                let clip = lines[0].0;
                assert!(clip.bottom() > height - 40.0, "preview does not fill remaining height");
                assert!(clip.right() <= 1100.0 && clip.left() >= 720.0, "preview escaped sidebar");
                assert!(!st.owns_keyboard(&ctx), "output preview must keep terminal typing active");
            }
        }
    }

}

#[cfg(test)]
mod workspace_panel_tests {
    use super::*;

    fn state() -> UiState {
        UiState {
            right_panel: RightPanel::Workspaces,
            ws_sel: Some(0),
            ws_rows: (0..10).map(|index| WsRow {
                index, count: if index < 3 { 2 } else { 0 },
                title: format!("Terminal {index}"),
                genre: if index == 1 { "Research".into() } else { String::new() },
                icon: crate::config::WorkspaceIcon::Layers,
                tabs: if index < 3 { (0..2).map(|tab| WsTab {
                    id: index * 10 + tab, title: format!("Terminal {index}-{tab}"),
                    selected: tab == 0, preview: None,
                }).collect() } else { Vec::new() },
                current: index == 0, shortcut: format!("Alt+Shift+{}", (index + 1) % 10),
            }).collect(),
            ..Default::default()
        }
    }

    fn frame(ctx: &egui::Context, st: &mut UiState, size: egui::Vec2, events: Vec<egui::Event>) -> Vec<egui::epaint::ClippedShape> {
        let mut output = ctx.run_ui(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events, ..Default::default()
        }, |ui| workspaces_panel(ui, st, egui::Color32::GREEN, egui::Color32::RED, 24.0, &[egui::Color32::CYAN; 10]));
        output.textures_delta.clear();
        output.shapes
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }
    }

    #[test]
    fn filtering_and_keyboard_navigation_follow_visible_workspace_indices() {
        let ctx = egui::Context::default();
        let mut st = state();
        let size = egui::vec2(900.0, 500.0);
        for _ in 0..3 { frame(&ctx, &mut st, size, vec![]); }
        st.ws_populated_only = true;
        st.ws_sel = Some(2);
        frame(&ctx, &mut st, size, vec![key(egui::Key::ArrowDown)]);
        assert_eq!(st.ws_sel, Some(0));
        st.ws_query = "research".into();
        frame(&ctx, &mut st, size, vec![]);
        assert_eq!(st.ws_sel, Some(1));
        frame(&ctx, &mut st, size, vec![key(egui::Key::Enter)]);
        assert_eq!(st.ws_out, Some(1));
        st.ws_query = "Terminal 2-1".into();
        assert_eq!(visible_workspaces(&st), vec![2]);
        st.ws_query = "missing".into();
        frame(&ctx, &mut st, size, vec![key(egui::Key::ArrowDown)]);
        assert_eq!(st.ws_sel, None);
        frame(&ctx, &mut st, size, vec![key(egui::Key::Enter)]);
        assert_eq!(st.ws_out, None);
    }

    #[test]
    fn left_and_right_focus_adjacent_tabs_in_the_selected_workspace_without_wrapping() {
        let ctx = egui::Context::default();
        let mut st = state();
        st.ws_sel = Some(2);
        let size = egui::vec2(1200.0, 800.0);
        for _ in 0..3 { frame(&ctx, &mut st, size, vec![]); }
        frame(&ctx, &mut st, size, vec![key(egui::Key::ArrowLeft)]);
        assert_eq!(st.ws_tab_out, None, "Left wrapped past the first tab");
        frame(&ctx, &mut st, size, vec![key(egui::Key::ArrowRight)]);
        assert_eq!(st.ws_tab_out, Some((2, 21)));
        assert_eq!(st.ws_out, None, "Right still committed the workspace instead of a tab");
        st.ws_rows[2].tabs[0].selected = false;
        st.ws_rows[2].tabs[1].selected = true;
        frame(&ctx, &mut st, size, vec![key(egui::Key::ArrowRight)]);
        assert_eq!(st.ws_tab_out, None, "Right wrapped past the last tab");
        frame(&ctx, &mut st, size, vec![key(egui::Key::ArrowLeft)]);
        assert_eq!(st.ws_tab_out, Some((2, 20)));
        st.ws_sel = Some(9);
        frame(&ctx, &mut st, size, vec![key(egui::Key::ArrowRight)]);
        assert_eq!(st.ws_tab_out, None, "an empty workspace has no adjacent tab");
    }

    #[test]
    fn expanded_preview_click_picks_the_exact_terminal() {
        let ctx = egui::Context::default();
        let mut st = state();
        let size = egui::vec2(1200.0, 800.0);
        let mut shapes = Vec::new();
        for _ in 0..3 { shapes = frame(&ctx, &mut st, size, vec![]); }
        let pos = shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(t) if t.galley.text() == "2  Terminal 0-1" => Some(t.pos + egui::vec2(8.0, 5.0)),
            _ => None,
        }).expect("expanded terminal caption missing");
        for pressed in [true, false] {
            frame(&ctx, &mut st, size, vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton {
                pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
            }]);
        }
        assert_eq!(st.ws_tab_out, Some((0, 1)));
        assert_eq!(st.ws_out, None, "a preview click also selected its parent card");
    }

    #[test]
    fn chevrons_expand_and_collapse_without_switching_workspaces() {
        let ctx = egui::Context::default();
        let mut st = state();
        let size = egui::vec2(1200.0, 800.0);
        let mut shapes = Vec::new();
        for _ in 0..3 { shapes = frame(&ctx, &mut st, size, vec![]); }
        let y = shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text() == "Workspace 1" => Some(t.pos.y + 17.0), _ => None,
        }).expect("workspace heading missing");
        let pos = egui::pos2(size.x - 28.0, y);
        for expected in [Some(0), None] {
            for pressed in [true, false] {
                frame(&ctx, &mut st, size, vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton {
                    pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
                }]);
            }
            assert_eq!(st.ws_collapsed, expected);
            assert_eq!(st.ws_out, None);
        }
    }

    #[test]
    fn every_workspace_follows_its_focus_and_buttons_focus_neighboring_tabs() {
        let ctx = egui::Context::default();
        let mut st = state();
        let size = egui::vec2(1200.0, 800.0);
        for workspace in 0..2 {
            let row = &mut st.ws_rows[workspace];
            row.tabs = (0..9).map(|index| WsTab {
                id: 100 * (workspace + 1) + index, title: format!("Terminal {workspace}-{index}"),
                selected: index == 8, preview: None,
            }).collect();
            row.count = row.tabs.len();
        }
        for _ in 0..3 { frame(&ctx, &mut st, size, vec![]); }
        assert!(st.ws_visible_tabs.contains(&108), "expanded workspace did not reveal its focused last tab");
        assert!(st.ws_visible_tabs.contains(&208), "compact workspace did not reveal its focused last tab");
        for (workspace, expanded) in [(0, true), (1, false)] {
            let id = egui::Id::unique("workspace_tab_strip").with((workspace, expanded));
            assert!(ctx.read_response(id.with(true)).is_some(), "missing left overflow arrow");
            assert!(ctx.read_response(id.with(false)).is_none(), "right arrow shown at the end");
            let pos = ctx.read_response(id.with(true)).unwrap().rect.center();
            for pressed in [true, false] {
                frame(&ctx, &mut st, size, vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton {
                    pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
                }]);
                assert_eq!(st.ws_out, None, "a tab arrow also selected its parent workspace card");
            }
            assert_eq!(st.ws_tab_out, Some((workspace, 100 * (workspace + 1) + 7)));
        }
        // A focus change follows again, independently in each workspace.
        for workspace in 0..2 {
            for (index, tab) in st.ws_rows[workspace].tabs.iter_mut().enumerate() { tab.selected = index == 0; }
        }
        for _ in 0..30 { frame(&ctx, &mut st, size, vec![]); }
        assert!(st.ws_visible_tabs.contains(&100));
        assert!(st.ws_visible_tabs.contains(&200));
        for (workspace, expanded) in [(0, true), (1, false)] {
            let id = egui::Id::unique("workspace_tab_strip").with((workspace, expanded));
            assert!(ctx.read_response(id.with(true)).is_none(), "left arrow shown at the beginning");
            assert!(ctx.read_response(id.with(false)).is_some());
        }
    }

    #[test]
    fn workspace_header_and_footer_fit_small_and_large_windows() {
        for size in [egui::vec2(480.0, 320.0), egui::vec2(900.0, 500.0), egui::vec2(1600.0, 1000.0)] {
            let ctx = egui::Context::default();
            let mut st = state();
            let mut shapes = Vec::new();
            for _ in 0..3 { shapes = frame(&ctx, &mut st, size, vec![]); }
            for label in ["Workspaces", "↑↓ workspace · ←→ tab · Enter go · Esc close"] {
                let (clip, t) = shapes.iter().find_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text() == label => Some((s.clip_rect, t)), _ => None,
                }).unwrap_or_else(|| panic!("{label} missing at {size:?}"));
                let rect = egui::Rect::from_min_size(t.pos, t.galley.size());
                assert!(clip.contains_rect(rect), "{label} clipped at {size:?}: {rect:?} by {clip:?}");
                assert!(rect.bottom() <= size.y, "{label} below the window at {size:?}");
            }
        }
    }
}
