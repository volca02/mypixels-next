import gi
gi.require_version('GdkPixbuf', '2.0')
from gi.repository import Gtk, Gdk, GdkPixbuf, GLib
import cairo


class TabMonitor(Gtk.Window):
    """Floating window showing live preview of a terminal tab"""

    def __init__(self, source_terminal, source_tab_name, config, app):
        super().__init__(title=f"Monitor: {source_tab_name}")

        self.source_terminal = source_terminal
        self.config = config
        self.update_interval = 200  # Update every 200ms

        # Setup window properties for floating in niri/Wayland compositors
        self.set_default_size(800, 600)  # Larger default size for better readability
        self.set_decorated(True)
        self.set_type_hint(Gdk.WindowTypeHint.DIALOG)  # DIALOG is better recognized as floating
        self.set_resizable(True)
        self.set_role("mypixels-monitor")  # Set role for compositor matching

        # Try to set startup notification ID to influence app_id on Wayland
        self.set_startup_id("mypixels-monitor")

        # Display area for terminal snapshot
        # Use DrawingArea instead of Image for better resize control
        self.drawing_area = Gtk.DrawingArea()
        self.drawing_area.connect('draw', self.on_draw)
        self.add(self.drawing_area)

        # Store the current pixbuf
        self.current_pixbuf = None

        # Start update loop
        self.timeout_id = GLib.timeout_add(self.update_interval, self.update_snapshot)

        self.connect('delete-event', self.on_close)
        self.show_all()

        # Force an initial snapshot so we have a frame before any tab switch
        try:
            self.update_snapshot()
        except Exception:
            pass

        # Grab focus to prevent Alt-hold issues with tab switcher
        self.present()

    def capture_terminal_snapshot(self, terminal):
        """Capture a snapshot of the terminal widget"""
        # If the terminal tab is hidden (not mapped) or not the current
        # page in its notebook, Gtk won't render it. Avoid capturing a
        # blank frame which would overwrite the last good snapshot.
        try:
            if hasattr(terminal, 'get_mapped') and not terminal.get_mapped():
                return None
            parent = terminal.get_parent()
            if isinstance(parent, Gtk.Notebook):
                try:
                    if parent.get_current_page() != parent.page_num(terminal):
                        return None
                except Exception:
                    pass
        except Exception:
            pass

        alloc = terminal.get_allocation()
        width = alloc.width
        height = alloc.height

        # Hidden notebook pages often report 0x0 or 1x1 allocations; skip those.
        if width <= 1 or height <= 1:
            return None

        surface = cairo.ImageSurface(cairo.FORMAT_ARGB32, width, height)
        context = cairo.Context(surface)
        terminal.draw(context)
        pixbuf = Gdk.pixbuf_get_from_surface(surface, 0, 0, width, height)

        return pixbuf

    def on_draw(self, widget, context):
        """Draw the scaled terminal snapshot"""
        if self.current_pixbuf is None:
            return False

        # Get current drawing area size
        alloc = widget.get_allocation()
        width = alloc.width
        height = alloc.height

        if width <= 0 or height <= 0:
            return False

        # Scale pixbuf to fit
        scaled = self.current_pixbuf.scale_simple(
            width,
            height,
            GdkPixbuf.InterpType.BILINEAR
        )

        # Draw the scaled pixbuf
        Gdk.cairo_set_source_pixbuf(context, scaled, 0, 0)
        context.paint()

        return False

    def update_snapshot(self):
        """Capture and display current terminal state"""
        # Check if source terminal still exists
        if not self.source_terminal.get_parent():  # Tab closed
            self.destroy()
            return False

        pixbuf = self.capture_terminal_snapshot(self.source_terminal)
        if pixbuf:
            self.current_pixbuf = pixbuf
            self.drawing_area.queue_draw()

        return True  # Continue updates

    def on_close(self, widget, event):
        """Clean up when window is closed"""
        if self.timeout_id:
            GLib.source_remove(self.timeout_id)
            self.timeout_id = None
        return False
