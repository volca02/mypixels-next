//! mypixels (Rust port): tabbed terminal.
//! alacritty_terminal (emulation + pty) rendered with glyphon (cosmic_text)
//! on wgpu in a winit window.
//!
//! Usage: mypixels-rs [command [args...]]   (defaults to $SHELL or bash)

mod clipboard;
mod config;
mod cursor;
mod damage;
mod egui_ui;
mod ui_style;
mod tab_bar;
mod fonts;
mod gpu;
mod kitty;
mod kitty_kb;
mod mcp;
mod notifications;
mod tab;
mod session;
mod theme;

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use alacritty_terminal::event::WindowSize;
use alacritty_terminal::event_loop::Msg;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::TermMode;

use glyphon::{
    Attrs, Buffer, FontSystem, Metrics, Resolution, Shaping, TextArea, TextBounds,
};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::wayland::WindowAttributesExtWayland as _;
use winit::window::{Window, WindowId};

use config::Config;
use cursor::Cursor;
use gpu::Gpu;
use damage::Damage;
use tab::{SemanticMark, SpikeListener, Tab};

const HISTORY: usize = 10_000;
const MAX_RUNS: usize = 4096;
const PALETTE: [[f32; 3]; 16] = [
    [0.0, 0.0, 0.0], [0.8, 0.2, 0.2], [0.2, 0.6, 0.2], [0.6, 0.6, 0.2],
    [0.2, 0.2, 0.8], [0.6, 0.2, 0.6], [0.2, 0.6, 0.6], [0.8, 0.8, 0.8],
    [0.4, 0.4, 0.4], [1.0, 0.4, 0.4], [0.4, 1.0, 0.4], [1.0, 1.0, 0.4],
    [0.4, 0.4, 1.0], [1.0, 0.4, 1.0], [0.4, 1.0, 1.0], [1.0, 1.0, 1.0],
];


fn cube_color(i: u8) -> [f32; 3] {
    let i = (i - 16) as usize;
    let (r, g, b) = (i / 36, (i / 6) % 6, i % 6);
    let c = |v: usize| if v == 0 { 0.0 } else { (55.0 + 40.0 * v as f32) / 255.0 };
    [c(r), c(g), c(b)]
}

#[derive(Clone, Copy)]
struct Dims {
    cols: usize,
    rows: usize,
}

impl Dimensions for Dims {
    fn total_lines(&self) -> usize {
        self.rows + HISTORY
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

#[derive(Clone)]
struct Palette {
    colors: Box<[Option<[f32; 3]>]>,
    /// 16-color palette from the config; used when the terminal itself has
    /// not overridden a slot at runtime (OSC 4).
    cfg16: Option<[[f32; 3]; 16]>,
}

impl Palette {
    fn from_colors(
        colors: &alacritty_terminal::term::color::Colors,
        cfg16: &Option<[config::Rgb; 16]>,
    ) -> Self {
        let mut v = Vec::with_capacity(269);
        for i in 0..269 {
            v.push(match &colors[i] {
                Some(rgb) => Some([rgb.r as f32 / 255.0, rgb.g as f32 / 255.0, rgb.b as f32 / 255.0]),
                None => None,
            });
        }
        let cfg16 = cfg16.as_ref().map(|p| p.iter().map(|c| c.to_f32()).collect::<Vec<_>>().try_into().unwrap());
        Self { colors: v.into(), cfg16 }
    }

    // Resolve a cell color; fg/bg supplied from config for the default
    // foreground/background/cursor named colors.
    fn resolve_with(&self, c: &alacritty_terminal::vte::ansi::Color, fg: [f32; 3], bg: [f32; 3], cursor: [f32; 3]) -> [f32; 3] {
        use alacritty_terminal::vte::ansi::{Color, NamedColor};
        match c {
            Color::Spec(rgb) => [rgb.r as f32 / 255.0, rgb.g as f32 / 255.0, rgb.b as f32 / 255.0],
            Color::Named(n) => {
                let i = *n as usize;
                if i < 269 {
                    if let Some(rgb) = &self.colors[i] {
                        return *rgb;
                    }
                }
                if i < 16 {
                    match &self.cfg16 {
                        Some(p) => p[i],
                        None => PALETTE[i],
                    }
                } else if i == NamedColor::Foreground as usize {
                    fg
                } else if i == NamedColor::Background as usize {
                    bg
                } else if i == NamedColor::Cursor as usize {
                    cursor
                } else if *n == NamedColor::BrightForeground {
                    fg
                } else if *n == NamedColor::DimForeground {
                    fg.map(|v| v * 0.66)
                } else if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&i) {
                    let slot = i - NamedColor::DimBlack as usize;
                    self.cfg16.as_ref().unwrap_or(&PALETTE)[slot].map(|v| v * 0.66)
                } else {
                    fg
                }
            },
            Color::Indexed(i) => {
                let j = *i as usize;
                if j < 269 {
                    if let Some(rgb) = &self.colors[j] {
                        return *rgb;
                    }
                }
                if *i < 16 {
                    match &self.cfg16 {
                        Some(p) => p[*i as usize],
                        None => PALETTE[*i as usize],
                    }
                } else if (16..232).contains(i) {
                    cube_color(*i)
                } else {
                    let v = (8.0 + 10.0 * (*i as u32 - 232) as f32) / 255.0;
                    [v, v, v]
                }
            },
        }
    }
}


// ---------------------------------------------------------------------------
// Draw list (owned data copied out of the terminal under the lock)
// ---------------------------------------------------------------------------

/// Diagnostic oracle for checking partial rendering against complete redraws.
fn full_redraw_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MYPIXELS_FULL_REDRAW").is_some()
        || std::env::var_os("MYPIXELS_NOCACHE").is_some())
}

/// Append a run, dropping the ones that could never draw anything: padding in
/// the default colours. A mostly-empty 4K grid is 80k cells, and this is the
/// difference between thousands of dead runs and the handful that matter. Runs
/// carrying a background colour are kept -- that is how a TUI's highlighted row
/// still shows.
fn push_run(runs: &mut Vec<Run>, mut r: Run, _def_fg: [f32; 3], def_bg: [f32; 3]) {
    // Keep the full width for background painting, but never shape invisible
    // end-of-line padding. A 4K find listing otherwise shapes ~75k glyphs per
    // refresh, most of them spaces after the path.
    r.text.truncate(r.text.trim_end_matches(' ').len());
    r.cells.truncate(r.cells.partition_point(|(byte, _)| *byte < r.text.len()));
    if r.bg == def_bg && r.text.is_empty() {
        return;
    }
    runs.push(r);
}

struct Run {
    row: u32,
    col: u32,
    width: u32,
    fg: [f32; 3],
    bg: [f32; 3],
    text: String,
    /// UTF-8 cluster origins mapped to terminal columns within this run.
    cells: Vec<(usize, u32)>,
    /// Font for this run. Runs split where the required face changes, so a
    /// starship prompt becomes "text | icon | text" instead of "text | tofu".
    attrs: &'static Attrs<'static>,
}

#[derive(Default)]
struct FrameData {
    fg: [f32; 3],
    bg: [f32; 3],
    cursor_color: [f32; 3],
    runs: Vec<Run>,
    cursor: Option<Cursor>,
}

/// Exact rendering inputs outside Alacritty's cell damage tracking.
#[derive(Clone, PartialEq)]
pub(crate) struct TerminalStyle {
    tab_id: usize,
    size: (u32, u32),
    cell: (f32, f32),
    strip_h: f32,
    fg: [f32; 3],
    bg: [f32; 3],
    cursor_color: [f32; 3],
    transparency: f32,
}

/// The Alt-held visual tab switcher. Standalone overview is intentionally absent.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OverviewMode {
    /// Alt-held popup: follows the active tab, dismissed on Alt release.
    Switcher,
}

/// The highlighted tile mirrors the active tab; releasing Alt commits it.
#[derive(Clone, Copy)]
struct OverviewState {
    mode: OverviewMode,
    selected: usize,
    /// Escape restores the tab that was active before this gesture.
    original: usize,
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

/// A closed tab kept alive in the graveyard: the terminal, its pty and
/// its child all keep running; only the window stopped looking at it.
struct GraveEntry {
    tab: Tab,
    closed_at: Instant,
}

/// A closed window keeps its live terminals and workspace selections, while
/// its native window, surface and GPU resources are released.
struct ClosedWindow {
    id: usize,
    workspaces: Vec<Workspace>,
    workspace: usize,
    size: winit::dpi::PhysicalSize<u32>,
    maximized: bool,
}

impl ClosedWindow {
    fn tabs(&self) -> impl Iterator<Item = &Tab> {
        self.workspaces.iter().flat_map(|ws| ws.tabs.iter())
    }
}

/// A persistent GPU thumbnail shared by switcher cards and tab hover previews.
/// Identity follows the tab across reordering; closed tabs release their texture.
#[derive(Clone)]
struct SwThumb {
    tab_id: usize,
    handle: egui::TextureId,
    texture: wgpu::Texture,
    size: (u32, u32),
    pixels: (u32, u32),
    revision: u64,
}

/// Workspace shortcuts live in the config (`goto_workspace_0..9`); the count
/// lives with them so the UI cannot drift from the bindings.
const WORKSPACES: usize = crate::config::WORKSPACES;

#[derive(Default)]
struct Workspace {
    tabs: Vec<Tab>,
    active: usize,
}

/// Everything that lives "in a window": the surface and its GPU, this
/// window's egui state, the tab strip as this window displays it, and the
/// per-window input/selection/switcher state. Windows in one process share
/// the config, the fonts, the clipboard and the session bus -- and moving a
/// tab between windows is a Vec::remove plus a Vec::push, which is exactly
/// why the windows live in one process at all: a terminal is its pty, and a
/// pty cannot cross a process boundary.
struct Win {
    /// Stable D-Bus identity inherited by this window's shells.
    session_id: u32,
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    /// egui side: config panel, switcher overlay, rename box -- one context
    /// per window on purpose: shared egui contexts leak hover state between
    /// windows, and two independent contexts cost only a font atlas.
    egui: Option<egui_ui::Egui>,
    tabs: Vec<Tab>,
    active: usize,
    workspace: usize,
    /// Inactive workspaces own live terminals; the current slot is empty.
    workspaces: Vec<Workspace>,
    dims: Dims,
    /// This window's grid. The App-level rows/cols are the template a new
    /// window is sized from; these are what is actually on screen.
    cols: usize,
    rows: usize,
    tab_hits: Vec<(f32, f32)>,
    add_hit: (f32, f32),
    tab_close_hits: Vec<egui::Rect>,
    tab_close_press: Option<usize>,
    pointer: (f32, f32),
    mouse_button: Option<MouseButton>,
    mouse_last: Option<(u16, u16)>,
    drag: Option<(Point, Side)>,
    click: Option<(u8, u32, (f32, f32), std::time::Instant)>,
    wheel_pixels: f32,
    hover_tab: usize,
    hover_add: bool,
    tablist_hidden: bool,
    /// The egui rename box is open: keys belong to it, not the terminal.
    renaming: bool,
    /// Alt-held visual switcher (only ever the focused window's; the Alt
    /// key itself is a global modifier).
    overview: Option<OverviewState>,
    sw_thumbs: Vec<SwThumb>,
    /// Buried-tab previews for the graveyard panel: same persistent GPU
    /// thumbnails as the switcher, keyed by the stable tab id, refreshed
    /// only while the panel is open.
    grave_thumbs: Vec<SwThumb>,
    thumb_rr: usize,
    ws_preview_rr: usize,
    ws_preview_next: Instant,
    preview_next: Instant,
    repaint_at: Option<Instant>,
    /// last BEL seen anywhere (visual bell; see bell_flash)
    autoscroll_next: Option<Instant>,
    /// One-shot: the next frame renders the active tab through the preview
    /// path and copies the result to the clipboard as a PNG.
    shot: bool,
    tab_drag: Option<usize>,
    /// The tab drag crossed into another cell, so the press has become a
    /// reorder and the release must not also count as a click.
    tab_drag_moved: bool,
    /// egui events synthesized from FIFO key specs, queued for the next
    /// RawInput.
    fifo_egui_events: Vec<egui::Event>,
    /// Per-window error/status bubble (see show_error).
    notice: Option<(String, Instant)>,
    /// Winit's focus report. The switcher and the attention request consult
    /// it, so a background window's blink never steals a foreground gesture.
    /// Tab cell ringing a red pulse after a BEL arrived unseen (drain_bell_pings).
    bell_pulse: Option<(usize, Instant)>,
    focused: bool,
    occluded: bool,
    /// UI generation, bumped on input and UI changes. Terminal contents and
    /// cursor blink use damage; compositor redraw requests do not bump this.
    frame_gen: u64,
}

impl Win {
    fn all_tabs(&self) -> impl Iterator<Item = &Tab> {
        self.tabs.iter().chain(self.workspaces.iter().flat_map(|w| w.tabs.iter()))
    }

    fn other_workspace(&self) -> Option<usize> {
        self.workspaces.iter().position(|w| !w.tabs.is_empty())
    }

    /// Move the active tab into a workspace's tab list; the terminal stays
    /// alive there (pty and scrollback intact, like a window move).
    fn move_tab_to_workspace(&mut self, target: usize) {
        if target >= self.workspaces.len() || target == self.workspace || self.tabs.is_empty() {
            return;
        }
        let from = self.active;
        let tab = self.tabs.remove(from);
        if !self.tabs.is_empty() {
            self.active = from.min(self.tabs.len() - 1);
        }
        self.workspaces[target].tabs.push(tab);
        self.workspaces[target].active = self.workspaces[target].tabs.len() - 1;
        self.frame_gen = self.frame_gen.wrapping_add(1);
    }

    fn select_workspace(&mut self, target: usize) {
        let previous = Workspace {
            tabs: std::mem::take(&mut self.tabs),
            active: self.active,
        };
        self.workspaces[self.workspace] = previous;
        let next = std::mem::take(&mut self.workspaces[target]);
        self.tabs = next.tabs;
        self.active = next.active;
        self.workspace = target;
    }
}

struct App {
    persistent: bool,
    wake: EventLoopProxy<()>,
    config: Config,
    /// The windows of this session, in creation order. `cur` is the one the
    /// event being handled belongs to (set by the router at the top of
    /// `window_event`); "current window" is what every helper below means.
    wins: Vec<Win>,
    cur: usize,
    next_id: usize,
    next_window_id: u32,
    cell_w: f32,
    cell_h: f32,
    tab_char_w: f32,
    tab_h: f32,
    tab_text_h: f32,
    /// Window scale factor, applied to every metric above. The whole render
    /// pipeline is in *physical* pixels (surface size, pointer and wheel
    /// events all are), so point sizes are converted to pixels with this
    /// factor: on a 2x display a 10pt font rasterises at double the physical
    /// size and a cell is double, while the grid and hit-testing work in the
    /// same units as before. `App::new` computes the metrics at 1.0 because
    /// no window exists yet; `resumed` applies the real factor before the
    /// window is created and `ScaleFactorChanged` keeps it current.
    scale: f32,
    /// Vertical metrics of the term/tablist fonts, read from the fonts
    /// themselves -- line height differs a lot per family (Cascadia 9pt lands on
    /// 15px, Terminus 12 on 17px) and one guessed factor fit neither.
    term_metrics: fonts::LineMetrics,
    tab_metrics: fonts::LineMetrics,
    /// New-window grid template (and the very first window's size): App keeps
    /// a cols/rows pair even though every window owns its real grid.
    cols: usize,
    rows: usize,
    clipboard: Option<clipboard::Clipboard>,
    theme_watcher: Option<theme::Watcher>,
    fg: [f32; 3],
    bg: [f32; 3],
    exited: Arc<Mutex<Vec<usize>>>,
    /// Every live window, for the wake closures that outlive any single one
    /// (pty listeners, clock, clipboard, theme watcher): a redraw request
    /// goes to all of them; drawing the wrong one is cheap and correct.
    window_slot: Arc<Mutex<Vec<Arc<Window>>>>,
    font_system: FontSystem,
    mono_attrs: Attrs<'static>,
    tab_attrs: Attrs<'static>,
    tab_face_key: String,
    /// Per-codepoint font routing, rebuilt whenever the terminal font changes.
    coverage: RefCell<Option<fonts::Coverage>>,
    /// Runtime terminal font state. `term_pt` is the live size (enlarge/
    /// shrink font mutates it); a config reload resets it to the config value.
    /// The family here is the RESOLVED one: a config spec of "Foo Bold 12"
    /// stores family "Foo" plus its style, so rebuilds do not re-parse.
    term_family: &'static str,
    term_weight: u16,
    term_slant: fonts::DbStyle,
    term_pt: f32,
    /// The size the terminal font was loaded at: what "Reset zoom" returns
    /// to, immune to whatever the config's string happens to say after a
    /// zoom-and-save cycle this session.
    orig_term_pt: f32,
    /// Alt-held tab switcher timing, shared with a background clock thread.
    /// `switcher_alt_at` is when Alt went down (None when released); the clock
    /// opens the switcher after the configured delay (`tab_overview_delay_ms`)
    /// and, while it's open, nudges a redraw on a fixed cadence so every
    /// thumbnail stays live. `switcher_show_req` is the clock's "show now"
    /// signal; `switcher_open` mirrors the open state for the clock.
    switcher_alt_at: Arc<std::sync::Mutex<Option<std::time::Instant>>>,
    switcher_show_req: Arc<AtomicBool>,
    switcher_open: Arc<AtomicBool>,
    /// Cursor-blink phase (toggled by the clock thread); render draws the
    /// cursor only when this is set.
    cursor_blink: Arc<AtomicBool>,
    /// Last terminal output time (shared with the pty listeners); the clock
    /// thread reads it to keep the cursor solid right after activity.
    last_activity: Arc<std::sync::Mutex<std::time::Instant>>,
    /// last BEL seen anywhere (visual bell; see bell_flash)
    bell: Arc<std::sync::Mutex<Option<std::time::Instant>>>,
    /// every BEL as (tab_id, time), drained into attention + pulses
    bell_pings: Arc<std::sync::Mutex<Vec<(usize, std::time::Instant)>>>,
    notification_requests: Arc<Mutex<Vec<notifications::Raw>>>,
    notification_parser: notifications::Parser,
    desktop_notifications: notifications::Desktop,
    /// OSC 133 marks as dispatched, drained into per-tab command blocks
    sems_ring: Arc<std::sync::Mutex<Vec<SemanticMark>>>,
    /// Opens an order stamp per command (see `CommandBlock::seq`).
    sem_seq: u64,
    /// Complete APC strings awaiting graphics interpretation.
    apcs_ring: Arc<std::sync::Mutex<Vec<(usize, Vec<u8>, usize, usize)>>>,
    pty_writes_ring: Arc<std::sync::Mutex<Vec<(usize, String)>>>,
    resets_ring: Arc<std::sync::Mutex<Vec<usize>>>,
    cell_sizes_ring: Arc<std::sync::Mutex<Vec<usize>>>,
    /// MYPIXELS_APC_DUMP=<file>: append every graphics frame verbatim for
    /// offline replay (tests a real sender's exact bytes against the
    /// parser). Debug hook, off by default.
    apc_dump: Option<std::path::PathBuf>,
    /// Per-tab Kitty graphics state, keyed by stable tab id; created on
    /// first use, removed when the tab dies.
    kitty: std::collections::HashMap<usize, kitty::TabKitty>,
    /// Per-tab command blocks, oldest first. Owned by the app, keyed by
    /// the stable tab id, so a tab that moves between windows keeps its
    /// history and a tab that dies simply leaves no trace.
    sem_blocks: std::collections::HashMap<usize, std::collections::VecDeque<CommandBlock>>,
    /// Daemon: tab/rise requests forwarded by other mypixels processes.
    session_rx: std::sync::mpsc::Receiver<session::Request>,
    mcp_rx: std::sync::mpsc::Receiver<mcp::Query>,
    /// The egui window picker's answer, applied next frame by the drain
    /// (which has the event loop; the draw path does not). usize::MAX =
    /// detach into a fresh window.
    /// One small expanding ring at the cursor's cell, re-armed whenever the
    /// cursor actually moves cells. That is the whole of it: a 180 ms pulse
    /// so the eye can keep the pointer's agent when it slides across the
    /// grid (mouse-reporting apps, search jumps, wide terminals).
    ripple: Option<Ripple>,
    /// Buried tabs, oldest first: closing a tab does not kill the shell.
    /// The tab keeps running here - pty, child, scrollback all live - until
    /// it is raised from the graveyard panel, the limit evicts it, its
    /// child exits on its own, or the session ends. Bounded by `graveyard_limit`.
    graveyard: Vec<GraveEntry>,
    closed_windows: Vec<ClosedWindow>,
    /// URLs on the row under the pointer (only that row can underline or
    /// be clicked); reparsed only when the row's content hash moves.
    url_spans: Vec<UrlSpan>,
    /// (row, content hash, spans) of the last row scanned, for the reuse
    /// fast path in `scan_row_cached`.
    url_cache: Option<(usize, u64, Vec<UrlSpan>)>,
    /// Index into url_spans under the pointer, for the underline.
    url_hover: Option<usize>,
    /// Where the cursor block was painted last frame, physical px, with its
    /// resolved colour. Key events need it without going through the frame
    /// builder: the ripple rings AT the cursor when a modifier is pressed.
    cursor_pt: Option<([f32; 2], [f32; 3])>,
    pending_move: Option<(WindowId, usize)>,
    /// A context-menu choice, run by the drain (which has the event loop).
    pending_action: Option<(WindowId, String)>,
    pending_close: Option<(WindowId, egui_ui::CloseChoice)>,
    /// A graveyard choice (raise/kill), deferred to the drain: applying it
    /// inside the draw path would shrink the graveyard mid-frame, and the
    /// preview index space built at the frame's top would go stale under it.
    pending_grave: Option<(WindowId, usize, String)>,
    /// Keeps the session-bus thread -- and with it the bus name -- alive for
    /// as long as the app runs.
    /// Keeps the session-bus thread -- and with it the bus name -- alive for
    /// as long as the app runs.
    _session: Option<session::Server>,
    modifiers: ModifiersState,
    program: String,
    args: Vec<String>,
    /// Test hook: receiver of key specs fed from the $MYPIXELS_KEYS FIFO by a
    /// reader thread (about_to_wait is rarely called on Wayland).
    key_rx: Option<std::sync::mpsc::Receiver<String>>,
}

impl App {
    fn new(wake: EventLoopProxy<()>, persistent: bool) -> Self {
        let config = Config::load();
        // Pango sizes are points; cosmic-text wants pixels (@96 dpi). The
        // parsed family may still carry style words ("Foo Bold 12");
        // resolve_font() splits those against the real database below.
        let (term_spec, term_pt) = config::parse_font(&config.term_font);
        let (tab_spec, tab_pt) = config::parse_font(&config.tablist_font);
        // Fallback chain must know the primary families up front, so powerline /
        // Nerd Font glyphs in prompts resolve to icons instead of tofu.
        let (mut font_system, fallback_chain) =
            fonts::build_font_system(&[term_spec.as_str(), tab_spec.as_str()]);
        log::info!("font fallback chain: {:?}", fallback_chain);
        let font_px = term_pt * 96.0 / 72.0;
        let tab_font_px = tab_pt * 96.0 / 72.0;
        // Font size comes from the Buffer Metrics (see Gpu::init), not Attrs.
        // mono_attrs/tab_attrs are 'static, so the family names must be too.
        let term = fonts::resolve_font(font_system.db(), &term_spec);
        let tab = fonts::resolve_font(font_system.db(), &tab_spec);
        let term_family: &'static str = Box::leak(term.family.into_boxed_str());
        let tab_family: &'static str = Box::leak(tab.family.into_boxed_str());
        let tab_face_key = egui_ui::preview_family_name(tab_family, tab.weight, tab.slant);
        let (term_weight, term_slant) = (term.weight, term.slant);
        let (mono_attrs, tab_attrs) = {
            let db = font_system.db();
            (
                fonts::pin_face(
                    db,
                    Attrs::new(),
                    term_family,
                    term_weight,
                    term_slant,
                ),
                fonts::pin_face(
                    db,
                    Attrs::new(),
                    tab_family,
                    tab.weight,
                    tab.slant,
                ),
            )
        };

        // Line height comes from the font itself, not a guessed factor.
        let term_metrics = fonts::line_metrics(&mut font_system, term_family, term_weight, term_slant)
            .unwrap_or(fonts::LineMetrics::DEFAULT);
        let tab_metrics = fonts::line_metrics(&mut font_system, tab_family, tab.weight, tab.slant)
            .unwrap_or(fonts::LineMetrics::DEFAULT);
        let term_lh = config.terminal_line_height(term_metrics.height_at(font_px));
        let tab_lh = tab_metrics.height_at(tab_font_px);

        let mut measure = Buffer::new(&mut font_system, Metrics::new(font_px, term_lh));
        measure.set_size(Some(800.0), None);
        measure.set_text("MMMM", &mono_attrs, Shaping::Basic, None);
        measure.shape_until_scroll(&mut font_system, false);
        let shaped = measure.lines[0].shape(&mut font_system, 8);
        let mut total = 0.0;
        for span in &shaped.spans {
            for word in &span.words {
                total += word.width(font_px);
            }
        }
        // Whole-pixel cell grid. A fractional advance puts every glyph on a
        // sub-pixel origin, so each stem is antialiased twice: text reads
        // heavier than the original, and solid shapes (powerline separators,
        // block glyphs) pick up a fringe along their edges. VTE snaps cells to
        // integers; matching that is also what keeps columns within a pixel or
        // two of the reference at the same point size.
        let cell_w = (total / 4.0).round().max(1.0);
        let cell_h = term_lh.round().max(1.0);

        let mut tmeasure =
            Buffer::new(&mut font_system, Metrics::new(tab_font_px, tab_lh));
        tmeasure.set_size(Some(800.0), None);
        tmeasure.set_text("MMMM", &tab_attrs, Shaping::Basic, None);
        tmeasure.shape_until_scroll(&mut font_system, false);
        let tshaped = tmeasure.lines[0].shape(&mut font_system, 8);
        let mut ttotal = 0.0;
        for span in &tshaped.spans {
            for word in &span.words {
                ttotal += word.width(tab_font_px);
            }
        }
        let tab_char_w = (ttotal / 4.0).max(1.0);
        let tab_h = ui_style::tab_height(tab_lh, 1.0, config.compact_tabs);

        log::info!(
            "font: term {term_family} {term_pt}pt, tablist {tab_family} {tab_pt}pt; cell = {cell_w:.2} x {cell_h:.2}, tab char = {tab_char_w:.2}, strip = {tab_h:.1}px"
        );

        let fg = config.foreground_color.to_f32();
        let bg = crate::config::terminal_bg(&config).to_f32();

        let (program, args) = term_command();

        let window_slot = Arc::new(Mutex::new(Vec::new()));
        let key_rx = spawn_key_reader(Arc::clone(&window_slot));
        // Claim this display's session unless asked for a private instance.
        // Remote launchers enqueue requests and wake the event loop, including
        // before the daemon has created its first window.
        let standalone = std::env::args()
            .skip(1)
            .take_while(|a| a.starts_with('-'))
            .any(|a| a == "--standalone" || a == "-s");
        let (session_tx, session_rx) = std::sync::mpsc::channel();
        let (mcp_tx, mcp_rx) = std::sync::mpsc::channel();
        let _session = if standalone {
            None
        } else {
            session::bus_name()
                .and_then(|name| {
                    let wake = wake.clone();
                    session::start(&name, session_tx, move || {
                        let _ = wake.send_event(());
                    })
                })
        };
        if persistent && _session.is_none() {
            eprintln!("mypixels --daemon: could not own the display session (already running or no session bus)");
            std::process::exit(1);
        }
        // LLM observability: read-only MCP endpoint on a per-display unix
        // socket (windows, tabs, captured command output). Same
        // standalone carve-out as the session bus.
        let _mcp = if standalone {
            None
        } else {
            let wake = wake.clone();
            mcp::start(mcp_tx, Arc::new(move || {
                let _ = wake.send_event(());
            }))
        };
        let shared = SpikeListener {
            wake: wake.clone(),
            window: Arc::clone(&window_slot),
            tab_id: 0,
            title: Arc::new(Mutex::new(String::new())),
            exited: Arc::new(Mutex::new(Vec::new())),
            last_activity: Arc::new(Mutex::new(std::time::Instant::now())),
            bell: Arc::new(Mutex::new(None)),
            bells: Arc::new(Mutex::new(Vec::new())),
            notifications: Arc::new(Mutex::new(Vec::new())),
            sems: Arc::new(Mutex::new(Vec::new())),
            apcs: Arc::new(Mutex::new(Vec::new())),
            pty_writes: Arc::new(Mutex::new(Vec::new())),
            resets: Arc::new(Mutex::new(Vec::new())),
            cell_sizes: Arc::new(Mutex::new(Vec::new())),
        };
        let notification_requests = Arc::clone(&shared.notifications);
        let notification_windows = Arc::clone(&window_slot);
        let desktop_notifications = notifications::Desktop::new(Arc::new(move || {
            for window in notification_windows.lock().unwrap().iter() { window.request_redraw(); }
        }));
        let bells = Arc::clone(&shared.bells);
        let sems = Arc::clone(&shared.sems);
        let apcs = Arc::clone(&shared.apcs);
        let pty_writes = Arc::clone(&shared.pty_writes);
        let resets = Arc::clone(&shared.resets);
        let cell_sizes = Arc::clone(&shared.cell_sizes);
        let exited = Arc::clone(&shared.exited);
        let last_activity = Arc::clone(&shared.last_activity);
        let bell = Arc::clone(&shared.bell);
        // Cursor blink phase, toggled by the clock thread (activity-aware).
        let cursor_blink = Arc::new(AtomicBool::new(true));

        // Alt-held switcher clock. Shared with the event-loop thread.
        let switcher_alt_at = Arc::new(Mutex::new(None));
        let switcher_show_req = Arc::new(AtomicBool::new(false));
        let switcher_open = Arc::new(AtomicBool::new(false));
        {
            let ws = Arc::clone(&window_slot);
            let alt_at = Arc::clone(&switcher_alt_at);
            let show_req = Arc::clone(&switcher_show_req);
            let open = Arc::clone(&switcher_open);
            let blink = Arc::clone(&cursor_blink);
            let last_act = Arc::clone(&last_activity);
            let delay_ms = config.tab_overview_delay_ms;
            let refresh_ms = 150u32; // thumbnail refresh cadence while open
            let blink_ms = 533u128; // cursor on/off cadence
            std::thread::spawn(move || {
                let mut last_refresh = std::time::Instant::now();
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(30));
                    let now = std::time::Instant::now();
                    // Cursor blink: solid right after activity, then blinks in
                    // blink_ms on/off steps measured from last activity.
                    let idle = now.duration_since(*last_act.lock().unwrap());
                    let visible = if idle.as_millis() < blink_ms {
                        true
                    } else {
                        (idle.as_millis() / blink_ms) % 2 == 0
                    };
                    if blink.load(Ordering::SeqCst) != visible {
                        blink.store(visible, Ordering::SeqCst);
                        for w in ws.lock().unwrap().iter() {
                            w.request_redraw();
                        }
                    }
                    if !open.load(Ordering::SeqCst) {
                        // Open after the configured Alt hold delay.
                        if let Some(at) = *alt_at.lock().unwrap() {
                            if now.duration_since(at).as_millis() >= delay_ms as u128
                                && !show_req.swap(true, Ordering::SeqCst)
                            {
                                for w in ws.lock().unwrap().iter() { w.request_redraw(); }
                            }
                        }
                    } else if now.duration_since(last_refresh).as_millis() >= refresh_ms as u128 {
                        // Keep every preview live while the switcher is open.
                        last_refresh = now;
                        for w in ws.lock().unwrap().iter() {
                            w.request_redraw();
                        }
                    }
                }
            });
        }

        // Initial grid from the configured window size; the Resized handler
        // adjusts it once the surface exists.
        let cols = ((config.default_width as f32 / cell_w) as usize).clamp(20, 512);
        let rows = (((config.default_height as f32 - tab_h) / cell_h) as usize).clamp(10, 400);

        App {
            persistent,
            wake,
            wins: Vec::new(),
            cur: 0,
            config,
            next_id: 0,
            next_window_id: 1,
            cols,
            rows,
            cell_w,
            cell_h,
            tab_char_w,
            tab_h,
            tab_text_h: tab_lh,
            scale: 1.0,
            term_metrics,
            tab_metrics,
            clipboard: None,
            theme_watcher: None,
            fg,
            bg,
            exited,
            window_slot,
            font_system,
            mono_attrs,
            tab_attrs,
            tab_face_key,
            coverage: RefCell::new(None),
            term_family,
            term_weight,
            term_slant,
            term_pt,
            orig_term_pt: term_pt,
            switcher_alt_at,
            switcher_show_req,
            switcher_open,
            cursor_blink,
            last_activity,
            bell,
            bell_pings: bells,
            notification_requests,
            notification_parser: notifications::Parser::default(),
            desktop_notifications,
            sems_ring: sems,
            sem_seq: 0,
            apcs_ring: apcs,
            pty_writes_ring: pty_writes,
            resets_ring: resets,
            cell_sizes_ring: cell_sizes,
            apc_dump: std::env::var("MYPIXELS_APC_DUMP").ok().map(std::path::PathBuf::from),
            kitty: Default::default(),
            sem_blocks: Default::default(),
            session_rx,
            mcp_rx,
            pending_move: None,
            pending_action: None,
            pending_close: None,
            pending_grave: None,
            ripple: None,
            cursor_pt: None,
            graveyard: Vec::new(),
            closed_windows: Vec::new(),
            url_spans: Vec::new(),
            url_cache: None,
            url_hover: None,
            _session,
            modifiers: ModifiersState::empty(),
            program,
            args,
            key_rx,
        }
    }

    fn init_gpu(&mut self, event_loop: &ActiveEventLoop, window: &Arc<Window>) {
        // Live sizes, not the config's: `term_pt` carries any zoom applied this
        // session, `self.scale` the display's factor, and cell_h the rounded
        // terminal line height, so the glyph buffers match the live grid.
        let term_px = self.term_pt * 96.0 / 72.0 * self.scale;
        let gpu = Gpu::init(
            event_loop,
            window,
            &mut self.font_system,
            term_px,
            self.cell_h,
        );
        // The font popup previews from the app's own database, so the list
        // is exactly what the terminal can actually load.
        let font_choices = fonts::font_choices(self.font_system.db());
        self.wins[self.cur].egui = Some(egui_ui::Egui::init(
            event_loop,
            &gpu.device,
            &gpu.queue,
            gpu.config.format,
            self.scale,
            font_choices,
        ));
        self.wins[self.cur].gpu = Some(gpu);
        self.apply_transparency();
    }

    fn add_tab(&mut self) {
        self.add_tab_with(None, None);
    }

    /// Open a tab, optionally overriding where it starts (`cwd`) and what it
    /// runs. The daemon's OpenTab uses both: a tab beside the shell that ran
    /// `mypixels`, in that shell's directory, doing what it asked.
    fn add_tab_with(
        &mut self,
        cwd: Option<std::path::PathBuf>,
        cmd: Option<(String, Vec<String>)>,
    ) {
        let tab = self.spawn_tab(cwd, cmd);
        // Placement is a setting: `next` (default) lands the tab right of
        // the current one, `end` after the last, `first` at the start.
        let win = &mut self.wins[self.cur];
        let pos = new_tab_index(&self.config.new_tab_position, win.tabs.len(), win.active);
        win.tabs.insert(pos, tab);
        win.active = pos;
        win.frame_gen = win.frame_gen.wrapping_add(1);
    }

    /// Create a terminal without choosing where it belongs in the tab strip.
    fn spawn_tab(
        &mut self,
        cwd: Option<std::path::PathBuf>,
        cmd: Option<(String, Vec<String>)>,
    ) -> Tab {
        let id = self.next_id;
        self.next_id += 1;
        let window_size = WindowSize {
            num_lines: self.wins[self.cur].rows as u16,
            num_cols: self.wins[self.cur].cols as u16,
            cell_width: self.cell_w as u16,
            cell_height: self.cell_h as u16,
        };
        let (program, args) = match cmd {
            Some(c) => c,
            None => (self.program.clone(), self.args.clone()),
        };
        Tab::new(
            id,
            &self.wins[self.cur].dims,
            window_size,
            program,
            args,
            &self.listener(),
            self.config.scrollback_lines,
            self.config.kitty_compatible,
            cwd,
            // The shell learns WHICH window of the session it lives in, so a
            // `mypixels` typed in it joins that window, not window 1. 1-based
            // because 0 in the environment means "standalone, stay out".
            if self._session.is_some() { self.wins[self.cur].session_id } else { 0 },
        )
    }

    /// Numbered shortcuts append missing tabs, preserving existing numbering
    /// regardless of the configured placement for manually opened tabs.
    fn goto_tab(&mut self, target: usize) {
        while self.wins[self.cur].tabs.len() <= target {
            let tab = self.spawn_tab(None, None);
            self.wins[self.cur].tabs.push(tab);
        }
        self.wins[self.cur].active = target;
    }

    fn switch_workspace(&mut self, target: usize) {
        // Gated: with workspaces off the bindings and the tab-bar button are
        // inert, and this is the one funnel every workspace switch flows through.
        if !self.config.workspaces_enabled {
            return;
        }
        if target >= WORKSPACES || target == self.wins[self.cur].workspace {
            return;
        }
        // The Alt-held switcher survives a workspace switch while Alt is still
        // down. The gesture is "hold Alt and look around", and with workspaces
        // on, another workspace is part of what is being looked around in -
        // closing it made the chords that work with the panel unusable here.
        // What cannot survive is the tile it was highlighting: `selected` and
        // `original` index the tab list of the workspace being left.
        let in_gesture = self.modifiers.alt_key() && self.switcher_open_here();
        if !in_gesture {
            self.close_switcher();
            *self.switcher_alt_at.lock().unwrap() = None;
            self.switcher_show_req.store(false, Ordering::SeqCst);
        }
        let win = &mut self.wins[self.cur];
        if let Some(tab) = win.tabs.get(win.active) {
            tab.term.lock().is_focused = false;
        }
        win.select_workspace(target);
        if let Some(e) = win.egui.as_mut()
            && e.ui_state.panel_is(egui_ui::RightPanel::Workspaces) {
            e.ui_state.ws_sel = Some(target);
            e.ui_state.ws_collapsed = None;
        }
        if in_gesture {
            // Land on this workspace's own front tab. Escape from here restores
            // it, rather than a tab index that belonged to the workspace the
            // user just left.
            if let Some(ov) = win.overview.as_mut() {
                ov.selected = win.active;
                ov.original = win.active;
            }
        }
        win.hover_tab = usize::MAX;
        win.hover_add = false;
        win.tab_drag = None;
        win.tab_close_press = None;
        win.tab_hits.clear();
        win.tab_close_hits.clear();
        win.bell_pulse = None;
        win.wheel_pixels = 0.0;
        win.click = None;
        win.mouse_button = None;
        win.mouse_last = None;
        if let Some(gpu) = win.gpu.as_mut() {
            gpu.invalidate_terminals();
        }
        if self.wins[self.cur].tabs.is_empty() {
            self.add_tab();
        }
        self.sync_term_size();
        if let Some(window) = &self.wins[self.cur].window {
            window.request_redraw();
        }
    }

    /// Handle what other mypixels processes asked of this session.
    /// Remove a window from the session: the wake-slot list first (dead
    /// windows must not be woken, nor kept alive by it), then the window
    /// itself; the last one exiting ends the session.
    /// Drop a dead tab's textures from EVERY window's cache: a tab that was
    /// once previewed or shown in another window left copies there, and the
    /// key (a never-reused id) would never be overwritten.
    fn forget_gpu_images(&mut self, tab_id: usize) {
        self.notification_parser.forget(tab_id);
        self.desktop_notifications.send(notifications::Command::Forget(tab_id));
        for w in self.wins.iter_mut() {
            if let Some(g) = w.gpu.as_mut() {
                g.forget_images(tab_id as u32);
            }
        }
    }

    /// True while a keybinding row waits for a chord. The recorder owns the
    /// keyboard outright, so keys go nowhere else - not the shell, not an
    /// action, and not egui (a focused row would eat Space or Enter).
    fn recording_binding(&self) -> bool {
        self.wins[self.cur].egui.as_ref().is_some_and(|e| e.ui_state.key_capture.is_some())
    }

    /// Feed one key to the waiting keybinding row. Always consumes the key
    /// while a row is armed, and reports false when none is, so the caller
    /// carries on with normal routing. Modifier presses only build the chord.
    fn capture_binding(&mut self, key: &Key, kev: Option<&winit::event::KeyEvent>) -> bool {
        use winit::keyboard::NamedKey as Nk;
        let Some(action) = self.wins[self.cur].egui.as_ref()
            .and_then(|e| e.ui_state.key_capture.clone()) else { return false };
        let stop = |app: &mut App| {
            if let Some(e) = app.wins[app.cur].egui.as_mut() { e.ui_state.key_capture = None; }
        };
        if matches!(key, Key::Named(Nk::Escape)) {
            stop(self);
            return true;
        }
        if matches!(key, Key::Named(Nk::Shift | Nk::Control | Nk::Alt | Nk::Super | Nk::Hyper | Nk::CapsLock)) {
            return true;
        }
        // The config spells three modifiers; Super cannot be written down.
        let mods = self.modifiers
            & (ModifiersState::ALT | ModifiersState::SHIFT | ModifiersState::CONTROL);
        let function_key = matches!(key, Key::Named(Nk::F1 | Nk::F2 | Nk::F3 | Nk::F4 | Nk::F5
            | Nk::F6 | Nk::F7 | Nk::F8 | Nk::F9 | Nk::F10 | Nk::F11 | Nk::F12));
        if mods.is_empty() && !function_key {
            self.show_error(format!("{} needs a modifier, or typing that key would stop reaching the shell.",
                egui_ui::action_label(&action)));
            return true;
        }
        // Store the key by position. The report carries Shift's symbol, which
        // is "@" on a US board and "č" on a Czech one, while the file and the
        // row should both read "2" for Alt+Shift+2.
        let mut key = key.clone();
        if mods.shift_key() {
            if let Some(k) = kev {
                key = config::unshifted_key(&key, k.physical_key);
            }
            if let Key::Character(c) = &key {
                if let Some(digit) = config::unshift_symbol(c.as_str()) {
                    key = Key::Character(digit.into());
                }
            }
        }
        let bound = match &key {
            Key::Named(n) => config::BoundKey::Named(n.clone()),
            Key::Character(c) => config::BoundKey::Char(c.to_string()),
            _ => {
                self.show_error("That key cannot be bound.".into());
                stop(self);
                return true;
            }
        };
        let binding = config::Binding { mods, key: bound };
        // A shared chord resolves to the earlier action in list order, so the
        // new one would never fire. Keep recording: a different chord is fine.
        if let Some(other) = config::binding_conflict(&self.config.bindings, &action, &binding) {
            self.show_error(format!("{} is already used by {}.", binding.label(), egui_ui::action_label(&other)));
            return true;
        }
        log::info!("bind {action} = {}", binding.spec());
        self.config.set_binding(&action, vec![binding]);
        if let Some(e) = self.wins[self.cur].egui.as_mut() {
            e.ui_state.key_capture = None;
            e.ui_state.bindings_dirty = true;
        }
        if let Some(window) = self.wins[self.cur].window.as_ref() { window.request_redraw(); }
        true
    }

    /// Which action this key means, if any. Tries the key as reported, then
    /// the same key by physical position: Shift turns a digit into a layout
    /// symbol, so a shortcut written Alt+Shift+2 could otherwise only ever
    /// work on one keyboard. Modifiers are still compared exactly, so an
    /// unshifted binding can never fire from a shifted press.
    fn action_for(&self, key: &Key, kev: Option<&winit::event::KeyEvent>, overview_active: bool) -> Option<String> {
        if let Some(action) = config::find_action(&self.config.bindings, key, self.modifiers, overview_active) {
            return Some(action);
        }
        let unshifted = config::unshifted_key(key, kev?.physical_key);
        if unshifted == *key {
            return None;
        }
        config::find_action(&self.config.bindings, &unshifted, self.modifiers, overview_active)
    }

    fn close_window(&mut self, i: usize, event_loop: &ActiveEventLoop) {
        let win = &mut self.wins[i];
        let workspace = win.workspace;
        let mut workspaces = std::mem::take(&mut win.workspaces);
        workspaces[workspace] = Workspace {
            tabs: std::mem::take(&mut win.tabs), active: win.active,
        };
        if workspaces.iter().any(|ws| !ws.tabs.is_empty()) {
            for tab in workspaces.iter().flat_map(|ws| &ws.tabs) {
                tab.term.lock().is_focused = false;
            }
            self.closed_windows.push(ClosedWindow {
                id: win.session_id as usize, workspaces, workspace,
                size: win.window.as_ref().unwrap().inner_size(),
                maximized: win.window.as_ref().unwrap().is_maximized(),
            });
            while self.closed_windows.len() > self.config.graveyard_limit {
                self.kill_closed_window(0);
            }
        }
        if let Some(w) = self.wins[i].window.as_ref() {
            let id = w.id();
            self.window_slot.lock().unwrap().retain(|w| w.id() != id);
        }
        self.wins.remove(i);
        for win in &self.wins {
            if let Some(window) = &win.window { window.request_redraw(); }
        }
        self.cur = self.cur.min(self.wins.len().saturating_sub(1));
        self.modifiers = ModifiersState::empty();
        *self.switcher_alt_at.lock().unwrap() = None;
        self.switcher_open.store(false, Ordering::SeqCst);
        self.switcher_show_req.store(false, Ordering::SeqCst);
        if self.wins.is_empty() && !self.persistent {
            event_loop.exit();
        }
    }

    fn window_retainer(&self) -> egui_ui::WindowRetainer {
        if self.persistent { egui_ui::WindowRetainer::Daemon }
        else if self.wins.len() > 1 { egui_ui::WindowRetainer::OtherWindow }
        else { egui_ui::WindowRetainer::None }
    }

    fn request_window_close(&mut self, event_loop: &ActiveEventLoop) {
        let retainer = self.window_retainer();
        if !self.config.close_window_dialog.should_show(retainer != egui_ui::WindowRetainer::None) {
            self.close_window(self.cur, event_loop);
            return;
        }
        self.open_close_dialog();
    }

    fn open_close_dialog(&mut self) {
        let retainer = self.window_retainer();
        self.close_switcher();
        let win = &mut self.wins[self.cur];
        let tabs = win.all_tabs().count();
        let workspaces = usize::from(!win.tabs.is_empty())
            + win.workspaces.iter().filter(|ws| !ws.tabs.is_empty()).count();
        win.renaming = false;
        win.tab_drag = None;
        win.tab_close_press = None;
        win.mouse_button = None;
        if let Some(egui) = win.egui.as_mut() {
            let st = &mut egui.ui_state;
            // Repeated WM requests neither confirm nor restart the countdown.
            if !st.confirm_open {
                st.rename_open = false;
                st.menu_open = false;
                st.move_open = false;
                st.info_page = None;
                st.font_popup = None;
                st.key_capture = None;
                st.confirm_open = true;
                st.confirm_seed = true;
                st.confirm_out = None;
                st.confirm = egui_ui::CloseDialog::new(retainer, tabs, workspaces, self.config.graveyard_limit);
                egui_ui::release_focus(&egui.ctx);
            }
        }
        if let Some(window) = &win.window { window.request_redraw(); }
    }

    fn destroy_window(&mut self, event_loop: &ActiveEventLoop) {
        let ids: Vec<_> = self.wins[self.cur].all_tabs().map(|tab| tab.id).collect();
        for id in ids { self.forget_tab(id); }
        self.wins[self.cur].tabs.clear();
        for ws in &mut self.wins[self.cur].workspaces { ws.tabs.clear(); }
        self.close_window(self.cur, event_loop);
    }

    fn kill_closed_window(&mut self, index: usize) {
        let window = self.closed_windows.remove(index);
        for tab in window.tabs() { self.forget_tab(tab.id); }
    }

    fn revive_window(&mut self, index: usize, event_loop: &ActiveEventLoop) {
        if index >= self.closed_windows.len() { return; }
        let mut closed = self.closed_windows.remove(index);
        self.make_window(event_loop, None, None, false);
        let win = &mut self.wins[self.cur];
        win.session_id = closed.id as u32;
        win.workspace = closed.workspace;
        let active = std::mem::take(&mut closed.workspaces[closed.workspace]);
        win.tabs = active.tabs;
        win.active = active.active;
        win.workspaces = closed.workspaces;
        // The selected workspace may have emptied while the window was buried.
        if win.tabs.is_empty() {
            if let Some(target) = win.other_workspace() { win.select_workspace(target); }
        }
        if let Some(window) = &win.window {
            window.set_maximized(closed.maximized);
            if !closed.maximized { let _ = window.request_inner_size(closed.size); }
            window.request_redraw();
        }
        self.sync_term_size();
    }

    fn all_tabs(&self) -> impl Iterator<Item = &Tab> {
        self.wins.iter().flat_map(|w| w.all_tabs())
            .chain(self.graveyard.iter().map(|entry| &entry.tab))
            .chain(self.closed_windows.iter().flat_map(|w| w.tabs()))
    }

    /// Reap children even when no native window exists to request a redraw.
    fn drain_exited(&mut self, event_loop: &ActiveEventLoop) {
        let exited = std::mem::take(&mut *self.exited.lock().unwrap());
        let original = self.wins.get(self.cur).and_then(|w| w.window.as_ref()).map(|w| w.id());
        for id in exited {
            if let Some(pos) = self.graveyard.iter().position(|g| g.tab.id == id) {
                self.kill_grave(pos);
                continue;
            }
            for window in &mut self.closed_windows {
                for ws in &mut window.workspaces {
                    remove_exited_tab(&mut ws.tabs, &mut ws.active, id);
                }
            }
            self.closed_windows.retain(|w| w.tabs().next().is_some());
            for wi in (0..self.wins.len()).rev() {
                let win = &mut self.wins[wi];
                let removed = remove_exited_tab(&mut win.tabs, &mut win.active, id);
                for ws in &mut win.workspaces {
                    remove_exited_tab(&mut ws.tabs, &mut ws.active, id);
                }
                if removed {
                    win.frame_gen = win.frame_gen.wrapping_add(1);
                    if win.tabs.is_empty() {
                        if let Some(target) = win.other_workspace() {
                            win.select_workspace(target);
                            self.cur = wi;
                            self.sync_term_size();
                        } else {
                            self.close_window(wi, event_loop);
                        }
                    }
                }
            }
            self.forget_tab(id);
        }
        if let Some(id) = original { self.select_window(id); }
    }

    /// Hand a tab from one window to another. The tab object itself moves:
    /// pty, child, terminal state, nothing respawned. The view follows the
    /// tab (the user should see where it went, and an unfocused target
    /// asks for attention), and a source window emptied of tabs closes.
    /// The moved shell's $MYPIXELS_WINDOW still names the OLD window --
    /// environment of a running process cannot change -- so `mypixels`
    /// typed there afterwards opens tabs in the window it left. Known wart.
    fn move_tab(
        &mut self,
        from: usize,
        idx: usize,
        to: usize,
        insert: usize,
        event_loop: &ActiveEventLoop,
    ) {
        if from == to || to >= self.wins.len() || idx >= self.wins[from].tabs.len() {
            return;
        }
        let insert = insert.min(self.wins[to].tabs.len());
        let flen = self.wins[from].tabs.len();
        let active = self.wins[from].active;
        let tab = self.wins[from].tabs.remove(idx);
        log::info!("moved tab {:?} to window {}", tab.display_title(), to + 1);
        self.wins[to].tabs.insert(insert, tab);
        let emptied = self.wins[from].tabs.is_empty() && self.wins[from].other_workspace().is_none();
        if self.wins[from].tabs.is_empty() && !emptied {
            self.cur = from;
            let target = self.wins[from].other_workspace().unwrap();
            self.switch_workspace(target);
        } else if !emptied {
            self.wins[from].active = move_active_follow(flen, active, idx).unwrap_or(0);
        }
        let mut cur = to;
        if emptied {
            if to > from {
                cur = to - 1;
            }
            self.close_window(from, event_loop);
        }
        self.cur = cur;
        self.wins[cur].active = insert;
        if let Some(w) = self.wins[cur].window.as_ref() {
            if !self.wins[cur].focused {
                w.request_user_attention(Some(winit::window::UserAttentionType::Critical));
            }
        }
        for w in &self.wins {
            if let Some(win) = &w.window {
                win.request_redraw();
            }
        }
    }

    /// Which of this session's windows contains a GLOBAL point, and where
    /// in that window's strip the point would insert a tab. Decorations
    /// make outer_position a few pixels off from the inner origin; the use
    /// (a drop target the width of a tab) forgives it.
    fn win_at(&self, gx: f32, gy: f32) -> Option<(usize, usize)> {
        for (i, w) in self.wins.iter().enumerate() {
            let Some(win) = w.window.as_ref() else { continue };
            let Ok(at) = win.outer_position() else { continue };
            let size = win.outer_size();
            let (x0, y0) = (at.x as f32, at.y as f32);
            if gx < x0 || gx >= x0 + size.width as f32 || gy < y0 || gy >= y0 + size.height as f32 {
                continue;
            }
            let lx = gx - x0;
            let mut insert = w.tabs.len();
            for (j, &(_, x1)) in w.tab_hits.iter().enumerate() {
                if lx < x1 {
                    insert = j;
                    break;
                }
            }
            return Some((i, insert));
        }
        None
    }

    fn drain_session_requests(&mut self, event_loop: &ActiveEventLoop) {
        let mut opened = false;
        while let Ok(req) = self.session_rx.try_recv() {
            match req {
                session::Request::OpenTab { window, cwd, argv } => {
                    // The number is the requesting shell's window; if it
                    // still exists the tab goes exactly there. A stale or
                    // absent number means "wherever the user is looking",
                    // which is already `cur`.
                    if let Some(index) = self.wins.iter().position(|w| w.session_id == window) {
                        self.cur = index;
                    }
                    // Trust but verify the directory: the caller may have
                    // raced its own rmdir; a vanished cwd means "who cares",
                    // not "fail".
                    let cwd = cwd.map(std::path::PathBuf::from).filter(|p| p.is_dir());
                    let cmd = if argv.is_empty() {
                        None
                    } else {
                        Some((argv[0].clone(), argv[1..].to_vec()))
                    };
                    // The remote could name a program that does not exist,
                    // and spawning one would take the whole session down
                    // with the panic. Refusing the request is the polite
                    // half of the deal; the log line is the other.
                    if let Some((program, _)) = cmd.as_ref() {
                        if !command_exists(program) {
                            log::warn!("session: refusing unknown command {program:?}");
                            continue;
                        }
                    }
                    if self.wins.is_empty() {
                        self.make_window(event_loop, cmd, cwd, true);
                    } else {
                        self.add_tab_with(cwd, cmd);
                    }
                    opened = true;
                }
                session::Request::NewWindow { cwd, argv } => {
                    let cmd = if argv.is_empty() {
                        None
                    } else {
                        Some((argv[0].clone(), argv[1..].to_vec()))
                    };
                    if let Some((program, _)) = cmd.as_ref() {
                        if !command_exists(program) {
                            log::warn!("session: refusing unknown command {program:?}");
                            continue;
                        }
                    }
                    let cwd = cwd.map(std::path::PathBuf::from).filter(|p| p.is_dir());
                    log::info!("session: opened a remote window ({} windows now)", self.wins.len() + 1);
                    self.make_window(event_loop, cmd, cwd, true);
                    self.cur = self.wins.len() - 1;
                    opened = true;
                }
                session::Request::RestoreWindow => {
                    if !self.closed_windows.is_empty() {
                        self.revive_window(self.closed_windows.len() - 1, event_loop);
                        opened = true;
                    }
                }
                session::Request::Raise => opened = !self.wins.is_empty(),
                session::Request::LastOutput { window, n, copy, respond } => {
                    // Fold any marks still in the ring first: the shell
                    // that typed `mypixels last` emitted its D mark
                    // moments ago, and that command is exactly what the
                    // user is asking about.
                    let text = self.last_output_text(window, n);
                    if copy && !text.is_empty() {
                        self.copy_text(clipboard::Kind::Clipboard, text.clone());
                    }
                    let _ = respond.send(text);
                }
            }
        }
        // The MCP endpoint's queries take the same channel shape as the
        // session bus's: answer on the main thread, reply on the oneshot.
        while let Ok(q) = self.mcp_rx.try_recv() {
            match q {
                mcp::Query::ListWindows { respond } => {
                    let v = serde_json::json!([
                        self.wins.iter().map(|w| serde_json::json!({
                            "window": w.session_id,
                            "active_workspace": w.workspace + 1,
                            "workspaces": (0..WORKSPACES).map(|index| {
                                let (tabs, active) = if index == w.workspace { (&w.tabs, w.active) }
                                    else { (&w.workspaces[index].tabs, w.workspaces[index].active) };
                                serde_json::json!({
                                    "workspace": index + 1,
                                    "active_tab": if tabs.is_empty() { None } else { Some(active + 1) },
                                    "tabs": tabs.iter().enumerate().map(|(j, t)| serde_json::json!({
                                        "tab": j + 1, "title": t.display_title(),
                                        "cwd": t.cwd_raw().map(|p| p.to_string_lossy().into_owned()),
                                    })).collect::<Vec<_>>()
                                })
                            }).collect::<Vec<_>>(),
                            "active_tab": w.active + 1,
                            "tabs": w.tabs.iter().enumerate().map(|(j, t)| serde_json::json!({
                                "tab": j + 1,
                                "title": t.display_title(),
                                "cwd": t.cwd_raw().map(|p| p.to_string_lossy().into_owned()),
                            })).collect::<Vec<_>>()
                        })).collect::<Vec<_>>()
                    ]);
                    let _ = respond.send(v);
                }
                mcp::Query::LastOutput { window, n, respond } => {
                    let _ = respond.send(self.last_output_text(window, n));
                }
            }
        }
        if opened && !self.wins.is_empty() {
            if let Some(w) = &self.wins[self.cur].window {
                // The compositor owns focus; an attention request is how a
                // polite app says "over here" without stealing the keyboard.
                w.request_user_attention(Some(winit::window::UserAttentionType::Critical));
            }
            log::info!("session: opened a remote tab (tabs={})", self.wins[self.cur].tabs.len());
        }
    }

    fn listener(&self) -> SpikeListener {
        SpikeListener {
            wake: self.wake.clone(),
            window: Arc::clone(&self.window_slot),
            tab_id: 0,
            title: Arc::new(Mutex::new(String::new())),
            exited: Arc::clone(&self.exited),
            last_activity: Arc::clone(&self.last_activity),
            bell: Arc::clone(&self.bell),
            bells: Arc::clone(&self.bell_pings),
            notifications: Arc::clone(&self.notification_requests),
            sems: Arc::clone(&self.sems_ring),
            apcs: Arc::clone(&self.apcs_ring),
            pty_writes: Arc::clone(&self.pty_writes_ring),
            resets: Arc::clone(&self.resets_ring),
            cell_sizes: Arc::clone(&self.cell_sizes_ring),
        }
    }

    /// Bury a tab: it keeps running in the graveyard, unseen, until raised
    /// or evicted. The per-tab state (command blocks, kitty images, the
    /// desktop-notification parser) all survive the burial with it.
    fn bury_tab(&mut self, tab: Tab) {
        self.graveyard.push(GraveEntry { tab, closed_at: Instant::now() });
        while self.graveyard.len() > self.config.graveyard_limit {
            let entry = self.graveyard.remove(0);
            self.forget_tab(entry.tab.id);
        }
    }

    /// Shared cleanup for a tab that dies for good (eviction, kill, a buried
    /// child that exited): per-tab state out, then the Tab's drop sends its
    /// pty thread the shutdown that kills the child.
    fn forget_tab(&mut self, id: usize) {
        self.sem_blocks.remove(&id);
        self.kitty.remove(&id);
        self.forget_gpu_images(id);
    }

    /// Kill one buried tab for good. Positions are the panel's, newest
    /// first, so the caller translates before calling.
    fn kill_grave(&mut self, pos: usize) {
        if pos >= self.graveyard.len() {
            return;
        }
        let id = self.graveyard[pos].tab.id;
        self.graveyard.remove(pos);
        self.forget_tab(id);
    }

    /// Raise a buried tab back into the current window, next to the active
    /// tab. The grid already follows the drawing window's size (the frame
    /// path keeps buried grids in step), so nothing else needs resizing.
    fn revive_tab(&mut self, pos: usize) {
        if pos >= self.graveyard.len() || self.wins.is_empty() {
            return;
        }
        let entry = self.graveyard.remove(pos);
        let win = &mut self.wins[self.cur];
        let insert = (win.active + 1).min(win.tabs.len());
        win.tabs.insert(insert, entry.tab);
        win.active = insert;
        win.frame_gen = win.frame_gen.wrapping_add(1);
    }

    /// Apply a deferred graveyard choice. The panel's index is newest-first;
    /// the graveyard is oldest-first.
    fn apply_pending_grave(&mut self) {
        let Some((window, index, action)) = self.pending_grave.take() else { return };
        if !self.select_window(window) { return; }
        let Some(pos) = grave_pos_for(self.graveyard.len(), index) else { return };
        if action == "raise" {
            self.revive_tab(pos);
        } else {
            self.kill_grave(pos);
        }
    }

    /// The workspace panel's rows: all ten slots of this window. The current
    /// workspace's tabs live in `Win::tabs`; the others are parked in their own
    /// list, so the current one is read from where it actually is.
    fn ws_rows(&self) -> Vec<egui_ui::WsRow> {
        let win = &self.wins[self.cur];
        (0..WORKSPACES).map(|i| {
            let (tabs, active) = if i == win.workspace {
                (&win.tabs, win.active)
            } else {
                (&win.workspaces[i].tabs, win.workspaces[i].active)
            };
            egui_ui::WsRow {
                index: i,
                count: tabs.len(),
                title: tabs.get(active).map(|t| t.display_title()).unwrap_or_default(),
                genre: self.config.workspace_genres[i].clone(),
                icon: self.config.workspace_icons[i],
                tabs: tabs.iter().enumerate().map(|(index, tab)| egui_ui::WsTab {
                    id: tab.id,
                    title: tab.display_title(),
                    selected: index == active,
                    preview: win.sw_thumbs.iter().find(|thumb| thumb.tab_id == tab.id)
                        .map(|thumb| (thumb.handle, thumb.size)),
                }).collect(),
                current: i == win.workspace,
                shortcut: config::workspace_shortcut(&self.config, i),
            }
        }).collect()
    }

    /// The graveyard panel's rows, newest first.
    fn grave_rows(&self) -> Vec<egui_ui::GraveRow> {
        self.graveyard.iter().rev().map(|g| {
            let secs = Instant::now().duration_since(g.closed_at).as_secs();
            egui_ui::GraveRow {
                id: g.tab.id,
                title: g.tab.display_title(),
                cwd: g.tab.current_dir().unwrap_or_default(),
                buried: buried_label(secs),
            }
        }).collect()
    }

    fn close_tab_at(&mut self, idx: usize, event_loop: &ActiveEventLoop) {
        if idx >= self.wins[self.cur].tabs.len() {
            return;
        }
        let tab = self.wins[self.cur].tabs.remove(idx);
        self.wins[self.cur].frame_gen = self.wins[self.cur].frame_gen.wrapping_add(1);
        self.bury_tab(tab);
        if self.wins[self.cur].tabs.is_empty() {
            if let Some(target) = self.wins[self.cur].other_workspace() {
                self.switch_workspace(target);
                return;
            }
            // The last tab closes its WINDOW, not the application: a second
            // window must survive closing all of the first one's tabs.
            // (close_window ends the app when this was the last window.)
            self.close_window(self.cur, event_loop);
            return;
        }
        if self.wins[self.cur].active >= self.wins[self.cur].tabs.len() {
            self.wins[self.cur].active = self.wins[self.cur].tabs.len() - 1;
        } else if self.wins[self.cur].active > idx {
            self.wins[self.cur].active -= 1;
        }
    }

    /// Whether the tab strip is currently shown (manual hide flag and
    /// `hide_single_tab` both apply).
    fn show_tab_strip(&self) -> bool {
        !self.wins[self.cur].tablist_hidden && !(self.config.hide_single_tab && self.wins[self.cur].tabs.len() <= 1)
    }

    /// Resize terminal grid of all tabs to fit the current surface size.
    fn sync_term_size(&mut self) {
        self.sync_term_size_with(false);
    }

    fn sync_term_size_with(&mut self, force: bool) {
        let (w, h) = match self.wins.get(self.cur).and_then(|w| w.gpu.as_ref()) {
            Some(g) => (g.config.width, g.config.height),
            None => return,
        };
        if w == 0 || h == 0 {
            return;
        }
        let strip_h = if self.show_tab_strip() { self.tab_h } else { 0.0 };
        let term_h = (h as f32 - strip_h) as usize;
        let (mut ncols, mut nrows) = (
            (w as f32 / self.cell_w) as usize,
            (term_h as f32 / self.cell_h) as usize,
        );
        // Benchmark hook: ask for a grid larger than the window allows, to
        // separate grid-walk and shaping cost from rasterisation when the test
        // display is software only.
        if let Ok(g) = std::env::var("MYPIXELS_GRID") {
            if let Some((c, r)) = g.split_once('x') {
                ncols = c.parse().unwrap_or(ncols);
                nrows = r.parse().unwrap_or(nrows);
            }
        }
        if force || (ncols, nrows) != (self.wins[self.cur].cols, self.wins[self.cur].rows) {
            eprintln!("term resize: {}x{} -> {}x{}", self.wins[self.cur].cols, self.wins[self.cur].rows, ncols, nrows);
            self.wins[self.cur].cols = ncols;
            self.wins[self.cur].rows = nrows;
            self.wins[self.cur].dims.cols = ncols;
            self.wins[self.cur].dims.rows = nrows;
            let window_size = WindowSize {
                num_lines: nrows as u16,
                num_cols: ncols as u16,
                cell_width: self.cell_w as u16,
                cell_height: self.cell_h as u16,
            };
            for tab in self.wins[self.cur].all_tabs() {
                tab.term.lock().resize(self.wins[self.cur].dims);
                tab.term.lock().set_cell_pixels(self.cell_w as u32, self.cell_h as u32);
                let _ = tab.sender.send(Msg::Resize(window_size));
            }
        }
    }

    fn sync_all_term_sizes(&mut self, force: bool) {
        let current = self.cur;
        for index in 0..self.wins.len() {
            self.cur = index;
            self.sync_term_size_with(force);
            if let Some(window) = &self.wins[index].window { window.request_redraw(); }
        }
        self.cur = current;
    }

    /// Re-apply everything a config reload can change. Colors, palette, tab
    /// colors, bindings and tablist visibility are read from `self.config` at
    /// draw time, so swapping the struct (done by the caller) covers them. We
    /// only need to recompute the values copied into App at startup: fg/bg and,
    /// when the font changed, the cell metrics + glyph pools.
    fn apply_transparency(&self) {
        let transparent = effective_transparency(self.config.term_transparency) > 0.0;
        for win in &self.wins {
            if let Some(window) = &win.window {
                // Wayland's opaque-region hint must follow the actual alpha.
                // On X11 the ARGB visual is reserved at window creation instead.
                window.set_transparent(transparent);
                window.request_redraw();
            }
        }
    }

    fn apply_config(&mut self, font_changed: bool, spacing_changed: bool) {
        self.apply_transparency();
        if !self.config.tab_overview_on_alt {
            *self.switcher_alt_at.lock().unwrap() = None;
            self.switcher_show_req.store(false, Ordering::SeqCst);
            self.switcher_open.store(false, Ordering::SeqCst);
        }
        self.fg = self.config.foreground_color.to_f32();
        self.bg = crate::config::terminal_bg(&self.config).to_f32();
        for win in &mut self.wins {
            if !self.config.tab_overview_on_alt { win.overview = None; }
            // Turning the feature off must not leave a panel open that can no
            // longer switch anything, with a check mark on a slot the window
            // is not looking at.
            if !self.config.workspaces_enabled
                && win.egui.as_ref().is_some_and(|e| e.ui_state.panel_is(egui_ui::RightPanel::Workspaces))
            {
                win.egui.as_mut().unwrap().ui_state.close_panel(egui_ui::RightPanel::Workspaces);
            }
            if let Some(gpu) = &mut win.gpu {
                gpu.invalidate_terminals();
            }
            if let Some(window) = &win.window { window.request_redraw(); }
        }
        if font_changed {
            self.apply_fonts();
        } else if spacing_changed {
            // Changing row spacing preserves the live font zoom.
            self.rebuild_term_font();
        }
        let tab_h = ui_style::tab_height(self.tab_text_h, self.scale, self.config.compact_tabs);
        if tab_h != self.tab_h {
            self.tab_h = tab_h;
            self.sync_all_term_sizes(false);
        }
    }

    fn reload_config(&mut self) {
        match Config::try_load() {
            Ok(config) => {
                let font_changed = config.term_font != self.config.term_font || config.tablist_font != self.config.tablist_font;
                let spacing_changed = config.line_spacing != self.config.line_spacing;
                self.config = config;
                self.apply_config(font_changed, spacing_changed);
                self.wins[self.cur].frame_gen = self.wins[self.cur].frame_gen.wrapping_add(1);
                if let Some(watcher) = &self.theme_watcher { watcher.set_paths(self.config.theme_files.clone()); }
                if let Some(window) = &self.wins[self.cur].window { window.request_redraw(); }
                log::info!("configuration and colors reloaded");
            }
            Err(e) => self.show_error(format!("Theme/config reload: {e}")),
        }
    }

    fn check_theme_files(&mut self) {
        if self.wins.is_empty() { return; }
        if self.theme_watcher.as_ref().is_some_and(|w| w.take_changed()) { self.reload_config(); }
    }

    /// Rebuild the terminal font from the live state (`term_family` +
    /// `term_pt`): attrs, cell metrics, the term glyph pool, and the grid.
    /// The base Metrics are baked into each pooled Buffer, so a change needs
    /// new buffers. Everything comes out in physical pixels: shaping happens
    /// at the size the glyphs are actually rasterised at, so HiDPI screens
    /// get the rounded cell of their real size, not a 1x cell doubled.
    fn rebuild_term_font(&mut self) {
        let font_px = self.term_pt * 96.0 / 72.0 * self.scale;
        self.term_metrics =
            fonts::line_metrics(&mut self.font_system, self.term_family, self.term_weight, self.term_slant)
                .unwrap_or(fonts::LineMetrics::DEFAULT);
        let term_lh = self.config.terminal_line_height(self.term_metrics.height_at(font_px));
        self.mono_attrs = {
            let db = self.font_system.db();
            fonts::pin_face(
                db,
                Attrs::new(),
                self.term_family,
                self.term_weight,
                self.term_slant,
            )
        };

        let mut measure =
            Buffer::new(&mut self.font_system, Metrics::new(font_px, term_lh));
        measure.set_size(Some(800.0), None);
        measure.set_text("MMMM", &self.mono_attrs, Shaping::Basic, None);
        measure.shape_until_scroll(&mut self.font_system, false);
        let shaped = measure.lines[0].shape(&mut self.font_system, 8);
        let mut total = 0.0;
        for span in &shaped.spans {
            for word in &span.words {
                total += word.width(font_px);
            }
        }
        let cell_w = (total / 4.0).round().max(1.0);
        let cell_h = term_lh.round().max(1.0);
        self.cell_w = cell_w;
        self.cell_h = cell_h;

        // Native scaling can rebuild metrics before the first window exists.
        for gpu in self.wins.iter_mut().filter_map(|w| w.gpu.as_mut()) {
            gpu.pool = (0..MAX_RUNS)
                .map(|_| Buffer::new(&mut self.font_system, Metrics::new(font_px, term_lh)))
                .collect();
            gpu.prompt_buf =
                Buffer::new(&mut self.font_system, Metrics::new(font_px, term_lh));
            gpu.invalidate_terminals();
        }
        // Metrics are global: resize every window and all its workspaces.
        // Force PTY pixel-size updates even when the row count stays the same.
        self.sync_all_term_sizes(true);
        // Font routing depends on the terminal font, so rebuild it with it.
        *self.coverage.borrow_mut() = Some(fonts::Coverage::new(
            &self.font_system,
            &self.mono_attrs,
            &[self.term_family.to_string()],
        ));
        log::info!(
            "term font: {} {}pt; cell = {:.2} x {:.2}",
            self.term_family, self.term_pt, cell_w, cell_h
        );
    }

    /// Attrs for a single codepoint: the terminal font, or whichever installed
    /// face actually covers the glyph when the terminal font does not.
    ///
    /// Built on demand because the startup path sets the font without going
    /// through `rebuild_term_font`; the font-rebuild path replaces it so a font
    /// change cannot leave a stale base behind.
    fn cell_attrs(&self, ch: char, flags: Flags) -> &'static Attrs<'static> {
        let mut cov = self.coverage.borrow_mut();
        let cov = cov.get_or_insert_with(|| {
            fonts::Coverage::new(
                &self.font_system,
                &self.mono_attrs,
                &[self.term_family.to_string()],
            )
        });
        cov.styled_attrs(self.font_system.db(), ch, flags.contains(Flags::BOLD), flags.contains(Flags::ITALIC))
    }

    /// Resolve the configured tab face for egui and refresh strip metrics.
    fn rebuild_tab_font(&mut self) {
        let (tab_spec, tab_pt) = config::parse_font(&self.config.tablist_font);
        let tab_font_px = tab_pt * 96.0 / 72.0 * self.scale;
        let tab = fonts::resolve_font(self.font_system.db(), &tab_spec);
        let tab_family: &'static str = Box::leak(tab.family.into_boxed_str());
        self.tab_face_key = egui_ui::preview_family_name(tab_family, tab.weight, tab.slant);
        self.tab_metrics =
            fonts::line_metrics(&mut self.font_system, tab_family, tab.weight, tab.slant)
                .unwrap_or(fonts::LineMetrics::DEFAULT);
        let tab_lh = self.tab_metrics.height_at(tab_font_px);
        self.tab_attrs = {
            let db = self.font_system.db();
            fonts::pin_face(db, Attrs::new(), tab_family, tab.weight, tab.slant)
        };

        let mut tmeasure =
            Buffer::new(&mut self.font_system, Metrics::new(tab_font_px, tab_lh));
        tmeasure.set_size(Some(800.0), None);
        tmeasure.set_text("MMMM", &self.tab_attrs, Shaping::Basic, None);
        tmeasure.shape_until_scroll(&mut self.font_system, false);
        let tshaped = tmeasure.lines[0].shape(&mut self.font_system, 8);
        let mut ttotal = 0.0;
        for span in &tshaped.spans {
            for word in &span.words {
                ttotal += word.width(tab_font_px);
            }
        }
        let tab_char_w = (ttotal / 4.0).max(1.0);
        self.tab_char_w = tab_char_w;
        self.tab_text_h = tab_lh;
        // Whole pixels again, for the same reason as the cell grid.
        self.tab_h = ui_style::tab_height(tab_lh, self.scale, self.config.compact_tabs);

        self.sync_all_term_sizes(false);
        log::info!(
            "tab font: {} {}pt; text {:.0}, strip {:.0}",
            tab_family, tab_pt, self.tab_text_h, self.tab_h
        );
    }

    /// Full font re-apply from config (used by config_reload). Resets the live
    /// term size to the config value, then rebuilds both fonts.
    fn apply_fonts(&mut self) {
        let (term_spec, term_pt) = config::parse_font(&self.config.term_font);
        let term = fonts::resolve_font(self.font_system.db(), &term_spec);
        self.term_family = Box::leak(term.family.into_boxed_str());
        self.term_weight = term.weight;
        self.term_slant = term.slant;
        self.term_pt = term_pt;
        self.rebuild_term_font();
        self.rebuild_tab_font();
    }

    /// Change the window scale factor. Every pixel metric is derived from
    /// point sizes, so rescaling is just re-running the two rebuilds, which
    /// also re-fit the grid to the surface. The compositor keeps the window's
    /// *physical* size across a scale change, so the grid shrinks to the
    /// bigger cells rather than the window growing.
    fn set_scale(&mut self, s: f32) {
        if s <= 0.0 || (s - self.scale).abs() < f32::EPSILON {
            return;
        }
        self.scale = s;
        self.rebuild_term_font();
        self.rebuild_tab_font();
    }

    /// enlarge_font / shrink_font: 1pt steps, unbounded up, floor of 1pt down
    /// (matching the Python zoom()).
    fn zoom_term(&mut self, zoom_in: bool) {
        if zoom_in {
            self.term_pt += 1.0;
        } else if self.term_pt > 1.0 {
            self.term_pt -= 1.0;
        } else {
            return;
        }
        self.rebuild_term_font();
    }

    /// The right-click menu, as (label, action, enabled) rows plus the
    /// indices where separators go. Actions are `do_action` names, so the
    /// menu can never offer something no binding could do.
    fn menu_rows(&self) -> (Vec<(String, String, bool)>, Vec<usize>) {
        let w = &self.wins[self.cur];
        let has_tab = w.active < w.tabs.len();
        let has_sel = w
            .tabs
            .get(w.active)
            .is_some_and(|t| t.term.lock().selection_to_string().is_some());
        let has_image = w.tabs.get(w.active).is_some_and(|t| {
            self
                .kitty
                .get(&t.id)
                .is_some_and(|k| k.placements().iter().any(|p| k.image(p.image_id).is_some()))
        });
        let mut items: Vec<(String, String, bool)> = Vec::new();
        let mut seps: Vec<usize> = Vec::new();
        let mut add = |label: &str, action: &str, enabled: bool, sep: bool| {
            if sep {
                seps.push(items.len());
            }
            items.push((label.to_string(), action.to_string(), enabled));
        };
        add("Copy", "copy", has_sel, false);
        add("Paste", "paste", true, false);
        add("Paste primary", "paste_primary", true, false);
        add("Select all", "select_all", has_tab, true);
        add("Copy as image", "copy_as_image", has_tab, true);
        add("Copy last image", "copy_last_image", has_image, false);
        add("Copy last output", "copy_last_output", has_tab, false);
        add("Command history", "show_history", has_tab, false);
        add("Graveyard", "graveyard", true, false);
        add("Workspaces", "show_workspaces", self.config.workspaces_enabled, false);
        add("New tab", "new_tab", true, true);
        add("Rename tab", "rename_tab", has_tab, false);
        add("Close tab", "close_tab", has_tab, false);
        add("New window", "new_window", true, true);
        add("Restore last closed window", "reopen_window", !self.closed_windows.is_empty(), false);
        add("Send tab to new window", "send_tab_new_window", has_tab, false);
        add("Zoom in", "enlarge_font", has_tab, true);
        add("Zoom out", "shrink_font", has_tab, false);
        add("Reset zoom", "reset_font", has_tab, false);
        add("Settings...", "show_config", true, true);
        (items, seps)
    }

    /// Placements shared by full-size tab textures and their previews.
    fn image_quads(&self, tab_index: usize, gpu: &mut Gpu, strip_h: f32, w: u32) -> Vec<gpu::ImageQuad> {
        match self.wins[self.cur].tabs.get(tab_index) {
            Some(tab) => self.image_quads_for(tab, gpu, strip_h, w),
            None => Vec::new(),
        }
    }

    /// Tab-based form: the graveyard's buried tabs share the same placement
    /// math, and they are not in any window's index space.
    fn image_quads_for(&self, tab: &Tab, gpu: &mut Gpu, strip_h: f32, w: u32) -> Vec<gpu::ImageQuad> {
        let mut draws = Vec::new();
        let tab_id = tab.id as u32;
        if let Some(k) = self.kitty.get(&tab.id) {
        let (hist, off) = {
            let term = tab.term.lock();
            (term.grid().history_size(), term.grid().display_offset())
        };
        let top_abs = hist.saturating_sub(off);
        let vy0 = strip_h;
        let vy1 = strip_h + self.wins[self.cur].rows as f32 * self.cell_h;
        for pl in k.placements_sorted() {
            let Some(img) = k.image(pl.image_id) else { continue };
            let Some(geometry) = pl.geometry(img, [self.cell_w, self.cell_h]) else { continue };
            let [iw, ih] = geometry.size;
            let x0 = pl.col as f32 * self.cell_w + pl.x_off as f32;
            let y0 = vy0
                + (pl.anchor_line as f32 - top_abs as f32) * self.cell_h
                + pl.y_off as f32;
            let (cx0, cy0) = (x0.max(0.0), y0.max(vy0));
            let (cx1, cy1) = ((x0 + iw).min(w as f32), (y0 + ih).min(vy1));
            if cx1 <= cx0 || cy1 <= cy0 {
                continue;
            }
            let key = (tab_id, pl.image_id, img.generation);
            if !gpu.has_image(key) {
                gpu.upload_image(key, &img.rgba, img.width, img.height);
            }
            draws.push(gpu::ImageQuad {
                key,
                dst: [cx0, cy0, cx1 - cx0, cy1 - cy0],
                uv: [
                    geometry.uv[0] + (cx0 - x0) / iw * geometry.uv[2],
                    geometry.uv[1] + (cy0 - y0) / ih * geometry.uv[3],
                    geometry.uv[0] + (cx1 - x0) / iw * geometry.uv[2],
                    geometry.uv[1] + (cy1 - y0) / ih * geometry.uv[3],
                ],
            });
        }
        }
        draws
    }

    fn build_frame(&self, tab: &Tab, show_cursor: bool, on_damage: impl FnOnce(Damage)) -> FrameData {
        let mut term = tab.term.lock();
        // Snapshot and reset under the same lock as the cells. Output arriving
        // after this snapshot remains damaged for the next frame. The callback
        // gives preview preparation an independent, accumulating damage queue.
        let damage = if full_redraw_enabled() { Damage::full() }
            else { Damage::collect(term.damage_content()) };
        term.reset_damage();
        on_damage(damage);
        let content = term.renderable_content();
        let palette = Palette::from_colors(content.colors, &self.config.palette);
        use alacritty_terminal::vte::ansi::{Color, NamedColor};
        let resolve = |color: &Color| palette.resolve_with(color, self.fg, self.bg, self.config.cursor_color.to_f32());
        let adjust = |color| self.config.adjust_terminal_color(color);
        let def_fg = adjust(resolve(&Color::Named(NamedColor::Foreground)));
        let def_bg = adjust(resolve(&Color::Named(NamedColor::Background)));
        // The cursor is the one element the user must never have to hunt
        // for, whatever palette the theme carries: boost it halfway to
        // white. A theme that already chose a bright cursor loses nothing
        // (the mix flattens as it approaches 1.0), a dark one gains a lot.
        let cursor_color = {
            let c = resolve(&Color::Named(NamedColor::Cursor));
            adjust(c.map(|v| v + (1.0 - v) * 0.5))
        };
        let rows = self.wins[self.cur].rows;
        let cols = self.wins[self.cur].cols;

        let mut runs: Vec<Run> = Vec::new();
        let mut cur: Option<Run> = None;

        for indexed in content.display_iter {
            let row = indexed.point.line.0 + content.display_offset as i32;
            if row < 0 { continue; }
            let row = row as usize;
            if row >= rows {
                break;
            }
            let col = indexed.point.column.0;
            if col >= cols {
                continue;
            }
            let cell = indexed.cell;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            // `put_tab` stores the tab byte in the cell; it is invisible and
            // shapes to a .notdef box, so it draws as a plain space.
            let c = if cell.c.is_control() { ' ' } else { cell.c };

            let mut fg = resolve(&cell.fg);
            let mut bg = resolve(&cell.bg);
            if cell.flags.contains(Flags::INVERSE) { std::mem::swap(&mut fg, &mut bg); }
            if cell.flags.contains(Flags::DIM) { fg = fg.map(|v| v * 0.66); }
            let fg = adjust(fg);
            let bg = adjust(bg);
            let attrs = self.cell_attrs(c, cell.flags);
            let width = if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };

            // Continue the current run in place. Building a `String` per cell is
            // ~80k allocations a frame at 4K, and a debug build feels every one.
            if let Some(r) = cur.as_mut() {
                if r.row == row as u32
                    && r.col + r.width == col as u32
                    && r.fg == fg
                    && r.bg == bg
                    && std::ptr::eq(r.attrs, attrs)
                {
                    r.cells.push((r.text.len(), r.width));
                    r.width += width;
                    r.text.push(c);
                    if let Some(zw) = cell.zerowidth() {
                        r.text.extend(zw.iter().copied());
                    }
                    continue;
                }
            }
            if let Some(r) = cur.take() {
                push_run(&mut runs, r, def_fg, def_bg);
            }
            let mut glyph = String::with_capacity(4);
            glyph.push(c);
            if let Some(zw) = cell.zerowidth() {
                glyph.extend(zw.iter());
            }
            cur = Some(Run {
                row: row as u32,
                col: col as u32,
                width,
                fg,
                bg,
                text: glyph,
                cells: vec![(0, 0)],
                attrs,
            });
        }
        if let Some(r) = cur.take() {
            push_run(&mut runs, r, def_fg, def_bg);
        }

        let cursor = if !show_cursor {
            None
        } else {
            match content.cursor.shape {
                alacritty_terminal::vte::ansi::CursorShape::Hidden => None,
                shape if (0..self.wins[self.cur].rows as i32).contains(&(content.cursor.point.line.0 + content.display_offset as i32)) => Some(Cursor {
                    column: content.cursor.point.column.0 as u32,
                    row: (content.cursor.point.line.0 + content.display_offset as i32) as u32,
                    shape,
                }),
                _ => None,
            }
        };

        FrameData { runs, cursor, fg: def_fg, bg: def_bg, cursor_color }
    }

    /// Which way (and how fast) a selection drag at pointer height `py`
    /// should scroll. `top`/`bottom` bound the text grid. Inside a band at
    /// an edge, the pull is answered: the deeper toward (and past) the edge
    /// the pointer is pushed, the more lines travel per tick -- grazing the
    /// band drifts a line, pinning the pointer at the window's border
    /// scrolls almost a screen. The pointer cannot leave the window, so
    /// how far INTO the edge you drove it is the only honest measure of
    /// how hard the drag is; depth is the speed. Positive direction = up
    /// into history, the wheel's own sign.
    fn autoscroll_tick(py: f32, top: f32, bottom: f32, cell: f32) -> Option<(i32, i32)> {
        if bottom <= top || cell <= 0.0 {
            return None;
        }
        // A band taller than the grid would scroll on a pointer parked in
        // the middle of a tiny window.
        let band = (cell * 2.2).min((bottom - top) / 3.0);
        let cap = 32;
        if py < top + band {
            let depth = (top + band - py).max(0.0);
            Some((1, ((depth / cell).ceil() as i32).clamp(1, cap)))
        } else if py > bottom - band {
            let depth = (py - (bottom - band)).max(0.0);
            Some((-1, ((depth / cell).ceil() as i32).clamp(1, cap)))
        } else {
            None
        }
    }

    /// Render the terminal and, while Alt is held, the egui tab switcher.
    /// Builds a frame per tab (before the mutable gpu borrow), then hands
    /// off to the gpu pass.
    fn draw(&mut self, window: &Window) {
        debug_assert_eq!(self.wins[self.cur].window.as_ref().map(|w| w.id()), Some(window.id()),
            "a redraw must use its own window's terminal and renderer");
        if self.wins[self.cur].notice.as_ref().is_some_and(|(_, at)| at.elapsed().as_secs() >= 4) { self.wins[self.cur].notice = None; }
        // Selection-drag autoscroll: dragging the selection edge into the
        // band at either grid edge scrolls that way, and dragging PAST the
        // edge scrolls faster -- one extra line per tick for every cell
        // height of over-drag. The signs match the wheel: Delta(+) travels
        // up into history, Delta(-) back down.
        {
            let (px, py) = self.wins[self.cur].pointer;
            let strip_h = if self.show_tab_strip() { self.tab_h } else { 0.0 };
            let bottom = strip_h + self.wins[self.cur].rows as f32 * self.cell_h;
            let dir = if self.wins[self.cur].drag.is_some() && !self.wins[self.cur].tabs.is_empty() {
                Self::autoscroll_tick(py, strip_h, bottom, self.cell_h)
            } else {
                None
            };
            if let Some((d, lines)) = dir {
                if self.wins[self.cur].autoscroll_next.map_or(true, |t| Instant::now() >= t) {
                    self.wins[self.cur].autoscroll_next = Some(Instant::now() + std::time::Duration::from_millis(50));
                    let tab = &self.wins[self.cur].tabs[self.wins[self.cur].active];
                    tab.scroll(alacritty_terminal::grid::Scroll::Delta(d * lines));
                    // Same buffer-line transform as `cell_at`, applied to
                    // the edge row of the post-scroll view.
                    let col = ((px / self.cell_w).floor() as usize).min(self.wins[self.cur].cols.saturating_sub(1));
                    // The edge row is the moving end of the selection. The
                    // pointer's half-cell side would clip it at the cell
                    // midline (and col 0, Left, wraps to the previous line
                    // and drops the whole row), so face the side toward the
                    // drag and select through the cell under the pointer.
                    let side = if d > 0 { Side::Left } else { Side::Right };
                    let vis_row = if d > 0 { 0 } else { self.wins[self.cur].rows as i32 - 1 };
                    let off = tab.term.lock().grid().display_offset() as i32;
                    tab.update_selection(
                        Point { line: Line(vis_row - off), column: Column(col) },
                        side,
                    );
                    window.request_redraw();
                }
            } else {
                self.wins[self.cur].autoscroll_next = None;
            }
        }
        let t_start = Instant::now();
        if self.wins[self.cur].tabs.get(self.wins[self.cur].active).is_none() { return; }
        // The active tab is "focused" iff the window has input focus
        // (covers tab switching, which produces no Focused event).
        {
            let active = &self.wins[self.cur].tabs[self.wins[self.cur].active];
            let mut term = active.term.lock();
            let now = window.has_focus();
            if term.is_focused != now {
                term.is_focused = now;
                // DECSET 1004: the foreground app tracks focus itself (codex's
                // unfocused notifications among them). Only the transition is
                // reported; this block runs on every frame.
                if term.focus_reporting() {
                    let report = if now { b"\x1b[I" } else { b"\x1b[O" };
                    drop(term);
                    active.write(report);
                }
            }
        }
        let blink = self.cursor_blink.load(Ordering::SeqCst);
        // The overview is an overlay on the live terminal, so the terminal is
        // still painted underneath it -- that is what the backdrop dims.
        // Candidate background snapshots are built before borrowing the GPU.
        // Visual bell: a brief lift of the background, VTE's built-in bell
        // with the sound removed. Run colors resolve from `self.bg` at build
        // time, so lifting it for the duration of the builds flashes the
        // whole terminal -- glyphs included -- not just the gaps between
        // runs. Frames re-request each other while lit.
        let bell_k = self.bell_flash();
        let saved_bg = self.bg;
        if bell_k > 0.0 {
            self.bg = bell_mix(self.bg, bell_k);
            window.request_redraw();
        }
        let preview_hover = (self.wins[self.cur].overview.is_none() && self.show_tab_strip() && self.wins[self.cur].tab_drag.is_none()
            && self.wins[self.cur].pointer.1 >= 0.0 && self.wins[self.cur].pointer.1 < self.tab_h)
            .then_some(self.wins[self.cur].hover_tab).filter(|&i| i < self.wins[self.cur].tabs.len());
        let n = self.wins[self.cur].tabs.len();
        let mut preview_order = Vec::with_capacity(n);
        if let Some(i) = preview_hover { preview_order.push(i); }
        let active = self.wins[self.cur].active;
        if !preview_order.contains(&active) { preview_order.push(active); }
        for k in 0..n {
            let i = (self.wins[self.cur].thumb_rr + k) % n;
            if !preview_order.contains(&i) { preview_order.push(i); }
        }
        // The active target is already the preview source. Build at most
        // three candidates, then update at most two background targets.
        // Idle snapshots run at most five times a second, driven by existing
        // terminal redraws; cold caches and visible previews take priority.
        let now = Instant::now();
        let due = self.wins[self.cur].overview.is_some() || preview_hover.is_some()
            || self.wins[self.cur].sw_thumbs.len() < n || now >= self.wins[self.cur].preview_next;
        preview_order.truncate(3);
        if due {
            self.wins[self.cur].preview_next = now + std::time::Duration::from_millis(200);
            self.wins[self.cur].thumb_rr = (self.wins[self.cur].thumb_rr + 1) % n.max(1);
        } else { preview_order.clear(); }
        let mut damage_events = Vec::new();
        let frame = self.build_frame(&self.wins[self.cur].tabs[active], blink,
            |damage| damage_events.push((active, damage)));
        let ov_frames: Vec<Option<FrameData>> = (0..n).map(|i| {
            (i != active && preview_order.contains(&i)).then(|| {
                self.build_frame(&self.wins[self.cur].tabs[i], false,
                    |damage| damage_events.push((i, damage)))
            })
        }).collect();
        // Buried tabs keep running: every frame their damage joins the same
        // queue, and their grids follow the drawing window's size so the
        // graveyard panel's previews line up with it. They occupy the
        // preview index space right after the window's own tabs.
        let g = self.graveyard.len();
        let grave_frames: Vec<FrameData> = (0..g).map(|gi| {
            let entry = &self.graveyard[gi];
            {
                let mut term = entry.tab.term.lock();
                if term.columns() != self.wins[self.cur].cols
                    || term.screen_lines() != self.wins[self.cur].rows {
                    let dims = self.wins[self.cur].dims;
                    term.resize(dims);
                    let _ = entry.tab.sender.send(Msg::Resize(WindowSize {
                        num_lines: self.wins[self.cur].rows as u16,
                        num_cols: self.wins[self.cur].cols as u16,
                        cell_width: self.cell_w as u16,
                        cell_height: self.cell_h as u16,
                    }));
                }
            }
            self.build_frame(&entry.tab, false,
                |d| damage_events.push((n + gi, d)))
        }).collect();
        // Parked workspaces share the thumbnail cache, but only two temporary
        // full-size GPU targets are needed. Rotate through visible cards at 5 Hz;
        // keep the small thumbnails when the panel closes or workspaces switch.
        let mut ws_preview_tabs = Vec::new();
        let ws_panel = self.wins[self.cur].egui.as_ref()
            .filter(|e| e.ui_state.panel_is(egui_ui::RightPanel::Workspaces));
        if let Some(e) = ws_panel {
            let visible_tabs = &e.ui_state.ws_visible_tabs;
            let query = e.ui_state.ws_query.trim().to_lowercase();
            let populated_only = e.ui_state.ws_populated_only;
            let win = &self.wins[self.cur];
            let mut candidates = Vec::new();
            for (workspace, slot) in win.workspaces.iter().enumerate() {
                if workspace == win.workspace || (populated_only && slot.tabs.is_empty()) { continue; }
                let matches = query.is_empty()
                    || format!("Workspace {} {}", workspace + 1, self.config.workspace_genres[workspace]).to_lowercase().contains(&query)
                    || slot.tabs.iter().any(|tab| tab.display_title().to_lowercase().contains(&query));
                if !matches { continue; }
                for (tab, terminal) in slot.tabs.iter().enumerate() {
                    if visible_tabs.contains(&terminal.id)
                        || (visible_tabs.is_empty() && tab == slot.active) {
                        candidates.push((workspace, tab));
                    }
                }
            }
            if now >= win.ws_preview_next && !candidates.is_empty() {
                let start = win.ws_preview_rr % candidates.len();
                for offset in 0..candidates.len().min(2) {
                    ws_preview_tabs.push(candidates[(start + offset) % candidates.len()]);
                }
                let win = &mut self.wins[self.cur];
                win.ws_preview_rr = (start + ws_preview_tabs.len()) % candidates.len();
                win.ws_preview_next = now + std::time::Duration::from_millis(200);
            }
        }
        let ws_preview_frames: Vec<FrameData> = ws_preview_tabs.iter().enumerate().map(|(pi, &(workspace, tab))| {
            self.build_frame(&self.wins[self.cur].workspaces[workspace].tabs[tab], false,
                |damage| damage_events.push((n + g + pi, damage)))
        }).collect();
        self.bg = saved_bg;
        let t_built = t_start.elapsed();
        let show_strip = self.show_tab_strip();
        let (w, h) = match &self.wins[self.cur].gpu {
            Some(g) => (g.config.width, g.config.height),
            None => return,
        };
        if w == 0 || h == 0 {
            return;
        }
        // egui (configuration window): run the UI before the GPU is moved
        // out of `self`, so applying a changed config can touch it.
        // What the drawing caches depend on; the rest of the config is read
        // live from `self.config` every frame.
        let old_text_state = (
            self.config.term_font.clone(),
            self.config.tablist_font.clone(),
            self.config.foreground_color,
            crate::config::terminal_bg(&self.config),
            self.config.col,
            self.config.line_spacing,
            self.config.compact_tabs,
            self.config.terminal_brightness,
            self.config.term_transparency,
        );
        // The UI wears the terminal's EFFECTIVE colours (theme, palette and
        // overrides resolved upstream - not the raw config fields, which a
        // themed session bypasses entirely). Applied every frame: live
        // theme changes edit the palette under a running UI, and egui
        // reads visuals at layout time, not paint time.
        if let Some(e) = self.wins[self.cur].egui.as_ref() {
            e.ctx.set_visuals(ui_style::theme(&self.config, self.bg, self.fg));
            ui_style::apply_motion(&e.ctx, self.config.animation_speed);
        }
        let tab_faces: Vec<tab_bar::TabFace> = if show_strip {
            self.wins[self.cur].tabs.iter().enumerate().map(|(i, tab)| tab_bar::TabFace {
                id: tab.id,
                preview: self.wins[self.cur].sw_thumbs.iter().find(|thumb| thumb.tab_id == tab.id)
                    .map(|thumb| (thumb.handle, thumb.size)),
                title: tab_title(tab, i == self.wins[self.cur].active, self.config.tab_title_max_length,
                    self.config.tab_title_short_current, &self.config.tab_shorten_str),
                tooltip: format!("{}\n{}", tab.display_title(), tab.current_dir().unwrap_or_default()),
                group: if tab.group < 8 { Some(ui_style::rgb_c32(self.config.col[tab.group])) } else { None },
                entangled: tab.entangled,
                attention: self.wins[self.cur].bell_pulse.filter(|(index, _)| *index == i)
                    .map_or(0.0, |(_, at)| (1.0 - at.elapsed().as_secs_f32() / 0.9).clamp(0.0, 1.0)),
            }).collect()
        } else { Vec::new() };
        let tab_font = egui::FontId::new(config::parse_font(&self.config.tablist_font).1 * 96.0 / 72.0,
            egui::FontFamily::Name(self.tab_face_key.clone().into()));
        let mut strip_geometry = tab_bar::Geometry::default();
        let mut full_output: Option<egui::FullOutput> = None;
        // The switcher draws through egui; its state machine stays in the
        // app (Alt-hold open, navigation, Alt release dismiss). The older
        // hand-drawn overlay was deleted once the egui cut was confirmed:
        // two renderers of the same UI meant two of every bug.
        let sw_open = matches!(
                self.wins[self.cur].overview.map(|o| o.mode),
            Some(OverviewMode::Switcher));
        let sw_selected = self.wins[self.cur].active;
        let (sw_preview_w, sw_preview_h) = egui_ui::thumbnail_size(
            w as f32 / self.scale.max(1.0),
            w as f32 / (h as f32 - if show_strip { self.tab_h } else { 0.0 }).max(1.0),
            self.config.tab_overview_scale,
        );
        let sw_cards: Vec<egui_ui::SwitcherCard> = if sw_open {
            self.wins[self.cur].tabs
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let title = t.display_title();
                    // Cards truncate within their header; the tooltip retains
                    // the full title and working directory.
                    let cwd_line = t.current_dir().filter(|c| c != &title);
                    let thumb = self.wins[self.cur].
                        sw_thumbs
                        .iter().find(|thumb| thumb.tab_id == t.id)
                        .map(|thumb| (thumb.handle, thumb.size));
                    let status = self.sem_blocks.get(&t.id)
                        .and_then(|blocks| blocks.iter().rev().find(|b| b.is_history_command()))
                        .map(|b| if !b.done { egui_ui::SwitcherStatus::Running }
                            else if b.exit.is_some_and(|e| e != 0) { egui_ui::SwitcherStatus::Failed }
                            else if b.exit == Some(0) { egui_ui::SwitcherStatus::Passed }
                            else { egui_ui::SwitcherStatus::Completed })
                        .unwrap_or_default();
                    egui_ui::SwitcherCard {
                        id: t.id,
                        status,
                        title,
                        active: i == self.wins[self.cur].active,
                        marker: t.entangled,
                        cwd: cwd_line,
                        thumb,
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        let sw_accent = if self.config.workspaces_enabled {
            ui_style::rgb_c32(self.config.workspace_colors[self.wins[self.cur].workspace])
        } else {
            ui_style::rgb_c32(self.config.col[2])
        };
        // Which workspace the switcher's tiles came from, for its header. With
        // workspaces off there is one set of tabs and nothing to name.
        let sw_workspace = self.config.workspaces_enabled
            .then_some(self.wins[self.cur].workspace);
        let sw_dim = egui_ui::dim_fill(
            self.bg,
            (self.config.tab_overview_opacity.clamp(0.0, 1.0) * 255.0) as u8,
        );
        let close_retainer = self.window_retainer();
        let close_tabs = self.wins[self.cur].all_tabs().count();
        let close_workspaces = usize::from(!self.wins[self.cur].tabs.is_empty())
            + self.wins[self.cur].workspaces.iter().filter(|ws| !ws.tabs.is_empty()).count();
        let close_retained_tabs = self.graveyard.len()
            + self.closed_windows.iter().map(|w| w.tabs().count()).sum::<usize>();
        {
            let mut egui = self.wins[self.cur].egui.take();
            if let Some(egui) = egui.as_mut() {
                if egui.ui_state.confirm_open {
                    let st = &mut egui.ui_state;
                    if st.confirm.set_retainer(close_retainer) { st.confirm_seed = true; }
                    st.confirm.tabs = close_tabs;
                    st.confirm.workspaces = close_workspaces;
                    st.confirm.limit = self.config.graveyard_limit;
                    st.confirm.retained_tabs = close_retained_tabs;
                }
                let mut raw = egui.state.take_egui_input(window);
                raw.events
                    .extend(std::mem::take(&mut self.wins[self.cur].fifo_egui_events));
                let ctx = egui.ctx.clone();
                // The history panel's rows are a snapshot of the pool,
                // rebuilt every frame while it is open: new commands show
                // up in the list with no extra plumbing, and a tab switch
                // swaps the list without the app noticing.
                if egui.ui_state.panel_is(egui_ui::RightPanel::History) {
                    if let Some(tab) = self.wins[self.cur].tabs.get(self.wins[self.cur].active) {
                        egui.ui_state.history_rows = self.history_rows_for(tab.id);
                        let st = &mut egui.ui_state;
                        // A click changes selection during the UI pass. Decode
                        // it on the next frame, retaining the pool's compressed
                        // storage and caching only one command per window.
                        let selected = st.history_rows.iter().find(|r| Some(r.seq) == st.history_sel);
                        if let Some(row) = selected.filter(|r| r.done && !r.alt_seen) {
                            if st.history_preview.as_ref().is_none_or(|p| p.seq != row.seq) {
                                st.history_preview = self.sem_blocks.get(&tab.id)
                                    .and_then(|q| q.iter().find(|b| b.seq == row.seq))
                                    .map(|b| {
                                        let mut preview = egui_ui::HistoryPreview::new(b.seq, &b.output_text());
                                        if b.truncated_head || b.truncated_tail {
                                            preview.lines.insert(0, "[Captured output is truncated]".into());
                                        }
                                        preview
                                    });
                            }
                        } else {
                            st.history_preview = None;
                        }
                    }
                }
                // The workspace rows are a snapshot of this window's ten
                // slots, rebuilt every frame while the panel is open, so a tab
                // opened, closed or retitled elsewhere shows up immediately.
                if egui.ui_state.panel_is(egui_ui::RightPanel::Workspaces) {
                    egui.ui_state.ws_rows = self.ws_rows();
                }
                // The graveyard rows are a snapshot of the buried tabs,
                // rebuilt every frame while open: a buried shell that
                // retitles itself shows up in the list on the next frame.
                if egui.ui_state.panel_is(egui_ui::RightPanel::Graveyard) {
                    egui.ui_state.grave_rows = self.grave_rows();
                    egui.ui_state.grave_windows = self.closed_windows.iter().rev().map(|w| {
                        let ws = &w.workspaces[w.workspace];
                        let title = ws.tabs.get(ws.active).or_else(|| w.tabs().next())
                            .map(|tab| tab.display_title()).unwrap_or_default();
                        (w.id, title, w.tabs().count())
                    }).collect();
                }
                egui.ui_state.panel_top = if show_strip { self.tab_h / ctx.pixels_per_point() } else { 0.0 };
                let full = ctx.run_ui(raw, |ui| {
                    if show_strip {
                        strip_geometry = tab_bar::show(ui, self.tab_h / ctx.pixels_per_point(), tab_font.clone(),
                            &tab_faces, self.wins[self.cur].active,
                            egui.ui_state.menu_open && egui.ui_state.menu_app, self.wins[self.cur].workspace,
                            egui.ui_state.panel_is(egui_ui::RightPanel::Workspaces), &self.config);
                        if strip_geometry.menu_clicked {
                            let st = &mut egui.ui_state;
                            if st.menu_open && st.menu_app {
                                st.menu_open = false;
                                egui_ui::release_focus(&ctx);
                            } else {
                                st.menu_items = vec![
                                    ("New window".into(), "new_window".into(), true),
                                    ("Settings…".into(), "show_config".into(), true),
                                    ("Keyboard shortcuts".into(), "show_shortcuts".into(), true),
                                    ("About mypixels".into(), "show_about".into(), true),
                                ];
                                st.menu_seps = vec![1, 2];
                                st.menu_pos = [3.0, self.tab_h / ctx.pixels_per_point()];
                                st.menu_app = true;
                                st.menu_open = true;
                                st.menu_out = None;
                                st.menu_age = 0;
                            }
                            ctx.request_repaint();
                        }
                    }
                    if strip_geometry.workspace_clicked {
                        // The tab-bar button owns the workspace panel. It used
                        // to raise a popup menu, which could not show what a
                        // workspace held and sat outside the panel design.
                        let st = &mut egui.ui_state;
                        st.menu_open = false;
                        egui_ui::release_focus(&ctx);
                        st.toggle_panel(egui_ui::RightPanel::Workspaces);
                        st.ws_sel = Some(self.wins[self.cur].workspace);
                        st.ws_out = None;
                        st.ws_rows = self.ws_rows();
                        ctx.request_repaint();
                    }
                    let mut config_open = egui.ui_state.panel_is(egui_ui::RightPanel::Settings);
                    let ev = egui_ui::config_window(
                        ui,
                        &mut config_open,
                        &mut self.config,
                        &mut egui.ui_state,
                        &egui.font_choices,
                    );
                    if !config_open {
                        egui.ui_state.close_panel(egui_ui::RightPanel::Settings);
                    }
                    egui.config_changed = ev.apply;
                    egui.save_requested = ev.save;
                    egui.revert_requested = ev.revert;
                    if sw_open {
                        egui_ui::switcher_screen(
                            ui,
                            true,
                            sw_selected,
                            sw_dim,
                            sw_accent,
                            &sw_cards,
                            egui::vec2(sw_preview_w.round(), sw_preview_h.round()),
                            sw_workspace,
                            &mut egui.switcher_pick,
                        );
                    }
                    egui_ui::rename_box(ui, &mut egui.ui_state);
                    egui_ui::move_pick_box(ui, &mut egui.ui_state);
                    egui_ui::ctx_menu(ui, &mut egui.ui_state, &self.config);
                    egui_ui::app_info(ui, &mut egui.ui_state, &self.config);
                    if !sw_open && egui.ui_state.panel_is(egui_ui::RightPanel::History) {
                        // Status dots wear the palette's own green and red
                        // (the fixed pair only when no palette is set).
                        let pal = self.config.palette;
                        let ok = ui_style::rgb_c32(pal.map(|p| p[2]).unwrap_or(crate::config::Rgb(59, 193, 126)));
                        let err = ui_style::rgb_c32(pal.map(|p| p[1]).unwrap_or(crate::config::Rgb(239, 97, 109)));
                        let top = if show_strip { self.tab_h / ctx.pixels_per_point() } else { 0.0 };
                        egui_ui::history_panel(ui, &mut egui.ui_state, ok, err, top);
                    }
                    if !sw_open && egui.ui_state.panel_is(egui_ui::RightPanel::Graveyard) {
                        let pal = self.config.palette;
                        let ok = ui_style::rgb_c32(pal.map(|p| p[2]).unwrap_or(crate::config::Rgb(59, 193, 126)));
                        let err = ui_style::rgb_c32(pal.map(|p| p[1]).unwrap_or(crate::config::Rgb(239, 97, 109)));
                        let top = if show_strip { self.tab_h / ctx.pixels_per_point() } else { 0.0 };
                        let thumbs: Vec<_> = self.wins[self.cur].grave_thumbs
                            .iter()
                            .map(|t| (t.tab_id, t.handle, t.size))
                            .collect();
                        egui_ui::graveyard_panel(ui, &mut egui.ui_state, ok, err, top, &thumbs);
                    }
                    if !sw_open && egui.ui_state.panel_is(egui_ui::RightPanel::Workspaces) {
                        let pal = self.config.palette;
                        let ok = ui_style::rgb_c32(pal.map(|p| p[2]).unwrap_or(crate::config::Rgb(59, 193, 126)));
                        let err = ui_style::rgb_c32(pal.map(|p| p[1]).unwrap_or(crate::config::Rgb(239, 97, 109)));
                        let top = if show_strip { self.tab_h / ctx.pixels_per_point() } else { 0.0 };
                        let ws: Vec<_> = self.config.workspace_colors.iter().map(|c| ui_style::rgb_c32(*c)).collect();
                        egui_ui::workspaces_panel(ui, &mut egui.ui_state, ok, err, top, &ws);
                    }
                    if let Some(r) = self.ripple.as_ref() {
                        let t = r.at.elapsed().as_secs_f32() / 0.18;
                        if t < 1.0 {
                            let rad = self.cell_h * (0.45 + 1.9 * t);
                            let a = ((1.0 - t) * 120.0) as u8;
                            let col = egui::Color32::from_rgba_unmultiplied(
                                (r.color[0] * 255.0) as u8,
                                (r.color[1] * 255.0) as u8,
                                (r.color[2] * 255.0) as u8,
                                a,
                            );
                            ui.painter().circle_stroke(
                                egui::pos2(r.x / self.scale, r.y / self.scale),
                                rad,
                                egui::Stroke::new(2.0, col),
                            );
                            // The pulse is a countdown: every live pulse
                            // schedules the frame that fades it.
                            window.request_redraw();
                        } else {
                            self.ripple = None;
                        }
                    }
                    egui_ui::confirm_box(ui, &mut egui.ui_state);
                });
                egui.ui_state.release_passive_focus(&ctx, &full.platform_output);
                // egui owns animation/tooltip timing. Preserve delayed
                // deadlines as well as immediate animation repaint requests.
                if preview_hover.is_some() { ctx.request_repaint_after(std::time::Duration::from_millis(100)); }
                if let Some(output) = full.viewport_output.get(&egui::ViewportId::ROOT) {
                    let delay = if preview_hover.is_some() { output.repaint_delay.min(std::time::Duration::from_millis(100)) }
                        else { output.repaint_delay };
                    self.wins[self.cur].repaint_at = Instant::now().checked_add(delay);
                    if delay.is_zero() { window.request_redraw(); }
                }
                // A panel/popup that closed this frame must surrender
                // keyboard focus; a vanished TextEdit does not do it alone,
                // and while egui holds focus every keystroke reads as
                // consumed and never reaches the terminal.
                if let Some(res) = egui.ui_state.rename_out.take() {
                    self.wins[self.cur].renaming = false;
                    egui.ui_state.rename_open = false;
                    if let Some(name) = res {
                        log::info!("rename_tab -> {:?}", name);
                        let cur_active = self.wins[self.cur].active;
                        if let Some(t) = self.wins[self.cur].tabs.get_mut(cur_active) {
                            t.name = if name.is_empty() { None } else { Some(name) };
                        }
                    }
                    window.request_redraw();
                }
                if let Some(choice) = egui.ui_state.confirm_out.take() {
                    egui.ui_state.confirm_open = false;
                    egui_ui::release_focus(&ctx);
                    self.pending_close = Some((window.id(), choice));
                    window.request_redraw();
                }
                if let Some(pick) = egui.ui_state.move_out.take() {
                    let to_workspace = egui.ui_state.move_workspace;
                    egui.ui_state.move_open = false;
                    if let Some(t) = pick {
                        if to_workspace {
                            self.pending_action = Some((window.id(), format!("move_to_workspace_{t}")));
                        } else {
                            // Deferred: making a window needs the event loop,
                            // which the draw path does not have.
                            self.pending_move = Some((window.id(), t));
                        }
                    }
                    window.request_redraw();
                }
                if let Some((index, action)) = egui.ui_state.grave_out.take() {
                    // The panel's list is newest-first; the graveyard is
                    // oldest-first. Translate, then defer to the drain: the
                    // frame already counted the burials it is rendering.
                    self.pending_grave = Some((window.id(), index, action));
                    egui.ui_state.grave_sel = None;
                    window.request_redraw();
                }
                if let Some((id, raise)) = egui.ui_state.grave_window_out.take() {
                    let action = if raise { "reopen_window" } else { "kill_window" };
                    self.pending_action = Some((window.id(), format!("{action}_{id}")));
                    window.request_redraw();
                }
                if let Some(index) = egui.ui_state.ws_out.take() {
                    // Switching is the app's job (it moves whole tab lists), so
                    // it goes through the same action the chord uses. The panel
                    // stays open: hopping between workspaces and watching the
                    // check mark move is what a panel is for.
                    self.pending_action = Some((window.id(), format!("goto_workspace_{index}")));
                    egui.ui_state.ws_sel = Some(index);
                    window.request_redraw();
                }
                if let Some((workspace, id)) = egui.ui_state.ws_tab_out.take() {
                    self.pending_action = Some((window.id(), format!("workspace_tab_{workspace}_{id}")));
                    egui.ui_state.ws_sel = Some(workspace);
                    window.request_redraw();
                }
                if let Some(pick) = egui.ui_state.menu_out.take() {
                    egui.ui_state.menu_open = false;
                    // History can remain open, so the combined popup-open
                    // flag has no falling edge to release the menu's focus.
                    egui_ui::release_focus(&ctx);
                    if let Some(action) = pick {
                        self.pending_action = Some((window.id(), action));
                    }
                    window.request_redraw();
                }
                if let Some((seq, action)) = egui.ui_state.history_out.take() {
                    self.do_history_action(seq, &action);
                    window.request_redraw();
                }
                let popup_open = egui.ui_state.font_popup.as_ref().is_some_and(|p| p.open)
                    || egui.ui_state.rename_open
                    || egui.ui_state.confirm_open
                    || egui.ui_state.move_open
                    || egui.ui_state.menu_open
                    || egui.ui_state.info_page.is_some();
                egui_ui::release_focus_if_closed(&ctx, egui.ui_state.right_panel != egui_ui::RightPanel::Closed, popup_open, &mut egui.open_prev);
                full_output = Some(full);
            }
            self.wins[self.cur].egui = egui;
        }
        let scale = self.scale;
        self.wins[self.cur].tab_hits = strip_geometry.tabs.iter().map(|r| (r.left() * scale, r.right() * scale)).collect();
        self.wins[self.cur].add_hit = (strip_geometry.add.left() * scale, strip_geometry.add.right() * scale);
        self.wins[self.cur].tab_close_hits = strip_geometry.close.into_iter()
            .map(|r| egui::Rect::from_min_max(r.min * scale, r.max * scale)).collect();
        let (revert, changed, save, bindings) = self.wins[self.cur].
            egui
            .as_ref()
            .map(|e| (e.revert_requested, e.config_changed, e.save_requested, e.ui_state.bindings_dirty))
            .unwrap_or((false, false, false, false));
        if revert {
            self.reload_config();
        } else if changed {
            if old_text_state.8 != self.config.term_transparency {
                self.apply_transparency();
            }
            // Re-apply only when a cached value moved. Switcher and
            // geometry fields are read live per frame, and forcing a full
            // re-shape on every slider tick would stutter the drag.
            let font_changed = old_text_state.0 != self.config.term_font
                || old_text_state.1 != self.config.tablist_font;
            let colors_changed = old_text_state.2 != self.config.foreground_color
                || old_text_state.3 != crate::config::terminal_bg(&self.config)
                || old_text_state.4 != self.config.col
                || old_text_state.7 != self.config.terminal_brightness;
            let spacing_changed = old_text_state.5 != self.config.line_spacing;
            let compact_changed = old_text_state.6 != self.config.compact_tabs;
            if font_changed || colors_changed || spacing_changed || compact_changed {
                self.apply_config(font_changed, spacing_changed);
            }
        }
        if save {
            match self.config.save_with_bindings(bindings) {
                // Done on purpose: saving closes the panel, so the user
                // sees the result on the terminal, not in the dialog.
                Ok(()) => {
                    log::info!("configuration saved");
                    if let Some(e) = self.wins[self.cur].egui.as_mut() {
                        e.ui_state.close_panel(egui_ui::RightPanel::Settings);
                        // Kept until a save consumes them: a chord is recorded
                        // many frames before the user presses Save.
                        e.ui_state.bindings_dirty = false;
                        e.ui_state.key_capture = None;
                    }
                }
                Err(err) => self.show_error(err),
            }
        }
        if let Some(e) = self.wins[self.cur].egui.as_mut() {
            e.config_changed = false;
            e.save_requested = false;
            e.revert_requested = false;
            // A switcher card was clicked: activate that tab and dismiss.
            if let Some(i) = e.take_switcher_pick() {
                if i < self.wins[self.cur].tabs.len() {
                    self.wins[self.cur].active = i;
                }
                self.close_switcher();
            }
        }
        // Moved out of `self` so the drawing code below can keep borrowing
        // other `self` fields freely. Put back before every exit.
        let mut gpu = self.wins[self.cur].gpu.take().expect("gpu present, checked above");
        let wf = w as f32;
        let hf = h as f32;

        let strip_h = if show_strip { self.tab_h } else { 0.0 };

        // URLs: rescan the visible grid and locate the pointer on them.
        // Done before the window split-borrow below, which is where the
        // underline's rect actually gets pushed.
        self.url_spans = match self.visual_cell(
            self.wins[self.cur].pointer.0,
            self.wins[self.cur].pointer.1,
        ) {
            Some((row, _)) => self.scan_row_cached(row),
            None => Vec::new(),
        };
        self.url_hover = {
            let (px, py) = self.wins[self.cur].pointer;
            self.visual_cell(px, py).and_then(|(row, col)| {
                self.url_spans
                    .iter()
                    .position(|s| s.row == row && col >= s.start && col < s.end)
            })
        };
        let url_underline: Option<(f32, f32, f32)> = match self.url_hover.and_then(|i| self.url_spans.get(i)) {
            Some(sp) => {
                let strip_h = if self.show_tab_strip() { self.tab_h } else { 0.0 };
                Some((
                    sp.start as f32 * self.cell_w,
                    strip_h + (sp.row + 1) as f32 * self.cell_h - (self.cell_h * 0.07).max(2.0),
                    (sp.end - sp.start) as f32 * self.cell_w,
                ))
            }
            None => None,
        };
        let images = self.image_quads(active, &mut gpu, strip_h, w);
        let mut tab_ids: Vec<_> = self.wins[self.cur].tabs.iter().map(|tab| tab.id).collect();
        tab_ids.extend(self.graveyard.iter().map(|entry| entry.tab.id));
        tab_ids.extend(ws_preview_tabs.iter().map(|&(workspace, tab)| self.wins[self.cur].workspaces[workspace].tabs[tab].id));
        gpu.ensure_previews(&tab_ids, (w, h));
        for (i, damage) in damage_events {
            gpu.damage_terminal(i, &damage);
        }
        let style = TerminalStyle {
            tab_id: tab_ids[active], size: (w, h), cell: (self.cell_w, self.cell_h), strip_h,
            fg: frame.fg, bg: frame.bg, cursor_color: frame.cursor_color,
            transparency: self.config.term_transparency,
        };
        if gpu.prev_styles[active].as_ref() != Some(&style) {
            gpu.prev_damage[active] = Damage::full();
        }
        let selection = self.wins[self.cur].tabs[active].selection_rows();
        gpu.update_overlays(active, gpu::TerminalOverlays {
            selection: selection.clone(), cursor: frame.cursor,
            underline: url_underline.filter(|_| frame.cursor.is_some()),
            notice: self.wins[self.cur].notice.as_ref().map(|(text, _)| text.clone()),
            images: images.clone(),
        }, self.wins[self.cur].rows, self.cell_h, strip_h);
        gpu.prev_styles[active] = Some(style);
        let live_damage = std::mem::take(&mut gpu.prev_damage[active]);
        let live_scissors = live_damage.scissors((w, h), self.cell_h, strip_h, self.wins[self.cur].rows);
        gpu.rects.clear();
        // Convert pixel coords to clip space. Clip is -1..1 on both axes with
        // +y pointing up, screen pixels have y pointing down, so flip y.
        let push_rect =
            |v: &mut Vec<gpu::RectVertex>, x: f32, y: f32, rw: f32, rh: f32, c: [f32; 4]| {
                v.extend_from_slice(&gpu::rect_vertices(x, y, rw, rh, c, 0.0, wf, hf));
            };

        // Window background
        // The terminal background is the only translucent layer, matching the
        // original: it sets VTE's background to alpha = 1 - transparency and
        // leaves the tab strip opaque. `term_transparency` is therefore an
        // amount of *transparency*, not of opacity -- 0.1 means 10% see-through.
        let transparency = effective_transparency(self.config.term_transparency);
        let bg_a = if gpu.alpha_ok && transparency > 0.0 {
            1.0 - transparency
        } else {
            1.0
        };
        static BG_LOG: std::sync::Once = std::sync::Once::new();
        BG_LOG.call_once(|| {
            log::info!(
                "background alpha {:.3} (alpha_ok {}, term_transparency {:.2} -> effective {:.2}, surface alpha {:?})",
                bg_a,
                gpu.alpha_ok,
                self.config.term_transparency,
                transparency,
                gpu.config.alpha_mode
            );
        });
        push_rect(&mut gpu.rects, 0.0, 0.0, w as f32, h as f32, [frame.bg[0], frame.bg[1], frame.bg[2], bg_a]);

        // --- Text areas (tab labels + terminal runs), one prepare() per frame ---
        let mut areas: Vec<TextArea> = Vec::new();
        let bounds = TextBounds { left: 0, top: 0, right: w as i32, bottom: h as i32 };

        // --- Terminal: cell backgrounds ---
        for run in &frame.runs {
            let same_bg = (run.bg[0] - frame.bg[0]).abs() < 0.004
                && (run.bg[1] - frame.bg[1]).abs() < 0.004
                && (run.bg[2] - frame.bg[2]).abs() < 0.004;
            if !same_bg {
                push_rect(
                    &mut gpu.rects,
                    run.col as f32 * self.cell_w,
                    strip_h + run.row as f32 * self.cell_h,
                    run.width as f32 * self.cell_w,
                    self.cell_h,
                    [run.bg[0], run.bg[1], run.bg[2], 1.0],
                );
            }
        }
        // Selection wash: over the cell backgrounds, under the cursor and the
        // glyphs, and translucent so coloured text stays readable underneath --
        // an opaque wash would hide exactly the text the user selected.
        for (row, left, right) in selection {
            push_rect(
                &mut gpu.rects,
                left as f32 * self.cell_w,
                strip_h + row as f32 * self.cell_h,
                (right - left) as f32 * self.cell_w,
                self.cell_h,
                // Adwaita's selection blue, at the alpha terminals like VTE use
                // for an overlay rather than a colour swap.
                { let c = self.config.adjust_terminal_color(self.config.selection_color.to_f32()); [c[0], c[1], c[2], 0.35] },
            );
        }
        if let Some(cursor) = frame.cursor {
            let (cx, cy) = (cursor.column, cursor.row);
            for [x, y, width, height] in cursor.rects(self.cell_w, self.cell_h, strip_h) {
                push_rect(
                    &mut gpu.rects,
                    x, y, width, height,
                    [frame.cursor_color[0], frame.cursor_color[1], frame.cursor_color[2], 0.55],
                );
            }
            if let Some((ux, uy, uw)) = url_underline {
                // One hair under the word: enough to say "this is a link
                // and you are on it", too quiet to compete with real
                // underlines the shell itself may print.
                push_rect(
                    &mut gpu.rects,
                    ux,
                    uy,
                    uw,
                    (self.cell_h * 0.07).max(2.0),
                    [frame.fg[0], frame.fg[1], frame.fg[2], 0.7],
                );
            }
            self.cursor_pt = Some((
                [
                    cx as f32 * self.cell_w + self.cell_w * 0.5,
                    strip_h + cy as f32 * self.cell_h + self.cell_h * 0.5,
                ],
                frame.cursor_color,
            ));
        } else {
            self.cursor_pt = None;
        }

        // --- Terminal text ---
        // SAFETY: pool indices are disjoint (run index < MAX_RUNS), the pool
        // never reallocates after init, and the derived shared references are
        // only used this frame, before any further pool writes. The switcher
        // draws through egui and never touches this pool.
        {
            let fs = &mut self.font_system;
            let pool = &mut gpu.pool;
            for (i, run) in frame.runs.iter().filter(|run| live_damage.shapes_row(run.row)).take(MAX_RUNS).enumerate() {
                if self.wins[self.cur].notice.is_some() && run.row as usize == self.wins[self.cur].rows.saturating_sub(1) { continue; }
                let blank = run.text.chars().all(|c| c == ' ');
                let default_fg =
                    (run.fg[0] - frame.fg[0]).abs() + (run.fg[1] - frame.fg[1]).abs()
                        + (run.fg[2] - frame.fg[2]).abs()
                        < 0.01;
                if blank && default_fg {
                    continue;
                }
                let b = &mut pool[i];
                fonts::shape_terminal_run(b, fs, &run.text, run.attrs, &run.cells, self.cell_w);
                let buf: &Buffer = unsafe { &*(&pool[i] as *const Buffer) };
                areas.push(gpu::terminal_text_area(
                    buf,
                    run.col as f32 * self.cell_w,
                    strip_h + run.row as f32 * self.cell_h,
                    run.fg,
                    bounds,
                ));
            }
        }

        // --- Notice prompt (overlays the last terminal row; renaming moved
        // into egui and no longer paints over the grid) ---
        if self.wins[self.cur].notice.is_some() {
            let prompt_y = strip_h + self.wins[self.cur].rows.saturating_sub(1) as f32 * self.cell_h;
            push_rect(
                &mut gpu.rects,
                0.0,
                prompt_y,
                w as f32,
                self.cell_h,
                [frame.bg[0], frame.bg[1], frame.bg[2], 1.0],
            );
            let prompt = format!("  {}", self.wins[self.cur].notice.as_ref().unwrap().0);
            {
                let fs = &mut self.font_system;
                let b = &mut gpu.prompt_buf;
                b.set_size(Some(w as f32), None);
                b.set_text(&prompt, &self.mono_attrs, Shaping::Basic, None);
                b.set_monospace_width(Some(self.cell_w));
                b.shape_until_scroll(fs, false);
                let ptr: *const Buffer = b;
                let buf: &Buffer = unsafe { &*ptr };
                areas.push(TextArea {
                    buffer: buf,
                    left: 0.0,
                    top: prompt_y,
                    scale: 1.0,
                    bounds,
                    default_color: glyphon::Color::rgb(
                        (frame.fg[0] * 255.0) as u8,
                        (frame.fg[1] * 255.0) as u8,
                        (frame.fg[2] * 255.0) as u8,
                    ),
                    custom_glyphs: &[],
                });
            }
        }

        let t_text = t_start.elapsed();
        // Refresh changed preview content in bounded batches, whether the
        // switcher is visible or its shared hover/idle cache is being warmed.
        let mut jobs: Vec<gpu::PreviewJob> = Vec::new();
        {
            let n = self.wins[self.cur].tabs.len();
            let sel = preview_hover.unwrap_or(self.wins[self.cur].active).min(n.saturating_sub(1));
            let now = Instant::now();
            // Selected tab first, then every other tab exactly once, from a
            // rotating cursor so a backlog drains in order and nothing starves.
            let order = preview_order.clone();
            let mut built = 0usize;
            for i in order {
                let Some(fd) = ov_frames.get(i).and_then(Option::as_ref) else {
                    continue;
                };
                let style = TerminalStyle {
                    tab_id: tab_ids[i], size: (w, h), cell: (self.cell_w, self.cell_h), strip_h,
                    fg: fd.fg, bg: fd.bg, cursor_color: fd.cursor_color,
                    transparency: self.config.term_transparency,
                };
                if gpu.prev_styles[i].as_ref() != Some(&style) {
                    gpu.prev_damage[i] = Damage::full();
                }
                let images = self.image_quads(i, &mut gpu, strip_h, w);
                let selection = self.wins[self.cur].tabs[i].selection_rows();
                gpu.update_overlays(i, gpu::TerminalOverlays {
                    selection: selection.clone(), images: images.clone(), ..Default::default()
                }, self.wins[self.cur].rows, self.cell_h, strip_h);
                if gpu.prev_damage[i].is_empty() { continue; }
                let gate = if i == sel { 60 } else { 200 };
                let throttled = gpu.prev_revision[i] != 0
                    && now.duration_since(gpu.prev_last[i]) < std::time::Duration::from_millis(gate);
                if throttled || built >= 2 { continue; }
                let damage = std::mem::take(&mut gpu.prev_damage[i]);
                gpu.prev_styles[i] = Some(style);
                let metrics = gpu.pool[0].metrics();
                let buffers = &mut gpu.prev_bufs[i];
                let runs: Vec<_> = fd.runs.iter().filter(|run| damage.shapes_row(run.row)).take(MAX_RUNS).collect();
                buffers.resize_with(runs.len(), || Buffer::new(&mut self.font_system, metrics));
                for (buffer, run) in buffers.iter_mut().zip(&runs) {
                    buffer.set_metrics(metrics);
                    fonts::shape_terminal_run(
                        buffer, &mut self.font_system, &run.text, run.attrs, &run.cells, self.cell_w,
                    );
                }
                gpu.prev_last[i] = now;
                jobs.push(gpu::PreviewJob {
                    idx: i,
                    images,
                    scissors: damage.scissors((w, h), self.cell_h, strip_h, self.wins[self.cur].rows),
                    bounds,
                    text: runs.iter().map(|run| gpu::PreviewText {
                        left: run.col as f32 * self.cell_w,
                        top: strip_h + run.row as f32 * self.cell_h,
                        color: run.fg,
                    }).collect(),
                    rects: preview_rects(
                        fd,
                        &fd.bg,
                        &self.config.tablist_background.to_f32(),
                        &selection,
                        self.config.adjust_terminal_color(self.config.selection_color.to_f32()),
                        bg_a,
                        self.cell_w,
                        self.cell_h,
                        strip_h,
                        self.wins[self.cur].rows,
                        w,
                        h,
                    ),
                });
                built += 1;
            }
            // The graveyard panel keeps its previews live through the same
            // batched pipeline; the list is short by construction (limit),
            // and nothing is built while the panel is closed.
            {
                let grave_open = self.wins[self.cur].egui.as_ref()
                    .is_some_and(|e| e.ui_state.panel_is(egui_ui::RightPanel::Graveyard));
                let mut workspace_built = 0;
                for pi in 0..g + ws_preview_tabs.len() {
                    let parked = pi >= g;
                    if !parked && !grave_open { continue; }
                    let i = n + pi;
                    let (tab, fd) = if parked {
                        let (workspace, ti) = ws_preview_tabs[pi - g];
                        (&self.wins[self.cur].workspaces[workspace].tabs[ti], &ws_preview_frames[pi - g])
                    } else {
                        (&self.graveyard[pi].tab, &grave_frames[pi])
                    };
                    let style = TerminalStyle {
                        tab_id: tab_ids[i], size: (w, h), cell: (self.cell_w, self.cell_h), strip_h,
                        fg: fd.fg, bg: fd.bg, cursor_color: fd.cursor_color,
                        transparency: self.config.term_transparency,
                    };
                    if gpu.prev_styles[i].as_ref() != Some(&style) {
                        gpu.prev_damage[i] = Damage::full();
                    }
                    let images = self.image_quads_for(tab, &mut gpu, strip_h, w);
                    gpu.update_overlays(i, gpu::TerminalOverlays {
                        images: images.clone(), ..Default::default()
                    }, self.wins[self.cur].rows, self.cell_h, strip_h);
                    if gpu.prev_damage[i].is_empty() { continue; }
                    let throttled = gpu.prev_revision[i] != 0
                        && now.duration_since(gpu.prev_last[i]) < std::time::Duration::from_millis(200);
                    if parked {
                        if workspace_built >= 2 { continue; }
                    } else if throttled || built >= 2 { continue; }
                    let damage = std::mem::take(&mut gpu.prev_damage[i]);
                    gpu.prev_styles[i] = Some(style);
                    let metrics = gpu.pool[0].metrics();
                    let buffers = &mut gpu.prev_bufs[i];
                    let runs: Vec<_> = fd.runs.iter().filter(|run| damage.shapes_row(run.row)).take(MAX_RUNS).collect();
                    buffers.resize_with(runs.len(), || Buffer::new(&mut self.font_system, metrics));
                    for (buffer, run) in buffers.iter_mut().zip(&runs) {
                        buffer.set_metrics(metrics);
                        fonts::shape_terminal_run(
                            buffer, &mut self.font_system, &run.text, run.attrs, &run.cells, self.cell_w,
                        );
                    }
                    gpu.prev_last[i] = now;
                    jobs.push(gpu::PreviewJob {
                        idx: i,
                        images,
                        scissors: damage.scissors((w, h), self.cell_h, strip_h, self.wins[self.cur].rows),
                        bounds,
                        text: runs.iter().map(|run| gpu::PreviewText {
                            left: run.col as f32 * self.cell_w,
                            top: strip_h + run.row as f32 * self.cell_h,
                            color: run.fg,
                        }).collect(),
                        rects: preview_rects(
                            fd,
                            &fd.bg,
                            &self.config.tablist_background.to_f32(),
                            &[],
                            self.config.adjust_terminal_color(self.config.selection_color.to_f32()),
                            bg_a,
                            self.cell_w,
                            self.cell_h,
                            strip_h,
                            self.wins[self.cur].rows,
                            w,
                            h,
                        ),
                    });
                    if parked { workspace_built += 1; } else { built += 1; }
                }
            }
        }
        // Export and thumbnails sample the very same persistent tab target.
        let shot_at = self.wins[self.cur].shot.then_some(active);
        let generation = self.wins[self.cur].frame_gen;
        // UI effects must keep presenting even without terminal damage.
        let force_present = bell_k > 0.0
            || self.ripple.is_some()
            || self.wins[self.cur].notice.is_some()
            || self.wins[self.cur].overview.is_some()
            || full_output.as_ref().is_some_and(|o| o.viewport_output.get(&egui::ViewportId::ROOT)
                .is_some_and(|v| v.repaint_delay.is_zero()));
        let egui_frame = match (full_output.as_mut(), self.wins[self.cur].egui.as_mut()) {
            (Some(full), Some(egui)) => {
                // Take the shapes out: `full` is behind a reference, and the
                // tessellated jobs supersede them (only the texture deltas
                // are still read from `full`).
                let paint_jobs = egui
                    .ctx
                    .tessellate(std::mem::take(&mut full.shapes), full.pixels_per_point);
                Some(egui_ui::EguiFrame {
                    renderer: &mut egui.renderer,
                    full,
                    paint_jobs,
                    screen: egui_wgpu::ScreenDescriptor {
                        size_in_pixels: [w, h],
                        pixels_per_point: self.scale,
                    },
                })
            },
            _ => None,
        };
        gpu.render_frame(&mut self.font_system, areas, images, jobs, window, egui_frame, active, live_scissors, generation, force_present);

        // Finish the screenshot: read the offscreen frame (already rendered
        // this pass) at full resolution and hand a PNG to the clipboard.
        if let Some(a) = shot_at {
            self.wins[self.cur].shot = false;
            match gpu.readback_preview(a, 0, (w, h)) {
                Some(rgba) => {
                    let png = png_rgba(w, h, &rgba);
                    let png_len = png.len();
                    let result = match self.clipboard.as_ref() {
                        None => Err("no clipboard backend".to_string()),
                        Some(_) if png.is_empty() => Err("PNG encoding failed".to_string()),
                        Some(cb) => cb.set_image(clipboard::Kind::Clipboard, png),
                    };
                    match result {
                        Ok(()) => {
                            log::info!("copy as image: {w}x{h} png {png_len} bytes");
                            self.wins[self.cur].notice =
                                Some(("Screen copied as image".to_string(), Instant::now()));
                        }
                        Err(e) => {
                            log::warn!("copy as image failed: {e}");
                            self.show_error(format!("Copy as image: {e}"));
                        }
                    }
                }
                None => {
                    log::warn!("copy as image: readback failed");
                    self.show_error("Copy as image: capture failed".to_string());
                }
            }
        }

        // Downsample rendered previews on the GPU; egui samples the same
        // persistent textures. No map_async, CPU wait, or re-upload is needed.
        if self.wins[self.cur].egui.is_some() {
            let ids: Vec<_> = self.wins[self.cur].all_tabs().map(|tab| tab.id).collect();
            // Renderer registrations must be freed along with closed/moved tabs.
            let mut thumbs = std::mem::take(&mut self.wins[self.cur].sw_thumbs);
            let renderer = &mut self.wins[self.cur].egui.as_mut().unwrap().renderer;
            thumbs.retain(|thumb| {
                let keep = ids.contains(&thumb.tab_id);
                if !keep { renderer.free_texture(&thumb.handle); }
                keep
            });
            for thumb in &mut thumbs {
                if !tab_ids[..n].contains(&thumb.tab_id) { thumb.revision = 0; }
            }
            let term_h = ((h as f32) - strip_h).max(1.0);
            let (ptw, pth) = egui_ui::thumbnail_size(w as f32 / self.scale.max(1.0), w as f32 / term_h,
                self.config.tab_overview_scale);
            let (tw, th) = ((ptw * self.scale).round().max(1.0) as u32, (pth * self.scale).round().max(1.0) as u32);
            let mut updated = 0;
            let mut thumbnail_order = preview_order.clone();
            if !thumbnail_order.contains(&active) { thumbnail_order.insert(0, active); }
            thumbnail_order.extend((0..ws_preview_tabs.len()).map(|pi| n + g + pi));
            for &i in &thumbnail_order {
                if i < n && updated >= 2 { continue; }
                let revision = gpu.prev_revision.get(i).copied().unwrap_or(0);
                if revision == 0 { continue; }
                let id = tab_ids[i];
                let old = thumbs.iter().position(|t| t.tab_id == id);
                // A temporary parked target can start again at revision one.
                // Its content is fresh even when the old small texture had that revision.
                if i < n && old.is_some_and(|at| thumbs[at].pixels == (tw, th) && thumbs[at].revision == revision) { continue; }
                let old_texture = old.map(|at| &thumbs[at].texture);
                let Some(texture) = gpu.thumbnail(i, strip_h as u32, (tw, th), old_texture) else { continue; };
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                let handle = if let Some(at) = old {
                    let handle = thumbs[at].handle;
                    renderer.update_egui_texture_from_wgpu_texture(&gpu.device, &view, wgpu::FilterMode::Linear, handle);
                    handle
                } else {
                    renderer.register_native_texture(&gpu.device, &view, wgpu::FilterMode::Linear)
                };
                let thumb = SwThumb { tab_id: id, handle, texture, size: (ptw.round() as u32, pth.round() as u32),
                    pixels: (tw, th), revision: if i < n { revision } else { 0 } };
                if let Some(at) = old { thumbs[at] = thumb; } else { thumbs.push(thumb); }
                updated += 1;
            }
            // Changed thumbnails need one UI redraw. Unchanged textures never
            // schedule their own redraw, including completely empty terminals.
            if updated > 0 {
                // A new thumbnail changes the tab strip, so the last presented
                // frame is stale.
                self.wins[self.cur].frame_gen = self.wins[self.cur].frame_gen.wrapping_add(1);
                window.request_redraw();
            }
            self.wins[self.cur].sw_thumbs = thumbs;
        }
        // Buried-tab previews refresh through the same downsample pass, only
        // while the graveyard panel is open (a closed panel has no viewer, and
        // the list is short by construction anyway). Own block: the renderer
        // borrow must not span other windows access.
        if self.wins[self.cur].egui.as_ref()
            .is_some_and(|e| e.ui_state.panel_is(egui_ui::RightPanel::Graveyard))
        {
            let gids: Vec<_> = self.graveyard.iter().map(|g| g.tab.id).collect();
            let mut gthumbs = std::mem::take(&mut self.wins[self.cur].grave_thumbs);
            let term_h = ((h as f32) - strip_h).max(1.0);
            let (ptw, pth) = egui_ui::thumbnail_size(w as f32 / self.scale.max(1.0), w as f32 / term_h,
                self.config.tab_overview_scale);
            let (tw, th) = ((ptw * self.scale).round().max(1.0) as u32, (pth * self.scale).round().max(1.0) as u32);
            let renderer = &mut self.wins[self.cur].egui.as_mut().unwrap().renderer;
            gthumbs.retain(|thumb| {
                let keep = gids.contains(&thumb.tab_id);
                if !keep { renderer.free_texture(&thumb.handle); }
                keep
            });
            let mut gupdated = 0;
            for gi in 0..gids.len() {
                if gupdated >= 2 { break; }
                let i = n + gi;
                let revision = gpu.prev_revision.get(i).copied().unwrap_or(0);
                if revision == 0 { continue; }
                let id = gids[gi];
                let old = gthumbs.iter().position(|t| t.tab_id == id);
                if old.is_some_and(|at| gthumbs[at].pixels == (tw, th) && gthumbs[at].revision == revision) {
                    continue;
                }
                let old_texture = old.map(|at| &gthumbs[at].texture);
                let Some(texture) = gpu.thumbnail(i, strip_h as u32, (tw, th), old_texture) else { continue; };
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                let handle = if let Some(at) = old {
                    let handle = gthumbs[at].handle;
                    renderer.update_egui_texture_from_wgpu_texture(&gpu.device, &view, wgpu::FilterMode::Linear, handle);
                    handle
                } else {
                    renderer.register_native_texture(&gpu.device, &view, wgpu::FilterMode::Linear)
                };
                let thumb = SwThumb { tab_id: id, handle, texture,
                    size: (ptw.round() as u32, pth.round() as u32), pixels: (tw, th), revision };
                if let Some(at) = old { gthumbs[at] = thumb; } else { gthumbs.push(thumb); }
                gupdated += 1;
            }
            if gupdated > 0 {
                self.wins[self.cur].frame_gen = self.wins[self.cur].frame_gen.wrapping_add(1);
                window.request_redraw();
            }
            self.wins[self.cur].grave_thumbs = gthumbs;
        }
        self.wins[self.cur].gpu = Some(gpu);

        if std::env::var("MYPIXELS_TRACE").is_ok() {
            eprintln!(
                "frame build {t_built:?} text {t_text:?} total {:?} grid {}x{} runs {} damage {}",
                t_start.elapsed(),
                self.wins[self.cur].cols,
                self.wins[self.cur].rows,
                frame.runs.len(),
                if live_damage.full { "full".to_string() } else { format!("{} rows", live_damage.lines.len()) },
            );
        }
    }

    // -----------------------------------------------------------------------
    // Actions
    // -----------------------------------------------------------------------

    /// Test hook: drain key specs from the $MYPIXELS_KEYS reader thread and feed
    /// them through the same path as real key events.
    fn drain_test_keys(&mut self, event_loop: &ActiveEventLoop) {
        let rx = match self.key_rx.as_ref() {
            Some(rx) => rx,
            None => return,
        };
        let mut specs = Vec::new();
        while let Ok(line) = rx.try_recv() {
            specs.push(line);
        }
        if specs.is_empty() {
            return;
        }
        for spec in &specs {
            if self.wins.is_empty() { break; }
            // Explicit routing keeps multi-window tests independent of which
            // window happened to receive the most recent compositor event.
            let line = if let Some(rest) = spec.strip_prefix("window:") {
                let Some((number, line)) = rest.split_once(':') else { continue };
                let Some(index) = number.parse::<usize>().ok().and_then(|n| n.checked_sub(1)) else { continue };
                if index >= self.wins.len() { continue; }
                self.cur = index;
                line
            } else { spec.as_str() };
            if let Some(coords) = line.strip_prefix("click:") {
                if let Some((x, y)) = coords.split_once(',').and_then(|(x, y)| Some((x.parse::<f32>().ok()?, y.parse::<f32>().ok()?))) {
                    let pos = egui::pos2(x, y);
                    let win = &mut self.wins[self.cur];
                    let scale = win.egui.as_ref().map_or(1.0, |e| e.ctx.pixels_per_point());
                    win.pointer = (x * scale, y * scale);
                    win.fifo_egui_events.push(egui::Event::PointerMoved(pos));
                    for pressed in [true, false] {
                        win.fifo_egui_events.push(egui::Event::PointerButton {
                            pos, button: egui::PointerButton::Primary, pressed,
                            modifiers: egui::Modifiers::default(),
                        });
                    }
                }
                continue;
            }
            if line == "spy" {
                // Test hook: dump tab state
                let state: Vec<String> = self.wins[self.cur].
                    tabs
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        let title = t.display_title().chars().take(30).collect::<String>();
                        format!("{i}[:{}](g{}e{}){}", t.id, t.group, if t.entangled { 1 } else { 0 }, title)
                    })
                    .collect();
                let cursor = match self.wins[self.cur].tabs.get(self.wins[self.cur].active) {
                    Some(t) => {
                        let c = t.term.lock().renderable_content().cursor;
                        let focus = t.term.lock().is_focused;
                        format!("shape={:?} point=({},{}) focus={}", c.shape, c.point.column.0, c.point.line.0, focus)
                    },
                    None => "-".to_string(),
                };
                let surf = match self.wins[self.cur].gpu.as_ref() {
                    Some(g) => format!("{}x{}", g.config.width, g.config.height),
                    None => "-".to_string(),
                };
                let ren = if self.wins[self.cur].renaming {
                    format!(
                        "renaming={:?}",
                        self.wins[self.cur].egui.as_ref().map(|e| e.ui_state.rename_draft.clone())
                    )
                } else {
                    String::new()
                };
                // The panels are keyboard surfaces now, so a spy that reports
                // only the terminal cannot tell a dropped key from an ignored
                // one. Naming the open panel and its selected row lets a test
                // wait for "the panel is open and the arrow landed" instead of
                // sleeping and hoping.
                let panel = match self.wins[self.cur].egui.as_ref().map(|e| e.ui_state.right_panel) {
                    Some(p) => format!("{p:?}"),
                    None => "none".into(),
                };
                let sel = self.wins[self.cur]
                    .egui
                    .as_ref()
                    .and_then(|e| e.ui_state.ws_sel)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "-".into());
                log::info!("SPY active={} surf={} cursor=[{}] tabs=[{}] {} workspace={} switcher={} panel={} sel={} window={} windows={} menu={}", self.wins[self.cur].active, surf, cursor, state.join(","), ren, self.wins[self.cur].workspace, self.wins[self.cur].overview.is_some(), panel, sel, self.cur + 1, self.wins.len(), self.wins[self.cur].egui.as_ref().is_some_and(|e| e.ui_state.menu_open));
                continue;
            }
            if line == "switcher" {
                // Test hook: open the Alt-held switcher directly (bypass the
                // hold delay, which needs real ModifiersChanged events).
                if self.config.tab_overview_on_alt
                    && self.wins[self.cur].overview.map(|o| o.mode) != Some(OverviewMode::Switcher)
                    && !self.wins[self.cur].tabs.is_empty()
                    && !self.wins[self.cur].renaming
                {
                    self.wins[self.cur].overview = Some(OverviewState {
                        mode: OverviewMode::Switcher,
                        selected: self.wins[self.cur].active,
                        original: self.wins[self.cur].active,
                    });
                    if let Some(e) = self.wins[self.cur].egui.as_ref() {
                        egui_ui::release_focus(&e.ctx);
                    }
                    self.switcher_open.store(true, Ordering::SeqCst);
                }
                continue;
            }
            if line == "shot" {
                // Test hook: fire the context menu's "Copy as image".
                self.do_action("copy_as_image", event_loop);
                continue;
            }
            if line == "history" {
                // Test hook: toggle the command-history panel.
                self.do_action("show_history", event_loop);
                continue;
            }
            if line == "alt_hold" {
                self.modifiers = ModifiersState::ALT;
                if self.config.tab_overview_on_alt {
                    *self.switcher_alt_at.lock().unwrap() = Some(Instant::now());
                }
                continue;
            }
            if line == "alt_up" {
                // Test hook: simulate Alt release (dismiss the switcher).
                *self.switcher_alt_at.lock().unwrap() = None;
                if self.switcher_open.load(Ordering::SeqCst) {
                    self.close_switcher();
                }
                continue;
            }
            // Test hooks: raw pty writes to the active tab. Composing a
            // shell command out of key specs (or uinput) is how a harness
            // races the compositor and the user's own keyboard; these
            // directives speak to the child directly. `line:` appends \n.
            // The FIFO itself only exists when MYPIXELS_KEYS is set.
            if line == "colors" {
                // Test hook: dump every OSC-set entry in the terminal's
                // color table. These override the config palette, so a
                // shell/rc that emits OSC 4/10/11 can silently repaint
                // the theme - this shows exactly who did what.
                let Some(t) = self.wins[self.cur].tabs.get(self.wins[self.cur].active) else {
                    continue;
                };
                let term = t.term.lock();
                let content = term.renderable_content();
                let names = [
                    "fg", "bg", "cursor", "sel_fg", "sel_bg",
                ];
                let mut parts = Vec::new();
                for i in 0..256 {
                    if let Some(c) = &content.colors[i] {
                        parts.push(format!("{i}#{:02x}{:02x}{:02x}", c.r, c.g, c.b));
                    }
                }
                for (k, n) in (256..269).zip(names.into_iter().chain(std::iter::repeat("?"))) {
                    if let Some(c) = &content.colors[k] {
                        parts.push(format!("{k}{n}#{:02x}{:02x}{:02x}", c.r, c.g, c.b));
                    }
                }
                log::info!("COLORS [{}]", parts.join(" "));
                continue;
            }
            if let Some(rest) =
                line.strip_prefix("text:").or_else(|| line.strip_prefix("line:"))
            {
                let nl = line.starts_with("line:");
                if let Some(t) =
                    self.wins[self.cur].tabs.get(self.wins[self.cur].active)
                {
                    let mut bytes = rest.as_bytes().to_vec();
                    if nl {
                        bytes.push(b'\n');
                    }
                    t.write(&bytes);
                }
                continue;
            }
            if let Some((mods, key)) = parse_key_spec(line) {
                self.modifiers = mods;
                // Mirror the real path's egui-first routing: while an egui
                // widget owns the keyboard (rename box, font popup, config
                // fields), the spec goes to egui and never reaches the
                // terminal. winit's KeyEvent cannot be constructed from
                // outside winit (pub(crate) extras), so translate to egui
                // events directly -- the same things egui-winit would send.
                if !self.test_key_to_egui(&key, mods) {
                    self.handle_key(&key, None, event_loop);
                }
            }
        }
        // A spec may have closed the last window; the session is ending.
        if let Some(window) = self.wins.get(self.cur).and_then(|w| w.window.as_ref()) {
            window.request_redraw();
        }
    }

    /// The active terminal's mouse-reporting mode, or None when the terminal
    /// does not want the mouse. Shift is the bypass: while it is held the
    /// mouse always selects instead of reporting (the VTE/Alacritty rule).
    /// Feed a FIFO key spec to egui when egui holds the keyboard. Returns
    /// whether egui claimed it (so the caller skips the app's own handler).
    fn test_key_to_egui(&mut self, key: &Key, mods: ModifiersState) -> bool {
        if self.wins[self.cur].egui.is_none() {
            return false;
        }
        let m = egui_mods(mods);
        let action = self.action_for(key, None, false);
        if !self.egui_owns_keyboard()
            || action.is_some_and(|a| Self::is_panel_action(&a))
        {
            return false;
        }
        match key {
            // egui inserts characters from Text events, exactly as a real
            // IME-driven keypress arrives through egui-winit.
            Key::Character(c) => {
                self.wins[self.cur].fifo_egui_events
                    .push(egui::Event::Text(c.as_ref().to_owned()));
            },
            Key::Named(winit::keyboard::NamedKey::Space) => {
                // A real keyboard delivers a Space as text, not as a bare
                // key code; egui's TextEdit expects the same.
                self.wins[self.cur].fifo_egui_events.push(egui::Event::Text(" ".into()));
            },
            other => {
                if let Some(k) = egui_key_of(other) {
                    let ev = |pressed| egui::Event::Key {
                        key: k,
                        physical_key: None,
                        pressed,
                        repeat: false,
                        modifiers: m,
                    };
                    self.wins[self.cur].fifo_egui_events.push(ev(true));
                    self.wins[self.cur].fifo_egui_events.push(ev(false));
                }
            },
        }
        true
    }

    fn mouse_mode(&self) -> Option<TermMode> {
        if self.modifiers.shift_key() {
            return None;
        }
        let term = self.wins[self.cur].tabs.get(self.wins[self.cur].active)?.term.lock();
        let mode = *term.mode();
        let any = mode.contains(TermMode::MOUSE_REPORT_CLICK)
            || mode.contains(TermMode::MOUSE_DRAG)
            || mode.contains(TermMode::MOUSE_MOTION);
        any.then_some(mode)
    }

    /// Viewport cell under the pointer in protocol coordinates (1-based), or
    /// None over the tab strip or outside the grid.
    fn mouse_cell(&self, px: f32, py: f32) -> Option<(u16, u16)> {
        let strip_h = if self.show_tab_strip() { self.tab_h } else { 0.0 };
        if py < strip_h {
            return None;
        }
        let col = (px / self.cell_w).floor() as u16;
        let row = ((py - strip_h) / self.cell_h).floor() as i32;
        if col >= self.wins[self.cur].cols as u16 || row < 0 || row as usize >= self.wins[self.cur].rows {
            return None;
        }
        Some((col + 1, row as u16 + 1))
    }

    /// Modifier bits for a mouse report (4 shift, 8 alt, 16 ctrl). Shift can
    /// never be set here: it bypasses reporting entirely.
    fn mouse_mods(&self) -> u16 {
        let mut m = 0u16;
        if self.modifiers.alt_key() {
            m |= 8;
        }
        if self.modifiers.control_key() {
            m |= 16;
        }
        m
    }

    fn mouse_button_code(&self, button: MouseButton) -> u16 {
        let base = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            _ => 0,
        };
        base | self.mouse_mods()
    }

    /// Cell under the pointer, in grid coordinates, plus which half of it.
    /// None outside the terminal area: the tab strip is not text.
    /// Visual cell under a pointer position (no scroll-offset adjustment:
    /// url spans are keyed by visual rows, which is also what painting
    /// wants).
    fn visual_cell(&self, px: f32, py: f32) -> Option<(usize, usize)> {
        let strip_h = if self.show_tab_strip() { self.tab_h } else { 0.0 };
        if py < strip_h {
            return None;
        }
        let col = (px / self.cell_w).floor() as usize;
        let row = ((py - strip_h) / self.cell_h).floor();
        if row < 0.0 || col >= self.wins[self.cur].cols || row as usize >= self.wins[self.cur].rows {
            return None;
        }
        Some((row as usize, col))
    }

    /// The URL under a pointer position, rescanning that row through the
    /// hash cache. Clicks use this rather than `url_spans` because the
    /// pointer can move without a frame being drawn between; the click must
    /// answer about the word it physically landed on.
    fn url_at(&mut self, px: f32, py: f32) -> Option<String> {
        let (row, col) = self.visual_cell(px, py)?;
        let spans = self.scan_row_cached(row);
        spans
            .into_iter()
            .find(|s| s.row == row && col >= s.start && col < s.end)
            .map(|s| s.url)
    }

    /// Find the URLs on ONE visual row of the grid.
    ///
    /// Laziness is the whole design: an underline or a click can only ever
    /// concern the row the pointer is on, so only that row is ever scanned -
    /// one row of <= a few hundred cells, with an early exit from the cell
    /// iterator, no allocation per character, and a content hash so an
    /// unchanged row reuses its spans without re-parsing at all. The former
    /// version scanned every visible row into fresh Strings on every frame;
    /// that showed up as O(row_len^2) per row (a `String` of every suffix
    /// was built while hunting the scheme) multiplied by the whole screen
    /// multiplied by every mouse-move-triggered redraw - needlessly rude to
    /// the terminal lock the PTY reader also wants.
    fn scan_row_cached(&mut self, row: usize) -> Vec<UrlSpan> {
        let (hash, text) = self.row_text(row);
        if let Some((r, h, spans)) = &self.url_cache {
            if *r == row && *h == hash {
                return spans.clone(); // tiny: a row holds at most a handful of URLs
            }
        }
        let spans = url_spans_in_row(&text, row);
        self.url_cache = Some((row, hash, spans.clone()));
        spans
    }

    /// The row's visible text plus an FNV-1a hash of it, taken under one
    /// terminal lock. The hash is the cache key; the text is usually
    /// thrown away unexamined because the row has not moved.
    fn row_text(&self, row: usize) -> (u64, Vec<char>) {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let Some(tab) = self.wins[self.cur].tabs.get(self.wins[self.cur].active) else {
            return (hash, Vec::new());
        };
        let term = tab.term.lock();
        let content = term.renderable_content();
        let mut chars: Vec<char> = Vec::new();
        for cell in content.display_iter {
            let crow = cell.point.line.0 + content.display_offset as i32;
            if crow < 0 {
                continue;
            }
            if (crow as usize) < row {
                continue;
            }
            if (crow as usize) > row {
                break; // display_iter walks row by row
            }
            let col = cell.point.column.0;
            while chars.len() < col {
                chars.push(' ');
                hash = (hash ^ b' ' as u64).wrapping_mul(0x100_0000_01b3);
            }
            let raw = cell.c;
            let ch = if raw.is_whitespace() || (raw as u32) < 0x20 { ' ' } else { raw };
            if col == chars.len() {
                chars.push(ch);
            }
            hash = (hash ^ ch as u64).wrapping_mul(0x100_0000_01b3);
        }
        (hash, chars)
    }

    #[allow(dead_code)]
    fn scan_row(&self, row: usize) -> Vec<UrlSpan> {
        let Some(tab) = self.wins[self.cur].tabs.get(self.wins[self.cur].active) else {
            return Vec::new();
        };
        let term = tab.term.lock();
        let content = term.renderable_content();
        let mut chars: Vec<char> = Vec::new();
        for cell in content.display_iter {
            let crow = cell.point.line.0 + content.display_offset as i32;
            if crow < 0 {
                continue;
            }
            if (crow as usize) < row {
                continue;
            }
            if (crow as usize) > row {
                break; // display_iter walks row by row
            }
            let col = cell.point.column.0;
            if col > chars.len() {
                chars.extend(std::iter::repeat(' ').take(col - chars.len()));
            }
            let ch = cell.c;
            if col == chars.len() {
                chars.push(if ch.is_whitespace() || (ch as u32) < 0x20 { ' ' } else { ch });
            }
        }
        url_spans_in_row(&chars, row)
    }

    fn open_url(url: &str) {
        // The portal-first, xdg-standard route; detached so a browser that
        // never forks is not our child to reap.
        match std::process::Command::new("xdg-open")
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => log::info!("opened url: {url}"),
            Err(e) => log::warn!("xdg-open {url}: {e}"),
        }
    }

    fn cell_at(&self, px: f32, py: f32) -> Option<(Point, Side)> {
        let tab = self.wins[self.cur].tabs.get(self.wins[self.cur].active)?;
        let strip_h = if self.show_tab_strip() { self.tab_h } else { 0.0 };
        if py < strip_h {
            return None;
        }
        let col = (px / self.cell_w).floor() as usize;
        let row = ((py - strip_h) / self.cell_h).floor() as i32;
        if col >= self.wins[self.cur].cols || row < 0 || row as usize >= self.wins[self.cur].rows {
            return None;
        }
        let within = px / self.cell_w - (px / self.cell_w).floor();
        let side = if within < 0.5 { Side::Left } else { Side::Right };
        // Selection lines are grid coordinates: line 0 is the top of the
        // unscrolled screen and scrollback above it is negative, so the scroll
        // offset comes off the visual row.
        let off = tab.term.lock().grid().display_offset() as i32;
        Some((Point { line: Line(row - off), column: Column(col) }, side))
    }

    /// How many times this button was pressed in this spot within a double-click
    /// window, capped at three (three means "whole rows").
    fn count_click(&mut self, button: u8, at: (f32, f32)) -> u32 {
        let now = std::time::Instant::now();
        let count = match self.wins[self.cur].click {
            Some((b, c, p, t))
                if b == button
                    && now.duration_since(t) < std::time::Duration::from_millis(400)
                    && (at.0 - p.0).abs() < 6.0
                    && (at.1 - p.1).abs() < 6.0 =>
            {
                c + 1
            }
            _ => 1,
        };
        self.wins[self.cur].click = Some((button, count, at, now));
        count.min(3)
    }

    /// Publish the selection into the primary buffer, the way VTE does: selecting
    /// text *is* copying it, and middle click is the paste.
    fn publish_primary(&mut self) {
        if let Some(text) = self.wins[self.cur].tabs.get(self.wins[self.cur].active).and_then(|t| t.selected_text()) {
            self.copy_text(clipboard::Kind::Primary, text);
        }
    }

    fn show_error(&mut self, error: String) {
        log::warn!("{error}");
        if let Some(win) = self.wins.get_mut(self.cur) {
            win.notice = Some((error, Instant::now()));
            if let Some(window) = &win.window { window.request_redraw(); }
        }
    }

    fn copy_text(&mut self, kind: clipboard::Kind, text: String) {
        let result = self.clipboard.as_ref().ok_or_else(|| "Clipboard unavailable".to_string()).and_then(|c| c.set(kind, text));
        if let Err(e) = result { self.show_error(e); }
    }

    fn request_paste(&mut self, kind: clipboard::Kind) {
        let Some(tab) = self.wins[self.cur].tabs.get(self.wins[self.cur].active) else { return };
        let result = self.clipboard.as_ref().ok_or_else(|| "Clipboard unavailable".to_string()).and_then(|c| c.paste(kind, tab.id));
        if let Err(e) = result { self.show_error(e); }
    }

    /// Turn BELs into escalation where they are unseen: attention knocks at
    /// the compositor and the tab strip pulses that cell in the palette's
    /// red. A bell in the window you are looking at, on the tab you are
    /// reading, stays a bell — the visual flash already says what happened;
    /// escalation is for news nobody is watching. Shell integration hooks
    /// (scripts/shell_integration.*) ring this bell on failed commands,
    /// which is "notify me when the build breaks" without a D-Bus daemon.
    fn drain_bell_pings(&mut self) {
        for w in self.wins.iter_mut() {
            if w.bell_pulse.is_some_and(|(_, at)| at.elapsed().as_secs_f32() > 0.9) {
                w.bell_pulse = None;
                if let Some(win) = w.window.as_ref() {
                    win.request_redraw();
                }
            }
        }
        let pings: Vec<(usize, Instant)> =
            std::mem::take(&mut *self.bell_pings.lock().unwrap());
        for (tab_id, at) in pings {
            for (wi, w) in self.wins.iter_mut().enumerate() {
                if w.all_tabs().any(|t| t.id == tab_id) {
                    let ti = w.tabs.iter().position(|t| t.id == tab_id);
                    let seen = wi == self.cur && ti == Some(w.active) && w.focused;
                    if !seen {
                        w.bell_pulse = ti.map(|ti| (ti, at));
                        if let Some(win) = w.window.as_ref() {
                            win.request_user_attention(Some(
                                winit::window::UserAttentionType::Critical,
                            ));
                            win.request_redraw();
                        }
                    }
                    break;
                }
            }
        }
    }

    fn drain_notifications(&mut self) {
        let requests = std::mem::take(&mut *self.notification_requests.lock().unwrap());
        for raw in requests {
            let tab_id = raw.tab_id;
            let Some((wi, ti)) = self.wins.iter().enumerate().find_map(|(wi, win)| {
                win.tabs.iter().position(|tab| tab.id == tab_id).map(|ti| (wi, ti))
            }) else { continue };
            let win = &mut self.wins[wi];
            let focused = win.active == ti && win.focused;
            let visible = win.active == ti && !win.occluded
                && win.window.as_ref().is_none_or(|w| !w.is_minimized().unwrap_or(false));
            let (command, reply) = self.notification_parser.parse(raw, focused, visible);
            if let Some(reply) = reply { win.tabs[ti].write(reply.as_bytes()); }
            if let Some(command) = command {
                if matches!(command, notifications::Command::Show(_)) && !focused {
                    win.bell_pulse = Some((ti, Instant::now()));
                    win.frame_gen = win.frame_gen.wrapping_add(1);
                    if let Some(window) = &win.window {
                        window.request_user_attention(Some(winit::window::UserAttentionType::Informational));
                        window.request_redraw();
                    }
                }
                self.desktop_notifications.send(command);
            }
        }
        for event in self.desktop_notifications.events() {
            match event {
                notifications::Event::Reply { tab_id, text } => {
                    if let Some(tab) = self.wins.iter().flat_map(|win| &win.tabs).find(|tab| tab.id == tab_id) {
                        tab.write(text.as_bytes());
                    }
                },
                notifications::Event::Activate { tab_id, token } => {
                    if let Some((wi, ti)) = self.wins.iter().enumerate().find_map(|(wi, win)| {
                        win.tabs.iter().position(|tab| tab.id == tab_id).map(|ti| (wi, ti))
                    }) {
                        let current = self.cur;
                        self.cur = wi;
                        self.close_switcher();
                        self.cur = current;
                        let win = &mut self.wins[wi];
                        win.active = ti;
                        win.bell_pulse = None;
                        win.frame_gen = win.frame_gen.wrapping_add(1);
                        if let Some(window) = &win.window {
                            window.set_minimized(false);
                            notifications::activate(window, token.as_deref());
                            window.request_user_attention(None);
                            window.request_redraw();
                        }
                    }
                },
            }
        }
    }

    /// Fold dispatched OSC 133 marks into per-tab command blocks. The hard
    /// part - WHERE a mark sits - was done by the state machine at dispatch
    /// time; this only associates marks into blocks and forgets tabs that
    /// are gone (a mark racing its tab's death is simply irrelevant).
    fn drain_sems(&mut self) {
        let evs: Vec<SemanticMark> =
            std::mem::take(&mut *self.sems_ring.lock().unwrap());
        if !self.config.osc133 {
            return;
        }
        let cap = self.config.history_block_bytes;
        for SemanticMark { tab_id, code, exit, line, row, col, alt, command, output, when } in evs {
            // The tab the mark belongs to, wherever it lives now.
            let tab = match self.wins.iter().flat_map(|w| w.all_tabs())
                .chain(self.graveyard.iter().map(|g| &g.tab))
                .chain(self.closed_windows.iter().flat_map(|w| w.tabs()))
                .find(|t| t.id == tab_id)
            {
                Some(t) => t,
                None => continue,
            };
            let blocks = self.sem_blocks.entry(tab_id).or_default();
            // D closes the newest open block; note which one BEFORE the
            // fold, so the capture below can attribute the text even
            // though the fold is what flips `done`.
            let closing = if code == 3 {
                blocks.iter().rposition(|b| !b.done)
            } else {
                None
            };
            if code == 0 {
                self.sem_seq += 1;
            }
            apply_sem_mark(blocks, code, exit, line, row, alt, self.sem_seq);
            if code == 2 {
                if let Some(b) = blocks.iter_mut().rev().find(|b| !b.done) {
                    b.started_at.get_or_insert(when);
                    if let Some(command) = command {
                        b.cmd_line = command;
                        b.command_captured = true;
                    }
                }
            }
            if code != 3 {
                continue;
            }
            let Some(i) = closing else { continue };
            let Some(b) = blocks.get_mut(i) else { continue };
            if !b.done {
                continue; // a stray second D: the fold ignored it
            }
            b.finished_at = Some(when);
            if alt || b.alt_seen {
                // A full-screen program owned the rows this command
                // "wrote". There is no output to capture; say so.
                b.alt_seen = true;
                continue;
            }
            if b.executing {
                if let Some(output) = output {
                    b.truncated_head |= output.truncated_head;
                    push_block_text(b, &output.text, cap);
                }
                finish_block(b);
                continue;
            }
            // Compatibility for old integrations with only A/D markers:
            // without C, the output boundary cannot be separated from input.
            // Translate the block's row IDs into TODAY'S buffer rows:
            // buffer row 0 is whichever row the ring's oldest survivor
            // is, `scrolled_out - history_size` rows past the start of
            // time. This translation is exact even after eviction,
            // which is the whole reason marks carry `row` at all.
            let (hist, prod) = tab.grid_metrics();
            let base = prod.saturating_sub(hist);
            let from = b.start_global.saturating_sub(base);
            // D's cursor line is the fresh row the next prompt will
            // paint: the output is above it. +1 includes a final
            // partial line; max() keeps at least the prompt row itself.
            // D's cursor column says whether the command's last line
            // ended with a newline: col 0 means the cursor sits on a
            // fresh row the output never touched, so that row (which
            // the next prompt will paint, and HAS painted by the time
            // we capture) must stay out. col > 0 means output shares
            // the row - include it, prompt smear and all; there is no
            // way to split a row the terminal already merged.
            let to = if col > 0 { row + 1 } else { row }.saturating_sub(base).max(from + 1);
            if let Some((txt, readable, _)) = tab.capture_rows(from, to) {
                b.truncated_head |= b.start_global < base || readable > from;
                push_block_text(b, &txt, cap);
            }
            finish_block(b);
        }
        let budget = self.config.history_bytes;
        evict_to_budget(&mut self.sem_blocks, budget);
    }

    /// The last `n` commands' captured output of `window`'s active tab
    /// (stable session id; 0 or unknown = the window the user is looking at).
    /// Shared by the session bus's `mypixels last` and the MCP endpoint:
    /// one extraction core, thin transports.
    fn last_output_text(&mut self, window: u32, n: u64) -> String {
        self.drain_sems();
        let win = self.wins.iter().position(|w| w.session_id == window).unwrap_or(self.cur);
        self.wins
            .get(win)
            .and_then(|w| w.tabs.get(w.active))
            .and_then(|t| self.sem_blocks.get_mut(&t.id))
            .map(|q| format_last_output(q, n.max(1) as usize))
            .unwrap_or_default()
    }

    /// The history panel's rows for one tab: newest first, every field
    /// precomputed. A closed block's command line came from its C mark
    /// at dispatch; an idle prompt is kept out of the command list.
    fn history_rows_for(&self, tab_id: usize) -> Vec<egui_ui::HistoryRow> {
        self.sem_blocks.get(&tab_id).map(|q| {
            q.iter().rev().filter(|b| b.is_history_command()).map(|b| {
                let stamp = b.finished_at.or(b.started_at);
                let time = stamp
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| local_hms(d.as_secs() as i64))
                    .unwrap_or_default();
                let duration = match (b.started_at, b.finished_at) {
                    (Some(s), Some(f)) => f
                        .duration_since(s)
                        .ok()
                        .map(format_duration)
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                let cmd = if b.cmd_line.is_empty() && !b.executing {
                    b.text.lines().next().unwrap_or_default().to_string()
                } else {
                    b.cmd_line.clone()
                };
                egui_ui::HistoryRow {
                    seq: b.seq,
                    cmd,
                    exit: b.exit,
                    done: b.done,
                    alt_seen: b.alt_seen,
                    lines: b.line_count,
                    time,
                    duration,
                }
            }).collect()
        }).unwrap_or_default()
    }

    /// What the history panel's action buttons asked for. `seq` identifies
    /// the block; the panel's own dismissal is not an action. The block's
    /// data is copied out first: the actions need `&mut self` (clipboard,
    /// scroll), which cannot coexist with a live borrow of the pool.
    fn do_history_action(&mut self, seq: u64, action: &str) {
        let Some(tab) = self.wins[self.cur].tabs.get(self.wins[self.cur].active) else {
            return;
        };
        let tab_id = tab.id;
        let (prompt_line, cmd_line, text) = {
            let Some(b) = self
                .sem_blocks
                .get_mut(&tab_id)
                .and_then(|q| q.iter_mut().find(|b| b.seq == seq))
            else {
                // Evicted while the panel was open: say so, do not guess.
                self.show_error("That command left the output pool".into());
                return;
            };
            (b.prompt_line, b.cmd_line.clone(), b.readable_text().to_string())
        };
        match action {
            "copy" => {
                if text.is_empty() {
                    self.show_error("No captured output to copy".into());
                } else {
                    self.copy_text(clipboard::Kind::Clipboard, text);
                    self.wins[self.cur].notice =
                        Some(("Command output copied".to_string(), Instant::now()));
                }
            }
            "rerun" => {
                if cmd_line.is_empty() {
                    self.show_error("No command line to re-run".into());
                } else {
                    let mut bytes = cmd_line.as_bytes().to_vec();
                    bytes.push(b'\r');
                    tab.write(&bytes);
                }
            }
            // The same viewport math as `jump_command`: the offset that
            // puts the prompt line at the top is `history_size - line`.
            "view" => {
                let (hist, cur) = {
                    let term = tab.term.lock();
                    (term.grid().history_size(), term.grid().display_offset())
                };
                let want = hist.saturating_sub(prompt_line);
                let delta = want as i32 - cur as i32;
                if delta != 0 {
                    tab.scroll(alacritty_terminal::grid::Scroll::Delta(delta));
                    if let Some(w) = &self.wins[self.cur].window {
                        w.request_redraw();
                    }
                }
            }
            // The panel draws it disabled; a stray one is a no-op.
            _ => {}
        }
    }

    /// Scroll so the viewport top lands on the previous/next command's
    /// prompt. Positions are absolute logical lines; the viewport offset
    /// that shows one is `history_size - line`, clamped by the grid.
    fn jump_command(&mut self, back: bool) {
        self.drain_sems();
        let Some(tab) = self.wins[self.cur].tabs.get(self.wins[self.cur].active) else {
            return;
        };
        let blocks: Vec<usize> = self
            .sem_blocks
            .get(&tab.id)
            .map(|b| b.iter().map(|b| b.prompt_line).collect())
            .unwrap_or_default();
        if blocks.is_empty() {
            return;
        }
        let (hist, cur) = {
            let term = tab.term.lock();
            (term.grid().history_size(), term.grid().display_offset())
        };
        let cur_top = hist - cur;
        let target = if back {
            blocks.iter().filter(|l| **l < cur_top).max()
        } else {
            blocks.iter().filter(|l| **l > cur_top).min()
        };
        log::debug!("jump back={back} blocks={} hist={hist} cur={cur} target={target:?}", blocks.len());
        if let Some(t) = target {
            let want = hist.saturating_sub(*t);
            log::debug!("jump -> offset {want} (delta {})", want as i32 - cur as i32);
            tab.scroll(alacritty_terminal::grid::Scroll::Delta(
                want as i32 - cur as i32,
            ));
            if let Some(w) = &self.wins[self.cur].window {
                w.request_redraw();
            }
        }
    }

    /// Hand complete APC strings to the per-tab Kitty state machine.
    /// Decoding happens here, once, on the main thread: bounded, unit
    /// tested, and never under the terminal lock.
    fn drain_apcs(&mut self) {
        let evs: Vec<(usize, Vec<u8>, usize, usize)> =
            std::mem::take(&mut *self.apcs_ring.lock().unwrap());
        for (tab_id, payload, line, col) in evs {
            if let Some(path) = &self.apc_dump.clone() {
                use std::io::Write as _;
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                    let _ = writeln!(f, "APCFRAME {tab_id} {line} {col} {}", payload.len());
                    let _ = f.write_all(&payload);
                }
            }
            let live = self.all_tabs().any(|t| t.id == tab_id);
            if !live {
                continue;
            }
            log::debug!("apc drained for tab {tab_id}: {} bytes at {line}:{col}", payload.len());
            let reply = self.kitty.entry(tab_id).or_default().on_apc(&payload, line, col);
            if !reply.is_empty() {
                if let Some(tab) = self.all_tabs().find(|t| t.id == tab_id)
                {
                    tab.write(&reply);
                }
            }
        }
    }

    /// Encode a key RELEASE for the active tab. Only the enhanced protocol
    /// can carry a release; the legacy path simply has nothing to say, and
    /// "send nothing" is decided explicitly here rather than by an encoder
    /// accident. A release never touches selection, scroll or activity.
    fn send_key_release(&mut self, ke: &winit::event::KeyEvent) {
        let Some(tab) = self.wins[self.cur].tabs.get(self.wins[self.cur].active) else {
            return;
        };
        let flags = kitty_kb::active_flags(tab.term.lock().mode());
        if flags == 0 {
            return;
        }
        if let Some(bytes) = kitty_kb::encode(&kitty_kb::Input {
            key: &ke.logical_key,
            text: ke.text.as_deref(),
            physical: ke.physical_key,
            location: ke.location,
            release: true,
            repeat: false,
            mods: self.modifiers,
            flags,
        }) {
            if !bytes.is_empty() {
                tab.write(&bytes);
            }
        }
    }

    /// Deliver the replies the terminal queued for its programs (keyboard
    /// mode queries, cursor position reports). NOT entangled: a response
    /// belongs to the tab that was asked, not to every mirrored shell.
    fn drain_ptwrites(&mut self) {
        let evs: Vec<(usize, String)> =
            std::mem::take(&mut *self.pty_writes_ring.lock().unwrap());
        for (tab_id, text) in evs {
            if let Some(tab) = self.all_tabs().find(|t| t.id == tab_id)
            {
                tab.write(text.as_bytes());
            }
        }
    }

    /// A tab executed RIS: drop its whole graphics store and the GPU
    /// textures that came from it, everywhere they were painted.
    fn drain_resets(&mut self) {
        let ids: Vec<usize> = std::mem::take(&mut *self.resets_ring.lock().unwrap());
        if ids.is_empty() {
            return;
        }
        for id in ids {
            if let Some(k) = self.kitty.get_mut(&id) {
                k.clear_all();
            }
            self.forget_gpu_images(id);
        }
        for w in self.wins.iter_mut() {
            if let Some(win) = w.window.as_ref() {
                win.request_redraw();
            }
        }
    }

    /// CSI 16 t answered from live font metrics: `CSI 6 ; h ; w t`. This is
    /// how broot's kitty renderer (and sixel detectors) learn to scale
    /// pixels to cells - without it they refuse to draw anything.
    fn drain_cell_sizes(&mut self) {
        let ids: Vec<usize> = std::mem::take(&mut *self.cell_sizes_ring.lock().unwrap());
        if ids.is_empty() {
            return;
        }
        let reply = format!(
            "\x1b[6;{};{}t",
            self.cell_h.round() as u32,
            self.cell_w.round() as u32
        );
        for tab in self.all_tabs().filter(|tab| ids.contains(&tab.id)) {
            tab.write(reply.as_bytes());
        }
    }

    fn drain_clipboard(&mut self) {
        let events: Vec<_> = self.clipboard.as_ref().map(|c| c.events().collect()).unwrap_or_default();
        for event in events {
            match event {
                clipboard::Event::Paste { target, result: Ok(text) } => {
                    if let Some(tab) = self.wins.iter().flat_map(|w| w.all_tabs()).find(|t| t.id == target) { tab.paste(&text); }
                }
                clipboard::Event::Paste { result: Err(e), .. } | clipboard::Event::Error(e) => self.show_error(e),
            }
        }
    }

    fn handle_key(&mut self, key: &Key, kev: Option<&winit::event::KeyEvent>, event_loop: &ActiveEventLoop) {
        if self.wins[self.cur].egui.as_ref().is_some_and(|e| e.ui_state.confirm_open) { return; }
        if self.wins[self.cur].drag.take().is_some() { self.publish_primary(); }
        // A key release is not an action, never scrolls, never selects. Its
        // only legitimate use is the Kitty keyboard protocol, where apps
        // asked for release events; everything else about a release is the
        // same key's press having already happened.
        if let Some(ke) = kev {
            if ke.state == winit::event::ElementState::Released {
                self.send_key_release(ke);
                return;
            }
        }
        // 0. The egui rename box owns the keyboard: egui consumes the keys
        //    it needs, and anything left over is swallowed here so a half-
        //    typed name can never leak into the shell. Escape is the app's
        //    own cancel, in case egui already had the focus dropped.
        if self.wins[self.cur].renaming {
            if matches!(key, Key::Named(winit::keyboard::NamedKey::Escape)) {
                self.close_rename();
            }
            return;
        }
        // 0.4. A keybinding row is waiting for a chord. The recorder takes the
        //      key first: neither the shell nor the action table may act on
        //      the very keystroke the user is capturing.
        if self.capture_binding(key, kev) {
            return;
        }
        // 0.5. A transient egui picker (context menu, window picker) owns
        //      the keyboard the same way the rename box does: Escape closes
        //      it (through egui's own dismissal), and no key may also reach
        //      the shell underneath - the shell must not act on the very
        //      keystroke that dismisses a menu.
        if self.wins[self.cur].egui.as_ref().is_some_and(|e| e.ui_state.menu_open || e.ui_state.move_open || e.ui_state.info_page.is_some() || egui::Popup::is_any_open(&e.ctx)) {
            return;
        }
        // 1. The switcher claims only its own keys (arrows, Home/End, digits
        //    and Esc with plain Alt); anything else falls through to the
        //    normal handler, so configured bindings and the terminal keep
        //    working while the switcher is open.
        if self.wins[self.cur].overview.as_ref().map(|o| o.mode) == Some(OverviewMode::Switcher) {
            if self.handle_switcher_key(key) {
                return;
            }
        }
        // 2. Configured bindings
        let overview_active = self.wins[self.cur].overview.is_some();
        if let Some(action) = self.action_for(key, kev, overview_active) {
            self.do_action(&action, event_loop);
            // The action may have closed the last window; nothing left to
            // sync. A surviving one may have lost its active tab; keep the
            // highlighted tile in sync while the switcher is open.
            if let Some(win) = self.wins.get_mut(self.cur) {
                let cur_active = win.active;
                if let Some(ov) = win.overview.as_mut() {
                    ov.selected = cur_active;
                }
            }
            return;
        }
        // 3. Everything else goes to the active terminal
        let (app_cursor, kb_flags) = match self.wins[self.cur].tabs.get(self.wins[self.cur].active) {
            Some(t) => {
                let term = t.term.lock();
                let m = *term.mode();
                (
                    m.contains(alacritty_terminal::term::TermMode::APP_CURSOR),
                    kitty_kb::active_flags(&m),
                )
            },
            None => (false, 0),
        };
        let bytes = if kb_flags != 0 {
            match kev {
                // Enhancement flags are active: the enhanced encoder owns
                // this key. `None` from it means "nothing new to say" and
                // the legacy table (byte-identical for these keys) speaks.
                Some(ke) => kitty_kb::encode(&kitty_kb::Input {
                    key,
                    text: ke.text.as_deref(),
                    physical: ke.physical_key,
                    location: ke.location,
                    release: false,
                    repeat: ke.repeat,
                    mods: self.modifiers,
                    flags: kb_flags,
                })
                .or_else(|| key_bytes(key, self.modifiers, app_cursor)),
                None => key_bytes(key, self.modifiers, app_cursor),
            }
        } else {
            key_bytes(key, self.modifiers, app_cursor)
        };
        if let Some(bytes) = bytes.filter(|b| !b.is_empty()) {
            // Typing counts as activity: keep the cursor solid (no blink)
            // right after a keypress.
            *self.last_activity.lock().unwrap() = std::time::Instant::now();
            // A selection is something you are about to act on; typing means you
            // moved on, and leaving the wash over new output would be a lie.
            let cur_active = self.wins[self.cur].active;
            if let Some(tab) = self.wins[self.cur].tabs.get_mut(cur_active) {
                tab.clear_selection();
                tab.scroll(alacritty_terminal::grid::Scroll::Bottom);
            }
            if let Some(tab) = self.wins[self.cur].tabs.get(self.wins[self.cur].active) {
                // Entanglement: type into all entangled tabs
                if self.wins[self.cur].tabs[self.wins[self.cur].active].entangled {
                    for t in &self.wins[self.cur].tabs {
                        if t.entangled {
                            t.write(&bytes);
                        }
                    }
                } else {
                    tab.write(&bytes);
                }
            }
        }
    }

    /// Whether an egui surface is open that the user could plausibly be
    /// typing into right now. Keys belong to egui only under that
    /// condition; anything else belongs to the terminal or the bindings.
    fn egui_owns_keyboard(&self) -> bool {
        // The switcher owns navigation even when a sidebar was open before it.
        if self.wins[self.cur].overview.is_some() { return false; }
        let Some(e) = self.wins[self.cur].egui.as_ref() else {
            return false;
        };
        e.ui_state.owns_keyboard(&e.ctx)
    }

    /// The Alt-held switcher's state machine lives in the app (its arrows
    /// drive `prev_tab`/`next_tab`), so while it is open it owns the keys
    /// outright and nothing keyboard-related may go to egui.
    fn switcher_open_here(&self) -> bool {
        matches!(
            self.wins[self.cur].overview.as_ref().map(|o| o.mode),
            Some(OverviewMode::Switcher)
        )
    }

    /// Actions that belong to the app rather than to egui even while a panel
    /// has the keyboard: they open, close or steer the panels themselves, and
    /// the workspace chords have to keep switching with the panel open.
    fn is_panel_action(action: &str) -> bool {
        matches!(action, "show_config" | "show_history" | "graveyard" | "show_workspaces")
            || action.starts_with("goto_workspace_")
    }

    /// True while one of the list panels is the ONLY keyboard surface (menus,
    /// the rename box, the switcher and editors keep full ownership when
    /// they are open).
    fn sole_panel_owner(&self, panel: egui_ui::RightPanel) -> bool {
        !self.switcher_open_here()
            && self.wins[self.cur].egui.as_ref().is_some_and(|e| {
            let st = &e.ui_state;
            st.right_panel == panel
                && !st.rename_open && !st.confirm_open && !st.menu_open && !st.move_open
                && st.info_page.is_none() && !egui::Popup::is_any_open(&e.ctx)
        })
    }

    fn grave_only_owner(&self) -> bool {
        self.sole_panel_owner(egui_ui::RightPanel::Graveyard)
    }

    fn ws_only_owner(&self) -> bool {
        self.sole_panel_owner(egui_ui::RightPanel::Workspaces)
    }

    /// While the panel is the only surface, just its own keys reach egui;
    /// a command typed with the panel open must still land in the terminal.
    fn graveyard_passes_key(&self, key: &Key) -> bool {
        !self.grave_only_owner() || self.grave_wants_key(key)
    }

    /// As `graveyard_passes_key` for the workspace panel.
    fn workspaces_passes_key(&self, key: &Key) -> bool {
        !self.ws_only_owner() || self.ws_wants_key(key)
    }

    /// The panel is the only surface and this key is one of its own: it is
    /// queued to egui and acted on during the next draw, so it must not also
    /// reach the terminal (the egui wrapper does not report plain keys as
    /// consumed, which would leak them).
    fn graveyard_claims_key(&self, key: &Key) -> bool {
        self.grave_only_owner() && self.grave_wants_key(key)
            || self.ws_only_owner() && self.ws_wants_key(key)
    }

    /// The workspace panel's own keys. Its arrows and Enter are always its
    /// business: with the panel open they mean "move the check mark" and
    /// "go there", and a workspace switch is reversible with the same key.
    fn ws_wants_key(&self, key: &Key) -> bool {
        // Explicit search focus owns typing; workspace shortcuts stay global.
        let editing = self.wins[self.cur].egui.as_ref().is_some_and(|e| {
            e.ui_state.ws_search_open && e.ctx.memory(|m| m.focused())
                .is_some_and(|id| egui::text_edit::TextEditState::load(&e.ctx, id).is_some())
        });
        if editing {
            return !Self::is_panel_action(self.action_for(key, None, false).as_deref().unwrap_or_default());
        }
        Self::is_grave_nav_key(key) || matches!(key, Key::Named(NamedKey::Enter))
            || (self.modifiers.control_key() && matches!(key, Key::Character(c) if c.eq_ignore_ascii_case("f")))
    }

    /// The panel's own keys. Navigation is always its; the commit keys
    /// (Enter raises, Delete kills for good) are its only while a row is
    /// selected, so a bare Enter or Delete with the panel open stays the
    /// terminal's.
    fn grave_wants_key(&self, key: &Key) -> bool {
        match key {
            Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Delete) => {
                self.wins[self.cur].egui
                    .as_ref()
                    .is_some_and(|e| e.ui_state.grave_sel.is_some())
            }
            _ => Self::is_grave_nav_key(key),
        }
    }

    fn is_grave_nav_key(key: &Key) -> bool {
        matches!(key,
            Key::Named(NamedKey::ArrowUp) | Key::Named(NamedKey::ArrowDown)
            | Key::Named(NamedKey::ArrowLeft) | Key::Named(NamedKey::ArrowRight)
            | Key::Named(NamedKey::Escape)
            | Key::Named(NamedKey::Home) | Key::Named(NamedKey::End))
    }

    /// Close the rename box without committing (app-side Escape).
    fn close_rename(&mut self) {
        self.wins[self.cur].renaming = false;
        if let Some(egui) = self.wins[self.cur].egui.as_mut() {
            egui.ui_state.rename_open = false;
            egui_ui::release_focus(&egui.ctx);
        }
        if let Some(w) = &self.wins[self.cur].window {
            w.request_redraw();
        }
    }

    /// Dismiss the Alt-held switcher (only if that variant is open).
    fn close_switcher(&mut self) {
        if self.wins[self.cur].overview.as_ref().map(|o| o.mode) == Some(OverviewMode::Switcher) {
            self.wins[self.cur].overview = None;
            self.switcher_open.store(false, Ordering::SeqCst);
        }
        if let Some(w) = &self.wins[self.cur].window {
            w.request_redraw();
        }
    }

    /// Keys while the Alt-held switcher is open. Returns whether the key is
    /// one of the switcher's own bindings and was consumed. The switcher's
    /// bindings use Alt with arrows, Home/End, digits, Tab, Enter, or Esc.
    /// Shift-Tab cycles backwards. Other chords fall through to the normal
    /// handler (configured bindings, then the terminal). Navigation changes
    /// the active tab live, so the preview and the eventual commit are the
    /// same tab; Alt release just dismisses the popup.
    fn handle_switcher_key(&mut self, key: &Key) -> bool {
        use winit::keyboard::NamedKey;
        let alt_navigation = self.modifiers.alt_key()
            && !self.modifiers.control_key()
            && !self.modifiers.super_key()
            && (!self.modifiers.shift_key() || matches!(key, Key::Named(NamedKey::Tab)));
        if !alt_navigation {
            return false;
        }
        let n = self.wins[self.cur].tabs.len();
        if n == 0 {
            self.close_switcher();
            return true;
        }
        let (screen_w, screen_h) = self.wins[self.cur].window.as_ref()
            .map(|w| (w.inner_size().width as f32 / self.scale.max(1.0),
                      w.inner_size().height as f32 / self.scale.max(1.0)))
            .unwrap_or((1280.0, 800.0));
        // The drawing side sizes the cards from the same inputs, so the
        // arrow keys and the grid agree on the column count.
        let strip_h = self.show_tab_strip().then(|| self.tab_h).unwrap_or(0.0);
        let card_w = egui_ui::switcher_card_width(
            screen_w,
            screen_w / (screen_h - strip_h).max(1.0),
            self.config.tab_overview_scale,
        );
        let cols = egui_ui::switcher_columns(screen_w, n, card_w);
        let handled = match key {
            Key::Named(NamedKey::Tab) => {
                let step = if self.modifiers.shift_key() { n - 1 } else { 1 };
                self.wins[self.cur].active = (self.wins[self.cur].active + step) % n;
                true
            },
            Key::Named(NamedKey::Enter) => {
                self.close_switcher();
                true
            },
            Key::Named(NamedKey::ArrowLeft) => {
                self.wins[self.cur].active = (self.wins[self.cur].active + n - 1) % n;
                true
            },
            Key::Named(NamedKey::ArrowRight) => {
                self.wins[self.cur].active = (self.wins[self.cur].active + 1) % n;
                true
            },
            Key::Named(NamedKey::ArrowUp) => {
                let r = self.wins[self.cur].active / cols;
                if r > 0 {
                    let dc = self.wins[self.cur].active % cols;
                    self.wins[self.cur].active = ((r - 1) * cols + dc).min(n - 1);
                }
                true
            },
            Key::Named(NamedKey::ArrowDown) => {
                let r = self.wins[self.cur].active / cols;
                let t = (r + 1) * cols + (self.wins[self.cur].active % cols);
                if t < n && t / cols > r {
                    self.wins[self.cur].active = t;
                }
                true
            },
            Key::Named(NamedKey::Home) => {
                self.wins[self.cur].active = 0;
                true
            },
            Key::Named(NamedKey::End) => {
                self.wins[self.cur].active = n - 1;
                true
            },
            Key::Named(NamedKey::Escape) => {
                if let Some(overlay) = self.wins[self.cur].overview {
                    self.wins[self.cur].active = overlay.original.min(n - 1);
                }
                self.close_switcher();
                true
            },
            Key::Character(c) => {
                // Alt+digit: jump to that tab (Alt is held in the switcher).
                match c.chars().next().and_then(|ch| ch.to_digit(10)) {
                    Some(d) => {
                        let idx = if d == 0 { 9 } else { (d - 1) as usize };
                        self.goto_tab(idx);
                        true
                    },
                    None => false,
                }
            },
            _ => false,
        };
        if handled {
            // Keep the highlighted tile in sync with the active tab.
            let cur_active = self.wins[self.cur].active;
            if let Some(ov) = self.wins[self.cur].overview.as_mut() {
                ov.selected = cur_active;
            }
            if let Some(w) = &self.wins[self.cur].window {
                w.request_redraw();
            }
        }
        handled
    }

    fn select_window(&mut self, id: WindowId) -> bool {
        if let Some(index) = self.wins.iter().position(|w| w.window.as_ref().is_some_and(|w| w.id() == id)) {
            self.cur = index;
            true
        } else { false }
    }

    /// Menu results belong to the window that queued them, even when another
    /// window's redraw is the first opportunity to execute the deferred work.
    fn drain_window_actions(&mut self, event_loop: &ActiveEventLoop) {
        if let Some((window, choice)) = self.pending_close.take()
            && self.select_window(window) {
            match choice {
                egui_ui::CloseChoice::Cancel => {}
                egui_ui::CloseChoice::Graveyard => {
                    if self.window_retainer() == egui_ui::WindowRetainer::None {
                        self.open_close_dialog();
                    } else {
                        self.close_window(self.cur, event_loop);
                    }
                }
                egui_ui::CloseChoice::Permanent => {
                    let ready = self.wins[self.cur].egui.as_ref().is_some_and(|e|
                        e.ui_state.confirm.permanent_ready(e.ctx.input(|i| i.time)));
                    if ready { self.destroy_window(event_loop); }
                    else { self.open_close_dialog(); }
                }
            }
        }
        if let Some((window, action)) = self.pending_action.take() {
            if self.select_window(window) { self.do_action(&action, event_loop); }
        }
        if let Some((window, target)) = self.pending_move.take() {
            if !self.select_window(window) { return; }
            let from = self.cur;
            let idx = self.wins[from].active;
            if target == usize::MAX {
                self.make_window(event_loop, None, None, false);
                let to = self.wins.len() - 1;
                self.move_tab(from, idx, to, 0, event_loop);
            } else if target < self.wins.len() && target != from {
                let insert = self.wins[target].tabs.len();
                self.move_tab(from, idx, target, insert, event_loop);
            }
        }
    }

    fn do_action(&mut self, action: &str, event_loop: &ActiveEventLoop) {
        let n = self.wins[self.cur].tabs.len();
        match action {
            "reopen_window" => {
                if !self.closed_windows.is_empty() {
                    self.revive_window(self.closed_windows.len() - 1, event_loop);
                }
            }
            a if a.starts_with("reopen_window_") || a.starts_with("kill_window_") => {
                if let Some(id) = a.rsplit('_').next().and_then(|id| id.parse::<usize>().ok())
                    && let Some(index) = self.closed_windows.iter().position(|w| w.id == id) {
                    if a.starts_with("reopen_") { self.revive_window(index, event_loop); }
                    else { self.kill_closed_window(index); }
                }
            }
            a if a.starts_with("workspace_tab_") => {
                if let Some((workspace, id)) = a["workspace_tab_".len()..].split_once('_')
                    && let (Ok(workspace), Ok(id)) = (workspace.parse::<usize>(), id.parse::<usize>())
                    && self.config.workspaces_enabled && workspace < WORKSPACES {
                    self.switch_workspace(workspace);
                    let win = &mut self.wins[self.cur];
                    if let Some(index) = win.tabs.iter().position(|tab| tab.id == id) {
                        win.active = index;
                    }
                }
            }
            a if a.starts_with("goto_workspace_") => {
                if let Ok(target) = a["goto_workspace_".len()..].parse::<usize>() {
                    self.switch_workspace(target);
                }
            }
            // The picker's pick, drained after the frame that showed it.
            a if a.starts_with("move_to_workspace_") => {
                if let Ok(target) = a["move_to_workspace_".len()..].parse::<usize>() {
                    self.wins[self.cur].move_tab_to_workspace(target);
                }
            }
            "new_tab" => self.add_tab(),
            "new_window" => self.make_window(event_loop, None, None, true),
            "send_tab_to_window" => {
                if self.wins.len() < 2 {
                    self.wins[self.cur].notice = Some((
                        "No other window -- Ctrl+Shift+n opens one".to_string(),
                        Instant::now(),
                    ));
                } else if self.wins.len() == 2 {
                    // Exactly two: "to the other one" needs no dialog.
                    let from = self.cur;
                    let to = 1 - from;
                    let idx = self.wins[from].active;
                    let insert = self.wins[to].tabs.len();
                    self.move_tab(from, idx, to, insert, event_loop);
                } else {
                    let mut items = Vec::new();
                    let mut targets = Vec::new();
                    for (i, w) in self.wins.iter().enumerate() {
                        if i == self.cur {
                            continue;
                        }
                        let title = w
                            .tabs
                            .get(w.active)
                            .map(|t| t.display_title())
                            .unwrap_or_else(|| "empty".to_string());
                        items.push(format!("Window {} -- {} ({} tabs)", i + 1, title, w.tabs.len()));
                        targets.push(i);
                    }
                    items.push("New window".to_string());
                    targets.push(usize::MAX);
                    if let Some(egui) = self.wins[self.cur].egui.as_mut() {
                        let st = &mut egui.ui_state;
                        st.move_items = items;
                        st.move_targets = targets;
                        st.move_pos = [12.0, self.tab_h / egui.ctx.pixels_per_point() + 6.0];
                        st.move_title = String::new();
                        st.move_workspace = false;
                        st.move_open = true;
                        st.move_out = None;
                    }
                }
            }
            // The same picker, pointed at this window's other workspaces.
            "move_tab_to_workspace" => {
                if !self.config.workspaces_enabled {
                    return;
                }
                let current = self.wins[self.cur].workspace;
                let mut items = Vec::new();
                let mut targets = Vec::new();
                for (i, w) in self.wins[self.cur].workspaces.iter().enumerate() {
                    if i == current {
                        continue;
                    }
                    let title = w
                        .tabs
                        .get(w.active)
                        .map(|t| t.display_title())
                        .unwrap_or_else(|| "empty".to_string());
                    items.push(format!("Workspace {} -- {} ({} tabs)", i + 1, title, w.tabs.len()));
                    targets.push(i);
                }
                if let Some(egui) = self.wins[self.cur].egui.as_mut() {
                    let st = &mut egui.ui_state;
                    st.move_items = items;
                    st.move_targets = targets;
                    st.move_pos = [12.0, self.tab_h / egui.ctx.pixels_per_point() + 6.0];
                    st.move_title = "Move tab to workspace:".to_string();
                    st.move_workspace = true;
                    st.move_open = true;
                    st.move_out = None;
                }
            }
            "copy" => {
                if let Some(text) = self.wins[self.cur].tabs.get(self.wins[self.cur].active).and_then(|t| t.selected_text()) {
                    self.copy_text(clipboard::Kind::Clipboard, text);
                }
            },
            "paste" => {
                self.request_paste(clipboard::Kind::Clipboard);
            },
            "paste_primary" => self.request_paste(clipboard::Kind::Primary),
            "scroll_page_up" | "scroll_page_down" | "scroll_bottom" => {
                if let Some(tab) = self.wins[self.cur].tabs.get(self.wins[self.cur].active) {
                    use alacritty_terminal::grid::Scroll;
                    tab.scroll(match action { "scroll_page_up" => Scroll::PageUp, "scroll_page_down" => Scroll::PageDown, _ => Scroll::Bottom });
                }
            },
            "close_tab" => self.close_tab_at(self.wins[self.cur].active, event_loop),
            "close_window" => self.request_window_close(event_loop),
            "prev_command" => self.jump_command(true),
            "next_command" => self.jump_command(false),
            "reopen_tab" => {
                // The newest burial comes straight back, no panel needed:
                // Ctrl+Shift+undoes Ctrl+Shift+x in one motion. The tab is
                // the same live terminal - pty, child, scrollback intact.
                if !self.graveyard.is_empty() {
                    self.revive_tab(self.graveyard.len() - 1);
                }
            }
            "graveyard" => {
                let opening = !self
                    .wins[self.cur]
                    .egui
                    .as_ref()
                    .is_some_and(|e| e.ui_state.panel_is(egui_ui::RightPanel::Graveyard));
                let rows = if opening { self.grave_rows() } else { Vec::new() };
                if let Some(egui) = self.wins[self.cur].egui.as_mut() {
                    let st = &mut egui.ui_state;
                    st.toggle_panel(egui_ui::RightPanel::Graveyard);
                    egui_ui::release_focus(&egui.ctx);
                    st.grave_sel = None;
                    st.grave_out = None;
                    st.grave_rows = rows;
                }
            }
            "show_workspaces" => {
                // Same contract as the graveyard: open with a snapshot, start
                // the keyboard on the workspace the window is looking at. With
                // the feature off the panel would open onto ten rows that
                // cannot switch, so it does not open at all.
                if !self.config.workspaces_enabled {
                    return;
                }
                let current = self.wins[self.cur].workspace;
                let opening = !self
                    .wins[self.cur]
                    .egui
                    .as_ref()
                    .is_some_and(|e| e.ui_state.panel_is(egui_ui::RightPanel::Workspaces));
                let rows = if opening { self.ws_rows() } else { Vec::new() };
                if let Some(egui) = self.wins[self.cur].egui.as_mut() {
                    let st = &mut egui.ui_state;
                    st.toggle_panel(egui_ui::RightPanel::Workspaces);
                    egui_ui::release_focus(&egui.ctx);
                    st.ws_sel = if opening { Some(current) } else { None };
                    st.ws_out = None;
                    st.ws_rows = rows;
                }
            }
            "prev_tab" | "prev_tab_row" => {
                if n > 1 {
                    self.wins[self.cur].active = (self.wins[self.cur].active + n - 1) % n;
                }
            },
            "next_tab" | "next_tab_row" => {
                if n > 1 {
                    self.wins[self.cur].active = (self.wins[self.cur].active + 1) % n;
                }
            },
            "first_tab" => self.wins[self.cur].active = 0,
            "last_tab" => self.wins[self.cur].active = n.saturating_sub(1),
            "move_prev_tab" => {
                if n > 1 && self.wins[self.cur].active > 0 {
                    let a = self.wins[self.cur].active;
                    self.wins[self.cur].tabs.swap(a, a - 1);
                    self.wins[self.cur].active -= 1;
                }
            },
            "move_next_tab" => {
                if n > 1 && self.wins[self.cur].active < n - 1 {
                    let a = self.wins[self.cur].active;
                    self.wins[self.cur].tabs.swap(a, a + 1);
                    self.wins[self.cur].active += 1;
                }
            },
            "move_first_tab" => {
                if n > 1 && self.wins[self.cur].active > 0 {
                    let a = self.wins[self.cur].active;
                    let t = self.wins[self.cur].tabs.remove(a);
                    self.wins[self.cur].tabs.insert(0, t);
                    self.wins[self.cur].active = 0;
                }
            },
            "move_last_tab" => {
                if n > 1 && self.wins[self.cur].active < n - 1 {
                    let a = self.wins[self.cur].active;
                    let t = self.wins[self.cur].tabs.remove(a);
                    self.wins[self.cur].tabs.push(t);
                    self.wins[self.cur].active = n - 1;
                }
            },
            "toggle_entangle" => {
                let cur_active = self.wins[self.cur].active;
                if let Some(t) = self.wins[self.cur].tabs.get_mut(cur_active) {
                    t.entangled = !t.entangled;
                }
            },
            "next_color" => {
                let cur_active = self.wins[self.cur].active;
                if let Some(t) = self.wins[self.cur].tabs.get_mut(cur_active) {
                    t.group = if t.group >= 8 { 0 } else { t.group + 1 };
                }
            },
            "prev_color" => {
                let cur_active = self.wins[self.cur].active;
                if let Some(t) = self.wins[self.cur].tabs.get_mut(cur_active) {
                    t.group = if t.group <= 0 || t.group >= 8 { 7 } else { t.group - 1 };
                }
            },
            "toggle_hide_tabs" => {
                self.wins[self.cur].tablist_hidden = !self.wins[self.cur].tablist_hidden;
                self.sync_term_size();
            },
            "config_reload" => {
                self.reload_config();
            },
            "show_shortcuts" | "show_about" => {
                self.close_switcher();
                *self.switcher_alt_at.lock().unwrap() = None;
                self.switcher_show_req.store(false, Ordering::SeqCst);
                if let Some(e) = self.wins[self.cur].egui.as_mut() {
                    e.ui_state.info_page = Some(if action == "show_shortcuts" {
                        egui_ui::InfoPage::Shortcuts
                    } else {
                        egui_ui::InfoPage::About
                    });
                    egui_ui::release_focus(&e.ctx);
                }
                if let Some(w) = &self.wins[self.cur].window { w.request_redraw(); }
            },
            "show_config" => {
                self.close_switcher();
                *self.switcher_alt_at.lock().unwrap() = None;
                self.switcher_show_req.store(false, Ordering::SeqCst);
                // Toggle: the chord that summoned the panel is the natural
                // way to dismiss it (same as every transient overlay here).
                let workspace = self.wins[self.cur].workspace;
                if let Some(e) = self.wins[self.cur].egui.as_mut() {
                    e.ui_state.workspace_edit = workspace;
                    e.ui_state.toggle_panel(egui_ui::RightPanel::Settings);
                    egui_ui::release_focus(&e.ctx);
                }
                if let Some(w) = &self.wins[self.cur].window {
                    w.request_redraw();
                }
            },
            "goto_tab_0" | "goto_tab_1" | "goto_tab_2" | "goto_tab_3" | "goto_tab_4"
            | "goto_tab_5" | "goto_tab_6" | "goto_tab_7" | "goto_tab_8" | "goto_tab_9" => {
                let target = action.chars().last().unwrap().to_digit(10).unwrap() as usize;
                self.goto_tab(target);
            },
            "enlarge_font" => self.zoom_term(true),
            "reset_font" => {
                if (self.term_pt - self.orig_term_pt).abs() >= 0.5 {
                    self.term_pt = self.orig_term_pt;
                    self.rebuild_term_font();
                }
            }
            "send_tab_new_window" => {
                self.pending_move = self.wins[self.cur].window.as_ref().map(|w| (w.id(), usize::MAX));
                for w in self.window_slot.lock().unwrap().iter() {
                    w.request_redraw();
                }
            }
            "copy_as_image" => {
                self.wins[self.cur].shot = true;
                for w in self.window_slot.lock().unwrap().iter() {
                    w.request_redraw();
                }
            }
            // The in-app twin of `mypixels last --copy`: the newest
            // finished block of the pool, straight onto the clipboard.
            "copy_last_output" => {
                let text = self.last_output_text(0, 1);
                if text.is_empty() {
                    self.show_error("No captured output to copy".into());
                } else {
                    self.copy_text(clipboard::Kind::Clipboard, text);
                    self.wins[self.cur].notice =
                        Some(("Last output copied".to_string(), Instant::now()));
                }
            }
            "show_history" => {
                let opening = !self
                    .wins[self.cur]
                    .egui
                    .as_ref()
                    .is_some_and(|e| e.ui_state.panel_is(egui_ui::RightPanel::History));
                // The rows are built before the egui borrow: the builder
                // reads the pool, the toggle writes the UI state.
                let rows = if opening {
                    self.wins[self.cur]
                        .tabs
                        .get(self.wins[self.cur].active)
                        .map(|t| self.history_rows_for(t.id))
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                if let Some(egui) = self.wins[self.cur].egui.as_mut() {
                    let st = &mut egui.ui_state;
                    st.toggle_panel(egui_ui::RightPanel::History);
                    egui_ui::release_focus(&egui.ctx);
                    st.history_sel = None;
                    st.history_preview = None;
                    st.history_rows = rows;
                }
            }
            // The lossless path: the store keeps the untouched upload bytes
            // per image id, so this never round-trips through pixels.
            "copy_last_image" => {
                let src = self
                    .wins[self.cur]
                    .tabs
                    .get(self.wins[self.cur].active)
                    .and_then(|t| {
                        self.kitty.get(&t.id).and_then(|k| {
                            k.placements()
                                .iter()
                                .rev()
                                .find_map(|p| k.image(p.image_id).map(|img| (*img.source).to_vec()))
                        })
                    });
                match src {
                    Some(bytes) => match self.clipboard.as_ref() {
                        None => self.show_error("Copy last image: no clipboard backend".into()),
                        Some(cb) => match cb.set_image(clipboard::Kind::Clipboard, bytes) {
                            Ok(()) => {
                                self.wins[self.cur].notice =
                                    Some(("Image copied to clipboard".to_string(), Instant::now()));
                            }
                            Err(e) => self.show_error(format!("Copy last image: {e}")),
                        },
                    },
                    None => self.show_error("No image to copy".into()),
                }
            }
            "shrink_font" => self.zoom_term(false),
            "select_all" => {
                let cur_active = self.wins[self.cur].active;
                if let Some(tab) = self.wins[self.cur].tabs.get_mut(cur_active) {
                    tab.select_all();
                }
                // VTE feeds the primary selection from select-all too.
                self.publish_primary();
            },
            "rename_tab" => {
                // The rename box is an egui text entry, so it and the Alt-
                // held switcher cannot share the screen: they share Mod1,
                // and only one of them may hold the keyboard. With the
                // switcher open, the chord therefore *closes* it and hands
                // over to the rename UI.
                self.close_switcher();
                let title =
                    self.wins[self.cur].tabs.get(self.wins[self.cur].active).map(|t| t.display_title()).unwrap_or_default();
                if let Some(egui) = self.wins[self.cur].egui.as_mut() {
                    let y = self.tab_h / egui.ctx.pixels_per_point() + 6.0;
                    let st = &mut egui.ui_state;
                    st.rename_draft = title;
                    st.rename_seed = true;
                    st.rename_pos = [12.0, y];
                    st.rename_open = true;
                    st.rename_out = None;
                    self.wins[self.cur].renaming = true;
                } else {
                    log::warn!("rename_tab: egui unavailable");
                }
            },
            // Not yet ported
            "show_search" | "show_search_all" | "search_prev" | "search_next" => {
                log::debug!("unported action: {action}");
            },
            _ => {},
        }
        // The action may have closed the last window (a buried tab's window
        // dies with its last tab); the session is ending, nothing to redraw.
        if let Some(w) = self.wins.get(self.cur).and_then(|w| w.window.as_ref()) {
            w.request_redraw();
        }
    }
}

/// Local HH:MM:SS for a unix timestamp (the history panel's clocks).
fn local_hms(secs: i64) -> String {
    // SAFETY: `localtime_r` writes the caller's own tm and is thread-safe.
    unsafe {
        let t = secs as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!(
            "{:02}:{:02}:{:02}",
            tm.tm_hour.max(0),
            tm.tm_min.max(0),
            tm.tm_sec.max(0)
        )
    }
}

/// A duration the panel can read at a glance: ms under a second, one
/// decimal second under a minute, minutes and seconds beyond.
fn format_duration(d: std::time::Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{} s", ms as f32 / 1000.0)
    } else {
        format!("{} m {:02} s", ms / 60_000, (ms / 1000) % 60)
    }
}

fn tab_title(t: &Tab, selected: bool, max_len: usize, cur_len: usize, shorten: &str) -> String {
    let name = t.display_title();
    let limit = if selected { cur_len } else { max_len };
    if limit > 0 && name.chars().count() > limit {
        name.chars().take(limit).collect::<String>() + shorten
    } else { name }
}

/// Background tab rectangles, rendered into the same retained targets used
/// by active terminals. Selection and images remain part of the tab picture.
#[allow(clippy::too_many_arguments)]
fn preview_rects(
    fd: &FrameData,
    bg: &[f32; 3],
    bar: &[f32; 3],
    selection: &[(usize, usize, usize)],
    selection_color: [f32; 3],
    alpha: f32,
    cell_w: f32,
    cell_h: f32,
    strip_h: f32,
    rows: usize,
    w: u32,
    h: u32,
) -> Vec<gpu::RectVertex> {
    let wf = w as f32;
    let hf = h as f32;
    let mut v: Vec<gpu::RectVertex> = Vec::with_capacity((fd.runs.len() + 3) * 6);
    let mut push = |x: f32, y: f32, rw: f32, rh: f32, c: [f32; 4]| {
        v.extend_from_slice(&gpu::rect_vertices(x, y, rw, rh, c, 0.0, wf, hf))
    };
    // Retain the window's alpha; downsampling flattens it for thumbnails.
    push(0.0, 0.0, wf, hf, [bg[0], bg[1], bg[2], alpha]);
    if strip_h > 0.0 {
        push(0.0, 0.0, wf, strip_h, [bar[0], bar[1], bar[2], 1.0]);
    }
    for run in &fd.runs {
        if (run.row as usize) >= rows {
            continue;
        }
        let same = (run.bg[0] - bg[0]).abs() < 0.004
            && (run.bg[1] - bg[1]).abs() < 0.004
            && (run.bg[2] - bg[2]).abs() < 0.004;
        if !same {
            push(
                run.col as f32 * cell_w,
                strip_h + run.row as f32 * cell_h,
                run.width as f32 * cell_w,
                cell_h,
                [run.bg[0], run.bg[1], run.bg[2], 1.0],
            );
        }
    }
    for &(row, left, right) in selection {
        push(left as f32 * cell_w, strip_h + row as f32 * cell_h,
            (right - left) as f32 * cell_w, cell_h,
            [selection_color[0], selection_color[1], selection_color[2], 0.35]);
    }
    v
}

/// The command line's leading flags and its command, cleanly separated.
/// Flags are only flags *before* the first positional argument: `mypixels -s
/// sh -c "sleep 1"` hands `-s` to us and `-c "sleep 1"` to the shell.
fn leading_flags() -> Vec<String> {
    std::env::args().skip(1).take_while(|a| a.starts_with('-')).collect()
}

/// A launcher joins the existing process with a new window by default.
/// The last explicit window/tab flag wins, matching command-line order.
fn opens_new_window(flags: &[String]) -> bool {
    flags.iter().rev().find_map(|flag| match flag.as_str() {
        "--new-tab" | "-t" => Some(false),
        "--new-window" | "-n" => Some(true),
        _ => None,
    }).unwrap_or(true)
}

fn positional_terms() -> Vec<String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().position(|a| !a.starts_with('-')) {
        Some(i) => args[i..].to_vec(),
        None => vec![],
    }
}

/// A PATH lookup, because a remote request is not trusted to name a real
/// program and pty spawn failures are fatal in this codebase.
fn command_exists(program: &str) -> bool {
    if program.contains('/') {
        return std::path::Path::new(program).exists();
    }
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path)
            .any(|dir| dir.join(program).is_file())
    })
}

/// The program every new tab runs when nothing else was asked for.
fn term_command() -> (String, Vec<String>) {
    let terms = positional_terms();
    if terms.is_empty() {
        (std::env::var("SHELL").unwrap_or_else(|_| "bash".into()), vec![])
    } else {
        (terms[0].clone(), terms[1..].to_vec())
    }
}

/// Parse a test key spec like "alt+shift+t", "ctrl+c", "esc", "f5".
/// Test hook: read key specs from the $MYPIXELS_KEYS FIFO in a background
/// thread and forward them over a channel. Returns None when unset.
fn spawn_key_reader(
    window_slot: Arc<Mutex<Vec<Arc<Window>>>>,
) -> Option<std::sync::mpsc::Receiver<String>> {
    let path = std::env::var("MYPIXELS_KEYS")
        .or_else(|_| std::env::var("SPIKE_KEYS"))
        .ok()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader};
        // Open the FIFO read-write: a FIFO opened O_RDONLY sees EOF (and
        // blocks on the next open) every time the last writer closes, and a
        // harness that closes and reopens per line can slip its next write
        // into the gap between EOF and reopen -- a broken pipe the harness
        // blames on a dead app. Holding a writer ourselves means the read
        // side never EOFs and a reader always exists.
        loop {
            let Ok(file) = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
            else {
                return;
            };
            for line in BufReader::new(file).lines() {
                let Ok(line) = line else {
                    break; // EOF: reopen below
                };
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                if tx.send(line).is_err() {
                    return;
                }
                // Wake the event loop: keys are drained on RedrawRequested, and
                // an idle terminal produces none on its own.
                if let Ok(slot) = window_slot.lock() {
                    for w in slot.iter() {
                        w.request_redraw();
                    }
                }
                // Wake the UI thread so the spec is processed promptly.
                for window in window_slot.lock().unwrap().iter() {
                    window.request_redraw();
                }
            }
        }
    });
    Some(rx)
}

impl App {
/// One more window of this session: window, GPU, egui, and a first tab
    /// running `cmd` (default shell) in `cwd`. Called at startup and on the
    /// session bus' NewWindow -- `mypixels -n` from any shell lands here.
    /// `with_tab` = false builds a bare window, for callers that immediately
    /// hand it a tab of their own (detach-to-new-window).
    fn make_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        cmd: Option<(String, Vec<String>)>,
        cwd: Option<std::path::PathBuf>,
        with_tab: bool,
    ) {
        {
            // The metrics were computed at scale 1.0 and the window is
            // created in physical pixels, so apply the display's factor
            // first (a no-op on a 1x screen).
            if let Some(m) = event_loop.primary_monitor() {
                self.set_scale(m.scale_factor() as f32);
            }
            // The template grid (App.cols/rows), not any window's: a new
            // window starts from the configured size like the first one did.
            let size = winit::dpi::PhysicalSize::new(
                (self.cols as f32 * self.cell_w) as u32,
                (self.rows as f32 * self.cell_h + self.tab_h) as u32,
            );
            let window = Arc::new(
                event_loop
                    .create_window(
                        Window::default_attributes()
                            .with_inner_size(size)
                            .with_maximized(self.config.default_maximize)
                            // Reserve an alpha-capable visual even when starting
                            // opaque: X11 cannot change the visual after creation.
                            .with_transparent(true)
                            .with_title("mypixels")
                            // Compositor identity: this is the wayland app_id
                            // (and the X11 WM_CLASS), which the original set
                            // to "mypixels" through Gtk.Application. Rules
                            // people wrote for the original -- open on
                            // workspace, no decorations, opacity exceptions
                            // -- must keep matching the rewrite.
                            .with_name("mypixels", "mypixels")
                            .with_window_icon(app_icon()),
                    )
                    .expect("window"),
            );
            // The window joins the session list before its GPU: init_gpu and
            // the egui init it performs fill in the new Win's fields, and
            // everything in between already speaks wins[cur].
            let cols = self.cols;
            let rows = self.rows;
            self.wins.push(Win {
                session_id: self.next_window_id,
                window: Some(Arc::clone(&window)),
                gpu: None,
                egui: None,
                tabs: Vec::new(),
                active: 0,
                workspace: 0,
                workspaces: (0..WORKSPACES).map(|_| Workspace::default()).collect(),
                dims: Dims { cols, rows },
                cols,
                rows,
                tab_hits: Vec::new(),
                add_hit: (0.0, 0.0),
                tab_close_hits: Vec::new(),
                tab_close_press: None,
                pointer: (0.0, 0.0),
                mouse_button: None,
                mouse_last: None,
                drag: None,
                click: None,
                wheel_pixels: 0.0,
                hover_tab: usize::MAX,
                hover_add: false,
                tablist_hidden: self.config.tablist_hide,
                renaming: false,
                overview: None,
                sw_thumbs: Vec::new(),
                grave_thumbs: Vec::new(),
                thumb_rr: 0,
                ws_preview_rr: 0,
                ws_preview_next: Instant::now(),
                preview_next: Instant::now(),
                repaint_at: None,
                autoscroll_next: None,
                shot: false,
                tab_drag: None,
                tab_drag_moved: false,
                fifo_egui_events: Vec::new(),
                notice: None,
                bell_pulse: None,
                focused: true,
                occluded: false,
                frame_gen: 0,
            });
            self.next_window_id += 1;
            // The newest window is the one the user just asked for.
            self.cur = self.wins.len() - 1;
            // The window may have landed on a monitor with a different factor
            // than the primary (or the backend still reports 1.0 before the
            // first configure); the first frame must already be right.
            let sf = window.scale_factor() as f32;
            if sf != self.scale {
                self.set_scale(sf);
                let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(
                    (self.cols as f32 * self.cell_w).round() as u32,
                    (self.rows as f32 * self.cell_h + self.tab_h).round() as u32,
                ));
            }
            // Never keep a window larger than its monitor (the configured
            // size is usually the whole screen; nested/test displays are
            // smaller).
            if !self.config.default_maximize {
                if let Some(monitor) = window.current_monitor() {
                    let ms = monitor.size();
                    let ws = window.inner_size();
                    if ws.width > ms.width || ws.height > ms.height {
                        let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(
                            ws.width.min(ms.width),
                            ws.height.min(ms.height),
                        ));
                    }
                }
            }
            self.init_gpu(event_loop, &window);
            let slot = Arc::clone(&self.window_slot);
            if self.theme_watcher.is_none() {
            match theme::Watcher::new(self.config.theme_files.clone(), move || {
                if let Ok(slot) = slot.lock() {
                    for w in slot.iter() {
                        w.request_redraw();
                    }
                }
            }) {
                Ok(watcher) => self.theme_watcher = Some(watcher),
                Err(e) => self.show_error(format!("Theme watcher unavailable: {e}")),
            }
            }
            if self.clipboard.is_none() {
            let slot = Arc::clone(&self.window_slot);
            match clipboard::Clipboard::new(clipboard::Backend::for_window(&window), move || {
                if let Ok(slot) = slot.lock() {
                    for w in slot.iter() {
                        w.request_redraw();
                    }
                }
            }) {
                Ok(clipboard) => self.clipboard = Some(clipboard),
                Err(e) => self.show_error(format!("Clipboard unavailable: {e}")),
            }
            }
            self.window_slot.lock().unwrap().push(Arc::clone(&window));
            log::info!("session: window {} up", self.wins.len());
            self.sync_term_size();
            if with_tab {
                // Every ordinary window is born with one tab: the shell the
                // request asked for, in the directory it asked from.
                self.add_tab_with(cwd, cmd);
            }
        }
        let _ = event_loop;
    }

    /// Visual-bell strength: 1.0 at the BEL, fading to 0 over 180ms.
    fn bell_flash(&self) -> f32 {
        let b = self.bell.lock().unwrap();
        match *b {
            Some(t) => {
                let e = t.elapsed().as_secs_f32();
                if e < 0.18 {
                    1.0 - e / 0.18
                } else {
                    0.0
                }
            }
            None => 0.0,
        }
    }
}

/// Lift `bg` toward a neutral grey by `k` (see bell_flash).
fn bell_mix(bg: [f32; 3], k: f32) -> [f32; 3] {
    if k <= 0.0 {
        return bg;
    }
    let w = k * 0.5;
    std::array::from_fn(|i| bg[i] + (0.35 - bg[i]) * w)
}

/// A 32x32 window icon drawn in code (a prompt bar and block cursor on a
/// dark field) -- no asset to ship, and the XDG icon lookup is not our job.
/// Wayland largely ignores window icons; X11 and window switchers use it.
fn app_icon() -> Option<winit::window::Icon> {
    static ICON: OnceLock<Option<Arc<winit::window::Icon>>> = OnceLock::new();
    ICON.get_or_init(|| {
        const S: usize = 32;
        let mut px = vec![0u8; S * S * 4];
        for y in 0..S {
            for x in 0..S {
                let inside = (2..30).contains(&x) && (2..30).contains(&y);
                let bar = (20..24).contains(&y) && (6..14).contains(&x);
                let block = (8..24).contains(&y) && (18..26).contains(&x);
                let c: [u8; 4] = if bar || block {
                    [0xa8, 0xc8, 0x44, 0xff]
                } else if inside {
                    [0x18, 0x18, 0x18, 0xff]
                } else {
                    [0, 0, 0, 0]
                };
                px[(y * S + x) * 4..][..4].copy_from_slice(&c);
            }
        }
        winit::window::Icon::from_rgba(px, S as u32, S as u32).ok().map(Arc::new)
    })
    .as_ref()
    .map(|a| (**a).clone())
}

/// Where `active` lands when tabs `a` and `b` trade places.
/// One URL found on one VISUAL row of the grid, in cell columns.
#[derive(Clone)]
struct UrlSpan {
    row: usize,
    start: usize,
    end: usize,
    url: String,
}

/// URL spans within one row of cells. Deliberately dumb: terminals print
/// URLs inside quotes, brackets and prose, so the rule is "start at the
/// scheme, stop at whitespace", then peel trailing punctuation one
/// character at a time - a ')' is prose only while it outranks its
/// openings, a '.' belongs to the URL until it ends a sentence.
///
/// Cost shape matters as much as the rules: the scheme test compares
/// characters in place (no per-suffix Strings - that made this O(n^2) in
/// the row length), and exactly one small String is allocated per URL
/// found, not per character examined.
fn url_spans_in_row(chars: &[char], row: usize) -> Vec<UrlSpan> {
    fn starts(chars: &[char], i: usize, pat: &str) -> bool {
        if i + pat.chars().count() > chars.len() {
            return false;
        }
        chars[i..].iter().zip(pat.chars()).all(|(c, p)| *c == p)
    }
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] != 'h' || !(starts(chars, i, "https://") || starts(chars, i, "http://")) {
            i += 1;
            continue;
        }
        let mut j = i;
        let mut opens = 0usize;
        let mut closes = 0usize;
        while j < chars.len() && chars[j] != ' ' && !chars[j].is_control() {
            match chars[j] {
                '(' => opens += 1,
                ')' => closes += 1,
                _ => {}
            }
            j += 1;
        }
        // Peel prose from the end, index-based: balance-aware for ')'.
        let mut e = j;
        while e > i {
            let last = chars[e - 1];
            let prose = match last {
                ')' => closes > opens,
                '.' | ',' | ';' | ':' | '!' | '?' | '"' | '\'' | ']' | '}' | '>' => true,
                _ => break,
            };
            if !prose {
                break;
            }
            e -= 1;
            if last == ')' {
                closes -= 1;
            }
        }
        let host_starts = i + if starts(chars, i, "https://") { 8 } else { 7 };
        let has_dot = chars[host_starts..e].iter().any(|c| *c == '.');
        let word: String = chars[i..e].iter().collect();
        if word.len() >= 9 && has_dot {
            out.push(UrlSpan { row, start: i, end: e, url: word });
        }
        i = j.max(i + 1);
    }
    out
}

/// One shell command delimited by OSC 133 A..D marks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandBlock {
    /// Absolute logical line of the prompt, as the state machine saw it
    /// when the prompt-start mark was dispatched.
    pub prompt_line: usize,
    /// Command exit status, when the shell reported one.
    pub exit: Option<i32>,
    /// A D mark has landed. `exit == None` means "closed without a
    /// status", not "still open" - the two states must not share a
    /// sentinel, or a second D would reopen a finished command.
    pub done: bool,
    /// Monotonic open order across the whole session, so budget
    /// eviction can find the globally oldest closed block instead of
    /// guessing per tab.
    pub seq: u64,
    /// C and D wall clocks: command execution time excludes time spent
    /// sitting at the prompt or editing the input.
    pub started_at: Option<std::time::SystemTime>,
    pub finished_at: Option<std::time::SystemTime>,
    /// A prompt alone is not a running command. Set only by C.
    pub executing: bool,
    /// Prompt row ID, used only by the legacy A/D-only capture fallback.
    pub start_global: usize,
    /// The alternate screen owned the display at one of this command's
    /// marks: its "output" was a TUI's redraws, not capturable text.
    pub alt_seen: bool,
    /// Captured output between C and D, copied by the terminal parser at D.
    /// Empty while the block's text lives in `compressed`. Legacy A/D-only
    /// integrations include input because they do not mark the output start.
    pub text: String,
    /// The closed block's text, compressed at the D mark. The in-progress
    /// block stays plain (it is appended to hot); only finished output is
    /// worth a codec. The budget counts this, not the inflated size.
    /// Layout: 8-byte little-endian inflated length, then the lz4 frame
    /// (the codec wants the destination length up front).
    pub compressed: Option<Vec<u8>>,
    /// The capture could not reach as far back as the command began
    /// (output larger than the scrollback), or the block cap clipped the
    /// tail. Say so; never silently drop bytes.
    pub truncated_head: bool,
    pub truncated_tail: bool,
    /// Input copied at the C mark, before the command's output can erase
    /// it. Available while running as well as after completion.
    pub cmd_line: String,
    /// B marks input-aware integration; C may capture an explicitly empty line.
    pub input_marked: bool,
    pub command_captured: bool,
    /// Captured output's line count, for the history panel's details.
    pub line_count: usize,
}

impl CommandBlock {
    pub fn is_history_command(&self) -> bool {
        (self.executing || self.done)
            && (!self.input_marked || self.executing)
            && (!self.command_captured || !self.cmd_line.trim().is_empty())
    }

    /// Stored size for budget accounting: the compressed form when the
    /// block closed, the live text while it runs.
    pub fn stored_len(&self) -> usize {
        self.compressed.as_ref().map(|c| c.len()).unwrap_or(self.text.len())
    }

    /// Readable output. A closed block inflates on first read and drops
    /// its compressed copy, so repeated queries pay the codec once.
    pub fn readable_text(&mut self) -> &str {
        if self.compressed.is_some() {
            self.text = self.output_text().into_owned();
            self.compressed = None;
        }
        &self.text
    }

    /// Borrow plain output or decode a copy without expanding pool storage.
    fn output_text(&self) -> std::borrow::Cow<'_, str> {
        if let Some(c) = &self.compressed {
            let (len, frame) = if c.len() > 8 {
                let len = u64::from_le_bytes(c[0..8].try_into().unwrap());
                (len as usize, &c[8..])
            } else {
                (0, &c[..])
            };
            std::borrow::Cow::Owned(match lz4_flex::decompress(frame, len) {
                Ok(t) => String::from_utf8_lossy(&t).into_owned(),
                Err(e) => format!("# (pool block unreadable: {e})"),
            })
        } else {
            std::borrow::Cow::Borrowed(&self.text)
        }
    }
}

/// Finalize output metadata and compress it. The command was captured at C,
/// using terminal cell positions, before any output could erase it. A/D-only
/// integrations retain the legacy first-line fallback.
fn finish_block(b: &mut CommandBlock) {
    b.line_count = b.text.lines().count();
    if !b.executing && !b.command_captured && b.cmd_line.is_empty() {
        b.cmd_line = b.text.lines().next().unwrap_or_default().trim_end().to_string();
    }
    // The compression decision is unchanged: blocks too small to be worth
    // a frame header, and ones the codec cannot shrink, stay plain - the
    // budget counts stored bytes either way.
    if b.text.len() < 512 {
        return;
    }
    let frame = lz4_flex::compress(b.text.as_bytes());
    let mut c = Vec::with_capacity(8 + frame.len());
    c.extend_from_slice(&(b.text.len() as u64).to_le_bytes());
    c.extend_from_slice(&frame);
    if c.len() < b.text.len() {
        b.compressed = Some(c);
        b.text.clear();
    }
}

/// Append output to a block honoring the per-block byte cap by keeping
/// the TAIL (the interesting end of a build log is its last error) and
/// flagging the clip. Zero-cap callers mean "no cap".
fn push_block_text(b: &mut CommandBlock, chunk: &str, cap: usize) {
    if chunk.is_empty() {
        return;
    }
    b.text.push_str(chunk);
    if cap > 0 && b.text.len() > cap {
        let keep = cap / 2;
        let lost = b.text.len() - keep;
        // Cut on a char boundary.
        let mut split = b.text.len() - keep;
        while !b.text.is_char_boundary(split) {
            split += 1;
        }
        b.text = format!("[... {lost} bytes omitted ...]\n{}", &b.text[split..]);
        b.truncated_tail = true;
    }
}

/// Enforce the global pool budget: evict the globally oldest CLOSED
/// block until under budget. Open blocks are never evicted (their
/// command is still writing), and a tab is never emptied of its last
/// closed block unless there is nothing else to give.
fn evict_to_budget(
    blocks: &mut std::collections::HashMap<usize, std::collections::VecDeque<CommandBlock>>,
    budget: usize,
) {
    loop {
        let total: usize =
            blocks.values().flat_map(|q| q.iter()).map(|b| b.stored_len()).sum();
        if total <= budget {
            return;
        }
        // Globally oldest CLOSED block goes first (LRU by open order).
        // A tab keeps its last closed block for as long as any other
        // tab can pay; only when the budget demands the impossible do
        // we strip those too. Open blocks are sacred: their command is
        // still writing.
        let mut done = false;
        for protect_last_in_line in [true, false] {
            let mut pick: Option<(u64, usize, usize)> = None; // (seq, tab, idx)
            for (tab, q) in blocks.iter() {
                let closed = q.iter().filter(|b| b.done).count();
                if protect_last_in_line && closed < 2 {
                    continue;
                }
                for (i, b) in q.iter().enumerate() {
                    if b.done && pick.map_or(true, |(s, _, _)| b.seq < s) {
                        pick = Some((b.seq, *tab, i));
                    }
                }
            }
            if let Some((_, tab, i)) = pick {
                if let Some(q) = blocks.get_mut(&tab) {
                    q.remove(i);
                }
                done = true;
                break;
            }
        }
        if !done {
            return; // nothing closed anywhere: over budget is temporarily sacred
        }
    }
}

/// Fold one OSC 133 mark into a tab's block ring (position and text are
/// the caller's business; this only owns block identity and closure).
fn apply_sem_mark(
    blocks: &mut std::collections::VecDeque<CommandBlock>,
    code: u8,
    exit: Option<i32>,
    line: usize,
    row: usize,
    alt: bool,
    seq: u64,
) {
    match code {
        // A: prompt start opens a block.
        0 => {
            blocks.push_back(CommandBlock {
                prompt_line: line,
                seq,
                exit: None,
                done: false,
                start_global: row,
                alt_seen: alt,
                text: String::new(),
                compressed: None,
                truncated_head: false,
                truncated_tail: false,
                cmd_line: String::new(),
                input_marked: false,
                command_captured: false,
                line_count: 0,
                started_at: None,
                finished_at: None,
                executing: false,
            });
            while blocks.len() > 512 {
                blocks.pop_front();
            }
        },
        // An input-aware shell may emit D without C for a blank Enter.
        1 => {
            if let Some(b) = blocks.iter_mut().rev().find(|b| !b.done) {
                b.input_marked = true;
            }
        },
        // C starts execution; A only opened a prompt awaiting input.
        2 => {
            if let Some(b) = blocks.iter_mut().rev().find(|b| !b.done) {
                b.executing = true;
                b.alt_seen |= alt;
            }
        },
        // D: command end closes the newest still-open block. A D with no
        // open A (marks started mid-session, or a shell that skips A) is
        // not an error - nothing to close, and the next A opens fresh.
        3 => {
            if let Some(b) = blocks.iter_mut().rev().find(|b| !b.done) {
                b.exit = exit;
                b.done = true;
            }
        },
        _ => {},
    }
}

/// Render the last `n` commands' captured output for `mypixels last`.
/// A single block is bare text - it is meant to be piped or pasted.
/// Several blocks get `#` headers carrying position and exit status,
/// because then the reader is navigating, not consuming. Truncation and
/// full-screen-program notes ride along as comments; a block that
/// captured nothing still says so rather than vanishing.
fn format_last_output(q: &mut std::collections::VecDeque<CommandBlock>, n: usize) -> String {
    // The newest n blocks that actually have something to show, emitted
    // oldest-first: the newest command lands last, right where the eye
    // already is - the prompt below. v1 captures a block's text only at
    // its D mark, so the very command that invoked this query - still
    // open, therefore textless - would otherwise occupy slot 1 of
    // `last 1 --copy` and copy nothing. It is skipped, not listed.
    let mut blocks: Vec<&mut CommandBlock> =
        q.iter_mut().rev().filter(|b| b.done || !b.text.is_empty()).take(n).collect();
    blocks.reverse();
    let count = blocks.len();
    let mut out = String::new();
    for (i, b) in blocks.iter_mut().enumerate() {
        // Copy the flags before inflating: the returned `&str` holds the
        // mutable borrow for the rest of the body.
        let (done, exit, alt, th, tt) = (b.done, b.exit, b.alt_seen, b.truncated_head, b.truncated_tail);
        // Inflate once; the rest of the loop reads `text`, never the
        // compressed form.
        let text = b.readable_text();
        if count > 1 {
            let status = match (done, exit) {
                (false, _) => "running".into(),
                (true, Some(0)) => "ok".into(),
                (true, Some(code)) => format!("exit {code}"),
                (true, None) => "no status".into(),
            };
            out.push_str(&format!(
                "\n# ── {} of {} · {status} ──\n",
                i + 1,
                count
            ));
        }
        if alt {
            out.push_str("# (full-screen program; output not captured)\n");
        } else if text.is_empty() && done && !th {
            out.push_str("# (no output captured)\n");
        } else {
            out.push_str(text);
            if !text.is_empty() && !text.ends_with('\n') {
                out.push('\n');
            }
        }
        if th || tt {
            let which = match (th, tt) {
                (true, true) => "head and tail",
                (true, false) => "head",
                _ => "tail",
            };
            out.push_str(&format!("# (output truncated: {which})\n"));
        }
    }
    out
}

/// The panel lists burials newest-first; the graveyard stores them
/// oldest-first. Translate a panel index to a graveyard position.
fn grave_pos_for(len: usize, panel_index: usize) -> Option<usize> {
    (panel_index < len).then(|| len - 1 - panel_index)
}

/// "42 s" / "3 m" / "1 h 04 m" since burial, without the clock reaching
/// into the panel.
fn buried_label(secs: u64) -> String {
    if secs < 60 {
        format!("{secs} s")
    } else if secs < 3600 {
        format!("{} m", secs / 60)
    } else {
        format!("{} h {:02} m", secs / 3600, (secs % 3600) / 60)
    }
}

/// State of the cursor's movement pulse (see `App::ripple`).
struct Ripple {
    /// Centre in PHYSICAL pixels (the draw path works there; egui divides
    /// by the scale when painting).
    x: f32,
    y: f32,
    at: Instant,
    color: [f32; 3],
}

/// RGBA pixels -> PNG. The screenshot path; egui textures and glyphon
/// agree on RGBA, so this is a header plus a pass-through.
fn png_rgba(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        if let Ok(mut writer) = enc.write_header() {
            let _ = writer.write_image_data(rgba);
        }
    }
    out
}

/// Which index is active in a tab list after the tab at `removed` leaves
/// for another window: slots after it shift down; if it WAS the active
/// tab, the neighbour that slides into its place takes over; a list that
/// held one tab simply has none (the window closes).
fn move_active_follow(len: usize, active: usize, removed: usize) -> Option<usize> {
    if len <= 1 {
        return None;
    }
    if active > removed {
        Some(active - 1)
    } else if active < removed {
        Some(active)
    } else {
        Some(active.min(len - 2))
    }
}

fn remove_exited_tab(tabs: &mut Vec<Tab>, active: &mut usize, id: usize) -> bool {
    let Some(index) = tabs.iter().position(|tab| tab.id == id) else { return false };
    *active = move_active_follow(tabs.len(), *active, index).unwrap_or(0);
    tabs.remove(index);
    true
}

fn new_tab_index(position: &str, len: usize, active: usize) -> usize {
    match position {
        "end" => len,
        "first" => 0,
        // An empty window has active = 0 but no tab to insert after.
        _ => (active + 1).min(len),
    }
}

fn swap_follow(a: usize, b: usize, active: usize) -> usize {
    if active == a {
        b
    } else if active == b {
        a
    } else {
        active
    }
}

fn egui_mods(m: ModifiersState) -> egui::Modifiers {
    let mut out = egui::Modifiers::NONE;
    if m.shift_key() {
        out |= egui::Modifiers::SHIFT;
    }
    if m.control_key() {
        out |= egui::Modifiers::CTRL;
    }
    if m.alt_key() {
        out |= egui::Modifiers::ALT;
    }
    if m.super_key() {
        out |= egui::Modifiers::COMMAND;
    }
    out
}

/// The named keys worth handing to egui from the FIFO hook; characters go
/// through `Event::Text` instead, so they never appear here.
fn egui_key_of(key: &Key) -> Option<egui::Key> {
    use winit::keyboard::NamedKey;
    Some(match key {
        Key::Named(NamedKey::Enter) => egui::Key::Enter,
        Key::Named(NamedKey::Escape) => egui::Key::Escape,
        Key::Named(NamedKey::Backspace) => egui::Key::Backspace,
        Key::Named(NamedKey::Delete) => egui::Key::Delete,
        Key::Named(NamedKey::Tab) => egui::Key::Tab,
        Key::Named(NamedKey::Home) => egui::Key::Home,
        Key::Named(NamedKey::End) => egui::Key::End,
        Key::Named(NamedKey::ArrowLeft) => egui::Key::ArrowLeft,
        Key::Named(NamedKey::ArrowRight) => egui::Key::ArrowRight,
        Key::Named(NamedKey::ArrowUp) => egui::Key::ArrowUp,
        Key::Named(NamedKey::ArrowDown) => egui::Key::ArrowDown,
        _ => return None,
    })
}

fn parse_key_spec(spec: &str) -> Option<(ModifiersState, Key)> {
    let mut mods = ModifiersState::empty();
    let mut keypart = "";
    for part in spec.split('+') {
        match part {
            "alt" => mods |= ModifiersState::ALT,
            "ctrl" | "control" => mods |= ModifiersState::CONTROL,
            "shift" => mods |= ModifiersState::SHIFT,
            "super" => mods |= ModifiersState::SUPER,
            other => keypart = other,
        }
    }
    let key = match keypart {
        "" => return None,
        "esc" | "escape" => Key::Named(NamedKey::Escape),
        "return" | "enter" => Key::Named(NamedKey::Enter),
        "tab" => Key::Named(NamedKey::Tab),
        "backspace" => Key::Named(NamedKey::Backspace),
        "delete" | "del" => Key::Named(NamedKey::Delete),
        "home" => Key::Named(NamedKey::Home),
        "end" => Key::Named(NamedKey::End),
        "pgup" => Key::Named(NamedKey::PageUp),
        "pgdn" | "pagedown" => Key::Named(NamedKey::PageDown),
        "up" => Key::Named(NamedKey::ArrowUp),
        "down" => Key::Named(NamedKey::ArrowDown),
        "left" => Key::Named(NamedKey::ArrowLeft),
        "right" => Key::Named(NamedKey::ArrowRight),
        "space" => Key::Named(NamedKey::Space),
        // "+" can't be written literally (it's the modifier separator).
        "plus" => Key::Character("+".into()),
        // "+" and "-" are also how the zoom bindings match the numpad
        // (NumLock on => winit delivers characters), so the harness can
        // reach enlarge/shrink with alt+shift+plus / alt+shift+minus.
        "minus" => Key::Character("-".into()),
        "braceleft" => Key::Character("{".into()),
        "braceright" => Key::Character("}".into()),
        "f1" => Key::Named(NamedKey::F1),
        "f2" => Key::Named(NamedKey::F2),
        "f3" => Key::Named(NamedKey::F3),
        "f4" => Key::Named(NamedKey::F4),
        "f5" => Key::Named(NamedKey::F5),
        "f6" => Key::Named(NamedKey::F6),
        "f7" => Key::Named(NamedKey::F7),
        "f8" => Key::Named(NamedKey::F8),
        "f9" => Key::Named(NamedKey::F9),
        "f10" => Key::Named(NamedKey::F10),
        "f11" => Key::Named(NamedKey::F11),
        "f12" => Key::Named(NamedKey::F12),
        c if c.chars().count() == 1 => Key::Character(c.into()),
        _ => return None,
    };
    Some((mods, key))
}

/// The terminal's answer to a key press, in the encoding legacy programs
/// expect (xterm's modified-keys conventions, DECCKM included):
///   * Ctrl+letter is the control byte; Alt prefixes ESC; both combine.
///   * Arrows/Home/End/PgUp/PgDn/Ins/Del/F-keys carry their modifiers as
///     `ESC [ 1 ; m X` (m = 1 + shift + 2*alt + 4*ctrl), which is what
///     vim, emacs, readline and friends parse. Dropping the modifier (the
///     old behaviour) made Ctrl+Right a plain Right and Alt+Enter a plain
///     Enter - the shell could not tell the chord had ever happened.
///   * DECCKM (application cursor keys, set by vim/tmux with `ESC [ ? 1 h`)
///     swaps the unmodified arrows/Home/End to the `ESC O X` forms.
/// A `None` here means genuinely nothing to send (unknown keys), not "app
/// shortcut" - those were matched by `find_action` upstream.
fn key_bytes(k: &Key, mods: ModifiersState, app_cursor: bool) -> Option<Vec<u8>> {
    let ctrl = mods.control_key();
    let alt = mods.alt_key();
    let shift = mods.shift_key();
    let esc = |body: &[u8]| -> Vec<u8> {
        let mut v = vec![0x1b];
        v.extend_from_slice(body);
        v
    };
    // xterm's modifier parameter: present only when something is actually
    // held - `ESC [ 1 ; 5 C` is Ctrl+Right, `ESC [ C` is plain Right, and
    // the difference is the only way the program can tell them apart.
    let mcode = 1 + shift as u8 + (alt as u8) * 2 + (ctrl as u8) * 4;
    // `ESC [ <n>;<m>~` for the numbered function keys.
    let csn = |n: u8| -> Vec<u8> {
        if mcode > 1 {
            esc(format!("[{n};{mcode}~").as_bytes())
        } else {
            esc(format!("[{n}~").as_bytes())
        }
    };
    match k {
        Key::Character(c) => {
            if ctrl {
                let byte = match c.to_ascii_lowercase().as_str() {
                    " " | "@" | "2" => Some(0x00u8),
                    "3" => Some(0x1b),
                    "6" => Some(0x1e),
                    "7" => Some(0x1f),
                    "8" => Some(0x7f),
                    "[" => Some(0x1b),
                    "\\" | "/" | "." | "," => Some(0x1c),
                    "]" => Some(0x1d),
                    "-" | "_" => Some(0x1f),
                    s if s.len() == 1 && s.as_bytes()[0].is_ascii_alphabetic() => {
                        Some(s.as_bytes()[0] & 0x1f)
                    }
                    // A control chord with no legacy control meaning: send
                    // the character plain rather than swallow the keypress.
                    // A dropped key is worse than a dropped modifier.
                    s if s.len() == 1 => Some(s.as_bytes()[0]),
                    s => {
                        return if s.is_empty() {
                            None
                        } else {
                            Some(if alt { esc(s.as_bytes()) } else { s.as_bytes().to_vec() })
                        }
                    }
                };
                let b = byte.unwrap();
                Some(if alt { vec![0x1b, b] } else { vec![b] })
            } else if alt {
                Some(esc(c.as_bytes()))
            } else {
                Some(c.as_bytes().to_vec())
            }
        }
        Key::Named(n) => Some(match n {
            // Space arrives here, not as Character(" "), whenever Ctrl or
            // Alt was involved - winit reports the chord as a named key,
            // and an old table that only knew plain space here silently
            // turned Ctrl+Space (the classic emacs set-mark) into a space.
            NamedKey::Space => {
                if ctrl {
                    vec![0x00]
                } else if alt {
                    vec![0x1b, 0x20]
                } else {
                    vec![0x20]
                }
            }
            NamedKey::Enter => {
                if alt {
                    vec![0x1b, 0x0d]
                } else {
                    vec![0x0d]
                }
            }
            // Ctrl+Backspace is C-h (xterm and VTE both send BS there);
            // Alt gets the meta prefix, and meta+C-h is readline's
            // backward-kill-word. Word-rubout proper remains Ctrl+W.
            NamedKey::Backspace => match (ctrl, alt) {
                (true, true) => vec![0x1b, 0x08],
                (true, false) => vec![0x08],
                (false, true) => vec![0x1b, 0x7f],
                (false, false) => vec![0x7f],
            },
            NamedKey::Tab => {
                if shift {
                    vec![0x1b, b'[', b'Z']
                } else if alt {
                    vec![0x1b, 0x09]
                } else {
                    vec![0x09]
                }
            }
            NamedKey::Escape => {
                if alt {
                    vec![0x1b, 0x1b]
                } else {
                    vec![0x1b]
                }
            }
            NamedKey::ArrowUp => cursor_key(b'A', mcode, app_cursor),
            NamedKey::ArrowDown => cursor_key(b'B', mcode, app_cursor),
            NamedKey::ArrowRight => cursor_key(b'C', mcode, app_cursor),
            NamedKey::ArrowLeft => cursor_key(b'D', mcode, app_cursor),
            NamedKey::Home => cursor_key(b'H', mcode, app_cursor),
            NamedKey::End => cursor_key(b'F', mcode, app_cursor),
            NamedKey::PageUp => csn(5),
            NamedKey::PageDown => csn(6),
            NamedKey::Delete => csn(3),
            NamedKey::Insert => csn(2),
            NamedKey::F1 => lettered_f(b'P', mcode),
            NamedKey::F2 => lettered_f(b'Q', mcode),
            NamedKey::F3 => lettered_f(b'R', mcode),
            NamedKey::F4 => lettered_f(b'S', mcode),
            NamedKey::F5 => csn(15),
            NamedKey::F6 => csn(17),
            NamedKey::F7 => csn(18),
            NamedKey::F8 => csn(19),
            NamedKey::F9 => csn(20),
            NamedKey::F10 => csn(21),
            NamedKey::F11 => csn(23),
            NamedKey::F12 => csn(24),
            _ => return None,
        }),
        _ => None,
    }
}

/// Arrows/Home/End: modified keys take the `ESC [ 1;m X` form; plain keys
/// take `ESC [ X`, or `ESC O X` while the program asked for application
/// cursor keys (DECCKM) - vim and less set that, and they read the two
/// forms differently.
fn cursor_key(fin: u8, mcode: u8, app_cursor: bool) -> Vec<u8> {
    if mcode > 1 {
        vec![0x1b, b'[', b'1', b';', b'0' + mcode, fin]
    } else if app_cursor {
        vec![0x1b, b'O', fin]
    } else {
        vec![0x1b, b'[', fin]
    }
}

/// F1-F4: `ESC O P` plain, `ESC [ 1;m P` with any modifier.
fn lettered_f(fin: u8, mcode: u8) -> Vec<u8> {
    if mcode > 1 {
        vec![0x1b, b'[', b'1', b';', b'0' + mcode, fin]
    } else {
        vec![0x1b, b'O', fin]
    }
}

/// The byte sequence of one mouse report in the protocol the terminal has
/// enabled. `code` is the xterm button code (0/1/2 press, 3 release, 32
/// motion, 64/65 wheel) already OR'd with the modifier bits (4 shift, 8 alt,
/// 16 ctrl); coordinates are 1-based viewport cells.
fn mouse_report_bytes(mode: TermMode, code: u16, col: u16, row: u16, release: bool) -> Vec<u8> {
    let tail = if release { b'm' } else { b'M' };
    if mode.contains(TermMode::SGR_MOUSE) {
        // SGR mode (?1006): CSI < Cb ; Cx ; Cy M/m - the leading < is what
        // distinguishes it, and parsers without it drop the report.
        format!("\x1b[<{};{};{}{}", code, col, row, tail as char).into_bytes()
    } else {
        // X10/normal mode: CSI M plus three binary bytes, each value +32.
        vec![0x1b, b'[', b'M', (code + 32).min(255) as u8, (col + 32).min(255) as u8, (row + 32).min(255) as u8]
    }
}

// ---------------------------------------------------------------------------
// ApplicationHandler
// ---------------------------------------------------------------------------

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.wins.is_empty() && !self.persistent {
            self.make_window(event_loop, None, None, true);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _: ()) {
        self.drain_exited(event_loop);
        self.drain_session_requests(event_loop);
        for win in &self.wins {
            if let Some(window) = &win.window { window.request_redraw(); }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.check_theme_files();
        self.drain_clipboard();
        self.drain_bell_pings();
        self.drain_notifications();
        self.drain_sems();
        self.drain_apcs();
        self.drain_ptwrites();
        self.drain_resets();
        self.drain_cell_sizes();
        self.drain_test_keys(event_loop);
        self.drain_session_requests(event_loop);
        self.drain_exited(event_loop);
        let now = Instant::now();
        for win in &mut self.wins {
            if win.repaint_at.is_some_and(|at| at <= now) {
                win.repaint_at = None;
                win.frame_gen = win.frame_gen.wrapping_add(1);
                if let Some(window) = &win.window { window.request_redraw(); }
            }
        }
        let next = self.wins.iter().filter_map(|w| w.repaint_at).min();
        event_loop.set_control_flow(next.map_or(winit::event_loop::ControlFlow::Wait,
            winit::event_loop::ControlFlow::WaitUntil));
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        // Route first: every helper below means "the window this event is
        // for" via `cur`. Events for windows we do not know (one just
        // closed, or a stray) are dropped -- there is nothing correct left
        // to do with them.
        if !self.select_window(id) { return; }
        let Some(window) = self.wins[self.cur].window.clone() else {
            return;
        };
        // Any real input invalidates the last presented frame. RedrawRequested
        // is excluded: it is the compositor's echo, not a change, and bumping
        // on it would defeat the present-dedup.
        if !matches!(event, WindowEvent::RedrawRequested) {
            self.wins[self.cur].frame_gen = self.wins[self.cur].frame_gen.wrapping_add(1);
        }
        // Decide ownership BEFORE enqueueing the key. egui-winit always
        // consumes Tab and queues it for focus traversal, even with no editor
        // focused. Passing it through after enqueueing would let one Tab both
        // reach the terminal and focus a sidebar control on the next frame.
        let route_to_egui = match &event {
            WindowEvent::KeyboardInput { event: key, .. } => {
                let action = self.action_for(&key.logical_key, Some(key), false);
                // The switcher is drawn by egui but steered by the app; while
                // it is open, keys must not reach egui (the graveyard panel
                // would steer an invisible selection with them).
                !self.switcher_open_here()
                    && !self.recording_binding()
                    && self.egui_owns_keyboard()
                    && self.graveyard_passes_key(&key.logical_key)
                    && self.workspaces_passes_key(&key.logical_key)
                    && !Self::is_panel_action(action.as_deref().unwrap_or_default())
            }
            _ => true,
        };
        // State and pointer events still reach egui regardless of keyboard ownership.
        let egui_consumed = route_to_egui && self.wins[self.cur].egui.as_mut()
            .is_some_and(|e| e.on_event(&window, &event));
        // A move and click can arrive before the next UI pass. Use the
        // actual event position for passive sidebars, and give native input
        // back immediately when the last interactive surface closed: egui
        // can still report the previous popup's stale pointer ownership.
        let egui_consumed = match &event {
            WindowEvent::CursorMoved { .. } | WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. }
                if self.wins[self.cur].overview.is_none() => {
                self.wins[self.cur].egui.as_ref().filter(|e| {
                    let st = &e.ui_state;
                    !st.menu_open && !st.move_open
                        && !st.rename_open && !st.confirm_open && st.info_page.is_none() && !egui::Popup::is_any_open(&e.ctx) && !st.font_popup.as_ref().is_some_and(|p| p.open)
                }).map(|e| {
                    if e.ui_state.right_panel == egui_ui::RightPanel::Closed { return false; }
                    let (x, y) = match &event {
                        WindowEvent::CursorMoved { position, .. } => (position.x as f32, position.y as f32),
                        _ => self.wins[self.cur].pointer,
                    };
                    let scale = e.ctx.pixels_per_point();
                    egui_ui::panel_pointer_input(&e.ctx, egui::pos2(x / scale, y / scale))
                }).unwrap_or(egui_consumed)
            }
            _ => egui_consumed,
        };
        // egui owns the strip's layout and drawing; native window tab drags
        // retain the pointer so a drop can cross the client/window boundary.
        let pointer_at = match &event {
            WindowEvent::CursorMoved { position, .. } => Some((position.x as f32, position.y as f32)),
            WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. } => Some(self.wins[self.cur].pointer),
            _ => None,
        };
        let strip_pointer = pointer_at.is_some_and(|(_, y)| self.show_tab_strip() && y >= 0.0 && y < self.tab_h)
            && self.wins[self.cur].overview.is_none()
            && !self.wins[self.cur].egui.as_ref().is_some_and(|e| e.ui_state.menu_open || e.ui_state.move_open || e.ui_state.info_page.is_some() || egui::Popup::is_any_open(&e.ctx) || e.ui_state.rename_open || e.ui_state.confirm_open);
        let egui_consumed = if pointer_at.is_some() && (strip_pointer || self.wins[self.cur].tab_drag.is_some()
            || self.wins[self.cur].tab_close_press.is_some()) { false } else { egui_consumed };
        // An event egui claimed was claimed *for* a visual change -- hover,
        // press, drag feedback all only exist in the next frame. Every
        // motion over the panel is claimed, so without this the panel's
        // state would advance invisibly and only become visible when an
        // unrelated frame (the cursor-blink timer at best) happened to
        // repaint it -- the one-interaction-behind feel. winit coalesces
        // queued redraws, so a burst of events costs one frame.
        // ... and motion/click must ask for a frame even when egui has not
        // *yet* claimed the event: `consumed` is decided from egui's state
        // before it sees this event, so the very move that carries the
        // pointer onto a widget is unclaimed -- the frame that would render
        // the resulting hover is exactly the one that would go unscheduled.
        // Terminal input paths request their own frames already; this only
        // adds motion-paced redraws, which winit coalesces to one per frame.
        if egui_consumed
            || matches!(
                event,
                winit::event::WindowEvent::CursorMoved { .. }
                    | winit::event::WindowEvent::MouseInput { .. }
            )
        {
            window.request_redraw();
        }
        if self.wins[self.cur].egui.as_ref().is_some_and(|e| e.ui_state.confirm_open)
            && matches!(event, WindowEvent::KeyboardInput { .. } | WindowEvent::CursorMoved { .. }
                | WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. }) {
            window.request_redraw();
            return;
        }
        match event {
            WindowEvent::CloseRequested => self.request_window_close(event_loop),
            WindowEvent::RedrawRequested => {
                // This frame belongs to the window that asked for it. The
                // Deferred work can change the selected window. Restore this
                // event's WindowId before touching any per-window state.
                self.check_theme_files();
                self.drain_clipboard();
                self.drain_bell_pings();
                self.drain_notifications();
                self.drain_sems();
                self.drain_apcs();
                self.drain_ptwrites();
                self.drain_resets();
                self.drain_cell_sizes();
                self.drain_test_keys(event_loop);
                self.drain_session_requests(event_loop);
                self.apply_pending_grave();
                self.drain_window_actions(event_loop);
                if !self.select_window(id) { return; }
                // Open the Alt-held switcher if the clock asked and Alt is
                // still held (guards against the press/release race).
                if self.switcher_show_req.swap(false, Ordering::SeqCst)
                    && self.config.tab_overview_on_alt
                    && (self.wins.len() == 1 || self.wins[self.cur].focused)
                    && self.switcher_alt_at.lock().unwrap().is_some()
                    && !self.wins[self.cur].tabs.is_empty()
                    && self.wins[self.cur].overview.map(|o| o.mode) != Some(OverviewMode::Switcher)
                    && !self.wins[self.cur].renaming
                {
                    self.wins[self.cur].overview = Some(OverviewState {
                        mode: OverviewMode::Switcher,
                        selected: self.wins[self.cur].active,
                        original: self.wins[self.cur].active,
                    });
                    if let Some(e) = self.wins[self.cur].egui.as_ref() {
                        egui_ui::release_focus(&e.ctx);
                    }
                    self.switcher_open.store(true, Ordering::SeqCst);
                }
                self.drain_exited(event_loop);
                if !self.select_window(id) { return; }
                // Draw unconditionally: winit only sends RedrawRequested when a
                // frame is actually wanted (our own request_redraw, resize, or
                // compositor damage). A hand-rolled damage flag here would have
                // to be remembered by every caller -- and silently swallow
                // frames for the ones that didn't (e.g. the cursor blink).
                self.draw(&window);
            },
            WindowEvent::CursorMoved { position, .. } => {
                let px = position.x as f32;
                let py = position.y as f32;
                // The pointer position is app state, not an action: keep it
                // current even while egui owns the pointer, so the next
                // unclaimed click hit-tests against the right cell.
                self.wins[self.cur].pointer = (px, py);
                if egui_consumed {
                    return;
                }
                // Terminal mouse reporting: all-motion mode reports every move,
                // drag mode only while a button is held. The protocol has no
                // sub-cell position, so one event per entered cell.
                {
                    let cell = self.mouse_cell(px, py);
                    if let (Some(mode), Some(cell)) = (self.mouse_mode(), cell) {
                        let motion = mode.contains(TermMode::MOUSE_MOTION)
                            || (mode.contains(TermMode::MOUSE_DRAG) && self.wins[self.cur].mouse_button.is_some());
                        if motion && self.wins[self.cur].mouse_last != Some(cell) {
                            self.wins[self.cur].mouse_last = Some(cell);
                            let code = match self.wins[self.cur].mouse_button {
                                Some(b) => self.mouse_button_code(b) | 32,
                                None => 32,
                            };
                            self.wins[self.cur].tabs[self.wins[self.cur].active].write(&mouse_report_bytes(mode, code, cell.0, cell.1, false));
                        }
                    }
                }
                let in_strip = self.show_tab_strip() && py < self.tab_h;
                let mut hover = usize::MAX;
                let mut hover_add = false;
                if in_strip {
                    for (i, (x0, x1)) in self.wins[self.cur].tab_hits.iter().enumerate() {
                        if px >= *x0 && px < *x1 {
                            hover = i;
                            break;
                        }
                    }
                    let (a0, a1) = self.wins[self.cur].add_hit;
                    hover_add = px >= a0 && px < a1;
                }
                // Drag reorder: the held tab trades places every time the
                // pointer crosses into another tab's cell. Swapping (rather
                // than shifting) keeps the neighbour order stable when the
                // pointer wobbles across a boundary.
                if let Some(from) = self.wins[self.cur].tab_drag {
                    if hover < self.wins[self.cur].tabs.len() && hover != from {
                        self.wins[self.cur].tabs.swap(from, hover);
                        self.wins[self.cur].active = swap_follow(from, hover, self.wins[self.cur].active);
                        self.wins[self.cur].tab_drag = Some(hover);
                        self.wins[self.cur].tab_drag_moved = true;
                        window.request_redraw();
                    }
                }
                if hover != self.wins[self.cur].hover_tab || hover_add != self.wins[self.cur].hover_add {
                    self.wins[self.cur].hover_tab = hover;
                    self.wins[self.cur].hover_add = hover_add;
                    window.request_redraw();
                }
                // Dragging extends the selection cell by cell: repainting on every
                // pixel step would spend a frame per mouse sample.
                if self.wins[self.cur].drag.is_some() {
                    if let Some((pt, side)) = self.cell_at(px, py) {
                        if self.wins[self.cur].drag != Some((pt, side)) {
                            self.wins[self.cur].drag = Some((pt, side));
                            let cur_active = self.wins[self.cur].active;
                            if let Some(tab) = self.wins[self.cur].tabs.get_mut(cur_active) {
                                tab.update_selection(pt, side);
                            }
                            window.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button, .. } => {
                if egui_consumed {
                    return;
                }
                // The app owns this click (terminal, tab strip): whoever held
                // egui keyboard focus is no longer who the user is typing to.
                if let Some(e) = self.wins[self.cur].egui.as_ref() {
                    egui_ui::release_focus(&e.ctx);
                }
                let at = self.wins[self.cur].pointer;
                let hit_tab = if self.show_tab_strip() && at.1 >= 0.0 && at.1 < self.tab_h {
                    self.wins[self.cur].tab_hits.iter().position(|&(left, right)| at.0 >= left && at.0 < right)
                } else { None };
                if button == MouseButton::Left {
                    if let Some(index) = hit_tab.filter(|&i| self.wins[self.cur].tab_close_hits.get(i)
                        .is_some_and(|rect| rect.contains(egui::pos2(at.0, at.1)))) {
                        self.wins[self.cur].tab_close_press = Some(self.wins[self.cur].tabs[index].id);
                        window.request_redraw();
                        return;
                    }
                }
                if button == MouseButton::Middle {
                    if let Some(index) = hit_tab {
                        self.close_tab_at(index, event_loop);
                        window.request_redraw();
                        return;
                    }
                }
                // While the app owns the mouse (and Shift is not bypassing it)
                // the click is the app's, not the selection's: no paste, no
                // word select, no tab-strip click.
                if matches!(button, MouseButton::Left | MouseButton::Middle | MouseButton::Right) {
                    if let (Some(cell), Some(mode)) = (self.mouse_cell(at.0, at.1), self.mouse_mode()) {
                        self.wins[self.cur].mouse_button = Some(button);
                        self.wins[self.cur].mouse_last = Some(cell);
                        self.wins[self.cur].tabs[self.wins[self.cur].active].write(&mouse_report_bytes(
                            mode,
                            self.mouse_button_code(button),
                            cell.0,
                            cell.1,
                            false,
                        ));
                        return;
                    }
                }
                if button == MouseButton::Right {
                    // The context menu: the app's own actions, offered to
                    // people who have not memorised the bindings. A right
                    // press the terminal wants to REPORT never reaches here
                    // (handled above), and one egui wants never reaches here
                    // either (egui_consumed) -- so vim and tmux keep their
                    // right button, exactly like every other one.
                    let (rows, seps) = self.menu_rows();
                    let (mx, my) = self.wins[self.cur].pointer;
                    if let Some(egui) = self.wins[self.cur].egui.as_mut() {
                        let st = &mut egui.ui_state;
                        st.menu_app = false;
                        st.menu_items = rows;
                        st.menu_seps = seps;
                        st.menu_pos = egui_ui::popup_position((mx, my), egui.ctx.pixels_per_point());
                        st.menu_open = true;
                        st.menu_out = None;
                        st.menu_age = 0;
                    }
                    window.request_redraw();
                } else if button == MouseButton::Middle {
                    // Middle click pastes the primary selection -- the X11
                    // convention the original gets free from VTE.
                    if self.cell_at(at.0, at.1).is_some() { self.request_paste(clipboard::Kind::Primary); }
                } else if button == MouseButton::Left {
                    if let Some(index) = hit_tab {
                        // Pressing a tab arms a drag but does *not* activate
                        // it: switching on press yanks the view -- and its
                        // scrollback position -- out from under a reorder
                        // started from a scrolled-up terminal. The release
                        // decides whether this was a click.
                        self.wins[self.cur].drag = None;
                        self.wins[self.cur].tab_drag = Some(index);
                        self.wins[self.cur].tab_drag_moved = false;
                    } else if self.show_tab_strip() && at.1 >= 0.0 && at.1 < self.tab_h
                        && at.0 >= self.wins[self.cur].add_hit.0 && at.0 < self.wins[self.cur].add_hit.1 {
                        self.wins[self.cur].drag = None;
                        let cur_active = self.wins[self.cur].active;
                        if let Some(tab) = self.wins[self.cur].tabs.get_mut(cur_active) { tab.clear_selection(); }
                        self.add_tab();
                        window.request_redraw();
                    } else if button == MouseButton::Left
                        && self.url_at(at.0, at.1).is_some()
                        && (self.modifiers.control_key() || self.modifiers.shift_key())
                    {
                        // A modifier+click on a link belongs to the link,
                        // not to the grid: Ctrl opens it, Shift copies it,
                        // and neither starts a selection. That last part is
                        // not politeness - selection autosends to the
                        // clipboard, so a "copy the link" click that also
                        // word-selected would have its own copy overwritten
                        // by the word's text one moment later.
                        let url = self.url_at(at.0, at.1).unwrap();
                        if self.modifiers.control_key() {
                            Self::open_url(&url);
                        } else if let Some(cb) = self.clipboard.as_ref() {
                            if let Err(e) = cb.set(clipboard::Kind::Clipboard, url) {
                                log::warn!("copy url: {e}");
                            }
                        }
                    } else if let Some((pt, side)) = self.cell_at(at.0, at.1) {
                        // Alt+drag is a rectangular selection gesture. It must
                        // not open the Alt-held switcher halfway through a drag.
                        if self.modifiers.alt_key() {
                            *self.switcher_alt_at.lock().unwrap() = None;
                            self.switcher_show_req.store(false, Ordering::SeqCst);
                        }
                        // One click starts a run, two take the word, three take
                        // whole rows: what VTE does, and why the clicks have to be
                        // counted by hand.
                        let count = self.count_click(0, at);
                        // Shift+drag is the rectangular (column) selection.
                        // Shift already bypasses the terminal's mouse
                        // reporting, so the gesture is the app's; Alt would
                        // race the Alt-held tab switcher for the same drag.
                        let ty = match count {
                            _ if self.modifiers.alt_key() || self.modifiers.shift_key() => SelectionType::Block,
                            3 => SelectionType::Lines,
                            2 => SelectionType::Semantic,
                            _ => SelectionType::Simple,
                        };
                        let cur_active = self.wins[self.cur].active;
                        if let Some(tab) = self.wins[self.cur].tabs.get_mut(cur_active) {
                            tab.begin_selection(pt, side, ty);
                        }
                        self.wins[self.cur].drag = Some((pt, side));
                        window.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput { state: ElementState::Released, button, .. } => {
                if button == MouseButton::Left {
                    if let Some(id) = self.wins[self.cur].tab_close_press.take() {
                        if let Some(index) = self.wins[self.cur].tabs.iter().position(|tab| tab.id == id) {
                            let (x, y) = self.wins[self.cur].pointer;
                            if self.wins[self.cur].tab_close_hits.get(index)
                                .is_some_and(|rect| rect.contains(egui::pos2(x, y))) {
                                self.close_tab_at(index, event_loop);
                            }
                        }
                        window.request_redraw();
                        return;
                    }
                }
                // A tab drag ends wherever the button comes up -- even over
                // egui, whose panels would otherwise swallow the release and
                // leave the drag armed. A release without a reorder was a
                // click: that is the moment the tab actually activates.
                if let Some(idx) = self.wins[self.cur].tab_drag.take() {
                    // Cross-window drop. X11 keeps reporting the pointer
                    // while it is outside the window, so a release can name
                    // one of our other windows (the tab moves there) or no
                    // window at all (the tab detaches into a new one).
                    // Wayland confines the pointer, so it never leaves and
                    // this branch never fires there; Alt+Shift+w with the
                    // picker is that gesture on Wayland.
                    let outside = self.wins[self.cur]
                        .window
                        .as_ref()
                        .and_then(|w| w.outer_position().ok().map(|_| w.outer_size()))
                        .is_some_and(|size| {
                            let (px, py) = self.wins[self.cur].pointer;
                            px < 0.0
                                || py < 0.0
                                || px >= size.width as f32
                                || py >= size.height as f32
                        });
                    if outside && idx < self.wins[self.cur].tabs.len() {
                        if let Some(win) = self.wins[self.cur].window.as_ref() {
                            if let Ok(at) = win.outer_position() {
                                let (px, py) = self.wins[self.cur].pointer;
                                let (gx, gy) = (at.x as f32 + px, at.y as f32 + py);
                                match self.win_at(gx, gy) {
                                    Some((to, insert)) if to != self.cur => {
                                        self.move_tab(self.cur, idx, to, insert, event_loop);
                                        return;
                                    }
                                    None => {
                                        // Creating the destination changes `cur`.
                                        // The dragged terminal still belongs to the source.
                                        let from = self.cur;
                                        self.make_window(event_loop, None, None, false);
                                        let to = self.wins.len() - 1;
                                        self.move_tab(from, idx, to, 0, event_loop);
                                        return;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    if !self.wins[self.cur].tab_drag_moved && idx < self.wins[self.cur].tabs.len() && idx != self.wins[self.cur].active {
                        let cur_active = self.wins[self.cur].active;
                        if let Some(t) = self.wins[self.cur].tabs.get_mut(cur_active) {
                            t.clear_selection();
                        }
                        self.wins[self.cur].active = idx;
                        window.request_redraw();
                    }
                    self.wins[self.cur].tab_drag_moved = false;
                }
                if egui_consumed {
                    return;
                }
                // A reported release goes back to the app; the coordinates fall
                // back to the last reported cell when the pointer left the
                // terminal area mid-drag.
                if self.wins[self.cur].mouse_button.take().is_some() {
                    if let Some(mode) = self.mouse_mode() {
                        if let Some(cell) =
                            self.mouse_cell(self.wins[self.cur].pointer.0, self.wins[self.cur].pointer.1).or(self.wins[self.cur].mouse_last)
                        {
                            self.wins[self.cur].tabs[self.wins[self.cur].active].write(&mouse_report_bytes(
                                mode,
                                3 | self.mouse_mods(),
                                cell.0,
                                cell.1,
                                true,
                            ));
                        }
                    }
                    self.wins[self.cur].mouse_last = None;
                }
                if button == MouseButton::Left && self.wins[self.cur].drag.take().is_some() {
                    self.publish_primary();
                    window.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event: key, .. } => {
                if key.state == ElementState::Pressed {
                    log::info!(
                        "keydown logical={:?} text={:?} tracked={:?} consumed={}",
                        key.logical_key, key.text, self.modifiers, egui_consumed
                    );
                }
                // The graveyard panel's own keys are queued to egui and act
                // on the next draw; they must not reach the terminal either
                // as a press or as the protocol's release half.
                let grave_claim = self.graveyard_claims_key(&key.logical_key);
                if key.state == ElementState::Pressed {
                    let action = self.action_for(&key.logical_key, Some(&key), false);
                    // Panel shortcuts remain global while a sidebar field is focused.
                    // Other consumed presses belong only to egui's editor.
                    let panel_shortcut = Self::is_panel_action(action.as_deref().unwrap_or_default());
                    let repeatable = action.as_deref().is_none_or(|action| matches!(action,
                        "prev_tab" | "next_tab" | "prev_tab_row" | "next_tab_row" |
                        "scroll_page_up" | "scroll_page_down" | "enlarge_font" | "shrink_font"));
                    if (!egui_consumed || panel_shortcut) && !grave_claim && (!key.repeat || repeatable) {
                        self.handle_key(&key.logical_key, Some(&key), event_loop);
                    }
                } else if !grave_claim {
                    // Releases flow to the enhanced protocol only (see
                    // handle_key's early branch); cheaply, they no-op.
                    self.handle_key(&key.logical_key, Some(&key), event_loop);
                }
            },
            WindowEvent::MouseWheel { delta, .. } => {
                if egui_consumed {
                    return;
                }
                // Ctrl+wheel zooms the font, matching the original's GTK
                // accelerator. Checked before mouse reporting on purpose:
                // GTK gave the accelerator priority over the terminal too,
                // so vim cannot take the wheel while Ctrl is held.
                if self.modifiers.control_key() {
                    let z = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y as f64,
                        MouseScrollDelta::PixelDelta(p) => p.y,
                    };
                    if z != 0.0 {
                        self.zoom_term(z > 0.0);
                        window.request_redraw();
                        return;
                    }
                }
                // The wheel is part of the click protocol: while the app owns
                // the mouse it scrolls the app, not the scrollback. Positive
                // delta is the content moving down, i.e. wheel up.
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64,
                    MouseScrollDelta::PixelDelta(p) => p.y,
                };
                if dy != 0.0 {
                    if let (Some(cell), Some(mode)) = (
                        self.mouse_cell(self.wins[self.cur].pointer.0, self.wins[self.cur].pointer.1),
                        self.mouse_mode(),
                    ) {
                        let code = (if dy > 0.0 { 64 } else { 65 }) | self.mouse_mods();
                        self.wins[self.cur].tabs[self.wins[self.cur].active].write(&mouse_report_bytes(mode, code, cell.0, cell.1, false));
                        return;
                    }
                }
                if self.cell_at(self.wins[self.cur].pointer.0, self.wins[self.cur].pointer.1).is_some() {
                    let lines = match delta {
                        MouseScrollDelta::LineDelta(_, y) => (y * 3.0) as i32,
                        MouseScrollDelta::PixelDelta(p) => {
                            self.wins[self.cur].wheel_pixels += p.y as f32;
                            let lines = (self.wins[self.cur].wheel_pixels / self.cell_h) as i32;
                            self.wins[self.cur].wheel_pixels -= lines as f32 * self.cell_h;
                            lines
                        }
                    };
                    if let Some(tab) = self.wins[self.cur].tabs.get(self.wins[self.cur].active) { tab.scroll(alacritty_terminal::grid::Scroll::Delta(lines)); }
                    window.request_redraw();
                }
            },
            WindowEvent::ModifiersChanged(m) => {
                let new = m.state();
                log::info!("mods changed -> {new:?}");
                let alt_up = new.alt_key() && !self.modifiers.alt_key();
                let alt_down = !new.alt_key() && self.modifiers.alt_key();
                // The ripple: a modifier - any modifier - being pressed
                // *bare* pulses the cursor. It answers "is my keyboard
                // landing HERE" at the only place that question is ever
                // asked: while typing, modifier state is invisible, and a
                // stuck or missed Ctrl is discovered late. A chord does not
                // ring (its keystroke already says where the cursor went);
                // typing does not ring at all.
                use winit::keyboard::ModifiersState as Ms;
                let old = self.modifiers;
                let fresh = (new.contains(Ms::CONTROL) && !old.contains(Ms::CONTROL))
                    || (new.contains(Ms::SHIFT) && !old.contains(Ms::SHIFT))
                    || (new.contains(Ms::ALT) && !old.contains(Ms::ALT))
                    || (new.contains(Ms::SUPER) && !old.contains(Ms::SUPER));
                // Adding a second modifier onto a held one is building a
                // chord, not asking "is my keyboard here" - no ring.
                let chorded = new.intersects(Ms::CONTROL | Ms::ALT | Ms::SUPER)
                    && old.intersects(Ms::CONTROL | Ms::ALT | Ms::SUPER);
                if fresh && !chorded {
                    if let Some(([x, y], color)) = self.cursor_pt {
                        self.ripple = Some(Ripple { x, y, at: Instant::now(), color });
                        window.request_redraw();
                    }
                }
                self.modifiers = new;
                if alt_up {
                    // Alt pressed: start the switcher hold timer. Not while
                    // the configuration window is open: Alt is a plain key
                    // in its text fields, and a held "Altica" must not raise
                    // the switcher behind the window.
                    if self.config.tab_overview_on_alt
                        && !self.wins[self.cur].egui.as_ref().is_some_and(|e| e.ui_state.panel_is(egui_ui::RightPanel::Settings)) {
                        *self.switcher_alt_at.lock().unwrap() = Some(std::time::Instant::now());
                    }
                    self.switcher_show_req.store(false, Ordering::SeqCst);
                } else if alt_down {
                    // Alt released: commit (active tab is already the pick) and
                    // dismiss the switcher if it's the Alt-held variant.
                    *self.switcher_alt_at.lock().unwrap() = None;
                    self.switcher_show_req.store(false, Ordering::SeqCst);
                    if self.switcher_open.load(Ordering::SeqCst) {
                        self.close_switcher();
                    }
                }
                window.request_redraw();
            },
            WindowEvent::Occluded(occluded) => {
                self.wins[self.cur].occluded = occluded;
            },
            WindowEvent::Focused(focused) => {
                self.wins[self.cur].focused = focused;
                if focused {
                    // The user answered an attention request by focusing us;
                    // stop asking.
                    window.request_user_attention(None);
                }
                // The per-frame focus block below sends the DECSET 1004
                // report; setting is_focused here would swallow the
                // transition. This redraw is what runs that block.
                window.request_redraw();
            },
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.set_scale(scale_factor as f32);
                window.request_redraw();
            },
            WindowEvent::Resized(size) => {
                if let Some(gpu) = self.wins[self.cur].gpu.as_mut() {
                    if size.width > 0
                        && size.height > 0
                        && (size.width != gpu.config.width || size.height != gpu.config.height)
                    {
                        gpu.config.width = size.width;
                        gpu.config.height = size.height;
                        gpu.surface.configure(&gpu.device, &gpu.config);
                        gpu.viewport
                            .update(&gpu.queue, Resolution { width: size.width, height: size.height });
                    }
                }
                self.sync_term_size();
                window.request_redraw();
            },
            _ => {},
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // `wins` is routinely empty here: closing the last window empties it
        // before winit fires Exiting. And say goodbye to every window's
        // tabs, not just the current one's.
        for w in self.wins.iter_mut() {
            for tab in w.all_tabs() {
                let _ = tab.sender.send(Msg::Shutdown);
            }
        }
    }
}

/// Transparency is the fraction of the background that shows through.
/// The environment override retains the old diagnostic escape hatch.
fn effective_transparency(cfg: f32) -> f32 {
    static OVERRIDE: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    let over = *OVERRIDE.get_or_init(|| {
        std::env::var("MYPIXELS_TRANSPARENCY")
            .or_else(|_| std::env::var("MYPIXELS_OPACITY"))
            .ok()
            .and_then(|s| s.trim().parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .map(|v| v.clamp(0.0, 1.0))
    });
    if let Some(v) = over {
        return v;
    }
    cfg.clamp(0.0, 1.0)
}

/// Report what this compositor + driver pair will actually do with our alpha,
/// then exit. The window is created with `visible(false)`, so nothing is ever
/// mapped and the desktop is undisturbed -- which matters because the answer
/// differs per backend, per driver and per compositor, and cannot be settled by
/// reading their sources.
struct AlphaProbe;

impl ApplicationHandler for AlphaProbe {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        alpha_probe(event_loop);
        event_loop.exit();
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}

fn alpha_probe(event_loop: &ActiveEventLoop) {
    let window = std::sync::Arc::new(
        event_loop
            .create_window(
                winit::window::Window::default_attributes()
                    .with_inner_size(winit::dpi::LogicalSize::new(64.0, 64.0))
                    .with_visible(false)
                    .with_transparent(true)
                    .with_title("alpha-probe"),
            )
            .expect("window"),
    );
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
        Box::new(event_loop.owned_display_handle()),
    ));
    let surface = instance.create_surface(window.clone()).expect("surface");
    for a in gpu::block_on(instance.enumerate_adapters(wgpu::Backends::all())) {
        let i = a.get_info();
        log::info!("probe: adapter available {:?} / {}", i.backend, i.name);
    }
    let adapter = gpu::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: Some(&surface),
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .expect("adapter");
    let info = adapter.get_info();
    let caps = surface.get_capabilities(&adapter);
    log::info!("probe: chosen {:?} / {}", info.backend, info.name);
    log::info!("probe: alpha_modes advertised = {:?}", caps.alpha_modes);
    log::info!("probe: formats = {:?}", caps.formats);
    let (device, _queue) =
        gpu::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .expect("device");
    // Some drivers advertise less than they accept, and vice versa: what decides
    // whether we get an ARGB buffer is whether configure() takes the mode.
    for m in [
        wgpu::CompositeAlphaMode::PreMultiplied,
        wgpu::CompositeAlphaMode::Inherit,
        wgpu::CompositeAlphaMode::PostMultiplied,
        wgpu::CompositeAlphaMode::Opaque,
    ] {
        let cfg = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: caps.formats[0],
            width: 64,
            height: 64,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: m,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            surface.configure(&device, &cfg);
        }))
        .is_ok();
        log::info!("probe: configure({m:?}) -> {}", if ok { "accepted" } else { "rejected" });
    }
    log::info!("probe: done");
}

/// Which commit this exact process was built from. Printed at startup and
/// by --version, because a single-instance session can outlive its own
/// binary and then argue about what it contains.
pub const BUILD: &str = env!("MYPIXELS_BUILD");

/// The stamp as a local clock string: build.rs carries epoch seconds
/// (constant-expression land has no formatter), we render it here.
pub fn build_stamp() -> String {
    let raw = BUILD;
    match raw.rsplit_once("built ") {
        Some((git, secs)) => {
            let secs: u64 = secs.parse().unwrap_or_default();
            let (h, m) = {
                let t = secs % 86400;
                (t / 3600 % 24, t / 60 % 60)
            };
            let d = secs / 86400;
            let (y, mo, da) = {
                let z = d + 719468;
                let era = z / 146097;
                let doe = z % 146097;
                let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
                let yy = yoe + era * 400;
                let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
                let mp = (5 * doy + 2) / 153;
                let day = doy - (153 * mp + 2) / 5 + 1;
                let mon = if mp < 10 { mp + 3 } else { mp - 9 };
                let year = if mon <= 2 { yy + 1 } else { yy };
                (year, mon, day)
            };
            format!("{git}, {y:04}-{mo:02}-{da:02} {h:02}:{m:02} UTC")
        }
        None => raw.to_string(),
    }
}

fn main() {
    if std::env::args().nth(1).is_some_and(|a| a == "--version" || a == "-V") {
        println!("mypixels, build {}", build_stamp());
        return;
    }

    env_logger::init();
    log::info!("mypixels build {}", build_stamp());
    let flags = leading_flags();
    if flags.iter().any(|a| a == "--help" || a == "-h") {
        println!(r#"usage: mypixels [options] [command args...]

Options:
  -n, --new-window   open a new window in the running instance (default)
  -t, --new-tab      open a tab in the originating or first session window
  -s, --standalone   run a private instance: never join or host a session
      --daemon      start a persistent session without opening a window
      --restore-window  reopen the last closed window with its live tabs
  -h, --help         show this text

  mypixels last [n] [--copy]
                   show (or clipboard-copy) the last n commands' output
                   from the running session's memory pool

Without flags, find the running instance on this display over D-Bus and
request a new window for this command in the current directory. The client
prints the instance and requested action to stdout before sending it.
Use --new-tab to add a tab instead. If no session answers, start a new instance."#);
        return;
    }
    // Forward new-window requests (or explicit --new-tab requests) to the
    // live session. Missing/incompatible sessions fall back to starting here.
    // `mypixels last [n] [--copy]`: the running session's memory pool of
    // command output, queried, not launched. Intercepted before the
    // open-a-tab path below, or "last" would spawn a program named last.
    {
        let terms = positional_terms();
        if terms.first().map(|s| s.as_str()) == Some("last") {
            let mut n: u64 = 1;
            let mut copy = false;
            for a in &terms[1..] {
                match a.parse::<u64>() {
                    Ok(v) if v >= 1 => n = v,
                    _ if a == "--copy" => copy = true,
                    _ => {
                        eprintln!("usage: mypixels last [n] [--copy]");
                        std::process::exit(2);
                    }
                }
            }
            match session::remote_last(n, copy) {
                Some(text) => {
                    if !copy {
                        print!("{text}");
                    }
                }
                None => {
                    eprintln!("mypixels last: no running session to ask");
                    std::process::exit(1);
                }
            }
            return;
        }
    }
    let standalone = flags.iter().any(|a| a == "--standalone" || a == "-s");
    let persistent = flags.iter().any(|a| a == "--daemon");
    let restore = flags.iter().any(|a| a == "--restore-window");
    if (persistent && (standalone || restore || !positional_terms().is_empty()
        || flags.iter().any(|a| matches!(a.as_str(), "-t" | "--new-tab" | "-n" | "--new-window"))))
        || (restore && (standalone || !positional_terms().is_empty())) {
        eprintln!("mypixels: --daemon and --restore-window must be used on their own");
        std::process::exit(2);
    }
    if restore {
        if !session::remote_restore_window() {
            eprintln!("mypixels: no running session supports window restoration");
            std::process::exit(1);
        }
        return;
    }
    if !standalone && !persistent {
        let new_window = opens_new_window(&flags);
        let cwd = std::env::current_dir().ok().map(|p| p.display().to_string());
        if session::remote_open(cwd, positional_terms(), new_window) {
            return;
        }
        // Nothing answered: a window of our own, session or not, is the
        // fallback -- with -n against a session too old to speak NewWindow,
        // that means a separate second process, the worst honest outcome.
    }
    // Running our own window from here. The shells this window spawns must
    // describe THIS window: a joiner's inherited $MYPIXELS_WINDOW would
    // point at the window we left, and --standalone must point at no
    // session at all (0). The bus thread overwrites 0 with the real slot
    // number in each tab's own environment (see Tab::new). Safe: no other
    // thread of this process exists yet.
    unsafe {
        if standalone {
            std::env::set_var("MYPIXELS_WINDOW", "0");
        } else {
            std::env::remove_var("MYPIXELS_WINDOW");
        }
    }
    let event_loop = EventLoop::new().expect("event loop");
    if std::env::args().any(|a| a == "--alpha-probe") {
        event_loop.run_app(&mut AlphaProbe).expect("alpha probe");
        return;
    }
    let mut app = App::new(event_loop.create_proxy(), persistent);
    event_loop.run_app(&mut app).expect("run");
    log::info!("exited");
}

#[cfg(test)]
mod graveyard_tests {
    use super::*;

    #[test]
    fn panel_index_maps_newest_first_onto_oldest_first_storage() {
        // Five burials, oldest at 0, newest at 4. Panel row 0 is the newest.
        assert_eq!(grave_pos_for(5, 0), Some(4));
        assert_eq!(grave_pos_for(5, 2), Some(2));
        assert_eq!(grave_pos_for(5, 4), Some(0));
        // A stale selection (an eviction shrank the list) maps to nothing
        // rather than the wrong tab.
        assert_eq!(grave_pos_for(5, 5), None);
        assert_eq!(grave_pos_for(0, 0), None);
    }

    #[test]
    fn burial_age_stays_short_and_readable() {
        assert_eq!(buried_label(0), "0 s");
        assert_eq!(buried_label(59), "59 s");
        assert_eq!(buried_label(60), "1 m");
        assert_eq!(buried_label(3600), "1 h 00 m");
        assert_eq!(buried_label(3600 + 240), "1 h 04 m");
    }
}

#[cfg(test)]
mod preview_tests {
    use super::*;

    fn run(row: u32, col: u32, text: &str) -> Run {
        Run {
            row,
            col,
            width: text.len() as u32,
            fg: [1.0, 1.0, 1.0],
            bg: [0.0, 0.0, 0.0],
            text: text.into(),
            cells: text.char_indices().enumerate().map(|(col, (byte, _))| (byte, col as u32)).collect(),
            attrs: Box::leak(Box::new(Attrs::new())),
        }
    }

    #[test]
    fn padding_is_not_shaped_but_background_width_is_preserved() {
        let mut runs = Vec::new();
        let mut padded = run(0, 0, "path.js      ");
        padded.bg = [0.2, 0.1, 0.0];
        push_run(&mut runs, padded, [1.0; 3], [0.0; 3]);
        assert_eq!(runs[0].text, "path.js");
        assert_eq!(runs[0].width, 13);
        assert_eq!(runs[0].cells.len(), 7);
    }

    #[test]
    fn invisible_padding_with_colored_foreground_is_discarded() {
        let mut runs = Vec::new();
        push_run(&mut runs, run(0, 0, "      "), [0.5; 3], [0.0; 3]);
        assert!(runs.is_empty());
    }


}

#[cfg(test)]
mod palette_tests {
    use super::*;
    use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

    #[test]
    fn brightness_adjusts_resolved_ansi_truecolor_and_runtime_overrides_once() {
        let mut colors = alacritty_terminal::term::color::Colors::default();
        colors[1] = Some(Rgb { r: 64, g: 96, b: 128 });
        let palette = Palette::from_colors(&colors, &Some([config::Rgb(20, 30, 40); 16]));
        let equivalent = [Color::Spec(Rgb { r: 64, g: 96, b: 128 }), Color::Indexed(1), Color::Named(NamedColor::Red)];
        let mut cfg = Config::default();
        for brightness in [-40.0, 0.0, 40.0] {
            cfg.terminal_brightness = brightness;
            let adjust = |color| cfg.adjust_terminal_color(palette.resolve_with(&color, [0.5; 3], [0.2; 3], [0.6; 3]));
            for color in equivalent {
                assert_eq!(adjust(color), cfg.adjust_terminal_color([64.0 / 255.0, 96.0 / 255.0, 128.0 / 255.0]));
            }
            assert_eq!(adjust(Color::Named(NamedColor::Background)), cfg.adjust_terminal_color([0.2; 3]));
            assert_eq!(adjust(Color::Named(NamedColor::Foreground)), cfg.adjust_terminal_color([0.5; 3]));
        }
    }

    #[test]
    fn osc_overrides_use_the_same_slots_for_named_and_indexed_colors() {
        let mut colors = alacritty_terminal::term::color::Colors::default();
        colors[1] = Some(Rgb { r: 24, g: 48, b: 72 });
        colors[17] = Some(Rgb { r: 12, g: 34, b: 56 });
        colors[256] = Some(Rgb { r: 255, g: 128, b: 0 });
        let palette = Palette::from_colors(&colors, &None);
        let resolve = |c| palette.resolve_with(&c, [0.8; 3], [0.1; 3], [0.4; 3]);
        assert_eq!(resolve(Color::Named(NamedColor::Red)), resolve(Color::Indexed(1)));
        assert_eq!(resolve(Color::Indexed(1)), config::Rgb(24, 48, 72).to_f32());
        assert_eq!(resolve(Color::Indexed(17)), config::Rgb(12, 34, 56).to_f32());
        assert_eq!(resolve(Color::Named(NamedColor::Foreground)), config::Rgb(255, 128, 0).to_f32());
        assert_eq!(resolve(Color::Named(NamedColor::Background)), [0.1; 3]);
        assert_eq!(resolve(Color::Named(NamedColor::Cursor)), [0.4; 3]);
    }

    #[test]
    fn theme_palette_leaves_the_256_color_cube_and_truecolor_intact() {
        let cfg = Config::from_text("theme: dracula", std::path::Path::new(".")).unwrap();
        let palette = Palette::from_colors(&Default::default(), &cfg.palette);
        let resolve = |c| palette.resolve_with(&c, [0.8; 3], [0.1; 3], [0.4; 3]);
        assert_eq!(resolve(Color::Indexed(1)), cfg.palette.unwrap()[1].to_f32());
        assert_eq!(resolve(Color::Indexed(196)), [1.0, 0.0, 0.0]);
        assert_eq!(resolve(Color::Indexed(17)), [0.0, 0.0, 95.0 / 255.0]);
        assert_eq!(resolve(Color::Indexed(232)), [8.0 / 255.0; 3]);
        assert_eq!(resolve(Color::Spec(Rgb { r: 1, g: 2, b: 3 })), config::Rgb(1, 2, 3).to_f32());
    }
}

#[cfg(test)]
mod mouse_report_tests {
    use super::*;

    fn mode_with(modes: &[TermMode]) -> TermMode {
        modes.iter().copied().fold(TermMode::NONE, |m, x| m | x)
    }

    #[test]
    fn x10_press_release_and_wheel() {
        let mode = mode_with(&[TermMode::MOUSE_REPORT_CLICK]);
        // Normal mode is CSI M plus binary bytes (value + 32), not text.
        assert_eq!(mouse_report_bytes(mode, 0, 42, 17, false), vec![0x1b, b'[', b'M', 32, 74, 49]);
        // release carries no tail in normal mode; the button code says it
        assert_eq!(mouse_report_bytes(mode, 3, 42, 17, true), vec![0x1b, b'[', b'M', 35, 74, 49]);
        assert_eq!(mouse_report_bytes(mode, 64, 1, 1, false), vec![0x1b, b'[', b'M', 96, 33, 33]);
        // ctrl + middle press: modifier bits are OR'd into the button code
        assert_eq!(mouse_report_bytes(mode, 1 | 16, 5, 6, false), vec![0x1b, b'[', b'M', 49, 37, 38]);
    }

    #[test]
    fn sgr_keeps_the_leading_bracket() {
        let mode = mode_with(&[TermMode::MOUSE_REPORT_CLICK, TermMode::SGR_MOUSE]);
        assert_eq!(String::from_utf8(mouse_report_bytes(mode, 0, 42, 17, false)).unwrap(), "\x1b[<0;42;17M");
        // motion with the left button held
        assert_eq!(String::from_utf8(mouse_report_bytes(mode, 0 | 32, 3, 9, false)).unwrap(), "\x1b[<32;3;9M");
        assert_eq!(String::from_utf8(mouse_report_bytes(mode, 3 | 8, 3, 9, true)).unwrap(), "\x1b[<11;3;9m");
        // wheel up and down
        assert_eq!(String::from_utf8(mouse_report_bytes(mode, 64, 10, 5, false)).unwrap(), "\x1b[<64;10;5M");
        assert_eq!(String::from_utf8(mouse_report_bytes(mode, 65, 10, 5, false)).unwrap(), "\x1b[<65;10;5M");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tab_placement_handles_empty_windows() {
        for position in ["next", "end", "first"] {
            let mut tabs = Vec::new();
            let index = new_tab_index(position, tabs.len(), 0);
            tabs.insert(index, "first tab");
            assert_eq!(index, 0, "{position}");
            assert_eq!(tabs[index], "first tab");
        }
    }

    #[test]
    fn new_tab_placement_respects_position_in_populated_windows() {
        for (position, active, expected) in [
            ("next", 0, 1),
            ("next", 1, 2),
            ("next", 2, 3),
            ("end", 0, 3),
            ("first", 2, 0),
        ] {
            let mut tabs = vec!["a", "b", "c"];
            let index = new_tab_index(position, tabs.len(), active);
            tabs.insert(index, "new tab");
            assert_eq!(index, expected, "{position}, active={active}");
            assert_eq!(tabs[index], "new tab");
        }
    }

    #[test]
    fn launches_default_to_windows_with_explicit_tab_override() {
        for (flags, window) in [
            (vec![], true), (vec!["-n"], true), (vec!["--new-window"], true),
            (vec!["--new-tab"], false), (vec!["-t"], false),
            (vec!["-n", "-t"], false), (vec!["-t", "-n"], true),
        ] {
            assert_eq!(opens_new_window(&flags.into_iter().map(String::from).collect::<Vec<_>>()), window);
        }
    }

    #[test]
    fn function_keys_use_ss3_without_an_extra_csi_prefix() {
        for (key, final_byte) in [(NamedKey::F1, b'P'), (NamedKey::F2, b'Q'), (NamedKey::F3, b'R'), (NamedKey::F4, b'S')] {
            assert_eq!(key_bytes(&Key::Named(key.clone()), ModifiersState::empty(), false),
                Some(vec![0x1b, b'O', final_byte]));
            assert_eq!(key_bytes(&Key::Named(key), ModifiersState::CONTROL, false),
                Some(vec![0x1b, b'[', b'1', b';', b'5', final_byte]));
        }
    }

    #[test]
    fn autoscroll_direction_follows_the_wheel_sign() {
        // Drag near the top edge: up into history, Delta(+). Near the
        // bottom: toward the prompt, Delta(-). Middle: dead.
        let (dir, lines) = App::autoscroll_tick(10.0, 0.0, 400.0, 16.0).unwrap();
        assert_eq!((dir.signum(), lines >= 1), (1, true));
        let (dir, _) = App::autoscroll_tick(395.0, 0.0, 400.0, 16.0).unwrap();
        assert_eq!(dir.signum(), -1);
        assert!(App::autoscroll_tick(200.0, 0.0, 400.0, 16.0).is_none());
    }

    #[test]
    fn autoscroll_speeds_up_with_over_drag() {
        // Edge band: one line per tick. Each cell height pulled past the
        // edge: one more. A furious pull still moves a screen, not a book.
        let cap = 32;
        // band = 2.2 cells deep: grazing its far edge drifts one line...
        assert_eq!(App::autoscroll_tick(58.0, 24.0, 400.0, 16.0), Some((1, 1)));
        // ...mid-band travels two...
        assert_eq!(App::autoscroll_tick(32.0, 24.0, 400.0, 16.0), Some((2, 2)).map(|(_, b)| (1, b)));
        // ...and the grid edge itself (a full band of depth) three.
        assert_eq!(App::autoscroll_tick(24.0, 24.0, 400.0, 16.0), Some((1, 3)));
        // Past it (the strip zone, or the window's own border) keeps ramping.
        assert_eq!(App::autoscroll_tick(4.0, 24.0, 400.0, 16.0), Some((1, 4)));
        assert_eq!(App::autoscroll_tick(420.0, 0.0, 400.0, 16.0), Some((-1, 4)));
        assert_eq!(App::autoscroll_tick(-9000.0, 0.0, 400.0, 16.0).map(|(d, _)| d), Some(1));
        assert_eq!(App::autoscroll_tick(1.0, 0.0, 400.0, 16.0), Some((1, 3))); // near the border: three
        assert!(App::autoscroll_tick(-9000.0, 0.0, 400.0, 16.0).unwrap().1 <= cap);
    }

    fn kb(spec: &str, app_cursor: bool) -> Option<Vec<u8>> {
        let (mods, key) = parse_key_spec(spec).expect("spec");
        key_bytes(&key, mods, app_cursor)
    }

    #[test]
    fn modified_named_keys_carry_the_modifier_to_the_terminal() {
        // The reported breakage, as a truth table. Every one of these used
        // to be sent as the *plain* key - the program could not tell the
        // chord had ever happened.
        assert_eq!(kb("ctrl+right", false), Some(b"\x1b[1;5C".to_vec()));
        assert_eq!(kb("ctrl+left", false), Some(b"\x1b[1;5D".to_vec()));
        assert_eq!(kb("alt+right", false), Some(b"\x1b[1;3C".to_vec()));
        assert_eq!(kb("ctrl+shift+left", false), Some(b"\x1b[1;6D".to_vec()));
        assert_eq!(kb("ctrl+delete", false), Some(b"\x1b[3;5~".to_vec()));
        assert_eq!(kb("ctrl+home", false), Some(b"\x1b[1;5H".to_vec()));
        assert_eq!(kb("shift+tab", false), Some(b"\x1b[Z".to_vec()));
        assert_eq!(kb("ctrl+space", false), Some(vec![0x00]));
        assert_eq!(kb("alt+space", false), Some(vec![0x1b, 0x20]));
        assert_eq!(kb("ctrl+backspace", false), Some(vec![0x08]));
        assert_eq!(kb("alt+backspace", false), Some(vec![0x1b, 0x7f]));
        assert_eq!(kb("ctrl+alt+backspace", false), Some(vec![0x1b, 0x08]));
        assert_eq!(kb("ctrl+f5", false), Some(b"\x1b[15;5~".to_vec()));
        assert_eq!(kb("alt+f1", false), Some(b"\x1b[1;3P".to_vec()));
    }

    #[test]
    fn control_letters_and_meta_still_encode_classically() {
        assert_eq!(kb("ctrl+w", false), Some(vec![0x17]));
        assert_eq!(kb("ctrl+c", false), Some(vec![0x03]));
        assert_eq!(kb("ctrl+[", false), Some(vec![0x1b]));
        assert_eq!(kb("alt+f", false), Some(vec![0x1b, b'f']));
        // The digits that DO have control meanings in xterm.
        assert_eq!(kb("ctrl+2", false), Some(vec![0x00]));
        assert_eq!(kb("ctrl+6", false), Some(vec![0x1e]));
        assert_eq!(kb("ctrl+7", false), Some(vec![0x1f]));
        // A control chord with no legacy meaning sends the character
        // rather than swallowing the keystroke.
        assert_eq!(kb("ctrl+9", false), Some(vec![b'9']));
    }

    #[test]
    fn plain_keys_keep_their_traditional_bytes() {
        // The regression guard for the rewrite: plain keys must not pick
        // up modifier parameters, or every vim on the machine breaks.
        assert_eq!(kb("up", false), Some(b"\x1b[A".to_vec()));
        assert_eq!(kb("home", false), Some(b"\x1b[H".to_vec()));
        assert_eq!(kb("end", false), Some(b"\x1b[F".to_vec()));
        assert_eq!(kb("pgup", false), Some(b"\x1b[5~".to_vec()));
        assert_eq!(kb("delete", false), Some(b"\x1b[3~".to_vec()));
        assert_eq!(kb("backspace", false), Some(vec![0x7f]));
        assert_eq!(kb("tab", false), Some(vec![0x09]));
        assert_eq!(kb("return", false), Some(vec![0x0d]));
        assert_eq!(kb("f5", false), Some(b"\x1b[15~".to_vec()));
    }

    #[test]
    fn application_cursor_mode_switches_the_plain_cursor_keys() {
        // vim/tmux set DECCKM; the program then *reads* ESC O A where it
        // reads ESC [ A otherwise.
        assert_eq!(kb("up", true), Some(b"\x1bOA".to_vec()));
        assert_eq!(kb("home", true), Some(b"\x1bOH".to_vec()));
        assert_eq!(kb("end", true), Some(b"\x1bOF".to_vec()));
        // Modified keys never take the SS3 form, application mode or not.
        assert_eq!(kb("ctrl+up", true), Some(b"\x1b[1;5A".to_vec()));
    }

    #[test]
    fn png_rgba_round_trips_through_a_real_decoder() {
        // A quarter of the encoder's contract, proved against the decoder:
        // size, colour type, and exact pixels.
        let (w, h) = (4u32, 2u32);
        let mut rgba = Vec::new();
        for i in 0..(w * h) as usize {
            rgba.extend_from_slice(&[i as u8, 255 - i as u8, (i * 7) as u8, 255]);
        }
        let png = png_rgba(w, h, &rgba);
        assert!(!png.is_empty() && png[..8] == *b"\x89PNG\r\n\x1a\n");
        let decoder = png::Decoder::new(std::io::Cursor::new(&png));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (w, h));
        assert_eq!(info.color_type, png::ColorType::Rgba);
        assert_eq!(&buf[..rgba.len()], &rgba[..]);
    }

    #[test]
    fn a_moved_tabs_window_chooses_a_new_active() {
        assert_eq!(move_active_follow(1, 0, 0), None, "the window closes");
        assert_eq!(move_active_follow(3, 2, 2), Some(1), "active leaves: the neighbour before");
        assert_eq!(move_active_follow(3, 0, 2), Some(0), "another tab leaves: unaffected");
        assert_eq!(move_active_follow(3, 2, 0), Some(1), "a lower tab leaves: shift down");
        assert_eq!(move_active_follow(2, 1, 1), Some(0), "last index clamps after removal");
    }

    #[test]
    fn tiny_windows_do_not_autoscroll_on_a_parked_pointer() {
        // Two-cell grid: the bands would overlap; the middle must hold.
        assert!(App::autoscroll_tick(16.0, 0.0, 32.0, 16.0).is_none());
    }

    use super::swap_follow;

    #[test]
    fn dragging_a_tab_moves_the_active_index_with_it() {
        // Dragging the active tab: it follows to the drop cell.
        assert_eq!(swap_follow(0, 2, 0), 2);
        // Dragging another tab onto the active one: the active tab is
        // displaced to the drag's old cell (it is the same tab, so its
        // terminal -- and the selection inside it -- stay put).
        assert_eq!(swap_follow(2, 0, 0), 2);
        // A reorder that does not involve the active tab leaves it alone.
        assert_eq!(swap_follow(1, 2, 0), 0);
    }
}

#[cfg(test)]
mod url_tests {
    use super::url_spans_in_row;

    fn spans(line: &str) -> Vec<(usize, String)> {
        let chars: Vec<char> = line.chars().collect();
        url_spans_in_row(&chars, 0)
            .into_iter()
            .map(|s| (s.start, s.url))
            .collect()
    }

    #[test]
    fn finds_links_and_drops_prose_punctuation() {
        let line = "see https://example.com/a?b=c. Also http://x.org, ok";
        let got = spans(line);
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].1, "http://x.org");
        assert_eq!(got[0].1, "https://example.com/a?b=c");
        assert_eq!(got[0].0, 4);
    }

    #[test]
    fn keeps_balanced_parens_and_peels_extra_ones() {
        let line = "https://en.wikipedia.org/wiki/Rust_(language)";
        assert_eq!(spans(line)[0].1, line);
        assert_eq!(spans("trailing close: https://a.io/x)")[0].1, "https://a.io/x");
    }

    #[test]
    fn ignores_schemeish_noise() {
        assert!(spans("go to https://here now").is_empty()); // host needs a dot
        assert!(spans("the http:// scheme").is_empty());
        assert!(spans("ftp://not.http").is_empty()); // http(s) only
        assert_eq!(spans("http://a.io").len(), 1);
    }

    /// The frame-budget guard. One row per redraw is the design; at 200
    /// columns it must cost microseconds, because a mouse move redraws and
    /// the terminal lock is not free. (Threshold is debug-build generous.)
    #[test]
    fn row_scan_is_fast() {
        let line = "curl -sS https://example.com/some/path?query=1&x=2 | jq .  ".repeat(4);
        let chars: Vec<char> = line.chars().collect();
        let n = 20_000;
        let t = std::time::Instant::now();
        for _ in 0..n {
            std::hint::black_box(url_spans_in_row(&chars, 0));
        }
        let per = t.elapsed() / n;
        eprintln!("{:?} per {}-char row", per, chars.len());
        assert!(per.as_micros() < 3000, "row scan too slow for a redraw path");
    }
}

#[cfg(test)]
mod sem_tests {
    use super::apply_sem_mark;
    use std::collections::VecDeque;

    #[test]
    fn marks_fold_into_blocks() {
        let mut b: VecDeque<super::CommandBlock> = VecDeque::new();
        apply_sem_mark(&mut b, 0, None, 10, 10, false, 1); // A
        apply_sem_mark(&mut b, 1, None, 10, 10, false, 1); // B
        apply_sem_mark(&mut b, 2, None, 11, 11, false, 1); // C
        apply_sem_mark(&mut b, 3, Some(0), 40, 40, false, 1); // D;0
        apply_sem_mark(&mut b, 0, None, 41, 41, false, 2); // A
        apply_sem_mark(&mut b, 3, Some(1), 55, 55, false, 2); // D;1
        assert_eq!(b.len(), 2);
        assert_eq!((b[0].prompt_line, b[0].exit), (10, Some(0)));
        assert_eq!((b[1].prompt_line, b[1].exit), (41, Some(1)));
    }

    #[test]
    fn idle_prompt_is_not_a_running_command() {
        let mut blocks = VecDeque::new();
        apply_sem_mark(&mut blocks, 0, None, 0, 0, false, 1);
        apply_sem_mark(&mut blocks, 1, None, 0, 0, false, 1);
        assert!(!blocks[0].is_history_command());
        apply_sem_mark(&mut blocks, 2, None, 1, 1, false, 1);
        blocks[0].cmd_line = "printf hello".into();
        assert!(blocks[0].is_history_command());
        assert!(!blocks[0].done);
        apply_sem_mark(&mut blocks, 3, Some(0), 2, 2, false, 1);
        blocks[0].text = "a multiline prompt\n$ printf hello\nhello".into();
        super::finish_block(&mut blocks[0]);
        apply_sem_mark(&mut blocks, 0, None, 3, 3, false, 2);
        assert!(blocks[0].done);
        assert_eq!(blocks[0].cmd_line, "printf hello", "finishing output must keep the captured input");
        assert_eq!(blocks.iter().filter(|b| b.is_history_command()).count(), 1);
    }

    #[test]
    fn blank_input_never_becomes_prompt_text_in_history() {
        for command in ["", "  ", "\t\n", "\u{2003}"] {
            let mut blocks = VecDeque::new();
            apply_sem_mark(&mut blocks, 0, None, 0, 0, false, 1);
            apply_sem_mark(&mut blocks, 1, None, 0, 0, false, 1);
            apply_sem_mark(&mut blocks, 2, None, 1, 1, false, 1);
            let b = &mut blocks[0];
            b.cmd_line = command.into();
            b.command_captured = true;
            assert!(!b.is_history_command(), "blank running command should be hidden");
            apply_sem_mark(&mut blocks, 3, Some(0), 1, 1, false, 1);
            let b = &mut blocks[0];
            b.text = "$ \n".into();
            super::finish_block(b);
            assert_eq!(b.cmd_line, command, "prompt must not replace captured blank input");
            assert!(!b.is_history_command(), "blank completed command should be hidden");
        }
        // Bash can emit A/B/D without C when Enter runs no command.
        let mut blocks = VecDeque::new();
        for code in [0, 1, 3] { apply_sem_mark(&mut blocks, code, Some(0), 0, 0, false, 1); }
        blocks[0].text = "$ \n".into();
        super::finish_block(&mut blocks[0]);
        assert!(!blocks[0].is_history_command());
        // Older integrations that emit only A/D keep their fallback.
        let mut blocks = VecDeque::new();
        for code in [0, 3] { apply_sem_mark(&mut blocks, code, Some(0), 0, 0, false, 1); }
        blocks[0].text = "$ echo hello\nhello".into();
        super::finish_block(&mut blocks[0]);
        assert!(blocks[0].is_history_command());
    }

    #[test]
    fn stray_and_missing_marks_survive() {
        let mut b: VecDeque<super::CommandBlock> = VecDeque::new();
        apply_sem_mark(&mut b, 3, Some(2), 5, 5, false, 0); // D without A: no block, no panic
        assert!(b.is_empty());
        apply_sem_mark(&mut b, 0, None, 6, 6, false, 1); // A
        apply_sem_mark(&mut b, 3, None, 9, 9, false, 1); // D with unknown status
        apply_sem_mark(&mut b, 3, Some(7), 9, 9, false, 1); // second D must not reopen D
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].exit, None); // first D won with "unknown"
    }

    #[test]
    fn ring_is_bounded() {
        let mut b: VecDeque<super::CommandBlock> = VecDeque::new();
        for i in 0..600 {
            apply_sem_mark(&mut b, 0, None, i, i, false, i as u64);
        }
        assert_eq!(b.len(), 512);
        assert_eq!(b.front().unwrap().prompt_line, 88);
    }

    fn block(text: &str, done: bool, seq: u64) -> super::CommandBlock {
        let mut q: VecDeque<super::CommandBlock> = VecDeque::new();
        apply_sem_mark(&mut q, 0, None, 0, 0, false, seq);
        let mut b = q.pop_front().unwrap();
        b.text = text.to_string();
        b.done = done;
        b
    }

    #[test]
    fn block_cap_keeps_the_tail() {
        let mut b = block("hello ", false, 1);
        super::push_block_text(&mut b, &"x".repeat(500), 100);
        assert!(b.text.len() < 200, "clipped to roughly the cap");
        assert!(b.text.ends_with('x'), "the tail survives");
        assert!(b.truncated_tail);
        assert!(b.text.starts_with("[... 456 bytes omitted ...]"));
    }

    #[test]
    fn block_cap_cuts_on_char_boundaries() {
        let mut b = block("", false, 1);
        // The keep-window may slice a multi-byte char in half.
        super::push_block_text(&mut b, &"é".repeat(200), 50);
        assert!(b.text.is_char_boundary(b.text.len()));
    }

    #[test]
    fn closed_block_compresses_and_round_trips() {
        let original = "line of build output\n".repeat(200);
        let mut b = block(&original, true, 1);
        super::finish_block(&mut b);
        assert!(b.compressed.is_some(), "repetitive text compresses");
        assert!(b.text.is_empty(), "the live buffer gives way to the codec");
        assert!(b.stored_len() < original.len() / 2, "the budget sees the compressed size");
        let stored = b.stored_len();
        assert_eq!(b.output_text(), original, "the preview gets the exact bytes back");
        assert!(b.compressed.is_some(), "previewing preserves pool compression");
        assert!(b.text.is_empty());
        assert_eq!(b.stored_len(), stored, "previewing preserves budget accounting");
        assert_eq!(b.readable_text(), original, "the query path gets the exact bytes back");
        assert!(b.compressed.is_none(), "a second query pays nothing");
        // Small blocks are not worth a frame header.
        let mut s = block("tiny", true, 2);
        super::finish_block(&mut s);
        assert!(s.compressed.is_none());
        assert_eq!(s.readable_text(), "tiny");
    }

    #[test]
    fn budget_evicts_oldest_closed_first() {
        let mut map: std::collections::HashMap<usize, VecDeque<super::CommandBlock>> =
            std::collections::HashMap::new();
        // Tab 9: three closed blocks; tab 4: one closed and one open.
        map.insert(9, vec![block("aaaa", true, 1), block("bbbb", true, 2), block("cccc", true, 3)].into());
        map.insert(4, vec![block("dddd", true, 4), block("eeee", false, 5)].into());
        // Budget for exactly two 4-byte blocks.
        super::evict_to_budget(&mut map, 8);
        assert_eq!(map[&9].len(), 0, "global LRU: all three of tab 9 predate tab 4's");
        assert_eq!(map[&4].len(), 2, "the open block is sacred and fills the budget exactly");
        // An impossible budget strips everything that can be stripped.
        super::evict_to_budget(&mut map, 0);
        assert_eq!(map[&4].len(), 1);
        assert!(!map[&4][0].done, "the open block survives any budget");
    }

    #[test]
    fn all_open_over_budget_is_tolerated() {
        let mut map: std::collections::HashMap<usize, VecDeque<super::CommandBlock>> =
            std::collections::HashMap::new();
        map.insert(1, vec![block("aaaa", false, 1), block("bbbb", false, 2)].into());
        super::evict_to_budget(&mut map, 1); // nothing is closed
        assert_eq!(map[&1].len(), 2, "running commands are sacred, over budget or not");
    }

    #[test]
    fn last_one_is_bare_pasteable_text() {
        let mut q: VecDeque<super::CommandBlock> = VecDeque::new();
        apply_sem_mark(&mut q, 0, None, 0, 0, false, 1);
        apply_sem_mark(&mut q, 3, Some(0), 4, 4, false, 1);
        q.back_mut().unwrap().text = "total 42\nREADME.md\n".into();
        let out = super::format_last_output(&mut q, 1);
        assert_eq!(out, "total 42\nREADME.md\n", "one block pipes like a file, headers would corrupt it");
    }

    #[test]
    fn last_many_navigates_with_headers() {
        let mut q: VecDeque<super::CommandBlock> = VecDeque::new();
        for (i, (exit, text)) in [(Some(2), "one\n"), (Some(0), "two\n")].iter().enumerate() {
            apply_sem_mark(&mut q, 0, None, i, i, false, i as u64 + 1);
            apply_sem_mark(&mut q, 3, *exit, i + 1, i + 1, false, i as u64 + 1);
            q.back_mut().unwrap().text = text.to_string();
        }
        let out = super::format_last_output(&mut q, 5);
        assert!(out.contains("1 of 2 · exit 2"), "oldest-numbered first, failing command visible: {out}");
        assert!(out.contains("2 of 2 · ok"));
        // Chronological: newest last, beside the prompt the user reads next.
        assert!(out.find("one").unwrap() < out.find("two").unwrap());
    }

    #[test]
    fn the_running_query_command_yields_its_slot() {
        // `mypixels last 1 --copy` runs AS a command: its own block is
        // open and textless. The answer must be the last finished
        // command, not an empty copy.
        let mut q: VecDeque<super::CommandBlock> = VecDeque::new();
        apply_sem_mark(&mut q, 0, None, 0, 0, false, 1);
        apply_sem_mark(&mut q, 3, Some(0), 4, 4, false, 1);
        q.back_mut().unwrap().text = "echoed\n".into();
        apply_sem_mark(&mut q, 0, None, 5, 5, false, 2); // the query itself
        let out = super::format_last_output(&mut q, 1);
        assert_eq!(out, "echoed\n", "the live query block is skipped, not served");
    }

    #[test]
    fn last_reports_what_cannot_be_shown() {
        let mut q: VecDeque<super::CommandBlock> = VecDeque::new();
        // A full-screen program and a head-truncated giant.
        apply_sem_mark(&mut q, 0, None, 0, 0, true, 1);
        apply_sem_mark(&mut q, 3, Some(0), 4, 4, false, 1);
        apply_sem_mark(&mut q, 0, None, 5, 5, false, 2);
        apply_sem_mark(&mut q, 3, Some(0), 9, 9, false, 2);
        q.back_mut().unwrap().truncated_head = true;
        let out = super::format_last_output(&mut q, 2);
        assert!(out.contains("full-screen program"));
        assert!(out.contains("truncated: head"));
    }
}
