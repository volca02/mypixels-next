"""Workspace cards, search, parked previews and exact-tab clicks.

Run under xvfb-run -a. Uses an isolated HOME, FIFO and terminal children.
"""
import os
from pathlib import Path
import re
import subprocess as sp
import tempfile
import time

from ui_geometry import control_rect, control_center

from PIL import Image, ImageChops

root = Path(__file__).resolve().parents[1]
work = Path(tempfile.mkdtemp(prefix='mypixels-workspace-panel-'))
config = work / 'config.yml'
config.write_text('''wal_palette: ''
default_width: 1500
default_height: 900
default_maximize: false
hide_single_tab: false
term_font: DejaVu Sans Mono 12
background_color: '#0b1216'
foreground_color: '#dce4e7'
workspaces_enabled: true
tab_overview_on_alt: false
workspace_genre_1: Development
workspace_genre_2: Research
workspace_genre_3: Operations
bindings:
  show_workspaces: MOD1+SHIFT+k
''')
fixture = work / 'fixture.py'
fixture.write_text(r'''
import os, sys, tty, threading, time
from pathlib import Path
work = Path(sys.argv[1])
index = len(list(work.glob('input-*')))
path = work / f'input-{index}'
path.touch()
tty.setraw(0)
os.write(1, (f'\x1b]2;Terminal {index}\x07\x1b[?25l'
    f'\x1b[36m$ workspace terminal {index}\x1b[0m\r\n\r\n'
    '\x1b[32m✓ service running\x1b[0m\r\n'
    '  Listening on localhost:5173\r\n\r\n'
    '\x1b[33mBuild output\x1b[0m\r\n'
    '  Compiling src/main.rs\r\n'
    '\x1b[32m✓ 1243 modules transformed\x1b[0m\r\n').encode())
def update():
    while not (work / f'update-{index}').exists(): time.sleep(.05)
    os.write(1, b'\x1b[10;1H\x1b[45m  Background output changed successfully  \x1b[0m\r\n')
threading.Thread(target=update, daemon=True).start()
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
    return sp.run(args, env=env, check=True, stdout=sp.PIPE, stderr=sp.PIPE, timeout=10).stdout


def wait(check, message):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        value = check()
        if value: return value
        assert app.poll() is None, log_path.read_text()
        time.sleep(.08)
    raise AssertionError(message + '\n' + log_path.read_text())


def send(spec):
    with keys.open('w') as out: out.write(f'window:1:{spec}\n')
    time.sleep(.16)


def spy():
    count = log_path.read_text().count('SPY active=')
    send('spy')
    wait(lambda: log_path.read_text().count('SPY active=') > count, 'spy missing')
    line = [line for line in log_path.read_text().splitlines() if 'SPY active=' in line][-1]
    return (int(re.search(r'workspace=(\d+)', line)[1]),
            int(re.search(r'SPY active=(\d+)', line)[1]),
            re.search(r'panel=(\w+)', line)[1], re.search(r'sel=(\S+)', line)[1])


def click(x, y):
    run('xdotool', 'mousemove', '--window', wid, str(round(x)), str(round(y)), 'click', '1')
    time.sleep(.3)


def shot(name):
    time.sleep(.5)
    path = work / f'{name}.png'
    run('import', '-window', wid, str(path))
    return Image.open(path).convert('RGB')


try:
    wid = wait(lambda: sp.run(['xdotool', 'search', '--onlyvisible', '--pid', str(app.pid)], env=env, stdout=sp.PIPE).stdout.decode().splitlines(), 'window missing')[-1]
    run('xdotool', 'windowfocus', '--sync', wid)
    wait(lambda: (work / 'input-0').exists(), 'PTY missing')
    for index in range(1, 3):
        send('ctrl+shift+t')
        wait(lambda: (work / f'input-{index}').exists(), 'new tab missing')
    for workspace in range(2, 7):
        send(f'alt+shift+{workspace}')
        wait(lambda: spy()[0] == workspace - 1, 'workspace switch failed')
        if workspace in (2, 3): send('ctrl+shift+t')
    send('alt+shift+1')
    send('alt+shift+k')
    wait(lambda: spy()[2:] == ('Workspaces', '0'), 'panel missing')
    time.sleep(2)
    image = shot('workspace-cards')
    width, height = image.size
    # Two Down presses expand workspace 3 without changing the live terminal.
    send('down')
    send('down')
    wait(lambda: spy()[3] == '2', 'keyboard selection failed')
    time.sleep(1)
    parked = shot('parked-workspace-expanded')
    well = tuple(round(value) for value in control_rect(log_path, 'workspace/3/tab/1'))
    assert any(high > 90 for low, high in parked.crop(well).getextrema()), 'parked preview has no terminal content'
    # A hidden PTY changing must update its preview without visiting it.
    (work / 'update-5').touch()
    wait(lambda: ImageChops.difference(parked.crop(well), shot('updated-parked-preview').crop(well)).getbbox(),
         'parked terminal preview did not update')
    # Expanded previews are separate buttons, below the card header.
    click(*control_center(log_path, 'workspace/3/tab/2'))
    wait(lambda: spy()[:2] == (2, 1), 'preview did not select the exact parked terminal')
    # Search uses real keyboard events so editor focus and routing are checked.
    click(*control_center(log_path, 'Search workspaces'))
    run('xdotool', 'type', '--clearmodifiers', 'Research')
    wait(lambda: spy()[3] == '1', 'search did not filter by genre')
    shot('genre-search')
    send('return')
    wait(lambda: spy()[0] == 1, 'search Enter did not switch workspace')
    send('escape')
    assert spy()[2] == 'Workspaces', 'first Escape should close search'
    send('escape')
    wait(lambda: spy()[2] == 'Closed', 'second Escape did not close panel')
    assert all(not path.read_bytes() for path in work.glob('input-*')), 'panel input leaked into a terminal'
    # Header + enters the first empty workspace, creating its initial terminal.
    send('alt+shift+k')
    click(*control_center(log_path, 'Open an empty workspace'))
    wait(lambda: spy()[0] == 6, 'add did not open the next empty workspace')
    shot('new-workspace')
    run('xdotool', 'windowsize', '--sync', wid, '480', '320')
    shot('small-workspace-panel')
    # Both expanded and compact rows must reveal a focused tab beyond the first
    # page; buttons and keys focus neighboring tabs while the view follows.
    send('escape')
    run('xdotool', 'windowsize', '--sync', wid, '1200', '800')
    for workspace in (1, 2):
        send(f'alt+shift+{workspace}')
        send('alt+9')
        wait(lambda: spy()[:2] == (workspace - 1, 8), 'ninth terminal did not become focused')
    send('alt+shift+1')
    send('alt+shift+k')
    wait(lambda: spy() == (0, 8, 'Workspaces', '0'), 'panel did not restore focused terminal')
    time.sleep(2)
    image = shot('focused-last-tabs')
    width = image.width
    click(*control_center(log_path, 'workspace/1/previous'))
    wait(lambda: spy() == (0, 7, 'Workspaces', '0'), 'left arrow did not focus the previous terminal')
    shot('workspace-tabs-focused-left')
    # Keys move relative to the actual focused tab and cross the viewport edge.
    send('left')
    send('left')
    wait(lambda: spy()[:2] == (0, 5), 'Left keys did not focus neighboring terminals')
    shot('workspace-tabs-recentered')
    send('right')
    wait(lambda: spy()[:2] == (0, 6), 'Right key did not focus the next terminal')
    shot('workspace-tabs-focused-right')
    print('Workspace panel checks passed. Artifacts:', work)
finally:
    app.terminate()
    try: app.wait(timeout=5)
    except sp.TimeoutExpired:
        app.kill()
        app.wait()
    log.close()
