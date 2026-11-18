MyPixels Terminal — Design Document

Purpose

- Describe the end‑to‑end design of the MyPixels terminal emulator so it can be re‑implemented in a different language/toolkit with equivalent behavior and UX.
- Capture architectural boundaries, data flow, user interactions, UI composition, configuration, and key invariants.

High‑Level Overview

- App type: Multi‑window terminal emulator built on a single OS process and one GUI application instance. Uses VTE (terminal widget), GTK (UI toolkit), and GLib/Gio (app, events, timers, file monitoring).
- Windows: One or more “Terminal Windows” per app instance. Each window hosts multiple terminal tabs.
- Tabs: GTK Notebook pages containing VTE terminals. A custom Tab Bar (TabList) renders metadata, colors, and controls.
- Overlays: UI layers posted above the terminal surface, used for the Tab Overview/Switcher and the Cross‑Tab Search panel.
- Global features: Keybinding system, color theming (including pywal), tab preview thumbnails, cross‑window operations, and an optional floating Monitor window per tab.

Key Components

1) Application (single process / multi window)

- Responsibilities
  - Owns the OS‑level application identity and single‑instance behavior (D‑Bus). Subsequent invocations forward args to the primary instance.
  - Manages the set of open windows and app‑wide configuration/shared state.
  - Creates new windows on activation or on command.

- Essential API and state
  - windows: list of Window objects.
  - config: consolidated configuration (defaults merged with user file).
  - tab_counter: global incremental ID generator.
  - independent flag: when true, spawns a unique app ID to bypass single‑instance behavior.

- Lifecycle
  - Startup: parse CLI flags, initialize Application, run main loop.
  - Activation: create a new window, spawn first tab, present the window.
  - Command line (secondary launch): handled via Application; forwards to main instance then triggers activation.

2) Window (Terminal Window)

- Composition
  - Top‑level `Gtk.Window` containing a root `Gtk.Overlay`.
  - Inside overlay: a vertical container with `Gtk.Notebook` (terminals) and the Tab Bar (custom `TabList`).
  - Overlay children: overlaid widgets (Search panel, Tab Overview/Switcher background + tiles).

- Responsibilities
  - Manages tabs: create, close, reorder, rename; navigate; move tab to another window.
  - Hosts the Tab Bar UI and coordinates its updates.
  - Handles top‑level window key events and maps configured bindings to actions.
  - Controls state for “Alt‑hold tab switcher” (on_tabswitch) and delayed switcher overlay.
  - Applies configuration (fonts, colors, CSS) and reacts to theme reloads (pywal).

- Essential state
  - notebook: container of terminal tabs (pages are VTE terminals).
  - tabs: mapping terminal widget → metadata (id, title/name, group color, etc.).
  - entangled: list/set of terminals bound for multi‑terminal keystroke broadcast.
  - overlay: shared overlay layer for temporary UI (search/overview).
  - search_panel: controller for cross‑tab search overlay.
  - tab_overview, tab_overview_full: controllers for tab switcher/overviews.
  - keybinds: parsed list of (keyval, modmask, function, arg) built from config.
  - on_tabswitch: boolean set while Alt (or configured modifier) is held.
  - tab_overview_timer: GLib timer ID used for delayed switcher overlay.

- Keystroke routing
  - Window receives key press/release events.
  - If a modal overlay is active (overview/search), delegate handling to that overlay first.
  - Otherwise, look up the key combo in the parsed keybinds and execute the mapped action.
  - If the pressed key equals the configured “tab_highlight_mod” (default Alt), set on_tabswitch, refresh Tab Bar to show indices, and optionally start a delay timer to open the switcher overlay.
  - On key release of that modifier: unset on_tabswitch, update Tab Bar, cancel delayed timer if pending, and hide switcher overlay if open.
  - Entanglement: if current terminal is in `entangled`, forward the key event to all entangled terminals (press and release).

- Notable actions exposed via keybindings
  - Tab navigation/move: prev/next/first/last; move tab left/right/up/down.
  - Tab row navigation (for multi‑row tab bars).
  - Create/close tab; create new window; send current tab to another window.
  - Rename tab; toggle tab bar visibility; toggle entanglement.
  - Zoom font (zoom in/out via keybinds and via Ctrl+mouse wheel).
  - Show search (in‑tab) and cross‑tab search panel.
  - Show Tab Overview (standalone) and Tab Switcher (on Alt hold) and Full Overview (all windows).
  - Open Monitor window for current tab (floating preview window).

3) Tabs and Terminals

- Terminal widget
  - VTE terminal per tab; configured with fonts, scrollback, word characters, and color palette.
  - Supports zoom via font size changes.
  - Emits events: child exit, EOF, title change; scroll events; used to update metadata and UI.

- Tab metadata
  - id: numeric ID from app‑wide counter.
  - title/name: default title is configurable; can be changed by user (rename) or tracked from window title.
  - group: optional color group assignment (affects tab label color).
  - entangled: if present in `entangled`, receives forwarded keystrokes.

- Tab creation
  - Spawn shell (or configured command) with VTE; insert into notebook; register signal handlers.
  - Apply font and colors; set scrollback and search behavior.
  - Focus newly created tab depending on config.

4) Tab Bar (TabList)

- Rendering
  - Custom widget made of an EventBox containing a horizontal box of tab label widgets and controls.
  - Optional prepend/append segments (e.g., a clock), rendered via markup from small callbacks.
  - Each tab shows: index, entanglement marker, title (possibly truncated), group color override.
  - Selected tab uses special foreground/background colors.
  - “Add tab” button rendered with distinct color; click adds new tab.
  - Hide tab bar when only one tab is present if configured.
  - Tab list position: top or bottom per config.

- Interaction
  - Left click selects a tab; add‑button click creates a new tab.
  - When on_tabswitch is true (Alt held), tab indices are emphasized to support numeric selection.

5) Tab Overview/Switcher

- Purpose
  - Provide a tiled overview of tabs to navigate quickly. Two modes:
    - Switcher mode: activated while holding the configured modifier (default Alt). Shows tabs of the current window only; typically triggered after a small delay while Alt is held.
    - Standalone/Full overview: activated via a dedicated keybinding; either current window only or full overview across all windows.

- Composition
  - An overlay (EventBox) with a semi‑transparent background dimming the terminals.
  - A centered VBox containing: a mode label and a grid of tiles (preview + header per tab).
  - Each tile: header (index + title) and a thumbnail preview of the tab (scaled snapshot).

- Snapshots
  - Captured by rendering the terminal widget into a cairo image surface, then converting to a pixbuf.
  - Hidden or unmapped notebooks return tiny/zero allocations; snapshot capture must skip such states to avoid blank frames overwriting the cache.
  - A per‑window cache stores snapshots keyed by terminal widget.
  - In switcher mode, a periodic timer updates the snapshot for the active tab and refreshes its tile image.

- Interaction
  - Keyboard navigation: Left/Right/Up/Down; Home/End; optional Shift+Arrows to move tabs.
  - Selection: Enter selects current tile; in switcher mode overview remains only while modifier held; in standalone mode overview closes on selection.
  - New tab: dedicated binding creates a tab and refreshes the grid.
  - Close tab (optional): via binding; overlays can integrate a small “×” control.
  - Numeric selection: digits map to tab indices when no conflicting modifiers are held (or when Alt is the only modifier in switcher mode).

- Focus policy
  - When showing the overlay, grab focus on the overlay root to consume navigation keys.
  - On hide, return focus to the current terminal.

6) Cross‑Tab Search Panel

- Purpose
  - Search across all open tabs in the current window; display results grouped per tab; navigate to selected matches.

- Composition
  - Overlay panel attached to the window overlay, anchored to bottom; layout: search box (entry + case toggle + close button), status line, and a results TreeView (tab headers + match lines).
  - Uses monospace font for consistency.

- Behavior
  - Entering 2+ characters starts a search; results grouped under each tab header, showing line snippets around matches.
  - Selecting a result navigates to that tab and optionally focuses the terminal; double‑click closes the panel.
  - ESC closes the panel and returns focus to the terminal; Down focuses results list.

7) Keybinding System

- Configuration driven
  - Bindings are provided in config as strings, e.g., “MOD1+SHIFT+Left”, “CONTROL+ALT+q”, or tuples of alternatives.
  - Parsing converts a string to (modmask, keyval) using toolkit keyval lookup and a small map of modifier names.
  - Window builds a `keybinds` list mapping key combos to window methods; overview/search overlay parse and handle their own overlay‑specific bindings where appropriate.

- Dispatch
  - Window `key_pressed` walks the parsed bindings and invokes the mapped handler; returns True if handled.
  - Overlay active → overlay handles first; unhandled events may be forwarded to the active terminal (especially key releases to avoid stuck mod states).
  - Entanglement: if current terminal is entangled, forward the event to each entangled terminal.

- Special: Alt‑hold switcher
  - Config `tab_highlight_mod` is parsed to a keyval representing the “hold” modifier.
  - On press: set `on_tabswitch=True`, refresh Tab Bar (to render indices), and (optionally) schedule a timer to show switcher overlay after `tab_overview_delay_ms`.
  - On release: unset flag, cancel timer if pending, hide overlay if shown.

8) Configuration

- Sources
  - Built‑in defaults in code: fonts, palette, tablist options, keybindings, overview behavior.
  - Optional user file `~/.mypixels` in YAML format; merged over defaults.

- Notable keys
  - Fonts: `term_font`, `tablist_font`.
  - Colors: foreground/background, tab colors (selected, unselected, entangled, group colors col_0..col_7), and add button color.
  - Behavior: `focus_new_tabs`, `hide_single_tab`, `tablist_top`, `tab_title_max_length`, `tab_title_short_current`, `tab_title_shorten_start`, `tablist_hide`.
  - Terminal: `scrollback_lines`, `term_opacity`, `bg_tint_color`.
  - Overview: `tab_overview_on_alt`, `tab_overview_delay_ms`, `tab_overview_opacity`, optional thumb sizes.
  - Keybindings: `bindings` (mapping action name → string or list of strings).
  - Pywal integration: `wal_palette` path (YAML) to dynamically recolor the app.

- Validation and application
  - Basic validation for colors, fonts, ranges, and keybindings on load; warnings logged for invalid entries.
  - Application of config updates CSS and terminal properties; can be reloaded (e.g., when pywal file changes).

9) Theming and CSS

- CSS usage
  - Styles applied via a `Gtk.CssProvider` at application or window scope.
  - Overview overlay uses CSS classes/IDs to render a semi‑transparent dark background and highlighted tiles.
  - Tab Bar labels use Pango markup to colorize indices and titles.

- Theme inputs
  - Default palette and pywal palette; foreground/background and tab colors.
  - Background tint/opacity for terminals; add‑button color.

10) Tab Monitor Window

- Purpose
  - Floating preview window that shows a live snapshot of a specific terminal tab.

- Composition
  - Separate `Gtk.Window` with a DrawingArea; periodically captures and scales the tab’s snapshot into the drawing area.
  - Marked with a role/name so compositors (e.g., Wayland/niri) can match rules to float it.

- Behavior
  - Timer captures every N ms; if the original tab is closed, monitor destroys itself.
  - Avoid capturing when tab is hidden/unmapped (skip to preserve last known frame).

11) Cross‑Window Operations

- New window
  - Creates a new terminal window attached to the same process and app instance.

- Move tab to window
  - Presents a window picker dialog with summaries and (optional) small thumbnails; on selection, re‑parents the terminal tab to the target window’s notebook.

12) Data Flow & Caches

- Snapshot cache
  - Key: terminal widget
  - Value: latest pixbuf snapshot; updated on demand or periodically in switcher mode; invalidated on tab close or content changes.

- Keybind map
  - Derived from config `bindings`; parsed once on window init/reload.

- Pywal watcher
  - GFileMonitor watches the wal YAML config path; on change, (re)load palette and apply CSS & terminal colors.

13) Focus and Input Invariants

- Overlays (overview/search) grab focus while visible to ensure keystrokes drive overlay navigation.
- On overlay dismissal, focus is restored to the active terminal tab.
- Entanglement forwarding never reenters the source terminal; events are emitted to other terminals only.

14) Error Handling & Robustness

- Snapshot capture guards against invalid allocations and unmapped widgets.
- Timers are canceled when overlays hide to prevent stale callbacks.
- Keybinding parsing tolerates lists and single strings; invalid binds are ignored with warnings.
- Missing icons/palettes degrade gracefully.

15) Porting Guide (to other toolkit/language)

Required building blocks

- Terminal widget: Must provide text rendering, scrollback, font/palette settings, search APIs, and drawing to an offscreen surface (for thumbnails).
- Container model: A tabbed container (Notebook equivalent) and an overlay layer allowing stacking UI on top of terminals.
- Global application object: Supports single‑instance behavior (or replicated behavior) and multiple top‑level windows.
- Event loop: Timers, idle callbacks, file change notifications.

Key abstractions to mirror

- Window controller managing tabs and an overlay layer.
- Tab Bar renderer with flexible coloring, index rendering, click handling, and dynamic hiding.
- Overview controller with:
  - Snapshot cache keyed by terminal component instance.
  - Overlay composition, grid layout, keyboard navigation, and selection logic.
  - Periodic updates in switcher mode and snapshot invalidation on tab changes.
- Search panel controller with entry, results tree, and tab navigation.
- Keybinding system:
  - Parse human‑readable bindings to (modmask, keyval).
  - Dispatch table mapping combos to actions (function, optional arg).
  - Overlay‑local bindings that supersede global ones when active.

Event flows to preserve

- Alt‑hold switcher:
  1) key‑press: if key == configured modifier → set `on_tabswitch=True`; refresh Tab Bar appearance.
  2) start timer (`tab_overview_delay_ms`) to open switcher overlay.
  3) key‑release: unset `on_tabswitch`; cancel pending timer; hide switcher overlay if showing; return focus to terminal.

- Overview selection:
  - Navigation keys update the selected tile index.
  - Enter moves to selected tab and hides overlay (standalone) or keeps showing (switcher) depending on intended behavior.

- Search navigation:
  - Changing search text triggers incremental search and repopulates results.
  - Double‑click/Enter on a result switches tab and (optionally) hides the panel.

Visual and UX invariants

- Overlays dim the background with configurable opacity.
- Overview tiles appear centered, with clear selected state and readable text.
- Tab Bar clearly differentiates selected/unselected tabs and conveys tab indices (especially during Alt‑hold).

Performance notes

- Snapshot capture should be throttled and avoid capturing hidden tabs.
- Reuse pixbuf thumbnails when possible; scale only when drawing.
- Prefer timer intervals that balance responsiveness and CPU usage (e.g., 250–1000 ms for previews).

16) Configuration Schema (reference)

- Fonts: `term_font`, `tablist_font`
- Terminal: `scrollback_lines`, `term_opacity`, `bg_tint_color`, `foreground_color`, `background_color`
- Tab Bar: `hide_single_tab`, `tablist_top`, `tab_title_max_length`, `tab_title_short_current`, `tab_title_shorten_start`, `tablist_hide`, `tab_prepend`, `tab_append`, `tab_format`, `tab_shorten_str`
- Colors: `tab_color_fg`, `tab_color_bg`, `tab_color_fg_selected`, `tab_color_bg_selected`, `tab_color_entangle`, `tab_color_idx`, `col_0..col_7`, `add_button_color`
- Overview: `tab_overview_on_alt`, `tab_overview_delay_ms`, `tab_overview_opacity`, optional `tab_overview_thumb_width`/`height`
- Pywal: `wal_palette` path
- Keybindings: `bindings` map of name → string or list; notable names include
  - Tab move: `move_prev_tab`, `move_next_tab`, `move_first_tab`, `move_last_tab`
  - Tab nav: `prev_tab`, `next_tab`, `prev_tab_row`, `next_tab_row`, `first_tab`, `last_tab`, `goto_tab_0..9`
  - Window/Tab ops: `close_tab`, `new_tab`, `new_window`, `send_tab_to_window`, `rename_tab`
  - Visuals: `toggle_hide_tabs`, `prev_color`, `next_color`, `toggle_entangle`, `font_selector`
  - Search: `show_search`, `show_search_all`, `search_prev`, `search_next`
  - Overviews: `show_tab_overview`, `overview_exit`, `overview_select`, `overview_new_tab`, `overview_nav_*`, `overview_move_*`, `overview_first`, `overview_last`, `overview_new_window`, `overview_move_tab`
- Monitor: `monitor_tab`
  - Alt‑hold modifier: `tab_highlight_mod` (e.g., `Alt_L`)

Configuration Structure (Detailed)

- Storage
  - Location: `~/.mypixels` (YAML). If absent, built‑in defaults apply. On write, the file is overwritten with current effective config.
  - Merge: User file overlays the defaults one‑for‑one; unspecified keys keep defaults.

- Types and Semantics
  - String colors: Hex `#RRGGBB` (or CSS‑parsable) for UI surfaces and text.
  - Numbers: Integers for sizes/counts; floats in [0.0, 1.0] for opacities.
  - Booleans: true/false.
  - Keybindings: A single string (`"MOD1+Left"`) or a list/tuple of strings for alternatives.

- Top‑level keys (defaults shown)
  - Visual/Fonts
    - `term_font: "Terminus 8"`: Monospace font for terminals (family + size).
    - `tablist_font: "Liberation Mono 7"`: Font for Tab Bar/overview UI elements.
    - `foreground_color: "#BBB"`, `background_color: "#000"`: Terminal palette base colors.
    - `tablist_background: "#303030"`: Background behind the Tab Bar row.
    - `bg_tint_color: "#140008"`: Tint applied with `term_opacity` (if used by theme code).
    - `term_opacity: 0.1`: Float opacity for terminal background compositing (0.0..1.0).
    - Group colors: `col_0..col_7` (defaults from Solarized‑like palette) used to color tab titles by group.
    - Add button color: `add_button_color: "#FFA500"`.

  - Terminal behavior
    - `scrollback_lines: 1000`: Scrollback buffer per terminal.
    - `default_tab_title: ""`: Title when a new tab has no name.

  - Tab Bar behavior
    - `focus_new_tabs: true`: Focus newly created tabs.
    - `hide_single_tab: false`: Hide Tab Bar when only one tab exists.
    - `tablist_top: true`: Place Tab Bar above terminals (false → bottom).
    - `tablist_hide: false`: Force hide Tab Bar regardless of count.
    - Title truncation:
      - `tab_title_max_length: 16`: For non‑selected tabs; `-1` disables.
      - `tab_title_short_current: 32`: For selected tab; `false` disables, integer enforces length.
      - `tab_title_shorten_start: false`: If true, keep the end of the title; else keep the start.
    - Tab label format:
      - `tab_format: '<span %(color)s>%(tcorner)s%(entangled)s%(tname)s]</span>'`.
      - Variables: `tcorner` (index display or `[`), `entangled` (glyph), `tname` (title), `color` (pango foreground/background attrs).
      - `tab_shorten_str: '…'`: Ellipsis used when truncating.
      - Colors: `tab_color_fg/bg` (unselected), `tab_color_fg_selected/bg_selected` (selected), `tab_color_idx` (index), `tab_color_entangle` (entangle glyph).
    - Prepend/Append content:
      - `tab_prepend: 'clock'` and `tab_append: null`: Names of registered callables that return Pango markup (clock example provided). Null disables.

  - Window geometry
    - `default_width: 640`, `default_height: 480`: Initial window size if not maximized.
    - `default_maximize: false`: Maximize on show.

  - Overview / Switcher
    - `tab_overview_on_alt: true`: Show switcher while holding the highlight modifier after a delay.
    - `tab_overview_delay_ms: 500`: Delay before showing the switcher overlay.
    - `tab_overview_opacity: 0.3`: Background scrim opacity (0.0..1.0). UX guidance prefers 0.3–0.85 depending on context.
    - Optional: `tab_overview_thumb_width/height`: Upper bounds for tile thumbnails (implementation may compute from window size otherwise).
    - `tab_highlight_mod: 'Alt_L'`: The modifier key used to trigger on‑tabswitch behavior (press/release detection).

  - Theming / Palette
    - `wal_palette: '~/.cache/wal/colors.yml'`: pywal palette file to watch; when present, recolor UI and terminals on change.
    - `palette: ''`: Reserved for custom palette inputs; empty by default.

  - Keybindings (bindings map)
    - Value can be a string or a sequence of strings. Each string is parsed into `(mods, keyval)` via a case‑insensitive map:
      - Modifiers: `MOD1|ALT`, `SHIFT`, `CONTROL|CTRL` (also `MOD4|SUPER` accepted in some parsers).
      - Key names: GTK/GDK key names such as `Left`, `Right`, `Up`, `Down`, `Home`, `End`, `Return`, `Escape`, `KP_Add`, `KP_Subtract`, `braceleft`, `braceright`, digits, letters, etc.
    - Multiple alternatives allow redundancy (e.g., Emacs‑style and arrow‑key navigation both available).

Bindings Reference (defaults)

- Tab movement
  - `move_prev_tab: "MOD1+SHIFT+Left"`
  - `move_next_tab: "MOD1+SHIFT+Right"`
  - `move_first_tab: "MOD1+SHIFT+Home"`
  - `move_last_tab: "MOD1+SHIFT+End"`

- Tab navigation
  - `prev_tab: ["MOD1+Left", "MOD1+SHIFT+f"]`
  - `next_tab: ["MOD1+Right", "MOD1+SHIFT+b"]`
  - `prev_tab_row: "MOD1+Up"`, `next_tab_row: "MOD1+Down"`
  - `first_tab: "MOD1+Home"`, `last_tab: "MOD1+End"`
  - Direct select: `goto_tab_0..9`: `MOD1+_1` .. `MOD1+_0` (underscore usage depends on layout; mapping to digits with MOD1 is recommended when porting).

- Tab and window ops
  - `new_tab: ["CONTROL+ALT+z", "CONTROL+SHIFT+t"]`
  - `close_tab: ["CONTROL+ALT+x", "CONTROL+ALT+q"]`
  - `new_window: "CONTROL+SHIFT+n"`
  - `send_tab_to_window: "MOD1+SHIFT+w"`
  - `rename_tab: "MOD1+SHIFT+n"`
  - `monitor_tab: "MOD1+SHIFT+m"`

- Visuals & behavior
  - `toggle_hide_tabs: "CONTROL+SHIFT+v"`
  - `prev_color: "MOD1+SHIFT+braceleft"`, `next_color: "MOD1+SHIFT+braceright"`
  - `font_selector: "CONTROL+SHIFT+f"`
  - `config_reload: "CONTROL+SHIFT+r"`

- Search
  - `show_search: "MOD1+SHIFT+s"`
  - `show_search_all: "MOD1+SHIFT+g"`
  - `search_prev: "MOD1+SHIFT+a"`, `search_next: "MOD1+SHIFT+d"`

- Overview / Switcher navigation
  - Exit/select/new: `overview_exit: "Escape"`, `overview_select: "Return"`, `overview_new_tab: "MOD1+Return"`
  - Navigation: `overview_nav_left/right/up/down: "Left"|"Right"|"Up"|"Down"`
  - Move within overview: `overview_move_left/right/up/down: "SHIFT+Left"|...`
  - Bounds: `overview_first: "Home"`, `overview_last: "End"`
  - Full overview extras: `overview_new_window: "CONTROL+n"`, `overview_move_tab: "CONTROL+m"`

Validation Rules

- Colors: Must parse via toolkit color parser; invalid strings flagged with warnings.
- Numbers: Ranges enforced – e.g., `term_opacity` and `tab_overview_opacity` in [0.0..1.0], geometry in reasonable pixel bounds, truncation lengths `-1` or positive.
- Fonts: Must resolve to a valid family; otherwise warnings logged.
- Keybindings: Every string must parse to a valid keyval; invalid entries are ignored with warnings.

Example: Minimal ~/.mypixels

```
term_font: "FiraCode Nerd Font 11"
tablist_font: "Inter 10"
hide_single_tab: true
tab_overview_on_alt: true
tab_overview_delay_ms: 400
tab_overview_opacity: 0.5
tab_color_fg_selected: "#FFFFFF"
tab_color_bg_selected: "#404040"
bindings:
  prev_tab: ["MOD1+Left", "CONTROL+Page_Up"]
  next_tab: ["MOD1+Right", "CONTROL+Page_Down"]
  show_tab_overview: "MOD1+o"
  show_search_all: "MOD1+SHIFT+g"
```

Example: Full customization snippet

```
focus_new_tabs: false
tablist_top: false
tab_title_max_length: 20
tab_title_short_current: 40
tab_title_shorten_start: true
col_0: "#FFAA00"
add_button_color: "#00BFA5"
wal_palette: "~/.cache/wal/colors.yml"
tab_highlight_mod: "Alt_L"
bindings:
  toggle_hide_tabs: "CONTROL+SHIFT+v"
  rename_tab: "F2"
  monitor_tab: "MOD1+SHIFT+m"
```


17) Implementation Notes & Pitfalls

- VTE snapshotting: drawing directly to a cairo surface works only when the widget is mapped and has valid allocation.
- Overlays: ensure they grab focus while active and return focus to the terminal on hide.
- Key repeat & release: forward key release events from overlays to terminals to avoid stuck modifier state in the terminal widget.
- Single instance: re‑launches should not create a second process controlling terminals; ensure the application ID is stable unless `independent` is requested.
- Wayland/Niri integration: set role/type hints for floating Monitor windows if a compositor rule depends on them.

18) Non‑Goals

- Full terminal emulation: delegated to VTE (or equivalent) widget.
- Persistent layouts/workspaces: out of scope; window/tab layout is runtime only.

19) Test Scenarios (behavioral)

- Create/close/reorder tabs; verify Tab Bar updates and selected colors.
- Hold Alt: Tab indices highlight; after delay, switcher overlay appears; release Alt hides overlay and focuses terminal.
- Trigger full overview; navigate with arrows; select; verify thumbnails; close via ESC.
- Open Search All; search for term across tabs; navigate results via keyboard/mouse; close via ESC; focus returns.
- Move tab to other window; verify previews and correct reparenting.
- Toggle entangle; verify keystrokes broadcast only to entangled set.
- Zoom font via keybinds and Ctrl+scroll.
- Apply pywal palette; verify colors update dynamically.

Appendix A: Window/Controller Responsibilities Summary

- Application
  - Single instance, windows list, shared config, tab ID generator
  - new_window(), remove_window(), do_activate(), do_command_line()

- Window
  - new_tab(), close_tab(), prev/next tab, move tab, rename
  - show_tab_overview() (full), send_tab_to_window(), monitor_tab()
  - key_pressed(), key_released(), search/show/hide, apply_config()
  - Manage overlays, TabList rendering, entanglement

- TabList
  - Render tab entries (label/markup/colors/indices) + [+] button
  - Callbacks for tab selection and add tab

- TabOverview / TabOverviewFull
  - Build overlay (background + tiles), snapshot caching and updates
  - Navigation/selection, move, new tab, exit
  - Focus management and cleanup on hide

- SearchPanel
  - Overlay with search entry, status, and results tree
  - Incremental search across tabs, navigate and focus management

- Monitor
  - Standalone window drawing periodic snapshots of a selected terminal
  - Stops on tab destruction; avoids overwriting with blank frames when hidden


Look & UX Design

Design Principles

- Minimal chrome: Prioritize content (terminal) over UI furniture; keep controls sparse and low-contrast until needed.
- Keyboard-first: All primary tasks are accessible without a mouse; overlays capture keys and hand input back cleanly.
- Calm and legible: Careful contrast, typographic hierarchy, and spacing; avoid visual noise.
- Immediate and reversible: Actions (new tab, close, move) feel instant; overlays dismiss quickly and restore focus predictably.
- Consistency: The same visual language across tabs, overlays, search, and dialogs.

Layout & Hierarchy

- Base canvas: A single-window canvas where the terminal consumes nearly all space; optional Tab Bar occupies a slim strip (top by default).
- Overlay layer: Transient UIs (overview, search) sit above the terminal on a dimmed scrim; they are centered and balanced.
- Density: Default spacing favors a compact but breathable layout; no cramped controls, no oversized paddings.

Typography

- Terminal text: Monospace font configured by user (e.g., Terminus), tuned for readability at smaller sizes; consistent zoom in/out by 1 pt steps.
- Chrome text: Tab Bar, Search, and Overview labels use a clean sans (configurable via `tablist_font`).
- Hierarchy: Titles/indices in overview use a slightly heavier weight; results list uses monospace for a code-centric feel.

Color & Theming

- Background/foreground: High legibility defaults; honor user palette; pywal integration recolors the entire UI surface.
- Tab states: Selected vs. unselected colors clearly differentiated; entangled state denoted by accent glyph (e.g., a red ‘*’).
- Overview scrim: Semi-transparent dark backdrop (`tab_overview_opacity`) dims terminals without hiding them; tiles use subtle cards with rounded borders.

Tab Bar UX

- Structure: Index (shows prominently during Alt-hold), optional entangle glyph, title (truncated intelligently), and colored group accent.
- Selected tab: Elevated contrast foreground and subtle background fill; never pure white on black—avoid eye strain.
- Interaction: Single left-click selects; a dedicated [+] affordance creates new tabs; hover/focus affordance is subtle, not flashy.
- Visibility: Auto-hide when a single tab (if enabled), or on demand; reveal should not shift terminal content unexpectedly.

Tab Switching UX

- Primary path: Keyboard navigation (Prev/Next/Home/End) with instant visual selection and terminal focus.
- Alt-hold switcher: Holding Alt (or configured modifier) overlays the switcher after a short delay; indices on tabs (and tiles) guide quick jumps.
- Numeric selection: With appropriate modifiers, digits jump directly to a tab; feedback is immediate and forgiving.

Overview/Switcher Overlay UX

- Composition: Centered header line (mode/status) and a grid of tiles (preview + concise title). Tiles are centered within the viewport.
- Visual language: Soft card backgrounds, rounded corners, thin borders; selected tile gets a bolder border and elevated background tone.
- Previews: Scaled thumbnails maintain aspect ratio and avoid blur via bilinear filtering; gracefully degrade to placeholders when hidden.
- Navigation: Arrow keys move selection; Enter selects; ESC exits; optional Shift+Arrows reorders; number keys select by index when allowed.
- Dismissal: ESC or releasing the hold modifier dismisses immediately and returns focus to terminal; no lingering focus traps.

Search UX

- Pattern entry: Debounced search on ≥2 chars; instant status feedback (“n matches in m tabs” or error on invalid regex if implemented).
- Results list: Tree shows tab headers and match rows; monospaced content; keyboard moves between entry and results; double-click selects; ESC closes.
- Placement: Bottom-docked overlay; does not obscure all screen real estate; keeps terminal context visible above.

Focus & Input

- Overlay focus: When an overlay is visible, it captures focus and keystrokes; native terminal does not consume input.
- Focus restoration: Hiding an overlay returns focus to the active terminal deterministically (via idle callback to avoid race conditions).
- Key release forwarding: Overlays forward key releases to the terminal to avoid “stuck” modifiers.

Mouse & Pointer Behavior

- Modest affordances: Hover effects on clickable items are subtle (soft border/opacity shift); no aggressive animations.
- Click targets: Adequate minimum target sizes (≈28 px) for interactive elements like the add button and optional close buttons.

Transitions & Feedback

- Transitions: Overlays appear/disappear instantly to reduce latency; if animations are used, keep them under 120 ms and never block input.
- Feedback: Selection changes are clearly visible (highlighted tile/tab); destructive actions (close) are reversible by muscle memory (easy re-open/new tab) rather than confirmations.

Responsive Behavior & Scaling

- Thumbnails: Scale to fit within reasonable grid cells (≈300×225 default), clamped for small/large windows.
- Fonts & DPI: Respect system DPI; keep sizes logical; avoid pixel-perfect assumptions.

Accessibility & Contrast

- Contrast: Ensure sufficient contrast for text in both selected and unselected states; allow palette overrides.
- Keyboard-only: Everything reachable via keyboard; focus rings or highlights are visible.
- Motion sensitivity: Avoid large, sweeping animations; prefer instant transitions.

Defaults & Personalization

- Sensible defaults: Clean look out of the box; useful keybindings; modest scrim opacity.
- Personalization: Fonts, colors, and behaviors (hide tab bar, delay for switcher, shortcuts) are all configurable without code changes.

Micro‑interactions

- Alt-hold scrim: The scrim gently dims behind the tiles; indices reinforce mapping; releasing instantly restores the terminal.
- Add tab: [+] uses an accent color that stands out but does not shout; hover darkens slightly for feedback.
- Entanglement: A clear, compact glyph communicates the state consistently in tabs and (optionally) overlay tiles.

Empty & Error States

- No tabs: Overview does not show; attempts to open display a brief, non-intrusive status.
- Invalid search: Status line communicates the problem succinctly without intrusive dialogs.

Performance UX

- Fluidness over fidelity: Update snapshots at modest cadence; never block input to render previews.
- Skip hidden capture: Preserve last good frame rather than showing blank when a tab is offscreen.

Porting Requirements & Interfaces (High‑Level, Toolkit‑Agnostic)

Intent

- Describe the abstract contracts and behaviors a reimplementation must honor, without prescribing a particular language, widget set, or event model.
- Focus on “what” and “why”, not “how” — to enable faithful ports across Rust/Qt, Rust/egui, Swift/AppKit, etc.

Terminal Component Contract

- Buffer access: Provide read‑only access to visible + scrollback text to support cross‑tab search. If native APIs cannot expose text, maintain a mirrored scrollback store while preserving correctness with respect to screen content.
- Rendering snapshot: Provide an image snapshot of the current terminal surface. If a tab is hidden/unmapped, skip capture and retain the last valid snapshot. Never block input to render snapshots.
- Font & zoom: Support a uniform zoom step (conceptually 1 pt or a consistent scale factor) with bounds to avoid extreme sizes. Zoom affects only terminal content, not the chrome.
- Palette application: Support setting foreground/background and core palette slots. Exact mapping may vary by backend, but the result must preserve intended contrast and the visual roles described in Look & UX.
- Events: Expose title changes, child exit/EOF, and scroll events sufficient to update metadata, clean up, and support zoom on scroll with a modifier.

Overlay Layer Semantics

- Layering: The UI must support a temporary layer above terminals (the “overlay layer”). Multiple overlays may exist but only one is active at a time.
- Focus: An active overlay captures focus and keyboard input exclusively. On dismissal, focus returns deterministically to the previously active terminal via a deferred/idle focus step to avoid event races.
- Dismissal: ESC always closes overlays. Releasing the “hold” modifier closes the switcher overlay immediately. Overlays must never leave the app in a focus‑trapped state.

Keybinding Model & Naming

- Representation: A binding is a tuple of (modifiers, key). Modifiers are an abstract set (Alt/Meta, Shift, Control, Super); keys use human‑readable names (arrows, digits, letters, function keys, keypad, Return, Escape, Home/End).
- Parsing: Accept one or multiple binding strings per action. Invalid strings are ignored with a warning; duplicates are deduplicated.
- Precedence: While an overlay is active, its bindings have priority. Unhandled key releases should be forwarded to the terminal to prevent “stuck” modifiers.
- Alt‑hold: A designated modifier (the “highlight” key) toggles the on‑tabswitch state (press → on; release → off), drives Tab Bar index highlighting, and can trigger the switcher overlay after a delay.

Process & Windows

- Single‑instance: A single application instance manages multiple windows. Secondary launches may signal the primary instance to open a new window or pass commands.
- Multi‑window: Each window has its own tab set and overlay layer; a global registry allows cross‑window operations (e.g., “Move tab to window” and full overview across all windows).

Tab Lifecycle & Reparenting

- Create: Spawn a terminal, assign metadata (id/title/group), attach to the window’s tab container, and focus it depending on configuration.
- Close: Remove from the container; update metadata; if the terminal process exited, release resources promptly.
- Reparent: Moving a tab between windows must preserve its process, buffer, and metadata. If reparenting is not supported by the target UI, declare a compliant fallback (e.g., disallow, or recreate terminal with PTY handoff) and document UX impact.

Snapshot Strategy

- Goal: Provide timely, low‑overhead thumbnails for the overview/switcher.
- Rules: Skip capture when a tab is hidden/unmapped or reports degenerate geometry; never overwrite the last good frame with blank. Throttle updates at a sensible cadence; the switcher may use a more frequent cadence than the standalone overview.

Cross‑Tab Search Model

- Matcher: Use a line‑based matcher with an option for case sensitivity. If regex is supported, report invalid patterns non‑intrusively.
- Results: Group by tab; each result includes (tab identifier, line number, excerpt around match with ellipses, match range). The UI shows tab headers + match rows in a monospace list.
- Navigation: Single‑click highlights without closing; double‑click (or Enter) selects and closes, returning focus to the terminal.

Styling & Theming Tokens

- Tokens: Define a small set of style tokens (colors, radii, border widths, spacing, scrim opacity) applied consistently across the app. Avoid toolkit‑specific CSS where possible; map tokens to the target toolkit’s styling facilities.
- States: Selected vs. unselected tile/tab styles are visually distinct but remain calm (no harsh contrasts). Entanglement is an accent glyph with a clear color.

Configuration Schema Guidelines

- Shape: A flat key–value map with nested maps for related concerns (e.g., `bindings`). Provide defaults and ranges for each key.
- Validation: Colors must parse; opacities in [0,1]; lengths either −1 for unlimited or a positive integer; fonts must resolve to an installed face.
- Extensibility: Unknown keys are ignored with warnings; future additions should not break existing files.

System Integration Expectations

- Live theme reload: Watch a palette file or theme source and reapply colors on change with debounce.
- Compositor hints: The monitor/floating window must be identifiable (role/title/app id) so compositor rules can float it; the main windows share a stable app id for single‑instance behavior.

Performance & Responsiveness Budgets

- Input: Keystrokes drive UI immediately; overlays appear/dismiss without noticeable lag.
- Snapshots: Throttle to a modest cadence (hundreds of ms). Never block input to render thumbnails.
- Memory: Snapshot caches should be bounded implicitly (limited by tab count) and drop entries when tabs close; avoid large retained images when not needed.

Accessibility & Input

- Keyboard reachability: All functionality is accessible without a mouse; visible focus indication is required.
- Contrast: Maintain sufficient contrast for text and selected states; provide user‑controlled palette overrides.
- Motion sensitivity: Avoid motion; if used, keep transitions short and never block interaction.
