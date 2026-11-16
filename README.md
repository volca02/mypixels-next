Mypixels
========

A tabbed terminal emulator written in Rust: `alacritty_terminal`
drives the emulation and pty, `glyphon` renders on `wgpu` in a
`winit` window, and an `egui` layer draws the tab strip, config
panel, font dialog and tab switcher.

This is a from-scratch rewrite of the original Python/GTK/VTE
terminal of the same name, kept unchanged at
[`legacy/mypixels`](legacy/mypixels) (a single script,
GPL-3.0-or-later) as the behavioural reference this rewrite
reproduces, down to the Alt-held tab switcher.

Design principles, carried over from the original:

* screen space effectivity - no menus or status lines that can't be
  hidden or take up too much space
* all keyboard operation

Building
========

    cargo build --release        # target/release/mypixels
    cargo test                   # unit tests, no display needed

Headless UI smoke tests:

    MYPIXELS_BIN=$PWD/target/release/mypixels xvfb-run -a python3 tests/preview_smoke.py

Usage
=====

    usage: mypixels [options] [command args...]
      -n, --new-window   open a new window in the running instance (default)
      -t, --new-tab      add a tab to the originating or first session window
      -s, --standalone   run a private instance: never join or host a session
          --daemon      start a persistent session without opening a window
          --restore-window  reopen the last closed window with its live tabs

Without flags, mypixels finds the running instance on this display over D-Bus
and asks it to open a new window for the command in the current directory.
It prints the instance and requested action to stdout before sending the request.
Use `-t` / `--new-tab` to add a tab to the originating window (or the first
session window when launched elsewhere). If no session answers, it starts a
new instance. `-n` remains an explicit spelling of the default window action.

### Persistent session and window graveyard

`mypixels --daemon` starts a session with **no windows and no terminal children**.
Launch `mypixels` normally to open windows in it. Closing the last window leaves
the daemon running; its terminals continue running in the window graveyard.
Closing a whole window preserves its tab order, workspaces, selected workspace,
and each workspace's selected tab. The native window and GPU resources are
released until restoration. Restored shells retain their window's session ID,
so `mypixels --new-tab` from one still opens in the restored window.

Restore the most recently closed window with `mypixels --restore-window` from
any launcher, or **Ctrl+Alt+Shift+Z** from another Mypixels window. The application
menu also offers **Restore last closed window**. The graveyard panel
(**Ctrl+Shift+G**) lists closed windows with **Restore window** and **Kill window**
buttons, alongside individually closed tabs. **Ctrl+Shift+Z** raises the last
closed tab into the current window.

`graveyard_limit` (default 5) bounds closed tabs and closed windows separately.
A whole window counts as one window burial regardless of its tab count. Each
list evicts its oldest entry when full. Children that exit while buried are
removed; restoring a window whose selected workspace emptied selects another
live workspace. These are live processes held in memory: stopping the service,
logging out, or a daemon crash ends them.

The close dialog is enabled by default. Under **Settings → Window → Close
window dialog**, choose **Never**, **When tabs would be lost**, or **Always**
(the default). The YAML key `close_window_dialog` accepts `never`, `loss`, or `always`.
Loss-only prompts when neither this session's daemon nor another open window
can retain the terminals. Choosing Never skips the dialog and its safeguards.

The dialog defaults to **Cancel**. **Move to Graveyard** appears only when
recovery is available. **Close permanently…** opens a separate danger step;
when recovery is unavailable the dialog opens directly at that step. The red
permanent-close button unlocks after five seconds and requires an explicit
action. Timer expiry never closes anything. Closing the remaining recovery
host while a dialog is open switches it to a fresh warning and countdown.

To pre-spawn the daemon for your graphical login:

```sh
cargo build --release
scripts/install.sh install
systemctl --user enable --now mypixels.service
```

The installer writes the unit to `$XDG_CONFIG_HOME/systemd/user/mypixels.service`
(default `~/.config/systemd/user/mypixels.service`) with the installed binary's
absolute path. It installs the unit without enabling or restarting it. Enable
it before launching the first window: a foreground instance already owning this
display must exit before a daemon can own the same session. Normal foreground
and `--standalone` instances still exit when their last window closes.

The service follows `graphical-session.target`, and needs the graphical display
environment in the user manager. On compositors that do not import it, run:

```sh
systemctl --user import-environment DISPLAY WAYLAND_DISPLAY XAUTHORITY
systemctl --user start mypixels.service
```

For compositors that do not activate `graphical-session.target`, run those
commands from the compositor's startup configuration. This is a windowless
graphical client: it connects to the display, but creates windows and GPU
surfaces only when requested. Check its logs with
`journalctl --user -u mypixels.service`.

`mypixels last [n] [--copy]` prints (or clipboard-copies) the output
of the last n commands from the running session's memory pool - a
query, not a launch.

Features
========

* Font sizes in Settings → Appearance → Fonts accept up to two decimal places,
  such as `11.2` or `11.25` pt. Click the size to type a value; the +/− buttons
  step by one point. Config values also accept decimals, e.g. `term_font: 'Cascadia Code 11.2'`.
* Adjustable line spacing in Settings → Appearance → Fonts. `0` keeps the
  normal spacing, `+0.1` adds 10% row height, and `-0.1` removes 10%, without
  changing font size. The `line_spacing` config key accepts -0.9 through 3.0;
  row heights are rounded to whole pixels. Changes apply live and persist.
* Tab switcher with cached GPU previews - hold Alt: each tab is a scaled,
  live view of its own terminal; arrows move the selection. Hover a tab for
  its preview. Selection and reordering animate in both the strip and switcher.
  With workspaces on, the switcher's header names the workspace its tiles came
  from, and a workspace switch keeps the switcher open while Alt is held - the
  tiles and the identity color follow the workspace you moved to.
  Settings → Navigation → Tab switcher enabled turns the switcher on or off without
  changing numbered shortcuts (`tab_overview_on_alt` in the config).
  Show tab titles controls whether tabs show captions or only their numbers
  (`show_tab_titles`); both options are enabled by default. Number-only tabs
  retain title tooltips and can be closed by middle-click or the close shortcut.
* Partial terminal redraws from Alacritty damage, into persistent per-tab GPU
  textures shared by the live window, previews, and image export.
* Optional compact tabs in Settings → Tabs → Tab bar (`compact_tabs: true`).
  Reduced padding and strip height keep the same typography, workspace colors,
  and close controls. The default spacing is unchanged.
* Terminal brightness in Settings → Appearance → Theme & colors
  (`terminal_brightness: -100` through `100`). `0` preserves the palette;
  negative percentages darken terminal colors and positive percentages brighten
  them. The theme and interface colors stay intact.
* Animation speed in Settings → Appearance → Motion. `1×` keeps the default
  timing, `0.25×`–`4×` changes UI transition speed, and `0` turns animations off.
  Changes preview live; Save persists `animation_speed` in the configuration.
* Workspaces - opt-in (`workspaces_enabled: true`; Settings → Navigation →
  Workspaces). Off by default, in which case the workspace bindings and the
  tab-bar button are inert and tabs keep their normal color. When on,
  Alt+Shift+1…0 select ten independent tab sets in each window. Alt+PgUp/PgDown
  cycle to the previous/next workspace, wrapping at the ends. Hold Alt to keep
  the tab switcher open while browsing workspaces. These bindings are configurable
  as `prev_workspace` and `next_workspace` in Settings → Keys. The letter
  chords Alt+Q/W/E/R/T/Y/U/I/O/P are the opt-in alternative
  (`workspace_letter_keys: true`), left off because Alt+letter belongs to
  shells and TUIs. Alt+1…9/0 select tabs within the current workspace, creating
  missing tabs up to that number automatically (0 selects tab 10). Existing tabs
  keep their numbers; new tabs are appended. Workspaces remember the selected tab
  and keep their terminals running. Visiting an empty workspace opens its
  first terminal. The workspace button, drawn in its own colour, opens the workspace panel.
  Choose **Left** (beside the menu) or **Right** (the default, at the end of the strip)
  under **Settings → Appearance → Workspaces → Button position**.
  This applies immediately and saves as `workspace_button_position: left` or `right`.
  In the same card, select a workspace to edit its name, color, and icon. Choose
  **Number**, **Name** (default), **Icon**, or **Icon and name** for the strip button.
  Names fall back to the workspace number when empty. The display choice applies
  to every workspace (`workspace_button_display: number|name|icon|icon_and_name`);
  icons are saved separately as `workspace_icon_1` through `workspace_icon_10`.
  The panel shows cards in their own colours, with tab counts and live terminal previews.
  The selected card expands; click a preview to open that exact tab. Each view
  scrolls to its focused tab, with left/right buttons when more cards are available.
  Up/Down select a workspace and Enter switches to it. Left/Right focus the
  previous/next tab, without wrapping; overflow buttons do the same. The preview
  strip animates to reveal the focused tab when needed. Search (or Ctrl+F) finds workspaces by
  number, genre or tab title; the filter hides empty slots, and + opens the next
  empty workspace. The panel stays open as you switch. Esc closes search first,
  then the panel. Workspace names retain the existing config keys
  (`workspace_genre_1` through `workspace_genre_10`); they describe a purpose,
  independent of terminal directories. `show_workspaces` is its action, shipped unbound
  so it collides with nothing, and bindable in Settings → Keys. Closing a workspace's last tab
  returns to another populated workspace; closing the window's final tab
  closes the window. Bindings are configurable as `goto_workspace_0` through
  `goto_workspace_9`, in Settings → Keys or under `bindings:`. Set each
  workspace's color in Settings → Appearance →
  Colors. Tabs inherit their workspace color; W1–W9 have distinct defaults.
  Colors are saved as `workspace_color_1` through `workspace_color_10` in the
  configuration.
* Entanglement - one command, many tabs: toggle a tab into the group
  (Alt+Shift+*) and keystrokes land in all of them.
* Tab colorization - mark a tab with a colour so it can be easily
  found (Alt+Shift+{ / }).
* All keyboard - Alt+Left/Right/Up/Down/Home/End move between tabs,
  Ctrl+Alt+Z new tab, Ctrl+Alt+X close tab, Ctrl+Shift+R reloads the
  config, Ctrl+Shift+C/V copy and paste (the GTK bindings),
  Shift+Insert pastes the primary selection.
* Kitty graphics protocol - PNG images (`icat`) render as textured
  quads in the grid and scrollback.
* Kitty keyboard protocol - full disambiguation, with real key
  releases.
* Desktop notifications from terminal applications over OSC 9, OSC 99, and
  OSC 777. Clicking a notification returns to its originating tab. OSC 99
  supports chunked titles/bodies, updates, close/activation reports, queries,
  and focus/visibility filtering; desktop calls run off the UI thread.
* Command observability - with the shell integration scripts (OSC
  133 marks) every command is captured with its output, exit status
  and location: Alt+Shift+H opens the history panel, and
  `mypixels last` / Ctrl+Shift+L read the last output from the shell.
* MCP endpoint - a read-only Model Context Protocol server on a
  per-display unix socket: LLM tooling lists the session's windows
  and tabs and reads captured command output without a shell.
* Config panel and font dialog (Alt+Shift+C); the file reloads live.

Configuration
=============

The terminal icon at the left of the tab bar opens the application menu:
New window, Settings, Keyboard shortcuts, and About. Shortcut labels and the
keyboard reference reflect your current configured bindings.

Config is `$XDG_CONFIG_HOME/mypixels/config.yml` (the legacy
`~/.mypixels` is read until an XDG file exists). Every key is
editable in the config panel. Settings → Keys records bindings: click a
chord, press a new one, and the keystroke goes to the recorder rather than the
shell. A recorded chord is stored by key position, so Alt+Shift+2 reads as `2`
on any layout and still fires where that cap types `@`, `"` or `č`. Saving
rewrites the `bindings:` block only when something was rebound, so hand-written
bindings survive unrelated changes.

Themes are YAML colour sets, discovered in this repo's `themes/`,
`~/.config/mypixels/themes`, `~/.local/share/mypixels/themes` and
the XDG data dirs; see [themes/README.md](themes/README.md).

For command capture, source the matching
`scripts/shell_integration.{bash,zsh,fish}` from your shell rc.

Layout
======

* `src/` - the terminal (Rust)
* `scripts/` - shell integration, test harnesses
* `tests/` - unit and headless UI smoke tests
* `themes/` - bundled themes
* `legacy/mypixels` - the original Python/GTK/VTE implementation
