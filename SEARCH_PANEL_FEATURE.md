# Cross-Tab Search Panel Feature

## Overview
A new IDE-style search panel has been added to mypixels that allows searching across all tabs simultaneously with a visual results panel.

## Usage

### Keybinding
- **Alt+Shift+G** - Show the cross-tab search panel (G for "Global search")

### Features
1. **Search Input**: Type search term (minimum 2 characters)
2. **Case Sensitivity**: Toggle with checkbox
3. **Results View**: Tree structure showing:
   - Tab headers with match counts (collapsible)
   - Individual match lines with line numbers and context
4. **Navigation**:
   - Click or press Enter on a result to jump to that tab
   - Arrow keys to navigate results
   - Escape to close panel
   - Down arrow from search box to move to results

### UI Layout
```
┌─────────────────────────────────────────┐
│ Search: [query________] ☐ Case sensitive  [✕] │
│ 5 matches in 2 tabs                     │
├─────────────────────────────────────────┤
│ ▸ Tab 1: ~/project (3 matches)          │
│   line 45: ... match context ...        │
│   line 67: ... match context ...        │
│ ▸ Tab 3: ~/logs (2 matches)             │
│   line 123: ... match context ...       │
└─────────────────────────────────────────┘
```

## Implementation Details

### New Class: SearchPanel (lines 1650-2030)
Located before the TabList class in mypixels file.

**Key Methods:**
- `show()` - Display the search panel at bottom of window
- `hide()` - Close panel and return focus to terminal
- `perform_search(text)` - Search all tabs using regex
- `search_terminal(terminal, pattern)` - Extract and search text from VTE widget
- `on_result_activated()` - Navigate to selected match

**Architecture:**
- Uses Gtk.TreeStore with collapsible tab groups
- Extracts terminal text using VTE's `get_text()` API
- Searches line-by-line with regex pattern matching
- Displays results in monospace font with context preview

### Integration Points

1. **Config** (line 140):
   ```python
   'show_search_all': 'MOD1+SHIFT+g'
   ```

2. **Window Init** (lines 2377-2380):
   ```python
   self.search_panel = SearchPanel(
       self.window, self.overlay, self.notebook,
       self.tabs, self.config, self
   )
   ```

3. **Action Handler** (line 2427):
   ```python
   'show_search_all': (self.show_search_all, None)
   ```

4. **Method** (lines 3181-3183):
   ```python
   def show_search_all(self, param=None):
       self.search_panel.show()
   ```

## Differences from Single-Tab Search

| Feature | Single-Tab (Alt+Shift+S) | Cross-Tab (Alt+Shift+G) |
|---------|--------------------------|-------------------------|
| UI | Minimal search box | Full results panel |
| Scope | Current tab only | All tabs in window |
| Results | Navigate with keybinds | Visual tree with click |
| Display | Inline highlighting | Line number + context |
| Persistence | Hidden after first match | Stays open for browsing |

## Future Enhancements

Potential improvements marked with TODO:
- Line positioning/scrolling after navigation (line 2024)
- Highlight matched text in result preview
- Search history
- Regex mode toggle (currently always enabled)
- Limit results per tab for performance
- Search across windows (not just current window)
