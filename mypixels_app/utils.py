import gi
from gi.repository import Gdk


def eventkey2gdkevent(eventkey):
    """Cast Gdk.EventKey to a generic Gdk.Event for emission."""
    gdkevent = Gdk.Event(eventkey.type)
    gdkevent.key.type = eventkey.type
    gdkevent.key.window = eventkey.window
    gdkevent.key.send_event = eventkey.send_event
    gdkevent.key.time = eventkey.time
    gdkevent.key.state = eventkey.state
    gdkevent.key.keyval = eventkey.keyval
    gdkevent.key.length = eventkey.length
    gdkevent.key.string = eventkey.string
    gdkevent.key.hardware_keycode = eventkey.hardware_keycode
    gdkevent.key.group = eventkey.group
    gdkevent.key.is_modifier = eventkey.is_modifier
    return gdkevent


# Modifier mapping and key parsing
str2mod = {
    'mod1':  Gdk.ModifierType.MOD1_MASK,
    'alt':  Gdk.ModifierType.MOD1_MASK,
    'shift': Gdk.ModifierType.SHIFT_MASK,
    'control': Gdk.ModifierType.CONTROL_MASK,
    'ctrl': Gdk.ModifierType.CONTROL_MASK,
}


def key_press_parse(bindstr):
    """Parse strings like 'MOD1+SHIFT+Left' into (mods, keyval)."""
    key = None
    mods = 0
    for kp in bindstr.split('+'):
        kl = kp.lower()
        if kl in str2mod:
            mods |= str2mod[kl]
        else:
            if key is None:
                key = Gdk.keyval_from_name(kp)
    return (mods, key)


def counter():
    """Simple incremental counter generator."""
    i = 1
    while True:
        yield i
        i += 1
