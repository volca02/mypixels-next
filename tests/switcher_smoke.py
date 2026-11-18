"""Real switcher interaction and visual artifacts with eight live terminals.

Run with dbus-run-session -- xvfb-run -a -s "-screen 0 3840x2160x24"
python3 tests/switcher_smoke.py. Uses an isolated HOME.
"""
import os
from pathlib import Path
import re
import subprocess as sp
import tempfile
import time

from ui_geometry import control_rect, control_center

from PIL import Image

root = Path(__file__).resolve().parents[1]
assert os.environ.get('DISPLAY'), 'Run under xvfb-run'
work = Path(tempfile.mkdtemp(prefix='mypixels-switcher-'))
config = work / 'config.yml'
config.write_text('''wal_palette: ''
term_font: Cascadia Code 12
tablist_font: Terminus (TTF) 12
default_width: 1500
default_height: 900
default_maximize: false
hide_single_tab: false
term_opacity: 0
bg_tint_color: '#0b1216'
background_color: '#0b1216'
foreground_color: '#dce4e7'
accent_color: '#00dbe6'
workspace_color_1: '#00dbe6'
workspace_color_2: '#e676dd'
workspaces_enabled: true
tab_overview_opacity: 0.65
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
commands = ['pnpm run build', 'nvim src/main.rs', 'pnpm test', 'pnpm dev', 'tail -f app.log', 'git status', 'python', 'ssh dev']
lines = [
    '> vite build\r\n\r\n\x1b[36mvite v5.4.2\x1b[0m building for production...\r\n\x1b[32m✓\x1b[0m 1,243 modules transformed.\r\n\x1b[32m✓\x1b[0m rendering chunks...\r\n\x1b[32m✓\x1b[0m built in 8.4s',
    '142  \x1b[31mexport function\x1b[0m renderTerm(\r\n143    ctx: RenderContext,\r\n144    grid: CellGrid,\r\n145  ) {\r\n146    \x1b[35mfor\x1b[0m (let y = 0; y < grid.h; y++) {\r\n147      const row = grid.rows[y];\r\n148      drawRow(ctx, row);\r\n149    }\r\n150  }',
    '\x1b[36mvitest v1.6.0\x1b[0m\r\n\x1b[32m✓\x1b[0m renderer/terminal.ts (18)\r\n\x1b[32m✓\x1b[0m core/grid.test.ts (9)\r\n\x1b[32m✓\x1b[0m ansi/parser.test.ts (6)\r\n\r\nTest Files  \x1b[32m4 passed\x1b[0m (4)\r\nDuration    1.32s',
    '\x1b[36mvite v5.4.2\x1b[0m ready in 312 ms\r\n\r\n  Local:   \x1b[36mhttp://localhost:5173/\x1b[0m\r\n  Network: \x1b[36mhttp://192.168.1.42:5173/\x1b[0m\r\n\r\n[vite] hmr update /src/ui/switcher.ts',
    '14:36:51  \x1b[36mINFO\x1b[0m session created\r\n14:36:53  \x1b[36mINFO\x1b[0m tab added #7 (python)\r\n14:36:59  \x1b[33mWARN\x1b[0m high render latency: 42ms\r\n14:37:01  \x1b[36mINFO\x1b[0m switcher opened (8 tabs)\r\n14:37:03  \x1b[36mINFO\x1b[0m preview updated (4.3ms)',
    "On branch main\r\nYour branch is up to date with 'origin/main'.\r\n\r\nChanges not staged for commit:\r\n  \x1b[31mmodified:  src/ui/switcher.ts\x1b[0m\r\n  \x1b[31mmodified:  src/renderer/grid.ts\x1b[0m\r\n\r\nno changes added to commit",
    'Python 3.12.2 on linux\r\nType "help", "copyright", "credits" or "license".\r\n>>> from mypixels import terminal\r\n>>> terminal.cells\r\n(240, 60)\r\n>>>',
    'top - 14:37:12 up 12 days, 4:21, 2 users\r\nTasks: 143 total, 1 running, 142 sleeping\r\n%Cpu(s): 4.8 us, 1.2 sy, 0.0 ni, 93.5 id\r\nMiB Mem: 31952.0 total, 8211.4 free\r\n\r\n  PID USER    %CPU %MEM COMMAND\r\n 5284 ubuntu  12.3  1.4 node\r\n 6149 volca    4.7  0.9 mypixels',
]
name = names[index % 8]
command = commands[index % 8]
os.write(1, (f'\x1b]2;{name}\x07\x1b[?25l\x1b]133;A\x07> \x1b]133;B\x07{command}\x1b]133;C\x07\r\n' + lines[index % 8] + '\r\n').encode())
if index in (0, 2, 5):
    os.write(1, b'\x1b]133;D;0\x07\x1b]133;A\x07> \x1b]133;B\x07')
while True:
    data = os.read(0, 65536)
    if not data: break
    with path.open('ab') as out: out.write(data)
''')
env = dict(os.environ, HOME=str(work), XDG_CONFIG_HOME=str(work / '.config'),
           MYPIXELS_CONFIG=str(config), RUST_LOG='info', MYPIXELS_UI_GEOMETRY='1')
env.pop('WAYLAND_DISPLAY', None)
env.pop('MYPIXELS_KEYS', None)
binary = os.environ.get('MYPIXELS_BIN', str(root / 'target/debug/mypixels'))
log_path = work / 'app.log'
log = log_path.open('w')
app = sp.Popen([binary, '-s', 'python3', str(fixture), str(work)], env=env, stdout=log, stderr=log)


def run(*args):
    return sp.run(args, env=env, check=True, stdout=sp.PIPE, stderr=sp.PIPE, timeout=5).stdout


def wait(check, message):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        value = check()
        if value: return value
        assert app.poll() is None, log_path.read_text()
        time.sleep(.08)
    raise AssertionError(message + '\n' + log_path.read_text())


def shot(name):
    time.sleep(.45)
    path = work / (name + '.png')
    run('import', '-window', wid, str(path))
    return Image.open(path).convert('RGB')


def key(value):
    run('xdotool', 'key', value)


def opened():
    run('xdotool', 'keydown', 'Alt_L')
    time.sleep(.9)


try:
    def windows():
        return sp.run(['xdotool', 'search', '--onlyvisible', '--pid', str(app.pid)],
                      env=env, stdout=sp.PIPE).stdout.decode().splitlines()
    wid = wait(windows, 'window missing')[-1]
    wait(lambda: (work / 'input-0').exists(), 'PTY missing')
    run('xdotool', 'windowfocus', '--sync', wid)
    time.sleep(.3)
    for count in range(2, 9):
        key('ctrl+shift+t')
        wait(lambda: (work / f'input-{count-1}').exists(), 'new tab missing')
    key('alt+1')
    time.sleep(.3)
    opened()
    run('xdotool', 'mousemove', '--window', wid, '10', '80')
    def painted(image, match):
        # The first Area pass only measures the surface. Wait for a painted
        # selection rather than assuming preview creation finishes in .9s.
        hit = any(match(image.getpixel((x, y)))
                  for y in range(100, image.height - 80, 2) for x in range(0, image.width, 2))
        return image if hit else None

    # workspace_colors[0], as configured above.
    def identity(p):
        return p[0] < 80 and p[1] > 180 and p[2] > 180

    # col[2], the accent the switcher uses when workspaces are off.
    def fallback(p):
        return p[0] > 170 and p[1] < 90 and p[2] < 90

    def switcher_visible(color):
        return lambda: painted(shot('eight-tabs'), color)

    image = wait(switcher_visible(identity), 'selected card outline missing')
    key('Tab')
    key('shift+Tab')
    key('Right')
    key('Return')
    run('xdotool', 'keyup', 'Alt_L')
    key('x')
    wait(lambda: (work / 'input-1').read_bytes() == b'x', 'Tab/Shift-Tab/Enter selected the wrong tab')
    assert (work / 'input-0').read_bytes() == b'', 'switcher keys leaked into PTY'
    opened()
    key('Right')
    key('Escape')
    run('xdotool', 'keyup', 'Alt_L')
    key('y')
    wait(lambda: (work / 'input-1').read_bytes() == b'xy', 'Escape did not restore the original tab')
    # The selected card's outline locates its hit target without depending on text metrics.
    opened()
    selected = shot('second-selected')
    click_x, click_y = control_center(log_path, 'switcher/tab/1')
    run('xdotool', 'mousemove', '--window', wid, str(round(click_x)), str(round(click_y)), 'click', '1')
    run('xdotool', 'keyup', 'Alt_L')
    key('z')
    wait(lambda: (work / 'input-0').read_bytes() == b'z', 'clicking a card did not activate it')
    # Opening over a focused history search must give navigation to the switcher.
    key('alt+shift+h')
    time.sleep(.4)
    run('xdotool', 'mousemove', '--window', wid, str(image.width - 64), '55', 'click', '1')
    time.sleep(.2)
    run('xdotool', 'type', 'search text')
    run('xdotool', 'windowsize', wid, '560', '480')
    opened()
    key('End')
    # Cached previews from the old size must not force the modal off-screen.
    panel_color = image.getpixel((image.width // 2, 35))
    def resized():
        small = shot('small-last-tab')
        return small.getpixel((small.width // 2, 35)) == panel_color
    wait(resized, 'switcher did not adapt to the smaller window')
    run('xdotool', 'keyup', 'Alt_L')
    key('q')
    wait(lambda: (work / 'input-7').read_bytes() == b'q', 'small-window navigation failed')
    # The left rail lists open workspaces and paints their parked terminals.
    # Selecting a miniature must switch to that exact tab while Alt stays held.
    run('xdotool', 'windowsize', wid, '1200', '800')
    key('alt+shift+2')
    wait(lambda: (work / 'input-8').exists(), 'second workspace missing')
    key('ctrl+shift+t')
    wait(lambda: (work / 'input-9').exists(), 'second workspace tab missing')
    key('alt+shift+1')
    opened()
    rail = shot('workspace-rail')
    well_rect = control_rect(log_path, 'workspace/2/tab/1')
    mini_x, mini_y = control_center(log_path, 'workspace/2/tab/1')
    well = rail.crop(tuple(round(value) for value in well_rect))
    assert any(high > 80 for low, high in well.getextrema()), 'parked workspace miniature has no content'
    run('xdotool', 'mousemove', '--window', wid, str(round(mini_x)), str(round(mini_y)), 'click', '1')
    time.sleep(.35)
    selected_workspace = shot('workspace-rail-selected')
    # The rail remains visible after a workspace choice (still held Alt).
    assert painted(selected_workspace, lambda p: p[0] > 210 and 100 < p[1] < 140 and p[2] > 200), \
        'workspace choice dismissed the switcher'
    run('xdotool', 'keyup', 'Alt_L')
    key('w')
    wait(lambda: (work / 'input-8').read_bytes() == b'w', 'workspace miniature selected the wrong terminal')
    assert not (work / 'input-9').read_bytes(), 'workspace miniature chose the workspace front tab instead'
    key('alt+shift+1')
    # Large native previews must still fit beside the workspace rail in two
    # columns. Locate the painted cards and verify vertical keyboard movement.
    config.write_text(config.read_text() + '\ntab_overview_scale: 0.16\n')
    key('ctrl+shift+r')
    run('xdotool', 'windowsize', wid, '3840', '2160')
    time.sleep(.4)
    key('alt+1')
    opened()
    shot('wide-two-columns')
    first = control_rect(log_path, 'switcher/tab/1')
    second = control_rect(log_path, 'switcher/tab/2')
    assert first[1] == second[1] and first[2] < second[0], (first, second)
    key('Down')
    run('xdotool', 'keyup', 'Alt_L')
    key('v')
    wait(lambda: (work / 'input-2').read_bytes() == b'v', 'Down did not move by two preview columns')
    # With workspaces off there is no workspace identity to draw, and the
    # switcher outlines the selected card with the UI accent instead. That is the
    # default configuration, so the fallback is checked rather than assumed.
    config.write_text(config.read_text().replace('workspaces_enabled: true', 'workspaces_enabled: false'))
    key('ctrl+shift+r')
    time.sleep(.4)
    run('xdotool', 'windowsize', wid, '1200', '800')
    time.sleep(.3)
    opened()
    wait(switcher_visible(fallback), 'the switcher lost its outline with workspaces off')
    run('xdotool', 'keyup', 'Alt_L')
    print('PASS: live previews, navigation, card clicks, small windows, workspace rail, exact parked-tab clicks and wide two-column layout; artifacts:', work, flush=True)
finally:
    app.terminate()
    try: app.wait(timeout=3)
    except sp.TimeoutExpired:
        app.kill()
        app.wait()
    log.close()
