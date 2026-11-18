"""Compact egui tab strip: application menu, tabs, overflow and detach.

Run: xvfb-run -a python3 tests/tab_bar_smoke.py
"""
import os
from pathlib import Path
import re
import subprocess as sp
import tempfile
import time

from ui_geometry import control_center, control_rect

from PIL import Image

root = Path(__file__).resolve().parents[1]
assert os.environ.get('DISPLAY'), 'Run under xvfb-run'
work = Path(tempfile.mkdtemp(prefix='mypixels-tab-bar-'))
config = work / 'config.yml'
config.write_text('''wal_palette: ''
term_font: Cascadia Code 12
tablist_font: Terminus (TTF) 12
default_width: 1000
default_height: 600
default_maximize: false
hide_single_tab: false
term_opacity: 0
background_color: '#0b1216'
bg_tint_color: '#0b1216'
foreground_color: '#dce4e7'
accent_color: '#00dbe6'
workspace_color_1: '#00dbe6'
''')
fixture = work / 'fixture.py'
fixture.write_text(r'''
import os, sys, tty
from pathlib import Path
work = Path(sys.argv[1])
index = len(list(work.glob('input-*')))
path = work / f'input-{index}'
path.touch()
tty.setraw(0)
names = ['build', 'editor', 'tests', 'server', 'logs', 'git', 'python', 'ssh']
os.write(1, (f'\x1b]2;{names[index % 8]}\x07Tab {index}\r\n').encode())
while True:
    data = os.read(0, 65536)
    if not data: break
    with path.open('ab') as out: out.write(data)
''')
keys = work / 'keys'
os.mkfifo(keys)
env = dict(os.environ, HOME=str(work), XDG_CONFIG_HOME=str(work / '.config'),
           MYPIXELS_CONFIG=str(config), MYPIXELS_KEYS=str(keys), RUST_LOG='info', MYPIXELS_UI_GEOMETRY='1')
env.pop('WAYLAND_DISPLAY', None)
binary = os.environ.get('MYPIXELS_BIN', str(root / 'target/debug/mypixels'))
log_path = work / 'app.log'
log = log_path.open('w')
app = sp.Popen([binary, '-s', 'python3', str(fixture), str(work)], env=env, stdout=log, stderr=log)


def run(*args):
    return sp.run(args, env=env, check=True, stdout=sp.PIPE, stderr=sp.PIPE, timeout=5).stdout


def wait(check, message):
    deadline = time.monotonic() + 12
    while time.monotonic() < deadline:
        result = check()
        if result: return result
        assert app.poll() is None, log_path.read_text()
        time.sleep(.08)
    raise AssertionError(message + '\n' + log_path.read_text())


def key(value):
    run('xdotool', 'key', value)
    time.sleep(.15)


def click(x, y, button=1):
    run('xdotool', 'mousemove', '--window', wid, str(round(x)), str(round(y)), 'click', str(button))
    time.sleep(.25)


def spy(window=1):
    count = log_path.read_text().count('SPY active=')
    with keys.open('w') as out: out.write(f'window:{window}:spy\n')
    wait(lambda: log_path.read_text().count('SPY active=') > count, 'spy response missing')
    line = [line for line in log_path.read_text().splitlines() if 'SPY active=' in line][-1]
    ids = list(map(int, re.findall(r'\d+\[:(\d+)\]', line)))
    active = int(re.search(r'SPY active=(\d+)', line).group(1))
    return ids, active


def shot(name):
    time.sleep(.3)
    path = work / f'{name}.png'
    run('import', '-window', wid, str(path))
    return Image.open(path).convert('RGB')


def selected():
    run('xdotool', 'mousemove', '--window', wid, '5', '200')
    def bounds():
        image = shot('tab-bar')
        points = [(x, y) for y in range(round(strip)) for x in range(image.width)
                  if (lambda p: p[0] < 90 and p[1] > 180 and p[2] > 180)(image.getpixel((x, y)))]
        if not points: return None
        return min(x for x, y in points), max(x for x, y in points), min(y for x, y in points), max(y for x, y in points)
    return wait(bounds, 'selected tab outline missing')


try:
    def windows():
        return sp.run(['xdotool', 'search', '--onlyvisible', '--pid', str(app.pid)], env=env, stdout=sp.PIPE).stdout.decode().splitlines()
    wid = wait(windows, 'window missing')[-1]
    wait(lambda: (work / 'input-0').exists(), 'PTY missing')
    run('xdotool', 'windowfocus', '--sync', wid)
    time.sleep(.3)
    strip = float(re.search(r'strip = ([0-9.]+)px', log_path.read_text()).group(1))
    assert strip <= 32, f'tab strip grew too tall: {strip}'
    terminal = shot('before-menu')
    menu_probe = (300, round(strip + 22))
    click(*control_center(log_path, 'tab_bar/menu'))
    assert shot('application-menu').getpixel(menu_probe) != terminal.getpixel(menu_probe), 'application menu missing'
    key('x')
    key('Escape')
    assert (work / 'input-0').read_bytes() == b'', 'menu keys leaked into the terminal'
    assert shot('menu-escaped').getpixel(menu_probe) == terminal.getpixel(menu_probe), 'Escape did not dismiss the menu'
    click(*control_center(log_path, 'tab_bar/menu'))
    click(*control_center(log_path, 'tab_bar/menu'))
    assert shot('menu-toggled').getpixel(menu_probe) == terminal.getpixel(menu_probe), 'second click did not close the menu'
    # Moving and right-clicking within the dismissed popup's former bounds
    # must reach the terminal controller even before egui drops its old layer.
    click(100, 80, 3)
    probe = (120, 100)
    assert shot('context-after-app-menu').getpixel(probe) != terminal.getpixel(probe), 'stale app menu swallowed right-click'
    key('Escape')
    assert (work / 'input-0').read_bytes() == b'', 'context-menu Escape leaked into the terminal'
    click(*control_center(log_path, 'tab_bar/menu'))
    click(350, 200)
    assert shot('menu-click-away').getpixel(menu_probe) == terminal.getpixel(menu_probe), 'click-away did not dismiss the menu'
    for action, name in [('show_shortcuts', 'keyboard-shortcuts'), ('show_about', 'about-mypixels')]:
        click(*control_center(log_path, 'tab_bar/menu'))
        click(*control_center(log_path, f'menu/{action}'))
        info = shot(name)
        center = (info.width // 2, info.height // 2)
        assert info.getpixel(center) != terminal.getpixel(center), f'{name} did not open'
        key('x')
        key('Escape')
        assert (work / 'input-0').read_bytes() == b'', f'{name} keys leaked into the terminal'
        assert shot(name + '-closed').getpixel(center) == terminal.getpixel(center), f'{name} did not close'
    click(*control_center(log_path, 'tab_bar/menu'))
    click(*control_center(log_path, 'menu/show_config'))
    probe = (terminal.width - 8, terminal.height // 2)
    assert shot('menu-settings').getpixel(probe) != terminal.getpixel(probe), 'menu did not open settings'
    key('alt+shift+c')
    left, right, top, bottom = selected()
    click(*control_center(log_path, 'tab_bar/add'))
    wait(lambda: len(spy()[0]) == 2, 'add button did not create a tab')
    click((left + right) / 2, strip / 2)
    wait(lambda: spy()[1] == 0, 'click did not select the first tab')
    key('x')
    wait(lambda: (work / 'input-0').read_bytes() == b'x', 'typing did not reach the selected terminal')
    # Close cancels when released outside its target.
    left, right, top, bottom = selected()
    close_x, close_y = control_center(log_path, 'tab_bar/close/1')
    run('xdotool', 'mousemove', '--window', wid, str(round(close_x)), str(round(close_y)), 'mousedown', '1')
    run('xdotool', 'mousemove', '--window', wid, '100', '100', 'mouseup', '1')
    assert len(spy()[0]) == 2, 'cancelled close still removed a tab'
    key('alt+2')
    left, right, top, bottom = selected()
    click(*control_center(log_path, 'tab_bar/close/2'))
    wait(lambda: len(spy()[0]) == 1, 'close button did not remove the tab')
    for total in range(2, 5):
        key('ctrl+shift+t')
        wait(lambda: len(spy()[0]) == total, 'new-tab shortcut failed')
    # Locate the third tab from its own selection outline. Its x position
    # changes with workspace chrome, captions, density, and the selected font.
    key('alt+3')
    target_left, target_right, _, _ = selected()
    key('alt+1')
    before, _ = spy()
    left, right, top, bottom = selected()
    run('xdotool', 'mousemove', '--window', wid, str(round((left + right) / 2)), str(round(strip / 2)), 'mousedown', '1')
    run('xdotool', 'mousemove', '--window', wid, str(round((target_left + target_right) / 2)), str(round(strip / 2)))
    time.sleep(.25)
    run('xdotool', 'mouseup', '1')
    wait(lambda: spy()[0][2] == before[0], 'drag did not reorder the tab')
    shot('reordered')
    for total in range(5, 21):
        key('ctrl+shift+t')
        wait(lambda: len(spy()[0]) == total, 'overflow tabs missing')
    left, right, top, bottom = selected()
    width = shot('overflow').width
    add = control_rect(log_path, 'tab_bar/add')
    assert right < add[0] and add[2] <= width and right - left >= 90, 'active overflow tab or add button was clipped'
    key('alt+1')
    left, right, top, bottom = selected()
    assert left > 0 and right - left >= 90, 'first tab was not scrolled into view'
    # Middle-click close preserves the neighboring active tab.
    count = len(spy()[0])
    click((left + right) / 2, strip / 2, 2)
    wait(lambda: len(spy()[0]) == count - 1, 'middle-click close failed')
    # Native out-of-window drops must move the actual terminal, not leave an
    # empty destination window after make_window changes the selected window.
    before_detach, before_active = spy()
    detached_id = before_detach[before_active]
    left, right, top, bottom = selected()
    run('xdotool', 'mousemove', '--window', wid, str(round((left + right) / 2)), str(round(strip / 2)), 'mousedown', '1')
    run('xdotool', 'mousemove', '--window', wid, '1100', '650')
    time.sleep(.2)
    run('xdotool', 'mouseup', '1')
    wait(lambda: len(windows()) == 2, 'dragging off the window did not detach the tab')
    wait(lambda: len(spy(1)[0]) == len(before_detach) - 1, 'detached terminal stayed in the source window')
    wait(lambda: spy(2) == ([detached_id], 0), 'new window did not receive the detached terminal')
    run('xdotool', 'windowfocus', '--sync', wid)
    # An unmanaged X server does not raise a focused window. The detached
    # window can otherwise intercept clicks intended for the original one.
    run('xdotool', 'windowraise', wid)
    click(*control_center(log_path, 'tab_bar/menu'))
    click(*control_center(log_path, 'menu/new_window'))
    wait(lambda: len(windows()) == 3, 'menu did not create a new window')
    print('PASS: application menu, settings, shortcuts/about, dismissal and keyboard ownership; compact tabs, add/select/close/cancel, reorder, 20-tab overflow, middle close, native detach, new window; artifacts:', work, flush=True)
finally:
    app.terminate()
    try: app.wait(timeout=3)
    except sp.TimeoutExpired:
        app.kill()
        app.wait()
    log.close()
