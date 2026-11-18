# CLAUDE.md

This file provides guidance to Claude Code when working with code in this repository.

## Project Overview

**mypixels** is a lightweight, Python-based terminal emulator built with GTK3 and VTE bindings. It emphasizes keyboard-driven operation, screen space efficiency, and multi-tab management with advanced features.

## Architecture

### Core Components

The project is a single-file application (`mypixels`) with the following key classes:

1. **TabOverview** (lines 342+): Per-window tab switcher/overview
   - Displays thumbnails of terminal tabs in a grid
   - Two modes: 'switcher' (triggered by Alt key hold) and 'standalone' (triggered by keybinding)
   - Handles keyboard navigation and tab selection

2. **TabOverviewFull** (lines 1154+): Multi-window tab overview
   - Shows tabs from all windows organized by window
   - Supports moving tabs between windows
   - Triggered by `show_tab_overview` keybinding (default: Alt+o)

3. **MypixelsWindow**: Main window class managing tabs and UI
   - Tab management and navigation
   - Keyboard event handling and routing
   - Configuration management
   - D-Bus integration for single-instance detection

4. **MypixelsApp**: Application instance manager
   - Handles application lifecycle
   - Manages multiple windows
   - Coordinates shared configuration

### Key Features

- **Tab Colorization**: Visual grouping with 8 configurable colors
- **Tab Entanglement**: Type in multiple terminals simultaneously
- **Tab Overview**: Visual tab switcher with thumbnails
- **Multi-Window Support**: Move tabs between windows
- **Configurable Keybindings**: All shortcuts customizable via YAML config
- **Search**: Terminal text search with prev/next navigation
- **Font Scaling**: Runtime font size adjustment

## Configuration

### Config File Location
`~/.mypixels` (YAML format)

### Config Structure (lines 56-178)
- Visual appearance (colors, fonts, opacity)
- Behavioral settings (focus, tab display)
- Keybindings dictionary
- Tab overview settings

### Important Config Options
- `tab_overview_on_alt`: Show tab overview when Alt is held (default: True)
- `tab_overview_opacity`: Background opacity for overview (0.0-1.0)
- `bindings`: Dictionary mapping action names to key combinations
  - Supports tuples for multiple bindings per action
  - Modifiers: MOD1 (Alt), SHIFT, CONTROL
  - Special keys: Home, End, Left, Right, Up, Down, Return, Escape

### Config Validation (lines 257+)
Range validation for numeric values with automatic clamping

## Key Event Flow

### Tab Switching with Overview (lines 2032-2083)
1. Alt key press detected → Check if `tab_overview_on_alt` enabled
2. Show overview in 'switcher' mode (if not already active)
3. User navigates with arrow keys
4. Alt key release → Hide overview and switch to selected tab

### Keybinding System (lines 1965-2010)
- Keybindings parsed from config into action → (keyval, modifier) mapping
- Key events first routed to active overview (if any)
- Then checked against action bindings
- Terminal receives unhandled keys

## Development Patterns

### Adding New Config Options
1. Add default value in `config` dict (lines 56+)
2. Add validation if needed in `validate_value()` (lines 257+)
3. Add to `CONFIG_ATTRS` list for categorization (lines 281+)
4. Use `self.config.get('option_name', default)` or `self.config['option_name']`

### Adding New Keybindings
1. Add binding to default `config['bindings']` dict (lines 109+)
2. Map action name to handler in `MypixelsWindow.__init__` (lines 2007+)
3. Handler receives optional parameter (None for simple key press)
4. Use `_check_keybind(keyval, mods, 'action_name')` to check key match

### Tab Overview Integration
- Overview instances cache terminal snapshots for performance
- Snapshots invalidated on tab updates via `invalidate_cache(tab)`
- Overview updates happen asynchronously to avoid UI blocking

## Common Tasks

### Adding a New Action
1. Define action name (e.g., 'my_action')
2. Add default keybinding in config
3. Create handler method in MypixelsWindow
4. Map in action_handlers dict
5. For overview-specific actions, prefix with 'overview_' and handle in TabOverview class

### Modifying Tab Behavior
- Tab state stored in `self.tabs` dict (tab widget → metadata dict)
- Current tab tracked in `self.active`
- Tab list UI updated via `update_tablist()`

### Working with D-Bus
- Single instance detection via org.mypixels.TerminalEmulator
- Window coordination for tab migration
- Methods exposed: ListWindows, TransferTab, FocusWindow

## Code Style Notes

- Snake_case for methods and variables
- Configuration accessed via `self.config` dict
- GLib/GTK timeout callbacks return False to cancel, True to continue
- Signal connections managed with handler IDs for cleanup
- CSS styling applied dynamically via Gtk.CssProvider

## Testing Approach

Manual testing workflow:
1. Modify config or code
2. Reload config with Ctrl+Shift+R (if config change)
3. Or restart terminal
4. Test keybindings and visual changes
5. Check for GTK warnings in stdout

## Recent Changes (Git History)

- Multi-window architecture with tab migration
- Configurable tab overview/switcher keybindings
- Single instance detection via D-Bus
- Unknown keypress propagation from tab switcher to terminal
