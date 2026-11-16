//! Configuration with defaults ported from the Python version's config dict,
//! plus keybinding parsing ("MOD1+SHIFT+Left", "CONTROL+ALT+x", "MOD1+_1" ...).

use winit::keyboard::{Key, ModifiersState, NamedKey, PhysicalKey};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceButtonPosition { Left, Right }

impl WorkspaceButtonPosition {
    pub fn as_str(self) -> &'static str {
        match self { Self::Left => "left", Self::Right => "right" }
    }

    pub fn label(self) -> &'static str {
        match self { Self::Left => "Left", Self::Right => "Right" }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value { "left" => Some(Self::Left), "right" => Some(Self::Right), _ => None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceButtonDisplay { Number, Name, Icon, IconAndName }

impl WorkspaceButtonDisplay {
    pub const ALL: [Self; 4] = [Self::Number, Self::Name, Self::Icon, Self::IconAndName];
    pub fn as_str(self) -> &'static str {
        match self { Self::Number => "number", Self::Name => "name", Self::Icon => "icon", Self::IconAndName => "icon_and_name" }
    }
    pub fn label(self) -> &'static str {
        match self { Self::Number => "Number", Self::Name => "Name", Self::Icon => "Icon", Self::IconAndName => "Icon and name" }
    }
    pub fn parse(value: &str) -> Option<Self> { Self::ALL.into_iter().find(|mode| mode.as_str() == value) }
    pub fn shows_icon(self) -> bool { matches!(self, Self::Icon | Self::IconAndName) }
    pub fn text(self, number: usize, name: &str) -> String {
        match self {
            Self::Icon => String::new(),
            Self::Number => number.to_string(),
            _ if name.trim().is_empty() => number.to_string(),
            _ => name.trim().to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceIcon { Code, Layers, Grid, Terminal, Cube, Branch, Database, Flask, Stack, Sliders }

impl WorkspaceIcon {
    pub const ALL: [Self; 10] = [Self::Code, Self::Layers, Self::Grid, Self::Terminal, Self::Cube,
        Self::Branch, Self::Database, Self::Flask, Self::Stack, Self::Sliders];
    pub fn as_str(self) -> &'static str {
        match self { Self::Code => "code", Self::Layers => "layers", Self::Grid => "grid", Self::Terminal => "terminal",
            Self::Cube => "cube", Self::Branch => "branch", Self::Database => "database", Self::Flask => "flask",
            Self::Stack => "stack", Self::Sliders => "sliders" }
    }
    pub fn label(self) -> &'static str {
        match self { Self::Code => "Code", Self::Layers => "Layers", Self::Grid => "Grid", Self::Terminal => "Terminal",
            Self::Cube => "Cube", Self::Branch => "Branch", Self::Database => "Database", Self::Flask => "Flask",
            Self::Stack => "Stack", Self::Sliders => "Sliders" }
    }
    pub fn parse(value: &str) -> Option<Self> { Self::ALL.into_iter().find(|icon| icon.as_str() == value) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseWindowDialog { Never, Loss, Always }

impl CloseWindowDialog {
    pub fn as_str(self) -> &'static str {
        match self { Self::Never => "never", Self::Loss => "loss", Self::Always => "always" }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value { "never" => Some(Self::Never), "loss" => Some(Self::Loss),
            "always" => Some(Self::Always), _ => None }
    }

    pub fn label(self) -> &'static str {
        match self { Self::Never => "Never", Self::Loss => "When tabs would be lost", Self::Always => "Always" }
    }

    pub fn should_show(self, recoverable: bool) -> bool {
        match self { Self::Never => false, Self::Loss => !recoverable, Self::Always => true }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn to_f32(&self) -> [f32; 3] {
        [self.0 as f32 / 255.0, self.1 as f32 / 255.0, self.2 as f32 / 255.0]
    }
}

impl Rgb {
    /// Legacy GDK configs also use #rrrrggggbbbb (16 bits per channel).
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('#');
        if !s.is_ascii() { return None; }
        if s.len() == 3 {
            let v = u32::from_str_radix(s, 16).ok()?;
            let (r, g, b) = ((v >> 8 & 0xf) as u8, (v >> 4 & 0xf) as u8, (v & 0xf) as u8);
            return Some(Rgb(r * 17, g * 17, b * 17));
        }
        if s.len() == 6 {
            let v = u32::from_str_radix(s, 16).ok()?;
            return Some(Rgb((v >> 16 & 0xff) as u8, (v >> 8 & 0xff) as u8, (v & 0xff) as u8));
        }
        if s.len() == 12 {
            let channel = |part| u16::from_str_radix(part, 16).ok().map(|v| ((v as u32 + 128) / 257) as u8);
            return Some(Rgb(channel(&s[0..4])?, channel(&s[4..8])?, channel(&s[8..12])?));
        }
        None
    }
}

pub fn parse_hex(s: &str) -> Rgb {
    Rgb::parse(s).unwrap_or(Rgb(0, 0, 0))
}

/// "Cascadia Code 9" -> ("Cascadia Code", 9.0). Pango-style: the size is
/// the last whitespace-separated token; everything before is the family.
pub fn parse_font(s: &str) -> (String, f32) {
    let s = s.trim();
    match s.rfind(char::is_whitespace) {
        Some(idx) if idx + 1 < s.len() => {
            let size: f32 = s[idx + 1..].trim().parse().unwrap_or(12.0);
            (s[..idx].trim().to_string(), size)
        },
        _ => (s.to_string(), 12.0),
    }
}

/// Colon-separated hex colors (terminator/wal format). Takes the first 16;
/// more than 16 (256-color palettes) is fine. Requires at least 16 so a
/// truncated palette doesn't silently pad the rest with black.
#[cfg(test)]
fn parse_palette(s: &str) -> Option<[Rgb; 16]> {
    crate::theme::parse_palette(&serde_yaml::Value::String(s.to_string())).ok()
}

#[derive(Clone)]
pub struct Config {
    // tablist
    pub tablist_background: Rgb,
    /// Workspace identity colors, W1 through W10. Tab chrome is derived from these.
    pub workspace_colors: [Rgb; 10],
    /// Optional purpose labels, independent of any terminal working directory.
    pub workspace_genres: [String; 10],
    pub workspace_icons: [WorkspaceIcon; 10],
    pub tab_color_bg: Rgb,
    pub tab_color_bg_selected: Rgb,
    pub tab_color_fg: Rgb,
    pub tab_color_fg_selected: Rgb,
    pub tab_color_entangle: Rgb,
    pub tab_color_idx: Rgb,
    pub add_button_color: Rgb,
    pub tab_title_max_length: usize,
    pub tab_title_short_current: usize,
    pub tab_shorten_str: String,
    // terminal
    pub foreground_color: Rgb,
    pub background_color: Rgb,
    pub selection_color: Rgb,
    pub cursor_color: Rgb,
    pub accent_color: Rgb,
    /// UI transition speed multiplier: 1 is normal, 0 disables animation.
    pub animation_speed: f32,
    /// Terminal color brightness in percent; leaves the source palette intact.
    pub terminal_brightness: f32,
    /// The selected theme (preset name, `wal`, or a YAML path); empty means
    /// none. Kept as written so the picker and the file agree after a save.
    pub theme: String,
    /// External color sources monitored by the theme worker, never by draw().
    pub theme_files: Vec<std::path::PathBuf>,
    /// 16-color palette (None = alacritty defaults). Terminal runtime OSC 4
    /// changes override these per color.
    pub palette: Option<[Rgb; 16]>,
    pub term_font: String,
    /// Extra row height as a fraction of the font's normal line height.
    pub line_spacing: f32,
    pub tablist_font: String,
    /// Scrollback depth. Costs width*24+40 bytes per line (full-width
    /// Vec<Cell>), so raising this is a real RAM decision; the command
    /// output pool (`history_bytes`) is what keeps deep history cheap.
    pub scrollback_lines: usize,
    /// Global budget for the command-output pool (all tabs, LRU-evicted
    /// oldest closed block first). 32 MB typical-use is never approached.
    pub history_bytes: usize,
    /// A single command's captured output is capped here, with an explicit
    /// truncation marker - one `ls -lR /` cannot eat the whole pool.
    pub history_block_bytes: usize,
    // tab color groups
    pub col: [Rgb; 8],
    // behavior
    /// Enables the Alt-held visual tab switcher. Numbered shortcuts work independently.
    pub tab_overview_on_alt: bool,
    pub show_tab_titles: bool,
    pub compact_tabs: bool,
    /// Ten independent tab sets per window. Off by default.
    pub workspaces_enabled: bool,
    /// Which end of the tab strip holds the workspace button.
    pub workspace_button_position: WorkspaceButtonPosition,
    pub workspace_button_display: WorkspaceButtonDisplay,
    /// Opt-in alternative workspace shortcut: Alt+Q..P. Off by default because
    /// bare Alt+letter collides with shell and editor bindings (word-wise
    /// backward word, readline shortcuts). The standard scheme is
    /// Alt+Shift+digit, which shares no range with those.
    pub workspace_letter_keys: bool,
    pub tab_overview_opacity: f32,
    pub tab_overview_delay_ms: u32,
    /// Switcher preview scale: the tab's frame shrunk by this factor (1.0 =
    /// window size). The preview keeps the terminal's own aspect ratio, so a
    /// single factor is the whole setting.
    pub tab_overview_scale: f32,
    pub focus_new_tabs: bool,
    /// Where a new tab lands: `next` (right of the current tab, the
    /// default), `end` (after the last tab) or `first` (at the start).
    pub new_tab_position: String,
    pub hide_single_tab: bool,
    /// How many closed tabs and, separately, whole closed windows to keep
    /// running. Each oldest burial dies when its respective limit is exceeded.
    pub graveyard_limit: usize,
    pub close_window_dialog: CloseWindowDialog,
    /// OSC 133 semantic shell integration: fold the shell's prompt/command
    /// marks into navigable command blocks (scripts/shell_integration.*).
    pub osc133: bool,
    /// kitty-compatible identity in TERMINAL ("mypixels-kitty") for
    /// allowlist-style detectors like broot. Never touches TERM (terminfo
    /// honesty); the claim is backed by passing kitty's own icat
    /// conformance. See docs/graphics notes.
    pub kitty_compatible: bool,
    pub multiline_tabs: bool,
    pub tablist_top: bool,
    pub tablist_hide: bool,
    pub tab_title_shorten_start: bool,
    pub default_tab_title: String,
    pub default_width: u32,
    pub default_height: u32,
    pub default_maximize: bool,
    /// Amount of *transparency*: 0 = opaque, 1 = fully see-through. Named
    /// for what it does; the inverted `term_opacity` spelling it replaces is
    /// still parsed (see term_opacity_legacy).
    pub term_transparency: f32,
    /// Deprecated `term_opacity:` value, kept only so a file predating the
    /// rename keeps working. The new key always wins when both are present.
    term_opacity_legacy: Option<f32>,
    transparency_explicit: bool,
    pub bg_tint_color: Rgb,
    pub tab_format: String,
    pub tab_append: Option<String>,
    pub autohide_delay: i32,
    // keybindings
    pub bindings: Vec<(String, Vec<Binding>)>,
}

impl Default for Config {
    fn default() -> Self {
        let mut bindings: Vec<(String, Vec<Binding>)> = Vec::new();
        let mut bind = |k: &str, v: &[&str]| {
            let list: Vec<Binding> = v.iter().filter_map(|s| Binding::parse(s)).collect();
            bindings.push((k.to_string(), list));
        };

        bind("move_prev_tab", &["MOD1+SHIFT+Left"]);
        bind("move_next_tab", &["MOD1+SHIFT+Right"]);
        bind("move_first_tab", &["MOD1+SHIFT+Home"]);
        bind("move_last_tab", &["MOD1+SHIFT+End"]);
        bind("prev_tab", &["MOD1+Left", "MOD1+SHIFT+f"]);
        bind("next_tab", &["MOD1+Right", "MOD1+SHIFT+b"]);
        bind("prev_tab_row", &["MOD1+Up"]);
        bind("next_tab_row", &["MOD1+Down"]);
        bind("prev_workspace", &["MOD1+Page_Up"]);
        bind("next_workspace", &["MOD1+Page_Down"]);
        bind("first_tab", &["MOD1+Home"]);
        bind("last_tab", &["MOD1+End"]);
        bind("close_tab", &["CONTROL+ALT+x", "CONTROL+ALT+q"]);
        bind("prev_command", &["CONTROL+ALT+Up"]);
        bind("next_command", &["CONTROL+ALT+Down"]);
        bind("reopen_tab", &["CONTROL+SHIFT+z"]);
        bind("reopen_window", &["CONTROL+MOD1+SHIFT+z"]);
        bind("graveyard", &["CONTROL+SHIFT+g"]);
        // Deliberately unbound: the tab-bar button is the panel's control, and
        // every Alt+Shift+letter that would suit it (W especially) already
        // belongs to something. Bind it in Settings > Keys if you want a chord.
        bind("show_workspaces", &[]);
        bind("new_tab", &["CONTROL+ALT+z", "CONTROL+SHIFT+t"]);
        bind("new_window", &["CONTROL+SHIFT+n"]);
        bind("send_tab_to_window", &["MOD1+SHIFT+w"]);
        bind("move_tab_to_workspace", &["MOD1+SHIFT+m"]);
        // Copy/paste follow the VTE and GNOME Terminal bindings the original gets
        // from GTK, so the fingers that already know them keep working. Selecting
        // text feeds the *primary* buffer separately and needs no key.
        bind("copy", &["CONTROL+SHIFT+c"]);
        bind("paste", &["CONTROL+SHIFT+v"]);
        // Copy the last command's captured output (the OSC 133 pool) without
        // leaving the terminal - the in-app twin of `mypixels last --copy`.
        bind("copy_last_output", &["CONTROL+SHIFT+l"]);
        // The command-history panel (list, filters, search, actions).
        bind("show_history", &["MOD1+SHIFT+h"]);
        bind("select_all", &["CONTROL+SHIFT+a"]);
        bind("paste_primary", &["SHIFT+Insert"]);
        bind("scroll_page_up", &["SHIFT+Page_Up"]);
        bind("scroll_page_down", &["SHIFT+Page_Down"]);
        bind("scroll_bottom", &["CONTROL+SHIFT+End"]);
        bind("shrink_font", &["MOD1+SHIFT+KP_Subtract"]);
        bind("enlarge_font", &["MOD1+SHIFT+KP_Add"]);
        bind("toggle_hide_tabs", &["CONTROL+ALT+v"]);
        bind("prev_color", &["MOD1+SHIFT+braceleft"]);
        bind("next_color", &["MOD1+SHIFT+braceright"]);
        bind("toggle_entangle", &["MOD1+SHIFT+KP_Multiply"]);
        bind("rename_tab", &["MOD1+SHIFT+n"]);
        bind("show_search", &["MOD1+SHIFT+s"]);
        bind("show_search_all", &["MOD1+SHIFT+g"]);
        bind("search_prev", &["MOD1+SHIFT+a"]);
        bind("search_next", &["MOD1+SHIFT+d"]);
        bind("config_reload", &["CONTROL+SHIFT+r"]);
        bind("show_config", &["MOD1+SHIFT+c"]);
        for (i, d) in (0..10).zip(["_1", "_2", "_3", "_4", "_5", "_6", "_7", "_8", "_9", "_0"]) {
            bind(&format!("goto_tab_{i}"), &[&format!("MOD1+{d}")]);
        }

        for (i, digit) in ["_1", "_2", "_3", "_4", "_5", "_6", "_7", "_8", "_9", "_0"].iter().enumerate() {
            // Alt+Shift+digit: the standard scheme. Bare Alt+letter is opt-in
            // (`workspace_letter_keys`) because it masks shell key combinations.
            bind(&format!("goto_workspace_{i}"), &[&format!("MOD1+SHIFT+{digit}")]);
        }

        Self {
            tablist_background: parse_hex("#303030"),
            workspace_colors: [
                "#5e9eff", "#f07b72", "#71c58c", "#e7b85a", "#b792ed",
                "#5fc7d0", "#e998be", "#ec9756", "#bbc96a", "#95a8c4",
            ].map(parse_hex),
            workspace_genres: std::array::from_fn(|_| String::new()),
            workspace_icons: [WorkspaceIcon::Layers; 10],
            tab_color_bg: parse_hex("#303030"),
            tab_color_bg_selected: parse_hex("#505020"),
            tab_color_fg: parse_hex("#888888"),
            tab_color_fg_selected: parse_hex("#ffffff"),
            tab_color_entangle: parse_hex("#ff0000"),
            tab_color_idx: parse_hex("#808080"),
            add_button_color: parse_hex("#ffa500"),
            tab_title_max_length: 16,
            tab_title_short_current: 32,
            tab_shorten_str: "\u{2026}".to_string(),
            foreground_color: parse_hex("#bbbbbb"),
            background_color: parse_hex("#000000"),
            selection_color: parse_hex("#3584e4"),
            cursor_color: Rgb(140, 140, 153),
            accent_color: Rgb(92, 191, 191),
            animation_speed: 1.0,
            terminal_brightness: 0.0,
            theme: String::new(),
            theme_files: Vec::new(),
            palette: None,
            term_font: "Terminus 8".to_string(),
            line_spacing: 0.0,
            tablist_font: "Liberation Mono 7".to_string(),
            scrollback_lines: 5000,
            history_bytes: 32 * 1024 * 1024,
            history_block_bytes: 8 * 1024 * 1024,
            col: [
                parse_hex("#b58900"),
                parse_hex("#cb4b16"),
                parse_hex("#dc322f"),
                parse_hex("#d33682"),
                parse_hex("#6c71c4"),
                parse_hex("#268bd2"),
                parse_hex("#2aa198"),
                parse_hex("#859900"),
            ],
            tab_overview_on_alt: true,
            show_tab_titles: true,
            compact_tabs: false,
            workspaces_enabled: false,
            workspace_button_position: WorkspaceButtonPosition::Right,
            workspace_button_display: WorkspaceButtonDisplay::Name,
            workspace_letter_keys: false,
            tab_overview_opacity: 0.3,
            tab_overview_delay_ms: 500,
            tab_overview_scale: 0.25,
            focus_new_tabs: true,
            new_tab_position: "next".to_string(),
            hide_single_tab: false,
            graveyard_limit: 5,
            close_window_dialog: CloseWindowDialog::Always,
            osc133: true,
            kitty_compatible: true,
            multiline_tabs: true,
            tablist_top: true,
            tablist_hide: false,
            tab_title_shorten_start: false,
            default_tab_title: String::new(),
            default_width: 640,
            default_height: 480,
            default_maximize: false,
            term_transparency: 0.1,
            term_opacity_legacy: None,
            transparency_explicit: false,
            bg_tint_color: parse_hex("#140008"),
            tab_format: "<span %(color)s>%(tcorner)s%(entangled)s%(tname)s]</span>".to_string(),
            tab_append: None,
            autohide_delay: -1,
            bindings,
        }
    }
}

impl Config {
    /// Startup can fall back; explicit/live reload uses try_load to keep the
    /// last valid configuration when a file is incomplete or invalid.
    pub fn load() -> Config {
        Self::try_load().unwrap_or_else(|e| { log::warn!("{e}; using defaults"); Self::default() })
    }

    /// The file the configuration is read from (and written to on save):
    /// $MYPIXELS_CONFIG (or the deprecated $SPIKE_CONFIG), else
    /// $XDG_CONFIG_HOME/mypixels/config.yml
    /// (~/.config/mypixels/config.yml by default). A legacy ~/.mypixels that
    /// predates the move keeps working while the XDG file does not exist --
    /// nobody's configuration gets orphaned by the relocation, and saving
    /// writes back to the very file that was loaded.
    pub fn config_path() -> Option<std::path::PathBuf> {
        Self::config_path_in(
            Self::env_override("MYPIXELS_CONFIG"),
            dirs::config_dir().as_deref(),
            dirs::home_dir().as_deref(),
        )
    }

    fn config_path_in(
        override_: Option<std::path::PathBuf>,
        xdg: Option<&std::path::Path>,
        home: Option<&std::path::Path>,
    ) -> Option<std::path::PathBuf> {
        if let Some(p) = override_.as_deref() {
            return Some(p.to_path_buf());
        }
        let modern = xdg?.join("mypixels").join("config.yml");
        if modern.is_file() {
            return Some(modern);
        }
        let legacy = home?.join(".mypixels");
        if legacy.is_file() {
            return Some(legacy);
        }
        Some(modern)
    }

    /// A path environment override, current name first. The SPIKE_ spellings
    /// predate the rename from prototype binary to product and still work --
    /// muscle memory is a real interface.
    pub(crate) fn env_override(name: &str) -> Option<std::path::PathBuf> {
        let legacy = format!("SPIKE_{}", name.strip_prefix("MYPIXELS_")?);
        std::env::var_os(name)
            .or_else(|| std::env::var_os(legacy))
            .map(std::path::PathBuf::from)
    }

    pub fn try_load() -> Result<Config, String> {
        let path = Self::config_path().ok_or("Cannot determine configuration path")?;
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("Cannot read {}: {e}", path.display())),
        };
        Self::from_text(&text, path.parent().unwrap_or(std::path::Path::new(".")))
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn adjust_terminal_color(&self, color: [f32; 3]) -> [f32; 3] {
        if self.terminal_brightness == 0.0 { return color; }
        let factor = 1.0 + self.terminal_brightness / 100.0;
        color.map(|channel| (channel * factor).clamp(0.0, 1.0))
    }

    pub fn terminal_line_height(&self, normal_height: f32) -> f32 {
        (normal_height * (1.0 + self.line_spacing)).round().max(1.0)
    }

    pub fn from_text(text: &str, base: &std::path::Path) -> Result<Config, String> {
        let text = dedupe_top_level(&text.replace("!!python/tuple", ""));
        let mut value: serde_yaml::Value = serde_yaml::from_str(&text).map_err(|e| format!("Invalid config YAML: {e}"))?;
        if value.is_null() { value = serde_yaml::Value::Mapping(serde_yaml::Mapping::new()); }
        if !value.is_mapping() { return Err("Configuration must be a YAML mapping".into()); }
        let mut cfg = Self::default();
        crate::theme::configure(&mut cfg, &value, base)?;
        // Legacy `term_opacity:` feeds the renamed setting only when the
        // file does not carry the new key - file order cannot matter, and a
        // save (which only ever writes the new key) supersedes the stale
        // old line that replace-in-place cannot remove.
        if let Some(legacy) = cfg.term_opacity_legacy {
            if !cfg.transparency_explicit {
                cfg.term_transparency = legacy;
            }
        }
        cfg.theme_files.sort();
        cfg.theme_files.dedup();
        cfg.apply_workspace_letter_keys();
        Ok(cfg)
    }

    /// Add the opt-in Alt+Q..P form alongside the standard Alt+Shift+digit
    /// scheme. Runs after a file is merged, because the default bindings are
    /// built before any setting is read. A binding the file states outright
    /// is left alone.
    fn apply_workspace_letter_keys(&mut self) {
        if !self.workspace_letter_keys {
            return;
        }
        for (i, letter) in WORKSPACE_LETTERS.chars().enumerate() {
            let action = format!("goto_workspace_{i}");
            let Some(letter_binding) = Binding::parse(&format!("MOD1+{letter}")) else { continue };
            if let Some((_, list)) = self.bindings.iter_mut().find(|(a, _)| *a == action) {
                if !list.iter().any(|b| b.key == letter_binding.key && b.mods == letter_binding.mods) {
                    list.push(letter_binding.clone());
                }
            } else {
                self.bindings.push((action, vec![letter_binding]));
            }
        }
    }

    /// Persist the fields the configuration UI edits, in place: each
    /// top-level key's value line is replaced, anything else (comments,
    /// bindings, palettes, keys the UI does not touch) stays byte for
    /// byte. Keys the file does not have yet are appended.
    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path().ok_or("Cannot determine configuration path")?;
        self.save_to(&path)
    }

    /// Save, optionally rewriting the `bindings:` block too. Bindings are
    /// only rewritten when the configuration UI actually changed one, so a
    /// hand-written block and its comments survive every other save.
    pub fn save_with_bindings(&self, include_bindings: bool) -> Result<(), String> {
        let path = Self::config_path().ok_or("Cannot determine configuration path")?;
        self.save_to_with(&path, include_bindings)
    }

    fn save_to_with(&self, path: &std::path::Path, include_bindings: bool) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let out = replace_top_level_values(&text, &Self::ui_keys(self));
        let out = if include_bindings {
            replace_top_level_block(&out, "bindings", &self.bindings_yaml())
        } else {
            out
        };
        std::fs::write(path, out).map_err(|e| format!("Cannot write {}: {e}", path.display()))
    }

    /// The actions and chords this config would ship with, for a reset.
    pub fn default_binding(action: &str) -> Vec<Binding> {
        DEFAULT_BINDINGS.iter()
            .find(|(a, _)| a == action)
            .map(|(_, list)| list.clone())
            .unwrap_or_default()
    }

    /// What this action is bound to now. Empty means deliberately unbound.
    pub fn bindings_for(&self, action: &str) -> &[Binding] {
        self.bindings.iter()
            .find(|(a, _)| a == action)
            .map(|(_, list)| list.as_slice())
            .unwrap_or(&[])
    }

    /// Bind an action to exactly these chords. An empty list unbinds it.
    pub fn set_binding(&mut self, action: &str, list: Vec<Binding>) {
        match self.bindings.iter_mut().find(|(a, _)| a == action) {
            Some(entry) => entry.1 = list,
            None => self.bindings.push((action.to_string(), list)),
        }
    }

    /// The `bindings:` block as a file writes it: one action per line, in the
    /// order the config holds them, quoted so symbols survive a YAML read.
    pub fn bindings_yaml(&self) -> String {
        let mut out = String::from("bindings:\n");
        for (action, list) in &self.bindings {
            let specs: Vec<String> = list.iter().map(|b| b.spec()).collect();
            match specs.len() {
                0 => out.push_str(&format!("  {action}: none\n")),
                1 => out.push_str(&format!("  {action}: '{}'\n", specs[0])),
                _ => out.push_str(&format!("  {action}: [{}]\n",
                    specs.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(", "))),
            }
        }
        out
    }

    pub fn save_to(&self, path: &std::path::Path) -> Result<(), String> {
        self.save_to_with(path, false)
    }

    /// (yaml key, value text) for every field the configuration UI can change.
    fn ui_keys(&self) -> Vec<(String, String)> {
        let rgb = |r: &Rgb| format!("'#{:02x}{:02x}{:02x}'", r.0, r.1, r.2);
        let str_ = |s: &str| format!("'{}'", s.replace('\'', "''"));
        let mut keys = vec![
            ("term_font".into(), str_(&self.term_font)),
            ("line_spacing".into(), self.line_spacing.to_string()),
            ("tablist_font".into(), str_(&self.tablist_font)),
            ("term_transparency".into(), self.term_transparency.to_string()),
            ("scrollback_lines".into(), self.scrollback_lines.to_string()),
            ("history_bytes".into(), self.history_bytes.to_string()),
            ("history_block_bytes".into(), self.history_block_bytes.to_string()),
            ("background_color".into(), rgb(&self.background_color)),
            ("foreground_color".into(), rgb(&self.foreground_color)),
            ("selection_color".into(), rgb(&self.selection_color)),
            ("cursor_color".into(), rgb(&self.cursor_color)),
            ("accent_color".into(), rgb(&self.accent_color)),
            ("animation_speed".into(), self.animation_speed.to_string()),
            ("terminal_brightness".into(), self.terminal_brightness.to_string()),
            ("bg_tint_color".into(), rgb(&self.bg_tint_color)),
            ("tablist_background".into(), rgb(&self.tablist_background)),
            ("add_button_color".into(), rgb(&self.add_button_color)),
            ("tab_title_max_length".into(), self.tab_title_max_length.to_string()),
            ("tab_title_short_current".into(), self.tab_title_short_current.to_string()),
            ("tab_overview_on_alt".into(), self.tab_overview_on_alt.to_string()),
            ("show_tab_titles".into(), self.show_tab_titles.to_string()),
            ("compact_tabs".into(), self.compact_tabs.to_string()),
            ("workspaces_enabled".into(), self.workspaces_enabled.to_string()),
            ("workspace_button_position".into(), str_(self.workspace_button_position.as_str())),
            ("workspace_button_display".into(), str_(self.workspace_button_display.as_str())),
            ("workspace_letter_keys".into(), self.workspace_letter_keys.to_string()),
            ("tab_overview_opacity".into(), self.tab_overview_opacity.to_string()),
            ("tab_overview_delay_ms".into(), self.tab_overview_delay_ms.to_string()),
            ("tab_overview_scale".into(), self.tab_overview_scale.to_string()),
            ("focus_new_tabs".into(), self.focus_new_tabs.to_string()),
            ("new_tab_position".into(), str_(&self.new_tab_position)),
            ("hide_single_tab".into(), self.hide_single_tab.to_string()),
            ("graveyard_limit".into(), self.graveyard_limit.to_string()),
            ("close_window_dialog".into(), str_(self.close_window_dialog.as_str())),
            ("osc133".into(), self.osc133.to_string()),
            ("kitty_compatible".into(), self.kitty_compatible.to_string()),
            ("multiline_tabs".into(), self.multiline_tabs.to_string()),
            ("tablist_top".into(), self.tablist_top.to_string()),
            ("tablist_hide".into(), self.tablist_hide.to_string()),
            ("tab_title_shorten_start".into(), self.tab_title_shorten_start.to_string()),
            ("default_width".into(), self.default_width.to_string()),
            ("default_height".into(), self.default_height.to_string()),
            ("default_maximize".into(), self.default_maximize.to_string()),
        ];
        for (i, color) in self.workspace_colors.iter().enumerate() {
            keys.push((format!("workspace_color_{}", i + 1), rgb(color)));
        }
        for (i, genre) in self.workspace_genres.iter().enumerate() {
            keys.push((format!("workspace_genre_{}", i + 1), str_(genre)));
        }
        for (i, icon) in self.workspace_icons.iter().enumerate() {
            keys.push((format!("workspace_icon_{}", i + 1), str_(icon.as_str())));
        }
        for (i, c) in self.col.iter().enumerate() {
            keys.push((format!("col_{i}"), rgb(c)));
        }
        if !self.theme.is_empty() {
            keys.push(("theme".into(), str_(&self.theme)));
        }
        keys
    }
}

/// Replace the value of each top-level `key:` line in `text`; absent keys
/// are appended. A top-level key line starts at column 0 with `key:`, so
/// replacing `col_0` cannot touch `col_1`, and indented keys (bindings,
/// palette lists) never match. Everything after the colon on the line is
/// the value -- true for every scalar in this file format.
pub fn replace_top_level_values(text: &str, keys: &[(String, String)]) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    for (key, value) in keys {
        let prefix = format!("{key}:");
        match lines.iter().position(|l| l == key || l.starts_with(&prefix)) {
            Some(i) => lines[i] = format!("{prefix} {value}"),
            None => lines.push(format!("{prefix} {value}")),
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Swap a whole top-level YAML block (the key line plus its indented body,
/// or its single-line flow value) for another. Used for `bindings:`, whose
/// body is many lines and so cannot go through replace_top_level_values.
pub fn replace_top_level_block(text: &str, key: &str, block: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let indent = |l: &str| l.starts_with(' ') || l.starts_with('\t');
    let Some(at) = lines.iter().position(|l| *l == key || l.starts_with(&format!("{key}:"))) else {
        // Not present: the block goes at the end, after any single-line keys.
        let mut out = text.trim_end().to_string();
        out.push('\n');
        out.push_str(block);
        return out;
    };
    let mut end = at + 1;
    while end < lines.len() {
        if indent(lines[end]) {
            end += 1;
        } else if lines[end].trim().is_empty()
            && lines.get(end + 1).is_some_and(|l| indent(l) && !l.trim().is_empty()) {
            end += 1;
        } else {
            break;
        }
    }
    let mut out: Vec<String> = lines[..at].iter().map(|l| l.to_string()).collect();
    out.extend(block.trim_end_matches('\n').lines().map(str::to_string));
    out.extend(lines[end..].iter().map(|l| l.to_string()));
    let mut out = out.join("\n");
    out.push('\n');
    out
}

fn top_level_key(line: &str) -> Option<String> {
    let mut chars = line.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() && first != '_' {
        return None;
    }
    for c in chars {
        if c == ':' {
            return line.split(':').next().map(String::from);
        }
        if !c.is_ascii_alphanumeric() && c != '_' {
            return None;
        }
    }
    None
}

/// Keep only the last occurrence of each top-level key (Python YAML
/// semantics); drop the earlier blocks.
fn dedupe_top_level(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut blocks: Vec<(String, usize)> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if let Some(k) = top_level_key(l) {
            blocks.push((k, i));
        }
    }
    let mut remove = vec![false; blocks.len()];
    for (bi, (key, _)) in blocks.iter().enumerate() {
        if let Some(last) = blocks.iter().rposition(|(k, _)| k == key) {
            if last != bi {
                remove[bi] = true;
            }
        }
    }
    if !remove.iter().any(|&r| r) {
        return text.to_string();
    }
    let mut out = String::new();
    for (i, l) in lines.iter().enumerate() {
        let bi = blocks.iter().rposition(|(_, s)| *s <= i);
        let keep = match bi {
            Some(b) => !remove[b],
            None => true, // header comments before the first key
        };
        if keep {
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

/// The file spelling of a named key. Mirrors the names `parse` accepts; the
/// debug form of most variants already matches, the rest are the exceptions.
fn named_spec(key: &NamedKey) -> String {
    String::from(match key {
        NamedKey::ArrowLeft => "Left",
        NamedKey::ArrowRight => "Right",
        NamedKey::ArrowUp => "Up",
        NamedKey::ArrowDown => "Down",
        NamedKey::Enter => "Return",
        NamedKey::PageUp => "Page_Up",
        NamedKey::PageDown => "Page_Down",
        other => return format!("{other:?}"),
    })
}

/// The digit a Shifted symbol came from, so a chord recorded as "@" with
/// Shift held is stored as the readable "2" (which matches both spellings).
pub(crate) fn unshift_symbol(c: &str) -> Option<&'static str> {
    ("1234567890").chars().position(|d| shifted_digit_of(&d.to_string()) == Some(c))
        .map(|i| &"1234567890"[i..i + 1])
}

/// The key as if Shift had not been pressed, read from the key's physical
/// position. Letters are already matched case-insensitively by the binding
/// table; this exists for digits, whose shifted form differs on every layout
/// (US "@", Czech "č"), so Alt+Shift+2 could otherwise only work on one board.
pub fn unshifted_key(key: &Key, physical: PhysicalKey) -> Key {
    use winit::keyboard::KeyCode;
    let PhysicalKey::Code(code) = physical else { return key.clone() };
    let base: &'static str = match code {
        KeyCode::Digit0 => "0", KeyCode::Digit1 => "1", KeyCode::Digit2 => "2",
        KeyCode::Digit3 => "3", KeyCode::Digit4 => "4", KeyCode::Digit5 => "5",
        KeyCode::Digit6 => "6", KeyCode::Digit7 => "7", KeyCode::Digit8 => "8",
        KeyCode::Digit9 => "9",
        KeyCode::KeyA => "a", KeyCode::KeyB => "b", KeyCode::KeyC => "c",
        KeyCode::KeyD => "d", KeyCode::KeyE => "e", KeyCode::KeyF => "f",
        KeyCode::KeyG => "g", KeyCode::KeyH => "h", KeyCode::KeyI => "i",
        KeyCode::KeyJ => "j", KeyCode::KeyK => "k", KeyCode::KeyL => "l",
        KeyCode::KeyM => "m", KeyCode::KeyN => "n", KeyCode::KeyO => "o",
        KeyCode::KeyP => "p", KeyCode::KeyQ => "q", KeyCode::KeyR => "r",
        KeyCode::KeyS => "s", KeyCode::KeyT => "t", KeyCode::KeyU => "u",
        KeyCode::KeyV => "v", KeyCode::KeyW => "w", KeyCode::KeyX => "x",
        KeyCode::KeyY => "y", KeyCode::KeyZ => "z",
        _ => return key.clone(),
    };
    // Only a typed character has a shifted form to undo; named keys are already
    // position-based and must keep whatever they reported.
    match key {
        Key::Character(_) => Key::Character(base.into()),
        _ => key.clone(),
    }
}


/// The US-layout symbol that Shift puts on a digit's key. Used so a binding
/// written as Alt+Shift+2 also fires when the terminal reports Shift+2 as
/// "@". Layouts that shift digits differently keep working if the binding
/// spells the symbol their keyboard actually produces.
pub(crate) fn shifted_digit_of(c: &str) -> Option<&'static str> {
    Some(match c {
        "1" => "!", "2" => "@", "3" => "#", "4" => "$", "5" => "%",
        "6" => "^", "7" => "&", "8" => "*", "9" => "(", "0" => ")",
        _ => return None,
    })
}

/// The bindings a pristine config ships with, for the settings panel's reset.
static DEFAULT_BINDINGS: std::sync::LazyLock<Vec<(String, Vec<Binding>)>> =
    std::sync::LazyLock::new(|| Config::default().bindings);

/// Workspaces per window. Sized to the digit row, not to the letter scheme.
pub const WORKSPACES: usize = 10;
/// The opt-in letter shortcut scheme (`workspace_letter_keys`), one key per
/// workspace. Kept separate from the standard Alt+Shift+digit scheme.
pub const WORKSPACE_LETTERS: &str = "qwertyuiop";

/// The human label of a workspace's shortcut, read from the real binding so
/// the UI never advertises a key the user did not choose.
pub fn workspace_shortcut(cfg: &Config, workspace: usize) -> String {
    cfg.bindings
        .iter()
        .find(|(action, _)| *action == format!("goto_workspace_{workspace}"))
        .and_then(|(_, list)| list.first())
        .map(|b| b.label())
        .unwrap_or_else(|| format!("Alt+Shift+{}", (workspace + 1) % 10))
}

pub(crate) fn workspace_color_index(key: &str) -> Option<usize> {
    let index = key.strip_prefix("workspace_color_")?.parse::<usize>().ok()?.checked_sub(1)?;
    (index < WORKSPACES).then_some(index)
}

fn workspace_genre_index(key: &str) -> Option<usize> {
    let index = key.strip_prefix("workspace_genre_")?.parse::<usize>().ok()?.checked_sub(1)?;
    (index < WORKSPACES).then_some(index)
}

fn workspace_icon_index(key: &str) -> Option<usize> {
    let index = key.strip_prefix("workspace_icon_")?.parse::<usize>().ok()?.checked_sub(1)?;
    (index < WORKSPACES).then_some(index)
}

pub(crate) fn merge(cfg: &mut Config, v: &serde_yaml::Value) {
        let Some(map) = v.as_mapping() else {
            return;
        };
        for (k, val) in map {
            let Some(key) = k.as_str() else {
                continue;
            };
            match key {
                key if workspace_color_index(key).is_some() => {
                    set_rgb(&mut cfg.workspace_colors[workspace_color_index(key).unwrap()], val);
                }
                key if workspace_genre_index(key).is_some() => {
                    if let Some(genre) = val.as_str() {
                        cfg.workspace_genres[workspace_genre_index(key).unwrap()] = genre.trim().to_string();
                    }
                }
                key if workspace_icon_index(key).is_some() => {
                    if let Some(icon) = val.as_str().and_then(WorkspaceIcon::parse) {
                        cfg.workspace_icons[workspace_icon_index(key).unwrap()] = icon;
                    }
                }
                "workspace_button_display" => {
                    if let Some(mode) = val.as_str().and_then(WorkspaceButtonDisplay::parse) {
                        cfg.workspace_button_display = mode;
                    }
                }
                "bindings" => merge_bindings(&mut cfg.bindings, val),
                "tablist_background" => set_rgb(&mut cfg.tablist_background, val),
                "tab_color_bg" => set_rgb(&mut cfg.tab_color_bg, val),
                "tab_color_bg_selected" => set_rgb(&mut cfg.tab_color_bg_selected, val),
                "tab_color_fg" => set_rgb(&mut cfg.tab_color_fg, val),
                "tab_color_fg_selected" => set_rgb(&mut cfg.tab_color_fg_selected, val),
                "tab_color_entangle" => set_rgb(&mut cfg.tab_color_entangle, val),
                "tab_color_idx" => set_rgb(&mut cfg.tab_color_idx, val),
                "add_button_color" => set_rgb(&mut cfg.add_button_color, val),
                "terminal_brightness" => {
                    if let Some(value) = val.as_f64().filter(|v| v.is_finite() && (-100.0..=100.0).contains(v)) {
                        cfg.terminal_brightness = value as f32;
                    } else {
                        log::warn!("terminal_brightness must be a finite percentage between -100 and 100");
                    }
                },
                "foreground_color" => set_rgb(&mut cfg.foreground_color, val),
                "background_color" => set_rgb(&mut cfg.background_color, val),
                "selection_color" => set_rgb(&mut cfg.selection_color, val),
                "cursor_color" => set_rgb(&mut cfg.cursor_color, val),
                "accent_color" => set_rgb(&mut cfg.accent_color, val),
                "bg_tint_color" => set_rgb(&mut cfg.bg_tint_color, val),
                key if key.starts_with("col_") && key.len() == 5 && matches!(key.as_bytes()[4], b'0'..=b'7') => {
                    let i = (key.as_bytes()[4] as u8 - b'0') as usize;
                    if i < 8 {
                        set_rgb(&mut cfg.col[i], val);
                    }
                },
                "palette" => {
                    if val.as_str().is_some_and(|s| s.trim().is_empty()) {
                        cfg.palette = None;
                    } else {
                        match crate::theme::parse_palette(val) {
                            Ok(palette) => cfg.palette = Some(palette),
                            Err(e) => log::warn!("{e}"),
                        }
                    }
                },
                "theme" => {
                    if let Some(s) = val.as_str() {
                        cfg.theme = s.to_string();
                    }
                },
                "line_spacing" => {
                    if let Some(value) = val.as_f64().filter(|v| v.is_finite() && (-0.9..=3.0).contains(v)) {
                        cfg.line_spacing = value as f32;
                    } else {
                        log::warn!("line_spacing must be a finite number between -0.9 and 3.0");
                    }
                },
                "term_font" => {
                    if let Some(s) = val.as_str() {
                        cfg.term_font = s.to_string();
                    }
                },
                "tablist_font" => {
                    if let Some(s) = val.as_str() {
                        cfg.tablist_font = s.to_string();
                    }
                },
                "tab_format" => {
                    if let Some(s) = val.as_str() {
                        cfg.tab_format = s.to_string();
                    }
                },
                "tab_shorten_str" => {
                    if let Some(s) = val.as_str() {
                        cfg.tab_shorten_str = s.to_string();
                    }
                },
                "default_tab_title" => {
                    if let Some(s) = val.as_str() {
                        cfg.default_tab_title = s.to_string();
                    }
                },
                "tab_append" => {
                    cfg.tab_append = val.as_str().map(String::from);
                },
                "scrollback_lines" => {
                    if let Some(n) = val.as_u64() {
                        cfg.scrollback_lines = n.max(16) as usize;
                    }
                },
                "history_bytes" => {
                    if let Some(n) = val.as_u64() {
                        cfg.history_bytes = (n as usize).max(64 * 1024);
                    }
                },
                "history_block_bytes" => {
                    if let Some(n) = val.as_u64() {
                        cfg.history_block_bytes = (n as usize).max(4096);
                    }
                },
                "default_width" => {
                    if let Some(n) = val.as_u64() {
                        cfg.default_width = n as u32;
                    }
                },
                "default_height" => {
                    if let Some(n) = val.as_u64() {
                        cfg.default_height = n as u32;
                    }
                },
                "tab_title_max_length" => {
                    if let Some(n) = val.as_i64().filter(|n| *n >= 0) {
                        cfg.tab_title_max_length = n as usize;
                    }
                },
                "tab_title_short_current" => {
                    if let Some(n) = val.as_i64().filter(|n| *n >= 0) {
                        cfg.tab_title_short_current = n as usize;
                    }
                },
                "tab_overview_delay_ms" => {
                    if let Some(n) = val.as_u64() {
                        cfg.tab_overview_delay_ms = n as u32;
                    }
                },
                "term_opacity" => {
                    if let Some(f) = val.as_f64() {
                        cfg.term_opacity_legacy = Some(f.clamp(0.0, 1.0) as f32);
                    }
                },
                "term_transparency" => {
                    if let Some(f) = val.as_f64() {
                        cfg.term_transparency = f.clamp(0.0, 1.0) as f32;
                        cfg.transparency_explicit = true;
                    }
                },
                "animation_speed" => {
                    if let Some(speed) = val.as_f64().filter(|speed| speed.is_finite()) {
                        cfg.animation_speed = if speed <= 0.0 { 0.0 } else { speed.clamp(0.25, 4.0) as f32 };
                    }
                },
                "tab_overview_opacity" => {
                    if let Some(f) = val.as_f64() {
                        cfg.tab_overview_opacity = f.clamp(0.0, 1.0) as f32;
                    }
                },
                "tab_overview_scale" => {
                    if let Some(f) = val.as_f64() {
                        cfg.tab_overview_scale = f.clamp(0.05, 1.0) as f32;
                    }
                },
                "autohide_delay" => {
                    if let Some(n) = val.as_i64() {
                        cfg.autohide_delay = n as i32;
                    }
                },
                "default_maximize" => {
                    if let Some(b) = val.as_bool() {
                        cfg.default_maximize = b;
                    }
                },
                "compact_tabs" => {
                    if let Some(b) = val.as_bool() { cfg.compact_tabs = b; }
                },
                "show_tab_titles" => {
                    if let Some(b) = val.as_bool() {
                        cfg.show_tab_titles = b;
                    }
                },
                "workspaces_enabled" => {
                    if let Some(b) = val.as_bool() {
                        cfg.workspaces_enabled = b;
                    }
                },
                "workspace_letter_keys" => {
                    if let Some(b) = val.as_bool() {
                        cfg.workspace_letter_keys = b;
                    }
                },
                "workspace_button_position" => {
                    if let Some(position) = val.as_str().and_then(WorkspaceButtonPosition::parse) {
                        cfg.workspace_button_position = position;
                    } else {
                        eprintln!("bad workspace_button_position value (expected left or right)");
                    }
                },
                "tab_overview_on_alt" => {
                    if let Some(b) = val.as_bool() {
                        cfg.tab_overview_on_alt = b;
                    }
                },
                "focus_new_tabs" => {
                    if let Some(b) = val.as_bool() {
                        cfg.focus_new_tabs = b;
                    }
                },
                "new_tab_position" => {
                    if let Some(s) = val.as_str() {
                        if matches!(s, "next" | "end" | "first") {
                            cfg.new_tab_position = s.to_string();
                        } else {
                            eprintln!("bad new_tab_position value: {s:?} (expected next, end or first)");
                        }
                    }
                },
                "hide_single_tab" => {
                    if let Some(b) = val.as_bool() {
                        cfg.hide_single_tab = b;
                    }
                },
                "graveyard_limit" => {
                    if let Some(n) = val.as_u64() {
                        cfg.graveyard_limit = n.clamp(1, 32) as usize;
                    }
                },
                "close_window_dialog" => {
                    if let Some(policy) = val.as_str().and_then(CloseWindowDialog::parse) {
                        cfg.close_window_dialog = policy;
                    } else {
                        eprintln!("bad close_window_dialog value (expected never, loss or always)");
                    }
                },
                "osc133" => {
                    if let Some(b) = val.as_bool() {
                        cfg.osc133 = b;
                    }
                },
                "kitty_compatible" => {
                    if let Some(b) = val.as_bool() {
                        cfg.kitty_compatible = b;
                    }
                },
                "multiline_tabs" => {
                    if let Some(b) = val.as_bool() {
                        cfg.multiline_tabs = b;
                    }
                },
                "tablist_top" => {
                    if let Some(b) = val.as_bool() {
                        cfg.tablist_top = b;
                    }
                },
                "tablist_hide" => {
                    if let Some(b) = val.as_bool() {
                        cfg.tablist_hide = b;
                    }
                },
                "tab_title_shorten_start" => {
                    if let Some(b) = val.as_bool() {
                        cfg.tab_title_shorten_start = b;
                    }
                },
                _ => {}, // unknown keys ignored, like in Python
            }
        }
    }

fn set_rgb(dst: &mut Rgb, v: &serde_yaml::Value) {
    if let Some(s) = v.as_str() {
        if let Some(c) = Rgb::parse(s) {
            *dst = c;
        } else {
            eprintln!("bad color value: {s:?}");
        }
    }
}

fn merge_bindings(bindings: &mut Vec<(String, Vec<Binding>)>, val: &serde_yaml::Value) {
    let Some(map) = val.as_mapping() else {
        return;
    };
    for (k, v) in map {
        let Some(action) = k.as_str() else {
            continue;
        };
        // Standalone overview was retired; old bindings must not swallow keys.
        if action == "show_tab_overview" || action.starts_with("overview_") { continue; }
        let specs: Vec<String> = match v {
            serde_yaml::Value::String(s) => vec![s.clone()],
            serde_yaml::Value::Sequence(seq) => {
                seq.iter().filter_map(|s| s.as_str().map(String::from)).collect()
            },
            _ => continue,
        };
        // An action can be unbound on purpose (`close_tab: none`). Without
        // this the empty result would fall through and the built-in default
        // would silently come back.
        if specs.iter().all(|s| matches!(s.trim().to_lowercase().as_str(), "" | "none" | "null" | "~")) {
            match bindings.iter_mut().find(|(a, _)| a == action) {
                Some(entry) => entry.1.clear(),
                None => bindings.push((action.to_string(), Vec::new())),
            }
            continue;
        }
        let parsed: Vec<Binding> = specs.iter().filter_map(|s| Binding::parse(s)).collect();
        if parsed.is_empty() {
            eprintln!("bad binding for {action:?}: {specs:?}");
            continue;
        }
        match bindings.iter_mut().find(|(a, _)| a == action) {
            Some(entry) => entry.1 = parsed,
            None => bindings.push((action.to_string(), parsed)),
        }
    }
}

// ---------------------------------------------------------------------------
// Keybindings
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum BoundKey {
    Named(NamedKey),
    Char(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub mods: ModifiersState,
    pub key: BoundKey,
}

impl Binding {
    /// Human-readable form of the effective binding, shared by menus and help.
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if self.mods.control_key() { parts.push("Ctrl".to_string()); }
        if self.mods.alt_key() { parts.push("Alt".to_string()); }
        if self.mods.shift_key() { parts.push("Shift".to_string()); }
        parts.push(match &self.key {
            BoundKey::Char(c) => match c.as_str() {
                "\r" => "Enter".into(),
                _ => c.to_uppercase(),
            },
            BoundKey::Named(key) => match key {
                NamedKey::ArrowLeft => "Left".into(),
                NamedKey::ArrowRight => "Right".into(),
                NamedKey::ArrowUp => "Up".into(),
                NamedKey::ArrowDown => "Down".into(),
                _ => format!("{key:?}"),
            },
        });
        parts.join("+")
    }

    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split('+').collect();
        let (mod_strs, key_str) = parts.split_at(parts.len().saturating_sub(1));
        let key_str = key_str[0].trim();
        if key_str.is_empty() {
            return None;
        }

        let mut mods = ModifiersState::empty();
        for m in mod_strs.iter().map(|s| s.trim()) {
            match m {
                "MOD1" | "ALT" | "Alt_L" | "Alt_R" => mods |= ModifiersState::ALT,
                "CONTROL" | "CTRL" => mods |= ModifiersState::CONTROL,
                "SHIFT" => mods |= ModifiersState::SHIFT,
                _ => return None,
            }
        }

        // "_1" style: underscore prefix means the literal character key
        if let Some(rest) = key_str.strip_prefix('_') {
            if rest.len() == 1 {
                return Some(Self { mods, key: BoundKey::Char(rest.to_string()) });
            }
        }

        let key = match key_str {
            "Left" => BoundKey::Named(NamedKey::ArrowLeft),
            "Right" => BoundKey::Named(NamedKey::ArrowRight),
            "Up" => BoundKey::Named(NamedKey::ArrowUp),
            "Down" => BoundKey::Named(NamedKey::ArrowDown),
            "Home" => BoundKey::Named(NamedKey::Home),
            "End" => BoundKey::Named(NamedKey::End),
            "Return" => BoundKey::Named(NamedKey::Enter),
            "Escape" => BoundKey::Named(NamedKey::Escape),
            // Numpad keys arrive as characters (NumLock on).
            "KP_Add" => BoundKey::Char("+".to_string()),
            "KP_Subtract" => BoundKey::Char("-".to_string()),
            "KP_Multiply" => BoundKey::Char("*".to_string()),
            "KP_Divide" => BoundKey::Char("/".to_string()),
            "KP_Enter" => BoundKey::Char("\r".to_string()),
            "Space" => BoundKey::Named(NamedKey::Space),
            "Page_Up" => BoundKey::Named(NamedKey::PageUp),
            "Page_Down" => BoundKey::Named(NamedKey::PageDown),
            "Insert" => BoundKey::Named(NamedKey::Insert),
            "Delete" => BoundKey::Named(NamedKey::Delete),
            "Tab" => BoundKey::Named(NamedKey::Tab),
            "Backspace" => BoundKey::Named(NamedKey::Backspace),
            "F1" => BoundKey::Named(NamedKey::F1),
            "F2" => BoundKey::Named(NamedKey::F2),
            "F3" => BoundKey::Named(NamedKey::F3),
            "F4" => BoundKey::Named(NamedKey::F4),
            "F5" => BoundKey::Named(NamedKey::F5),
            "F6" => BoundKey::Named(NamedKey::F6),
            "F7" => BoundKey::Named(NamedKey::F7),
            "F8" => BoundKey::Named(NamedKey::F8),
            "F9" => BoundKey::Named(NamedKey::F9),
            "F10" => BoundKey::Named(NamedKey::F10),
            "F11" => BoundKey::Named(NamedKey::F11),
            "F12" => BoundKey::Named(NamedKey::F12),
            "braceleft" => BoundKey::Char("{".to_string()),
            "braceright" => BoundKey::Char("}".to_string()),
            _ => {
                if key_str.len() == 1 {
                    BoundKey::Char(key_str.to_string())
                } else {
                    return None;
                }
            },
        };
        Some(Self { mods, key })
    }

    /// The spelling a configuration file stores this binding as - the inverse
    /// of `parse`, so a binding survives a save/reload unchanged.
    pub fn spec(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.mods.control_key() { parts.push("CONTROL".into()); }
        if self.mods.alt_key() { parts.push("MOD1".into()); }
        if self.mods.shift_key() { parts.push("SHIFT".into()); }
        parts.push(match &self.key {
            BoundKey::Char(c) => match c.as_str() {
                "+" => "KP_Add".into(),
                "-" => "KP_Subtract".into(),
                "*" => "KP_Multiply".into(),
                "/" => "KP_Divide".into(),
                "\r" => "Enter".into(),
                "{" => "braceleft".into(),
                "}" => "braceright".into(),
                other => other.to_string(),
            },
            BoundKey::Named(k) => named_spec(k),
        });
        parts.join("+")
    }

    /// Match a winit key + modifier state against this binding.
    pub fn matches(&self, key: &Key, mods: ModifiersState) -> bool {
        // Compare the three relevant modifier bits exactly.
        if mods.control_key() != self.mods.control_key()
            || mods.shift_key() != self.mods.shift_key()
            || mods.alt_key() != self.mods.alt_key()
        {
            return false;
        }
        match &self.key {
            BoundKey::Named(n) => matches!(key, Key::Named(k) if k == n),
            // winit reports Shift+C as "C", while legacy GTK binding files
            // spell CONTROL+SHIFT+c. Caps Lock must not break shortcuts either.
            BoundKey::Char(c) => matches!(key, Key::Character(k) if k == c ||
                (self.mods.intersects(ModifiersState::CONTROL | ModifiersState::ALT | ModifiersState::SHIFT)
                    && k.eq_ignore_ascii_case(c))
                // Shift turns a digit into a symbol before the key is ever
                // reported: Alt+Shift+2 arrives as "@", never "2", so a
                // digit written with Shift must also answer to its symbol.
                // Same US-shifted-symbol convention as `braceleft` -> "{`.
                || (self.mods.shift_key() && shifted_digit_of(c).is_some_and(|s| k == s))),
        }
    }


}

/// Find the first action whose any binding matches.
/// Overview bindings only apply while the overview is open; otherwise
/// they would swallow plain Return/Escape/arrows meant for the terminal.
pub fn find_action(bindings: &[(String, Vec<Binding>)], key: &Key, mods: ModifiersState, overview_active: bool) -> Option<String> {
    for (action, list) in bindings {
        if action.starts_with("overview_") && !overview_active {
            continue;
        }
        if list.iter().any(|b| b.matches(key, mods)) {
            return Some(action.clone());
        }
    }
    None
}

/// The first OTHER action that already uses this chord, if any. `find_action`
/// resolves a shared chord to the earlier action in list order, so a new
/// binding that collides would never fire; the capture rejects it instead.
pub fn binding_conflict(bindings: &[(String, Vec<Binding>)], action: &str, b: &Binding) -> Option<String> {
    for (a, list) in bindings {
        if a == action {
            continue;
        }
        if list.iter().any(|o| o.mods == b.mods && o.key == b.key) {
            return Some(a.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn workspace_button_position_round_trips_and_rejects_invalid_values() {
        assert_eq!(Config::default().workspace_button_position, WorkspaceButtonPosition::Right);
        let base = std::path::Path::new(".");
        for (value, position) in [("left", WorkspaceButtonPosition::Left), ("right", WorkspaceButtonPosition::Right)] {
            let cfg = Config::from_text(&format!("wal_palette: ''\nworkspace_button_position: {value}\n"), base).unwrap();
            assert_eq!(cfg.workspace_button_position, position);
            let saved = replace_top_level_values("# keep this comment\n", &cfg.ui_keys());
            assert!(saved.starts_with("# keep this comment\n"));
            assert_eq!(Config::from_text(&saved, base).unwrap().workspace_button_position, position);
        }
        for invalid in ["middle", "false", "12"] {
            let cfg = Config::from_text(&format!("wal_palette: ''\nworkspace_button_position: {invalid}\n"), base).unwrap();
            assert_eq!(cfg.workspace_button_position, WorkspaceButtonPosition::Right);
        }
    }
    #[test]
    fn close_window_dialog_policy_defaults_and_survives_save() {
        assert_eq!(Config::default().close_window_dialog, CloseWindowDialog::Always);
        let base = std::path::Path::new(".");
        for (name, policy, when_safe, when_loss) in [
            ("never", CloseWindowDialog::Never, false, false),
            ("loss", CloseWindowDialog::Loss, false, true),
            ("always", CloseWindowDialog::Always, true, true),
        ] {
            let cfg = Config::from_text(&format!("wal_palette: ''\nclose_window_dialog: {name}\n"), base).unwrap();
            assert_eq!(cfg.close_window_dialog, policy);
            assert_eq!(policy.should_show(true), when_safe);
            assert_eq!(policy.should_show(false), when_loss);
            let saved = replace_top_level_values("", &cfg.ui_keys());
            assert_eq!(Config::from_text(&saved, base).unwrap().close_window_dialog, policy);
        }
        let invalid = Config::from_text("wal_palette: ''\nclose_window_dialog: unknown\n", base).unwrap();
        assert_eq!(invalid.close_window_dialog, CloseWindowDialog::Always);
    }
    #[test]
    fn terminal_brightness_is_a_percentage_and_preserves_source_colors() {
        let base = std::path::Path::new(".");
        for brightness in [-100.0, -40.0, 0.0, 40.0, 100.0] {
            let cfg = Config::from_text(&format!("theme: dracula\nterminal_brightness: {brightness}\n"), base).unwrap();
            let source: [f32; 3] = [0.25, 0.5, 0.9];
            let expected = source.map(|c| (c * (1.0 + brightness / 100.0)).clamp(0.0, 1.0));
            assert_eq!(cfg.adjust_terminal_color(source), expected);
            let saved = replace_top_level_values("", &cfg.ui_keys());
            let restored = Config::from_text(&saved, base).unwrap();
            assert_eq!(restored.terminal_brightness, brightness);
            assert_eq!(restored.palette, cfg.palette);
            let mut changed = cfg.clone();
            crate::theme::apply_name(&mut changed, "catppuccin-latte").unwrap();
            assert_eq!(changed.terminal_brightness, brightness);
        }
        for invalid in [".nan", ".inf", "-101", "101", "'bright'", "null"] {
            let cfg = Config::from_text(&format!("wal_palette: ''\nterminal_brightness: {invalid}\n"), base).unwrap();
            assert_eq!(cfg.terminal_brightness, 0.0);
        }
        assert_eq!(Config::default().adjust_terminal_color([0.25, 0.5, 1.0]), [0.25, 0.5, 1.0]);
    }

    #[test]
    fn line_spacing_scales_the_normal_height_and_round_trips() {
        let base = std::path::Path::new(".");
        assert_eq!(Config::default().line_spacing, 0.0);
        for (spacing, height) in [(0.0, 20.0), (0.1, 22.0), (-0.1, 18.0), (0.5, 30.0), (-0.9, 2.0)] {
            let cfg = Config::from_text(&format!("wal_palette: ''\nline_spacing: {spacing}\n"), base).unwrap();
            assert_eq!(cfg.terminal_line_height(20.0), height);
            assert!(cfg.terminal_line_height(1.0) >= 1.0);
            let saved = replace_top_level_values("", &cfg.ui_keys());
            assert_eq!(Config::from_text(&saved, base).unwrap().line_spacing, spacing);
        }
        for invalid in [".nan", ".inf", "-.inf", "-1", "4", "'wide'", "null"] {
            let cfg = Config::from_text(&format!("wal_palette: ''\nline_spacing: {invalid}\n"), base).unwrap();
            assert_eq!(cfg.line_spacing, 0.0, "invalid spacing {invalid}");
        }
    }

    #[test]
    fn a_foreign_layouts_shifted_symbol_still_finds_the_digit_binding() {
        // German puts an underscore-section sign on Shift+3, which no US table
        // can predict; only the key's position connects it to Alt+Shift+3.
        let cfg = Config::default();
        let mods = ModifiersState::ALT | ModifiersState::SHIFT;
        let reported = Key::Character("\u{a7}".into());
        assert_eq!(find_action(&cfg.bindings, &reported, mods, false), None,
            "the symbol itself is not a binding");
        let by_position = unshifted_key(&reported, PhysicalKey::Code(winit::keyboard::KeyCode::Digit3));
        assert_eq!(by_position, Key::Character("3".into()));
        assert_eq!(find_action(&cfg.bindings, &by_position, mods, false),
            Some("goto_workspace_2".into()));
    }

    #[test]
    fn position_matching_never_broadens_an_unshifted_binding() {
        // Alt+2 selects tab 2. The shifted press must not also look like it.
        let cfg = Config::default();
        let reported = Key::Character("\u{a7}".into());
        let by_position = unshifted_key(&reported, PhysicalKey::Code(winit::keyboard::KeyCode::Digit2));
        assert_eq!(find_action(&cfg.bindings, &by_position, ModifiersState::ALT, false),
            Some("goto_tab_1".into()), "unshifted Alt+2 still selects the tab");
        assert_eq!(find_action(&cfg.bindings, &by_position, ModifiersState::ALT | ModifiersState::SHIFT, false),
            Some("goto_workspace_1".into()), "and the same position with Shift selects the workspace");
    }

    #[test]
    fn a_conflicting_chord_names_the_action_that_already_owns_it() {
        // Alt+Shift+W belongs to send_tab_to_window; the workspace move must
        // not be able to steal it, because list order would win the lookup.
        let cfg = Config::default();
        let taken = Binding::parse("MOD1+SHIFT+w").unwrap();
        assert_eq!(binding_conflict(&cfg.bindings, "move_tab_to_workspace", &taken),
            Some("send_tab_to_window".into()));
        // Re-binding an action to its own chord is not a conflict, and a free
        // chord is one too.
        assert_eq!(binding_conflict(&cfg.bindings, "send_tab_to_window", &taken), None);
        let free = Binding::parse("MOD1+SHIFT+m").unwrap();
        assert_eq!(binding_conflict(&cfg.bindings, "move_tab_to_workspace", &free), None);
    }

    #[test]
    fn binding_specs_round_trip_through_parse() {
        // Every shipped binding must survive a save, which writes the spec.
        for (action, list) in Config::default().bindings {
            for b in list {
                assert_eq!(Binding::parse(&b.spec()).as_ref().map(|p| (p.mods, p.key.clone())),
                    Some((b.mods, b.key.clone())), "{action}: {}", b.spec());
            }
        }
    }

    #[test]
    fn rebound_and_unbound_keys_round_trip_through_a_saved_file() {
        let base = std::path::Path::new(".");
        let mut cfg = Config::from_text("wal_palette: ''\nbindings:\n  new_tab: CONTROL+SHIFT+t\n", base).unwrap();
        cfg.set_binding("new_tab", vec![Binding::parse("MOD1+SHIFT+2").unwrap()]);
        cfg.set_binding("close_tab", vec![]);
        let block = cfg.bindings_yaml();
        let saved = replace_top_level_block("# keep me\nbindings:\n  new_tab: CONTROL+SHIFT+t\ncol_0: '#000000'\n", "bindings", &block);
        assert!(saved.starts_with("# keep me\n"), "header survives: {saved}");
        assert!(saved.contains("col_0: '#000000'"), "keys after the block survive");
        assert_eq!(saved.matches("bindings:").count(), 1, "the old block is gone, not stacked: {saved}");
        let back = Config::from_text(&saved, base).unwrap();
        assert_eq!(back.bindings_for("new_tab").len(), 1);
        assert_eq!(back.bindings_for("new_tab")[0].label(), "Alt+Shift+2");
        assert!(back.bindings_for("close_tab").is_empty(), "an unbound key stays unbound");
    }

    #[test]
    fn unbind_spelling_clears_the_default() {
        let cfg = Config::from_text("wal_palette: ''\nbindings:\n  close_tab: none\n", std::path::Path::new(".")).unwrap();
        assert!(cfg.bindings_for("close_tab").is_empty());
    }

    #[test]
    fn block_replace_appends_when_the_key_is_absent() {
        let out = replace_top_level_block("col_0: '#000000'\n", "bindings", "bindings:\n  new_tab: 'CONTROL+t'\n");
        assert!(out.contains("col_0: '#000000'"));
        assert!(out.contains("  new_tab: 'CONTROL+t'"));
    }

    #[test]
    fn workspaces_default_off_and_round_trip() {
        assert!(!Config::default().workspaces_enabled);
        let cfg = Config::from_text("wal_palette: ''\nworkspaces_enabled: true\n", std::path::Path::new(".")).unwrap();
        assert!(cfg.workspaces_enabled);
        let saved = replace_top_level_values("", &cfg.ui_keys());
        assert!(Config::from_text(&saved, std::path::Path::new(".")).unwrap().workspaces_enabled);
    }

    #[test]
    fn tab_presentation_preferences_default_on_and_round_trip() {
        let defaults = Config::default();
        assert!(defaults.tab_overview_on_alt);
        assert!(defaults.show_tab_titles);
        assert!(!defaults.compact_tabs);
        let cfg = Config::from_text("wal_palette: ''\ntab_overview_on_alt: false\nshow_tab_titles: false\ncompact_tabs: true\n", std::path::Path::new(".")).unwrap();
        assert!(!cfg.tab_overview_on_alt);
        assert!(!cfg.show_tab_titles);
        assert!(cfg.compact_tabs);
        let saved = replace_top_level_values("", &cfg.ui_keys());
        let restored = Config::from_text(&saved, std::path::Path::new(".")).unwrap();
        assert!(!restored.tab_overview_on_alt);
        assert!(!restored.show_tab_titles);
        assert!(restored.compact_tabs);
    }

    #[test]
    fn animation_speed_defaults_bounds_and_save_round_trip() {
        let base = std::path::Path::new(".");
        assert_eq!(Config::default().animation_speed, 1.0);
        for (written, expected) in [("0", 0.0), ("0.01", 0.25), ("0.5", 0.5), ("2", 2.0), ("-1", 0.0), ("999", 4.0),
            (".nan", 1.0), (".inf", 1.0), ("nope", 1.0)] {
            let cfg = Config::from_text(&format!("wal_palette: ''\nanimation_speed: {written}\n"), base).unwrap();
            assert_eq!(cfg.animation_speed, expected, "{written}");
            let saved = replace_top_level_values("", &cfg.ui_keys());
            assert_eq!(Config::from_text(&saved, base).unwrap().animation_speed, expected);
        }
    }

    #[test]
    fn workspace_identity_choices_round_trip_and_reject_invalid_values() {
        for mode in WorkspaceButtonDisplay::ALL {
            let mut cfg = Config::default();
            cfg.workspace_button_display = mode;
            cfg.workspace_icons = WorkspaceIcon::ALL;
            cfg.workspace_genres[9] = "Research: notes & ideas".into();
            let text = replace_top_level_values("", &cfg.ui_keys());
            let loaded = Config::from_text(&text, std::path::Path::new(".")).unwrap();
            assert_eq!(loaded.workspace_button_display, mode);
            assert_eq!(loaded.workspace_icons, WorkspaceIcon::ALL);
            assert_eq!(loaded.workspace_genres, cfg.workspace_genres);
        }
        let cfg = Config::from_text("workspace_button_display: invalid\nworkspace_icon_1: invalid\nworkspace_icon_0: code\nworkspace_icon_11: terminal\nworkspace_icon_2: cube\n", std::path::Path::new(".")).unwrap();
        assert_eq!(cfg.workspace_button_display, WorkspaceButtonDisplay::Name);
        assert_eq!(cfg.workspace_icons[0], WorkspaceIcon::Layers);
        assert_eq!(cfg.workspace_icons[1], WorkspaceIcon::Cube);
        assert_eq!(WorkspaceButtonDisplay::Name.text(3, "  "), "3");
        assert_eq!(WorkspaceButtonDisplay::IconAndName.text(3, " Development "), "Development");
        assert_eq!(WorkspaceButtonDisplay::Number.text(3, "Development"), "3");
        assert!(WorkspaceButtonDisplay::Icon.text(3, "Development").is_empty());
    }

    #[test]
    fn workspace_genres_are_optional_and_round_trip() {
        let cfg = Config::from_text("wal_palette: ''\nworkspace_genre_1: 'Development'\nworkspace_genre_10: \"Research & notes\"\nworkspace_genre_0: ignored\nworkspace_genre_11: ignored\nworkspace_genre_2: 42\n", std::path::Path::new(".")).unwrap();
        assert_eq!(cfg.workspace_genres[0], "Development");
        assert_eq!(cfg.workspace_genres[9], "Research & notes");
        assert!(cfg.workspace_genres[1..9].iter().all(String::is_empty));
        let saved = Config::from_text(&replace_top_level_values("", &cfg.ui_keys()), std::path::Path::new(".")).unwrap();
        assert_eq!(saved.workspace_genres, cfg.workspace_genres);
    }

    #[test]
    fn workspace_colors_load_save_and_override_themes_independently() {
        let base = std::path::Path::new(".");
        let mut cfg = Config::from_text("theme: dracula\nworkspace_color_1: '#123456'\nworkspace_color_9: '#fedcba'\nworkspace_color_10: '#abc'\n", base).unwrap();
        assert_eq!(cfg.workspace_colors[0], Rgb(0x12, 0x34, 0x56));
        assert_eq!(cfg.workspace_colors[8], Rgb(0xfe, 0xdc, 0xba));
        assert_eq!(cfg.workspace_colors[9], Rgb(0xaa, 0xbb, 0xcc));
        let defaults = Config::default();
        assert_eq!(cfg.workspace_colors[1..8], defaults.workspace_colors[1..8]);
        cfg.workspace_colors[4] = Rgb(5, 127, 250);
        let saved = replace_top_level_values("# keep comment\nbindings: {new_tab: 'Control+t'}\n", &cfg.ui_keys());
        let restored = Config::from_text(&saved, base).unwrap();
        assert_eq!(restored.workspace_colors, cfg.workspace_colors);
        assert!(saved.contains("# keep comment"));
        assert!(!saved.contains("tab_color_bg:"));
        assert!(saved.contains("workspace_color_10: '#aabbcc'"));
        // Global theme changes preserve workspace identities.
        crate::theme::apply_name(&mut cfg, "catppuccin-latte").unwrap();
        assert_eq!(cfg.workspace_colors, restored.workspace_colors);
    }

    #[test]
    fn invalid_workspace_color_keys_and_values_keep_defaults() {
        let cfg = Config::from_text("wal_palette: ''\nworkspace_color_0: '#fff'\nworkspace_color_11: '#fff'\nworkspace_color_bad: '#fff'\nworkspace_color_1: nope\nworkspace_color_2: 123\n", std::path::Path::new(".")).unwrap();
        assert_eq!(cfg.workspace_colors, Config::default().workspace_colors);
        let colors = Config::default().workspace_colors;
        for (i, color) in colors[..9].iter().enumerate() {
            assert!(!colors[..i].contains(color), "W{} duplicates a default workspace color", i + 1);
        }
    }

    #[test]
    #[test]
    fn workspace_shortcuts_use_alt_shift_digits_and_free_alt_letters() {
        let cfg = Config::default();
        // The standard scheme: Alt+Shift+digit, in physical digit order.
        for (i, digit) in "1234567890".chars().enumerate() {
            let key = Key::Character(digit.to_string().into());
            assert_eq!(
                find_action(&cfg.bindings, &key, ModifiersState::ALT | ModifiersState::SHIFT, false),
                Some(format!("goto_workspace_{i}")),
                "Alt+Shift+{digit}");
        }
        // Bare Alt+letter belongs to the shell (readline, word motions) and
        // must stay unclaimed unless the user opts into the letter scheme.
        for letter in WORKSPACE_LETTERS.chars() {
            let key = Key::Character(letter.to_string().into());
            assert_eq!(find_action(&cfg.bindings, &key, ModifiersState::ALT, false), None,
                "Alt+{letter} must reach the shell by default");
        }
    }

    #[test]
    fn shift_digit_bindings_also_match_the_shifted_symbol() {
        // The key report carries Shift's symbol ("@"), never the digit, so a
        // binding written as MOD1+SHIFT+_2 has to answer to both spellings.
        let cfg = Config::default();
        // "*" is left out: it is also Shift+KP_Multiply (toggle_entangle), and
        // the key report cannot tell the two keys apart.
        for (symbol, i) in [("!", 0), ("@", 1), ("#", 2), ("$", 3), ("%", 4), ("^", 5), ("&", 6), ("(", 8), (")", 9)] {
            let key = Key::Character(symbol.into());
            assert_eq!(
                find_action(&cfg.bindings, &key, ModifiersState::ALT | ModifiersState::SHIFT, false),
                Some(format!("goto_workspace_{i}")),
                "Alt+Shift should match {symbol}");
        }
        // A digit bound without Shift keeps the literal digit only.
        assert_eq!(find_action(&cfg.bindings, &Key::Character("2".into()), ModifiersState::ALT, false),
            Some("goto_tab_1".into()));
        assert_eq!(find_action(&cfg.bindings, &Key::Character("@".into()), ModifiersState::ALT, false), None,
            "the symbol of Shift+2 is not Alt+2");
    }

    #[test]
    fn workspace_letter_keys_adds_alt_letters_without_losing_the_digits() {
        let cfg = Config::from_text("wal_palette: ''\nworkspace_letter_keys: true\n", std::path::Path::new(".")).unwrap();
        for (i, letter) in WORKSPACE_LETTERS.chars().enumerate() {
            let key = Key::Character(letter.to_string().into());
            assert_eq!(find_action(&cfg.bindings, &key, ModifiersState::ALT, false),
                Some(format!("goto_workspace_{i}")), "Alt+{letter}");
            assert_eq!(find_action(&cfg.bindings, &key, ModifiersState::empty(), false), None);
        }
        // Both schemes coexist.
        assert_eq!(find_action(&cfg.bindings, &Key::Character("@".into()),
            ModifiersState::ALT | ModifiersState::SHIFT, false), Some("goto_workspace_1".into()));
        // Saved and reloaded, the scheme does not accumulate duplicates.
        let saved = replace_top_level_values("", &cfg.ui_keys());
        let again = Config::from_text(&saved, std::path::Path::new(".")).unwrap();
        for i in 0..WORKSPACES {
            let list = again.bindings.iter().find(|(a, _)| *a == format!("goto_workspace_{i}")).unwrap();
            assert_eq!(list.1.len(), 2, "workspace {i} bindings");
        }
    }

    #[test]
    fn binding_labels_use_readable_modifiers_and_keys() {
        for (spec, expected) in [
            ("CONTROL+SHIFT+n", "Ctrl+Shift+N"),
            ("MOD1+SHIFT+c", "Alt+Shift+C"),
            ("MOD1+_1", "Alt+1"),
            ("CONTROL+ALT+Left", "Ctrl+Alt+Left"),
            ("SHIFT+KP_Enter", "Shift+Enter"),
        ] {
            assert_eq!(super::Binding::parse(spec).unwrap().label(), expected);
        }
    }

    #[test]
    fn user_config_chords_probe() {
        let cfg = Config::load();
        use winit::keyboard::NamedKey;
        for (label, key, mods) in [
            ("ctrl+w", Key::Character("w".into()), M::CONTROL),
            ("ctrl+left", Key::Named(NamedKey::ArrowLeft), M::CONTROL),
            ("ctrl+right", Key::Named(NamedKey::ArrowRight), M::CONTROL),
            ("ctrl+backspace", Key::Named(NamedKey::Backspace), M::CONTROL),
        ] {
            let hit = find_action(&cfg.bindings, &key, mods, false);
            println!("{label} -> {hit:?}");
        }
        for (action, list) in &cfg.bindings {
            for b in list {
                println!("BIND {action} = {:?} {:?}", b.key, b.mods);
            }
        }
    }
    use super::*;
    use winit::keyboard::ModifiersState as M;

    #[test]
    fn window_actions_are_bound_by_default() {
        let cfg = Config::default();
        assert_eq!(
            find_action(&cfg.bindings, &Key::Character("N".into()), M::CONTROL | M::SHIFT, false)
                .as_deref(),
            Some("new_window"),
            "Ctrl+Shift+n (what winit reports for the shifted key) opens a window"
        );
        assert_eq!(
            find_action(&cfg.bindings, &Key::Character("W".into()), M::ALT | M::SHIFT, false)
                .as_deref(),
            Some("send_tab_to_window"),
            "Alt+Shift+w sends the tab to another window"
        );
    }

    #[test]
    fn clipboard_shortcuts_accept_the_shifted_keys_winit_reports() {
        let cfg = Config::default();
        assert_eq!(find_action(&cfg.bindings, &Key::Character("C".into()), M::CONTROL | M::SHIFT, false).as_deref(), Some("copy"));
        assert_eq!(find_action(&cfg.bindings, &Key::Character("V".into()), M::CONTROL | M::SHIFT, false).as_deref(), Some("paste"));
        assert_eq!(find_action(&cfg.bindings, &Key::Character("L".into()), M::CONTROL | M::SHIFT, false).as_deref(), Some("copy_last_output"));
        assert_eq!(find_action(&cfg.bindings, &Key::Named(NamedKey::Insert), M::SHIFT, false).as_deref(), Some("paste_primary"));
    }

    #[test]
    fn retired_standalone_overview_bindings_do_not_swallow_keys() {
        let mut cfg = Config::default();
        let legacy: serde_yaml::Value = serde_yaml::from_str("show_tab_overview: MOD1+o\noverview_exit: Escape\noverview_nav_left: Left\n").unwrap();
        merge_bindings(&mut cfg.bindings, &legacy);
        // Alt+o is no longer claimed: bare Alt+letters are left to the shell
        // unless `workspace_letter_keys` opts in.
        assert_eq!(find_action(&cfg.bindings, &Key::Character("o".into()), M::ALT, false), None);
        assert_eq!(find_action(&cfg.bindings, &Key::Named(NamedKey::Escape), M::empty(), false), None);
        assert!(cfg.tab_overview_on_alt, "the Alt-held visual switcher remains enabled");
    }

    #[test]
    fn rgb_parse_forms() {
        assert_eq!(Rgb::parse("#ff2c4b"), Some(Rgb(255, 44, 75)));
        assert_eq!(Rgb::parse("#141413"), Some(Rgb(20, 20, 19)));
        // #rgb shorthand expands each nibble to x17.
        assert_eq!(Rgb::parse("#f00"), Some(Rgb(255, 0, 0)));
        assert_eq!(Rgb::parse("#fff"), Some(Rgb(255, 255, 255)));
        assert_eq!(Rgb::parse("#ffff2c2c4b4b"), Some(Rgb(255, 44, 75)));
        assert_eq!(Rgb::parse("#ffff00000000"), Some(Rgb(255, 0, 0)));
        assert_eq!(Rgb::parse("#éééééé"), None);
        // invalid: wrong length / non-hex.
        assert_eq!(Rgb::parse("#12345"), None);
        assert_eq!(Rgb::parse("#zzzzzz"), None);
    }

    #[test]
    fn font_parse_family_and_size() {
        assert_eq!(parse_font("Cascadia Code 9"), ("Cascadia Code".to_string(), 9.0));
        // family may contain parentheses and spaces.
        assert_eq!(
            parse_font("Terminus (TTF) 12"),
            ("Terminus (TTF)".to_string(), 12.0)
        );
        assert_eq!(parse_font("Cascadia Code 11.2"), ("Cascadia Code".to_string(), 11.2));
        assert_eq!(parse_font("Cascadia Code 11.25"), ("Cascadia Code".to_string(), 11.25));
        // no size -> default 12.
        assert_eq!(parse_font("Monospace"), ("Monospace".to_string(), 12.0));
    }

    #[test]
    fn palette_badwolf_first_sixteen() {
        let full = "#141413:#ff0208:#aeee00:#fade3e:#0a9dff:#ff9eb8:#8cffba:#d9cec3:#998f84:#ff0208:#aeee00:#ffa724:#0a9dff:#ff9eb8:#8cffba:#ffffff";
        let p = parse_palette(&full).expect("16 colors");
        assert_eq!(p[0], Rgb(20, 20, 19));
        assert_eq!(p[1], Rgb(255, 2, 8));
        assert_eq!(p[2], Rgb(174, 238, 0));
        assert_eq!(p[15], Rgb(255, 255, 255));
        assert_eq!(parse_palette(""), None);
        // Too few is no longer an error -- short lists are completed, not
        // rejected: slots take the nearest given color in Lab space.
        let two = parse_palette("#000:#f00").expect("derived from two");
        assert_eq!(two[0], Rgb(0, 0, 0));
        assert_eq!(two[9], Rgb(255, 0, 0));
    }

    #[test]
    fn config_path_prefers_xdg_then_legacy() {
        let root = std::env::temp_dir().join(format!("mypixels-path-{}", std::process::id()));
        let xdg = root.join("config");
        let home = root.join("home");
        std::fs::create_dir_all(xdg.join("mypixels")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let modern = xdg.join("mypixels").join("config.yml");
        // Neither file exists yet: the modern path wins -- that is where a
        // first save belongs, legacy or not.
        assert_eq!(Config::config_path_in(None, Some(&xdg), Some(&home)).unwrap(), modern);
        // Only the legacy file exists: it is respected, not silently skipped.
        std::fs::write(home.join(".mypixels"), "").unwrap();
        assert_eq!(Config::config_path_in(None, Some(&xdg), Some(&home)).unwrap(),
            home.join(".mypixels"));
        // Once the XDG file exists it wins: the move happens on save, not on
        // read, and never duplicates state.
        std::fs::write(&modern, "").unwrap();
        assert_eq!(Config::config_path_in(None, Some(&xdg), Some(&home)).unwrap(), modern);
        // The test override still outranks everything.
        assert_eq!(
            Config::config_path_in(Some(root.join("env")), Some(&xdg), Some(&home)).unwrap(),
            root.join("env")
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn theme_name_round_trips_through_save() {
        let cfg = Config::from_text("theme: 'gruvbox-dark'\n", std::path::Path::new(".")).unwrap();
        assert_eq!(cfg.theme, "gruvbox-dark");
        let keys = cfg.ui_keys();
        assert!(keys.iter().any(|(k, v)| k == "theme" && v == "'gruvbox-dark'"));
        // Colors the theme set are saved alongside it: the file stays
        // readable by tools that do not know themes at all.
        assert!(keys.iter().any(|(k, _)| k == "background_color"));
        // No theme: no empty `theme: ''` noise appended to the file.
        let cfg = Config::default();
        assert!(!cfg.ui_keys().iter().any(|(k, _)| k == "theme"));
    }

    #[test]
    fn dedupe_keeps_last_top_level_key() {
        let text = "a: 1\nb: 2\na: 3\n  - x\nc: 4\n";
        let out = dedupe_top_level(text);
        // first `a: 1` block dropped; second `a: 3` (with its list item) kept.
        assert!(!out.contains("a: 1"));
        assert!(out.contains("a: 3"));
        assert!(out.contains("- x"));
        assert!(out.contains("b: 2"));
        assert!(out.contains("c: 4"));
    }

    #[test]
    fn dedupe_preserves_header_and_no_duplicates() {
        let text = "# comment\na: 1\nb: 2\n";
        assert_eq!(dedupe_top_level(text), text);
    }

    #[test]
    fn binding_parse_mods_and_key() {
        let b = Binding::parse("MOD1+SHIFT+Left").unwrap();
        assert_eq!(b.mods, M::ALT | M::SHIFT);
        assert!(matches!(b.key, BoundKey::Named(NamedKey::ArrowLeft)));

        let b = Binding::parse("CONTROL+ALT+x").unwrap();
        assert_eq!(b.mods, M::CONTROL | M::ALT);
        assert!(matches!(b.key, BoundKey::Char(ref c) if c == "x"));

        // underscore prefix -> literal char even for a digit.
        let b = Binding::parse("MOD1+_1").unwrap();
        assert_eq!(b.mods, M::ALT);
        assert!(matches!(b.key, BoundKey::Char(ref c) if c == "1"));

        // numpad add arrives as the "+" character.
        let b = Binding::parse("SHIFT+KP_Add").unwrap();
        assert_eq!(b.mods, M::SHIFT);
        assert!(matches!(b.key, BoundKey::Char(ref c) if c == "+"));

        assert!(Binding::parse("").is_none());
        assert!(Binding::parse("FOO+x").is_none()); // unknown modifier
    }

    #[test]
    fn replace_keeps_comments_and_replaces_in_place() {
        let text = "# upraveno\nautohide_delay: -1\nterm_opacity: 0.1\n# a comment\ncol_0: '#111111'\n";
        let out = replace_top_level_values(text, &[
            ("term_opacity".into(), "0.7".into()),
            ("col_0".into(), "'#ff0000'".into()),
        ]);
        assert_eq!(
            out,
            "# upraveno\nautohide_delay: -1\nterm_opacity: 0.7\n# a comment\ncol_0: '#ff0000'\n"
        );
    }

    #[test]
    fn replace_appends_missing_keys_at_end() {
        let text = "term_opacity: 0.1\n";
        let out = replace_top_level_values(text, &[("tablist_top".into(), "false".into())]);
        assert_eq!(out, "term_opacity: 0.1\ntablist_top: false\n");
    }

    #[test]
    fn replace_does_not_touch_indented_or_prefixed_keys() {
        let text = "  col_0: nested\ncol_0: '#111111'\ncol_1: '#222222'\n";
        let out = replace_top_level_values(text, &[("col_0".into(), "'#333333'".into())]);
        assert_eq!(out, "  col_0: nested\ncol_0: '#333333'\ncol_1: '#222222'\n");
    }

    #[test]
    fn save_roundtrip_preserves_bindings_and_comments() {
        let cfg = Config::default();
        let text = "# c\nbindings:\n  new_tab: CONTROL+SHIFT+t\ncol_0: '#000000'\nterm_opacity: 0.1\n";
        let out = replace_top_level_values(text, &cfg.ui_keys());
        // Bindings block and its indented entries survive byte for byte.
        assert!(out.contains("bindings:\n  new_tab: CONTROL+SHIFT+t\n"));
        assert!(out.starts_with("# c\n"));
        // Every managed key is present exactly once.
        assert_eq!(out.lines().filter(|l| l.starts_with("term_opacity:")).count(), 1);
        assert!(out.contains("term_opacity: 0.1")); // default is 0.1, unchanged
    }
}

/// The terminal background the ORIGINAL actually paints: with no 16-color
/// palette set, VTE's background is `bg_tint_color` (the pywal loader keeps
/// `bg_tint_color` equal to the palette background when one is active, and
/// `background_color` is not consulted on that path). Reading only
/// `background_color` made `bg_tint_color` a dead key in the rewrite.
pub fn terminal_bg(c: &Config) -> Rgb {
    if c.palette.is_some() { c.background_color } else { c.bg_tint_color }
}
#[cfg(test)]
mod theme_palette_tests {
    use super::*;
    #[test]
    fn legacy_term_opacity_feeds_term_transparency() {
        // Files predating the rename keep working...
        let cfg = Config::from_text("term_opacity: 0.4\n", std::path::Path::new(".")).unwrap();
        assert!((cfg.term_transparency - 0.4).abs() < 1e-6);
        // ...but the new key always wins, whatever the file order.
        let cfg = Config::from_text(
            "term_opacity: 0.4\nterm_transparency: 0.8\n",
            std::path::Path::new("."),
        )
        .unwrap();
        assert!((cfg.term_transparency - 0.8).abs() < 1e-6);
        let cfg = Config::from_text(
            "term_transparency: 0.8\nterm_opacity: 0.4\n",
            std::path::Path::new("."),
        )
        .unwrap();
        assert!((cfg.term_transparency - 0.8).abs() < 1e-6);
        // Saves write only the new spelling.
        let keys = Config::default().ui_keys();
        assert!(keys.iter().any(|(k, _)| k == "term_transparency"));
        assert!(!keys.iter().any(|(k, _)| k == "term_opacity"));
    }

    #[test]
    fn theme_supersedes_stale_palette_copy_lines() {
        // The user's exact trap: theme line added below a years-old custom
        // palette + col_* copies from an older theme save. The theme must
        // own the 16 slots; fg/bg stay file-overridable.
        let text = "col_0: '#141413'\ncol_4: '#0a9dff'\n\
            palette: \"#141413:#ff2c4b:#aeee00:#fade3e:#0a9dff:#ff9eb8:#8cffba:#d9cec3:\
            #000000:#ff2c4b:#aeee00:#ffa724:#0a9dff:#ff9eb8:#8cffba:#ffffff\"\n\
            foreground_color: '#f8f8f2'\nbackground_color: '#282a36'\ntheme: 'dracula'\n";
        let cfg = Config::from_text(text, std::path::Path::new(".")).unwrap();
        let p = cfg.palette.expect("theme supplies the palette");
        assert_eq!(p[0], Rgb(0x21, 0x22, 0x2c), "dracula black, not stale #141413");
        assert_eq!(p[4], Rgb(0xbd, 0x93, 0xf9), "dracula blue, not stale #0a9dff");
        assert_eq!(cfg.foreground_color, Rgb(0xf8, 0xf8, 0xf2), "explicit fg still wins");
        // No theme: the palette lines are the config, untouched.
        // No theme, and wal explicitly disabled: the palette line is the
        // config, untouched. (Without `wal_palette: ''` a live
        // ~/.cache/wal/colors.yml would win by design - implicit wal is
        // the default when no theme is named.)
        let cfg = Config::from_text(
            "wal_palette: ''\npalette: \"#141413:#ff2c4b:#aeee00:#fade3e:#0a9dff:#ff9eb8:#8cffba:#d9cec3:#000000:#ff2c4b:#aeee00:#ffa724:#0a9dff:#ff9eb8:#8cffba:#ffffff\"\n",
            std::path::Path::new("."),
        )
        .unwrap();
        assert_eq!(cfg.palette.unwrap()[2], Rgb(0xae, 0xee, 0x00));
        assert_eq!(cfg.palette.unwrap()[2], Rgb(0xae, 0xee, 0x00));
    }
}
