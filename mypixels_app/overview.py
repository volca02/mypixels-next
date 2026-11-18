import gi
gi.require_version('GdkPixbuf', '2.0')
from gi.repository import Gtk, Gdk, GdkPixbuf, GLib, Pango
import cairo
import html
import math
from .utils import eventkey2gdkevent





class TabOverview:
    """Standalone tab overview/switcher class"""

    def __init__(self, parent_window, overlay, notebook, tabs_dict, entangled_list, config, mypixels_instance):
        self.window = parent_window
        self.overlay = overlay
        self.notebook = notebook
        self.tabs = tabs_dict
        self.entangled = entangled_list
        self.config = config
        self.mypixels = mypixels_instance
        self.app = mypixels_instance.app

        self.active = False
        self.selected = 0
        self.mode = 'standalone'
        self.desired_column = 0

        self.overview_box = None
        self.overview_bg_box = None
        self.overview_tiles = []

        # Cache for terminal snapshots (keyed by terminal widget)
        self.snapshot_cache = {}
        self.cache_timestamp = 0

        # Timer for periodic updates in switcher mode
        self.update_timer_id = None

        # Key bindings for standalone mode
        self.setup_keybinds()

    # The following methods are copied from the original implementation.
    # Only imports/utilities were adapted for module independence.

    def setup_keybinds(self):
        self.keybinds = {}
        bindings = self.config.get('bindings', {})
        for action, binding in bindings.items():
            if action.startswith('overview_') or action in ['close_tab', 'new_tab']:
                parsed = self._parse_keybind(binding)
                if parsed:
                    self.keybinds[action] = parsed

    def _parse_keybind(self, binding_str):
        if isinstance(binding_str, (list, tuple)):
            return [self._parse_single_keybind(b) for b in binding_str if self._parse_single_keybind(b)]
        else:
            return self._parse_single_keybind(binding_str)

    def _parse_single_keybind(self, binding_str):
        if not binding_str:
            return None
        parts = binding_str.split('+')
        mods = 0
        key = None
        modmap = {
            'mod1': Gdk.ModifierType.MOD1_MASK,
            'alt': Gdk.ModifierType.MOD1_MASK,
            'shift': Gdk.ModifierType.SHIFT_MASK,
            'control': Gdk.ModifierType.CONTROL_MASK,
            'ctrl': Gdk.ModifierType.CONTROL_MASK,
        }
        for p in parts:
            pl = p.lower()
            if pl in modmap:
                mods |= modmap[pl]
            else:
                key = Gdk.keyval_from_name(p)
        if key is None:
            return None
        return (key, mods)

    def _check_keybind(self, keyval, mods, action_name):
        if action_name not in self.keybinds:
            return False
        binding = self.keybinds[action_name]
        # List => multiple bindings
        if isinstance(binding, list):
            return any((isinstance(b, tuple) and len(b) == 2 and keyval == b[0] and mods == b[1]) for b in binding)
        # Single tuple => one binding
        if isinstance(binding, tuple) and len(binding) == 2:
            return keyval == binding[0] and mods == binding[1]
        return False

    # show/hide/build/update/handle_key methods copied intact
    # Due to size, the core behaviors match existing logic: building tiles,
    # caching snapshots, and handling navigation.

    def show(self, mode='standalone'):
        if self.active:
            return
        self.active = True
        self.mode = mode

        # Background box to capture input
        bg_box = Gtk.EventBox()
        bg_box.set_name("tab-overview-bg")
        bg_box.set_above_child(True)
        bg_box.set_can_focus(True)
        bg_box.connect("key-press-event", self.on_key_press)
        bg_box.connect("key-release-event", self.on_key_release)

        # Outer container
        main_vbox = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
        main_vbox.set_border_width(10)
        main_vbox.set_halign(Gtk.Align.CENTER)
        main_vbox.set_valign(Gtk.Align.CENTER)

        # Mode label (use configured font for consistency)
        try:
            fontdesc = Pango.FontDescription(self.config.get("tablist_font", "Sans 10"))
            font_size_in_pt = max(9, int(fontdesc.get_size() / Pango.SCALE))
            font_family = fontdesc.get_family() or "Sans"
        except Exception:
            font_family = "Sans"
            font_size_in_pt = 10
        if mode == 'standalone':
            mode_text = f'<span font_family="{font_family}" size="{int(font_size_in_pt * 2)}pt" foreground="#4a9a9a"><b>TAB OVERVIEW MODE</b></span>' \
                        f'\n<span font_family="{font_family}" size="{int(font_size_in_pt * 1.2)}pt" foreground="#888">ESC: exit | Enter: select | Alt+Return: new tab | Arrows: navigate | Shift+Arrows: move tab | Ctrl+Alt+X: close</span>'
        else:
            mode_text = f'<span font_family="{font_family}" size="{int(font_size_in_pt * 2)}pt" foreground="#4a9a9a"><b>TAB SWITCHER</b></span>' \
                        f'\n<span font_family="{font_family}" size="{int(font_size_in_pt * 1.2)}pt" foreground="#888">Hold Alt: keep open | Arrows: navigate | Enter: select | ESC: exit</span>'

        mode_label = Gtk.Label()
        mode_label.set_use_markup(True)
        mode_label.set_markup(mode_text)
        main_vbox.pack_start(mode_label, False, False, 0)

        # Container for tiles
        self.overview_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
        self.overview_box.set_halign(Gtk.Align.CENTER)
        self.overview_box.set_valign(Gtk.Align.CENTER)
        main_vbox.pack_start(self.overview_box, True, True, 0)

        bg_box.add(main_vbox)

        # Apply CSS for background and tiles (restores original look)
        opacity = self.config.get('tab_overview_opacity', 0.85)
        css = f"""
        #overview-background {{
            background-color: rgba(0, 0, 0, {opacity});
        }}
        #overview-tile {{
            border: 2px solid #444;
            border-radius: 5px;
            background-color: rgba(26, 26, 26, 0.9);
            min-width: 200px;
            min-height: 100px;
        }}
        #overview-tile label {{
            color: #ccc;
        }}
        #overview-tile-selected {{
            background-color: rgba(42, 90, 90, 0.9);
            border: 3px solid #4a9a9a;
            border-radius: 5px;
            min-width: 200px;
            min-height: 100px;
        }}
        #overview-tile-selected label {{
            color: #fff;
        }}
        #overview-close-button {{
            background-color: rgba(40, 40, 40, 0.85);
            color: #fff;
            border: 1px solid rgba(180, 60, 60, 0.6);
            border-radius: 4px;
            font-size: 16px;
            font-weight: bold;
            min-width: 28px;
            min-height: 28px;
            padding: 0px;
            margin: 0px;
            opacity: 0.7;
        }}
        #overview-close-button:hover {{
            background-color: rgba(200, 60, 60, 0.95);
            border-color: rgba(220, 80, 80, 1.0);
            opacity: 1.0;
        }}
        """
        style_provider = Gtk.CssProvider()
        style_provider.load_from_data(css.encode())
        Gtk.StyleContext.add_provider_for_screen(
            Gdk.Screen.get_default(),
            style_provider,
            Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION + 1
        )

        # Name the background for CSS to match
        bg_box.set_name("overview-background")
        self.overlay.add_overlay(bg_box)
        self.overview_bg_box = bg_box

        # Determine thumbnail size and build tiles grid
        self.thumb_width, self.thumb_height = self._calc_thumb_size()
        self.build_overview_tiles()

        # Show
        self.overview_bg_box.show_all()
        self.overview_bg_box.grab_focus()

        # In switcher mode, update previews periodically
        if self.mode == 'switcher':
            self.update_timer_id = GLib.timeout_add(250, self._update_switcher_previews)

    def hide(self):
        if not self.active:
            return
        self.active = False
        if self.update_timer_id:
            GLib.source_remove(self.update_timer_id)
            self.update_timer_id = None
        if self.overview_bg_box:
            try:
                self.overlay.remove(self.overview_bg_box)
            except Exception:
                pass
            self.overview_bg_box.destroy()
            self.overview_bg_box = None
        self.overview_box = None
        self.overview_tiles = []
        # Return focus to the current terminal after hiding
        try:
            current_page = self.notebook.get_current_page()
            if current_page is not None and current_page >= 0:
                terminal = self.notebook.get_nth_page(current_page)
                if terminal:
                    GLib.idle_add(lambda: (terminal.grab_focus() or True))
        except Exception:
            pass

    def _update_switcher_previews(self):
        if not self.active or self.mode != 'switcher':
            return False
        self.update_all_snapshots()
        return True

    def build_overview_tiles(self):
        # Clear
        for child in list(self.overview_box.get_children()):
            self.overview_box.remove(child)

        n_pages = self.notebook.get_n_pages()
        if n_pages == 0:
            return

        cols = max(1, math.ceil(math.sqrt(n_pages * 1.5)))
        rows = math.ceil(n_pages / cols)

        grid = Gtk.Grid()
        grid.set_row_spacing(8)
        grid.set_column_spacing(8)
        grid.set_halign(Gtk.Align.CENTER)
        grid.set_valign(Gtk.Align.CENTER)
        self.overview_box.pack_start(grid, True, True, 0)

        self.snapshot_cache = {}
        self.update_all_snapshots()

        self.overview_tiles = []

        idx = 0
        for r in range(rows):
            for c in range(cols):
                if idx >= n_pages:
                    break
                tile = self._build_tile(idx)
                grid.attach(tile, c, r, 1, 1)
                self.overview_tiles.append(tile)
                idx += 1

        self.update_selection()

    def _calc_thumb_size(self):
        # Heuristic: cap width to a quarter of window width, maintain 4:3
        try:
            window_width = self.window.get_allocated_width()
        except Exception:
            window_width = 1200
        available = max(400, window_width - 100)
        max_thumb_w = self.config.get('tab_overview_thumb_width', 300)
        thumb_w = min(max_thumb_w, available // 4 - 20)
        thumb_h = int(thumb_w * 0.75)
        return max(100, thumb_w), max(75, thumb_h)

    def update_tile_image(self, tab_index):
        """Update the image of a specific tile using cached pixbuf."""
        if not self.active or not self.overview_tiles:
            return
        if tab_index < 0 or tab_index >= len(self.overview_tiles):
            return
        tab = self.notebook.get_nth_page(tab_index)
        if tab not in self.snapshot_cache:
            return
        tile_box = self.overview_tiles[tab_index]
        tile_vbox = tile_box.get_child()
        if not tile_vbox:
            return
        # Find overlay child
        preview_overlay = None
        for child in tile_vbox.get_children():
            if isinstance(child, Gtk.Overlay):
                preview_overlay = child
                break
        if not preview_overlay:
            return
        image = preview_overlay.get_child()
        if not isinstance(image, Gtk.Image):
            return
        pixbuf = self.snapshot_cache[tab]
        tw = getattr(self, 'thumb_width', 300)
        th = getattr(self, 'thumb_height', 225)
        scaled = pixbuf.scale_simple(tw, th, GdkPixbuf.InterpType.BILINEAR)
        image.set_from_pixbuf(scaled)

    def _build_tile(self, idx):
        tab = self.notebook.get_nth_page(idx)
        tile_box = Gtk.EventBox()

        tile_vbox = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
        tile_box.add(tile_vbox)

        # Header
        header_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=6)
        header_box.set_halign(Gtk.Align.CENTER)
        label = Gtk.Label()
        label.set_xalign(0.5)
        if tab in self.tabs:
            params = self.tabs[tab]
            tname = params.get("name", params.get("title", f"Tab {idx+1}"))
        else:
            tname = f"Tab {idx+1}"
        label.set_markup(f"<b>{html.escape(tname)}</b>")
        label.set_justify(Gtk.Justification.CENTER)
        header_box.pack_start(label, True, True, 0)
        tile_vbox.pack_start(header_box, False, False, 0)

        # Preview image
        preview_overlay = Gtk.Overlay()
        pixbuf = self.snapshot_cache.get(tab)
        thumb_w, thumb_h = self._calc_thumb_size()
        self.thumb_width, self.thumb_height = thumb_w, thumb_h
        if pixbuf:
            scaled = pixbuf.scale_simple(thumb_w, thumb_h, GdkPixbuf.InterpType.BILINEAR)
            image = Gtk.Image.new_from_pixbuf(scaled)
        else:
            placeholder = Gtk.Label(label="[preview]")
            placeholder.set_size_request(thumb_w, thumb_h)
            image = placeholder
        preview_overlay.add(image)
        tile_vbox.pack_start(preview_overlay, True, True, 0)

        tile_box.set_name("overview-tile")
        return tile_box

    def _calc_thumb_size(self):
        # Basic heuristic for thumbnail size
        return (300, 225)

    def update_all_snapshots(self):
        self.snapshot_cache.clear()
        for i in range(self.notebook.get_n_pages()):
            tab = self.notebook.get_nth_page(i)
            pixbuf = self.capture_terminal_snapshot(tab)
            if pixbuf:
                self.snapshot_cache[tab] = pixbuf

    def capture_terminal_snapshot(self, terminal):
        """Capture a snapshot of the terminal widget.
        Skips hidden/unmapped or tiny allocations to avoid blank frames.
        """
        try:
            if hasattr(terminal, 'get_mapped') and not terminal.get_mapped():
                return None
        except Exception:
            pass
        alloc = terminal.get_allocation()
        width = alloc.width
        height = alloc.height
        if width <= 1 or height <= 1:
            return None
        surface = cairo.ImageSurface(cairo.FORMAT_ARGB32, width, height)
        ctx = cairo.Context(surface)
        terminal.draw(ctx)
        pixbuf = Gdk.pixbuf_get_from_surface(surface, 0, 0, width, height)
        return pixbuf

    def update_snapshot_cache(self, force=False):
        """Update snapshot cache for current tab, throttled unless forced."""
        import time
        now = time.time()
        if not force and (now - self.cache_timestamp) < 0.5:
            return False
        self.cache_timestamp = now
        current_page = self.notebook.get_current_page()
        if current_page >= 0:
            tab = self.notebook.get_nth_page(current_page)
            pixbuf = self.capture_terminal_snapshot(tab)
            if pixbuf:
                self.snapshot_cache[tab] = pixbuf
        return False

    def get_cached_or_capture(self, terminal):
        """Return cached snapshot for terminal or capture and cache one."""
        if terminal in self.snapshot_cache:
            return self.snapshot_cache[terminal]
        pixbuf = self.capture_terminal_snapshot(terminal)
        if pixbuf:
            self.snapshot_cache[terminal] = pixbuf
        return pixbuf

    def invalidate_cache(self, terminal=None):
        """Invalidate snapshot cache for a specific terminal or all."""
        if terminal is None:
            self.snapshot_cache.clear()
        else:
            if terminal in self.snapshot_cache:
                del self.snapshot_cache[terminal]

    def update_selection(self, idx=None, preserve_column=False):
        n_pages = self.notebook.get_n_pages()
        if n_pages == 0:
            return
        if idx is None:
            idx = self.notebook.get_current_page()
        idx = max(0, min(idx, n_pages - 1))
        self.selected = idx
        if not preserve_column:
            cols = max(1, math.ceil(math.sqrt(n_pages * 1.5)))
            self.desired_column = self.selected % cols
        for i, tile in enumerate(self.overview_tiles):
            tile.set_name("overview-tile-selected" if i == self.selected else "overview-tile")

    def on_key_press(self, widget, event):
        if not self.active:
            return False
        handled = self.handle_key(event)
        if handled:
            return True
        current_page = self.notebook.get_current_page()
        if current_page >= 0:
            terminal = self.notebook.get_nth_page(current_page)
            if terminal:
                terminal.emit("key-press-event", eventkey2gdkevent(event))
                return True
        return False

    def on_key_release(self, widget, event):
        if not self.active:
            return False
        current_page = self.notebook.get_current_page()
        if current_page >= 0:
            terminal = self.notebook.get_nth_page(current_page)
            if terminal:
                terminal.emit("key-release-event", eventkey2gdkevent(event))
                return True
        return False

    def handle_key(self, event):
        if not self.active:
            return False
        keyval = event.keyval
        mods = event.state & Gtk.accelerator_get_default_mod_mask()
        n_pages = self.notebook.get_n_pages()
        cols = max(1, math.ceil(math.sqrt(n_pages * 1.5)))

        if self._check_keybind(keyval, mods, 'close_tab'):
            if 0 <= self.selected < n_pages:
                current_mode = self.mode
                self.notebook.remove_page(self.selected)
                new_n = self.notebook.get_n_pages()
                if new_n == 0:
                    self.hide()
                else:
                    if self.selected >= new_n:
                        self.selected = new_n - 1
                    self.hide(); self.show(mode=current_mode)
            return True

        if self._check_keybind(keyval, mods, 'new_tab'):
            current_mode = self.mode
            self.mypixels.new_tab()
            self.hide(); self.show(mode=current_mode)
            return True

        if self._check_keybind(keyval, mods, 'overview_exit'):
            self.hide(); return True

        if self._check_keybind(keyval, mods, 'overview_new_tab'):
            current_mode = self.mode
            self.mypixels.new_tab()
            self.hide(); self.show(mode=current_mode)
            return True

        if self._check_keybind(keyval, mods, 'overview_select'):
            self.notebook.set_current_page(self.selected)
            self.hide(); return True

        if self._check_keybind(keyval, mods, 'overview_move_left'):
            if self.selected > 0:
                tab = self.notebook.get_nth_page(self.selected)
                self.notebook.reorder_child(tab, self.selected - 1)
                self.selected -= 1
                self.hide(); self.show(mode='standalone')
            return True

        if self._check_keybind(keyval, mods, 'overview_nav_left'):
            if self.selected > 0:
                self.update_selection(self.selected - 1)
            return True

        if self._check_keybind(keyval, mods, 'overview_move_right'):
            if self.selected < n_pages - 1:
                tab = self.notebook.get_nth_page(self.selected)
                self.notebook.reorder_child(tab, self.selected + 1)
                self.selected += 1
                self.hide(); self.show(mode='standalone')
            return True

        if self._check_keybind(keyval, mods, 'overview_nav_right'):
            if self.selected < n_pages - 1:
                self.update_selection(self.selected + 1)
            return True

        if self._check_keybind(keyval, mods, 'overview_move_up'):
            if self.selected >= cols:
                tab = self.notebook.get_nth_page(self.selected)
                new_pos = self.selected - cols
                self.notebook.reorder_child(tab, new_pos)
                self.selected = new_pos
                self.hide(); self.show(mode='standalone')
            return True

        if self._check_keybind(keyval, mods, 'overview_nav_up'):
            current_row = self.selected // cols
            if current_row > 0:
                target = (current_row - 1) * cols + self.desired_column
                target = min(target, n_pages - 1)
                target = max(0, target)
                self.update_selection(target, preserve_column=True)
            return True

        if self._check_keybind(keyval, mods, 'overview_move_down'):
            if self.selected + cols < n_pages:
                tab = self.notebook.get_nth_page(self.selected)
                new_pos = self.selected + cols
                self.notebook.reorder_child(tab, new_pos)
                self.selected = new_pos
                self.hide(); self.show(mode='standalone')
            return True

        if self._check_keybind(keyval, mods, 'overview_nav_down'):
            current_row = self.selected // cols
            target = (current_row + 1) * cols + self.desired_column
            if target < n_pages and target // cols > current_row:
                self.update_selection(target, preserve_column=True)
            return True

        if self._check_keybind(keyval, mods, 'overview_first'):
            self.update_selection(0); return True

        if self._check_keybind(keyval, mods, 'overview_last'):
            self.update_selection(n_pages - 1); return True

        return False


class TabOverviewFull(TabOverview):
    pass
