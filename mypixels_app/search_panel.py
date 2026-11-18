import gi
from gi.repository import Gtk, Gdk, GLib, Pango, Vte
import html
import re


class SearchPanel:
    """Search panel for finding text across all tabs"""

    def __init__(self, parent_window, overlay, notebook, tabs_dict, config, mypixels_instance):
        self.window = parent_window
        self.overlay = overlay
        self.notebook = notebook
        self.tabs = tabs_dict
        self.config = config
        self.mypixels = mypixels_instance

        self.active = False
        self.search_text = ""
        self.case_sensitive = False

        # UI components
        self.main_box = None
        self.search_entry = None
        self.results_tree = None
        self.tree_model = None
        self.status_label = None

        # Search results cache
        self.current_results = []  # List of (tab, line_num, line_text, match_start, match_end)

    def show(self):
        """Show the search panel"""
        if self.active:
            return

        self.active = True

        # Create main container
        main_vbox = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
        main_vbox.set_size_request(-1, 400)  # Minimum height
        main_vbox.set_can_focus(False)  # Prevent from stealing focus

        # Search box at top
        search_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=10)
        search_box.set_can_focus(False)  # Prevent from stealing focus
        search_box.set_margin_start(10)
        search_box.set_margin_end(10)
        search_box.set_margin_top(10)
        search_box.set_margin_bottom(5)

        search_label = Gtk.Label(label="Search:")
        search_box.pack_start(search_label, False, False, 0)

        self.search_entry = Gtk.Entry()
        self.search_entry.set_placeholder_text("Enter search term...")
        self.search_entry.connect("changed", self.on_search_changed)
        self.search_entry.connect("key-press-event", self.on_search_key_press)
        search_box.pack_start(self.search_entry, True, True, 0)

        # Case sensitive checkbox
        case_check = Gtk.CheckButton(label="Case sensitive")
        case_check.connect("toggled", self.on_case_toggled)
        search_box.pack_start(case_check, False, False, 0)

        # Close button
        close_btn = Gtk.Button(label="✕")
        close_btn.connect("clicked", lambda w: self.hide())
        search_box.pack_start(close_btn, False, False, 0)

        main_vbox.pack_start(search_box, False, False, 0)

        # Status label
        self.status_label = Gtk.Label()
        self.status_label.set_markup('<i>Enter search term</i>')
        self.status_label.set_halign(Gtk.Align.START)
        self.status_label.set_margin_start(10)
        self.status_label.set_margin_bottom(5)
        main_vbox.pack_start(self.status_label, False, False, 0)

        # Scrolled window for results
        scrolled = Gtk.ScrolledWindow()
        scrolled.set_policy(Gtk.PolicyType.AUTOMATIC, Gtk.PolicyType.AUTOMATIC)
        scrolled.set_vexpand(True)

        # TreeView for results
        # Columns: display_text, tab_widget (hidden), line_num, is_header (hidden), match_index (hidden)
        self.tree_model = Gtk.TreeStore(str, object, int, bool, int)
        self.results_tree = Gtk.TreeView(model=self.tree_model)
        self.results_tree.set_headers_visible(False)

        # Single text column
        renderer = Gtk.CellRendererText()
        renderer.set_property("font", "monospace")
        column = Gtk.TreeViewColumn("Results", renderer, text=0)
        self.results_tree.append_column(column)

        # Connect selection signals
        self.results_tree.connect("row-activated", self.on_result_activated)
        self.results_tree.connect("key-press-event", self.on_tree_key_press)
        # Also handle single click
        self.results_tree.connect("cursor-changed", self.on_result_selected)

        scrolled.add(self.results_tree)
        main_vbox.pack_start(scrolled, True, True, 0)

        # Event box for background
        event_box = Gtk.EventBox()
        event_box.set_name("search-panel-background")
        event_box.set_can_focus(False)  # Prevent event box from stealing focus
        event_box.add(main_vbox)

        # Apply CSS styling
        css = b"""
        #search-panel-background {
            background-color: rgba(30, 30, 30, 0.95);
            border-top: 2px solid #4a9a9a;
        }
        """
        style_provider = Gtk.CssProvider()
        style_provider.load_from_data(css)
        Gtk.StyleContext.add_provider_for_screen(
            Gdk.Screen.get_default(),
            style_provider,
            Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
        )

        self.main_box = event_box
        self.main_box.set_valign(Gtk.Align.END)
        self.main_box.set_vexpand(False)

        self.overlay.add_overlay(self.main_box)
        self.main_box.show_all()

        # Focus search entry - use single delayed attempt
        def grab_focus_delayed():
            if self.active and self.search_entry:
                self.search_entry.grab_focus()
            return False  # Don't repeat

        GLib.timeout_add(100, grab_focus_delayed)

    def hide(self):
        """Hide the search panel"""
        if not self.active:
            return

        self.active = False

        if self.main_box:
            self.overlay.remove(self.main_box)
            self.main_box.destroy()
            self.main_box = None

        # Clear references
        self.search_entry = None
        self.results_tree = None
        self.tree_model = None
        self.status_label = None
        self.current_results = []

        # Return focus to current tab
        current_tab = self.mypixels.get_current_tab()
        if current_tab:
            current_tab.grab_focus()

    def on_case_toggled(self, widget):
        """Handle case sensitivity toggle"""
        self.case_sensitive = widget.get_active()
        # Re-run search with new setting
        if self.search_text:
            self.perform_search(self.search_text)

    def on_search_changed(self, widget):
        """Handle search text changes"""
        text = widget.get_text()
        self.search_text = text

        if len(text) < 2:
            self.tree_model.clear()
            if len(text) == 0:
                self.status_label.set_markup('<i>Enter search term</i>')
            else:
                self.status_label.set_markup('<i>Type at least 2 characters</i>')
            return

        self.perform_search(text)

    def on_search_key_press(self, widget, event):
        """Handle key presses in search entry"""
        if event.keyval == Gdk.KEY_Escape:
            self.hide()
            return True
        elif event.keyval == Gdk.KEY_Down:
            # Move focus to results tree
            if self.results_tree:
                self.results_tree.grab_focus()
            return True
        return False

    def on_tree_key_press(self, widget, event):
        """Handle key presses in tree view"""
        if event.keyval == Gdk.KEY_Escape:
            self.hide()
            return True
        elif event.keyval == Gdk.KEY_Return:
            # Activate selected row
            selection = self.results_tree.get_selection()
            model, tree_iter = selection.get_selected()
            if tree_iter:
                path = model.get_path(tree_iter)
                self.on_result_activated(self.results_tree, path, None)
            return True
        return False

    def perform_search(self, search_text):
        """Search across all tabs"""
        self.tree_model.clear()
        self.current_results = []

        if not search_text:
            return

        try:
            if self.case_sensitive:
                pattern = re.compile(search_text)
            else:
                pattern = re.compile(search_text, re.IGNORECASE)
        except re.error as e:
            self.status_label.set_markup(f'<span foreground="red">Invalid pattern: {html.escape(str(e))}</span>')
            return

        total_matches = 0
        tabs_with_matches = 0
        global_match_index = 0

        # Search each tab
        for i in range(self.notebook.get_n_pages()):
            tab = self.notebook.get_nth_page(i)
            if not tab:
                continue

            # Get tab title
            tab_title = "Untitled"
            if tab in self.tabs:
                params = self.tabs[tab]
                tab_title = params.get("name", params.get("title", "Untitled"))

            # Extract text from terminal
            matches = self.search_terminal(tab, pattern)

            if matches:
                tabs_with_matches += 1
                match_count = len(matches)
                total_matches += match_count

                # Add tab header
                header_text = f"▸ Tab {i+1}: {tab_title} ({match_count} matches)"
                header_iter = self.tree_model.append(None, [header_text, tab, 0, True, -1])

                # Add matches as children
                for line_num, line_text, match_start, match_end in matches:
                    global_match_index += 1
                    # Create preview with context
                    preview = self.create_match_preview(line_text, match_start, match_end)
                    match_text = f"    line {line_num}: {preview}"
                    self.tree_model.append(header_iter, [match_text, tab, line_num, False, global_match_index])
                    self.current_results.append((tab, line_num, line_text, match_start, match_end))

        # Update status
        if total_matches == 0:
            self.status_label.set_markup(f'<i>No matches found for "{html.escape(search_text)}"</i>')
        else:
            self.status_label.set_markup(
                f'<b>{total_matches}</b> matches in <b>{tabs_with_matches}</b> tabs'
            )

        # Expand all by default
        self.results_tree.expand_all()

    def search_terminal(self, terminal, pattern):
        """Search text in a terminal
        Returns a list of (line_num, line_text, match_start, match_end)
        """
        matches = []
        try:
            full_text = terminal.get_text_format(Vte.Format.TEXT)
            if not full_text:
                return matches
            lines = full_text.split('\n')
            for line_num, line in enumerate(lines, 1):
                for match in pattern.finditer(line):
                    matches.append((line_num, line, match.start(), match.end()))
        except Exception as e:
            print(f"Error searching terminal: {e}")
        return matches

    def create_match_preview(self, line_text, match_start, match_end, context_chars=40):
        """Create a preview of the match with context"""
        stripped = line_text.strip()
        if not stripped:
            return line_text[:80]
        max_len = 100
        if len(stripped) > max_len:
            if match_start < len(stripped):
                start_pos = max(0, match_start - context_chars)
                end_pos = min(len(stripped), match_end + context_chars)
                preview = stripped[start_pos:end_pos]
                if start_pos > 0:
                    preview = "..." + preview
                if end_pos < len(stripped):
                    preview = preview + "..."
                return html.escape(preview)
            else:
                return html.escape(stripped[:max_len] + "...")
        else:
            return html.escape(stripped)

    def on_result_selected(self, tree_view):
        """Handle single click on result - highlight but don't close panel"""
        selection = tree_view.get_selection()
        model, tree_iter = selection.get_selected()
        if not tree_iter:
            return
        is_header = model.get_value(tree_iter, 3)
        if is_header:
            return
        self.highlight_match(tree_view, tree_iter, close_panel=False)

    def highlight_match(self, tree_view, tree_iter, close_panel=True):
        """Navigate to a match in the terminal"""
        model = tree_view.get_model()
        is_header = model.get_value(tree_iter, 3)
        tab_widget = model.get_value(tree_iter, 1)
        if is_header:
            return
        if tab_widget:
            for i in range(self.notebook.get_n_pages()):
                if self.notebook.get_nth_page(i) == tab_widget:
                    self.notebook.set_current_page(i)
                    if close_panel:
                        tab_widget.grab_focus()
                    else:
                        GLib.idle_add(lambda: self.search_entry.grab_focus() if self.active else False)
                    break
            if close_panel:
                self.hide()

    def on_result_activated(self, tree_view, path, column):
        """Handle double-click on result"""
        model = tree_view.get_model()
        tree_iter = model.get_iter(path)
        if not tree_iter:
            return
        is_header = model.get_value(tree_iter, 3)
        if is_header:
            if tree_view.row_expanded(path):
                tree_view.collapse_row(path)
            else:
                tree_view.expand_row(path, False)
            return
        self.highlight_match(tree_view, tree_iter, close_panel=True)
