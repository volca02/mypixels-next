# Terminal UI styling

`src/ui_style.rs` owns the reusable visual language: the theme palette,
typography, vector icons, outlined surfaces, number badges, action rows, form
sections, toggles, and picker density. Change these components here so history,
settings, the switcher, and the tab strip stay consistent. Screen modules own
content, state, and layout rather than copies of these treatments.

Menus use `TermStyle::menu_frame` for the theme-derived background, thin border,
nine-point corners and padding. Wrap their content in `menu_contents` for shared
typography and density; use `menu_item` and `menu_separator` for rows and groups.
Application and terminal context menus use these components.
Long menus scroll within the window. `action_row` is the underlying reusable
control, also used by history details: optional vector icon, truncated label,
right-aligned shortcut, muted disabled state, and accent outline on hover/focus.
Add or change this treatment in the library rather than drawing local copies.

Popup anchors are logical points. Convert a native physical pointer with
`egui_ui::popup_position` and the window's current `pixels_per_point` once at the
event boundary. Strip-relative anchors use the strip height divided by that same
scale. Popup contents own keyboard input until dismissal.

`TermStyle` resolves the effective terminal theme. `TabStyle` adds the strip's
state colors and preserves explicit tab color overrides; legacy default colors
adopt the shared chrome. Font previews and tab labels use the same installed
font provider, so the configured tab font remains effective.

`src/tab_bar.rs` lays out and draws the compact strip through egui. It provides
clipped geometry in logical points to the window controller. The controller
converts this to physical pixels and retains native drag/reorder/detach input;
no tab labels or backgrounds pass through the terminal's glyphon/wgpu renderer.
The strip is at least 28 logical pixels high and grows for larger configured
fonts. Horizontal scrolling keeps the active tab and close control reachable.

Sidebars and the tab strip remain passive keyboard surfaces. Explicitly focused
text and number editors own editing input; panel shortcuts stay global.

## Layout ownership

Components own their internal spacing and report their actual geometry.
`measure_section` runs the same content in an invisible sizing UI;
`bottom_section` reserves that measured footer and gives the body the remaining
rectangle. Settings, workspace and graveyard lists use this composition. Close
dialogs measure their footer before constraining scrollable details. History
details lay out naturally, with an explicit list/preview space policy.

`SwitcherLayout` owns the frame, measured header/footer and preview grid. It
reserves at least two rows and two columns when the window can fit them,
reducing preferred preview size to fit beside the workspace rail. Narrow
windows use one column and vertical scrolling. Texture arrival does not change
the layout. Keyboard navigation consumes the published column count.
`WorkspaceCarouselLayout` similarly owns tile sizes and overflow gutters;
callers provide a rectangle and compact/expanded presentation.

Area-based sidebars derive their inner size from the frame's actual margins.
egui Window size constraints already describe the outer frame. Context menus
use the available screen rectangle and frame margins; information dialogs
subtract their measured footer and the header space already consumed before
allocating a scroll viewport. Font descriptions reach their buttons in full,
so text fitting follows the actual font and available width.

The tab strip reserves the add-button rectangle before sizing the tab viewport.
When tabs occupy less than that viewport, the same button follows the last tab.
The viewport and tab width limits use that allocation rather than separate
allowances for the button.

Control geometry is recorded by semantic name. `MYPIXELS_UI_GEOMETRY=1` logs
visible clipped bounds in native pixels for graphical tests; tests should use
these bounds to interact, then check the resulting behavior or pixels instead
of duplicating the component's sizing formulas.

The terminal text-buffer pool grows to the prepared run count before shaping
and borrowing buffers. Damage filtering and blank-run elimination determine
which runs need drawing; pool capacity never truncates terminal contents.

## Shared motion and previews

Components declare target states through `transition`, `animated_rect`, and
`animated_rect_in`. egui's animation manager owns interpolation, interrupted
transitions and repaint requests. The shared policy is 160 ms, with cubic easing
for state changes and linear geometry movement. `animated_rect_in` keeps a
selection inside its viewport. Stable tab IDs identify movement; grid-local
coordinates keep scrolling separate from tab reordering. New surfaces snap to
their initial layout; position and size changes share the same interpolation.
The tab bar and switcher both use moving selection outlines and fading state
fills. Add future motion through these helpers instead
of per-widget timers or frame loops.

The window controller preserves both immediate and delayed egui repaint
deadlines, including tooltip delays. It wakes the event loop when needed rather
than relying on unrelated terminal output or cursor blinking.

Switcher and hover previews share `preview_image` and persistent GPU thumbnails
keyed by tab ID. Changed frames render in batches of at most two; idle candidate
preparation is limited to five times a second, with active/hovered tabs first.
The GPU box-filter downsamples at native display density, and egui samples those
textures directly. Closing the switcher keeps the cache; closing/moving tabs
releases their renderer registrations. CPU readback is reserved for explicit
copy-as-image exports.

Animation speed is a user preference under Appearance → Motion. Shared
transitions use 160 ms at 1×, scaled by `animation_speed`; 0 disables motion.
Workspace preview strips use the same duration when a newly focused tab needs
to be revealed. Compact tabs are an optional density setting under Tabs → Tab
bar: they change padding and height while retaining this palette and typography.
Terminal brightness affects resolved terminal colors, including cached previews,
and does not recolor the interface or overwrite theme colors.
