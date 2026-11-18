#!/usr/bin/env python3
"""Windowless session, live window restoration, protocol replies and eviction.

Run: dbus-run-session -- xvfb-run -a python3 tests/daemon_smoke.py
Uses only a private display/bus and temporary files; never the desktop service.
"""
import json
import os
from pathlib import Path
import socket
import subprocess as sp
import tempfile
import time

from ui_geometry import control_center, control_rect
from PIL import Image, ImageChops

from x11_helpers import request_close


root = Path(__file__).resolve().parents[1]
binary = Path(os.environ.get('MYPIXELS_BIN', root / 'target/debug/mypixels')).resolve()
work = Path(tempfile.mkdtemp(prefix='mypixels-daemon-'))
config = work / 'config.yml'
config.write_text('''wal_palette: ''
default_width: 900
default_height: 600
default_maximize: false
hide_single_tab: false
workspaces_enabled: true
graveyard_limit: 1
bindings:
  close_window: CONTROL+SHIFT+w
''')
fixture = work / 'shell'
fixture.write_text('''#!/usr/bin/python3
import os, tty, threading, time, subprocess
from pathlib import Path
work = Path(os.environ['DAEMON_TEST_WORK'])
index = 0
while True:
    try:
        fd = os.open(work / f'pid-{index}', os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        os.write(fd, str(os.getpid()).encode())
        os.close(fd)
        break
    except FileExistsError:
        index += 1
tty.setraw(0)
(work / f'grid-{index}').write_text(str(os.get_terminal_size()))
os.write(1, f'\\x1b]2;terminal-{index}\\x07live terminal {index}\\r\\n'.encode())
def background():
    sent = False
    launched = False
    updated = False
    while True:
        (work / f'alive-{index}').touch()
        if (work / f'stop-{index}').exists(): os._exit(0)
        if (work / f'update-{index}').exists() and not updated:
            (work / f'grid-check-{index}').write_text(str(os.get_terminal_size()))
            os.write(1, b'\\x1b[3;1H\\x1b[45m  Closed window is still running  \\x1b[0m')
            updated = True
        if (work / f'query-{index}').exists() and not sent:
            os.write(1, b'\\x1b[6n\\x1b[16t\\x1b_Gi=123,a=q,t=d,f=24,s=1,v=1;AAAA\\x1b\\\\')
            sent = True
        if (work / f'launch-{index}').exists() and not launched:
            subprocess.run([os.environ['DAEMON_TEST_BINARY'], '--new-tab'], check=True)
            launched = True
        time.sleep(.05)
threading.Thread(target=background, daemon=True).start()
while True:
    data = os.read(0, 65536)
    if not data: break
    with (work / f'input-{index}').open('ab') as out: out.write(data)
''')
fixture.chmod(0o755)
keys = work / 'keys'
os.mkfifo(keys)
env = dict(os.environ, HOME=str(work), XDG_CONFIG_HOME=str(work / '.config'),
           MYPIXELS_CONFIG=str(config), MYPIXELS_KEYS=str(keys),
           MYPIXELS_BUS='com.github.volca.mypixels.daemon_test',
           MYPIXELS_MCP=str(work / 'mcp.sock'), SHELL=str(fixture),
           DAEMON_TEST_WORK=str(work), DAEMON_TEST_BINARY=str(binary), MYPIXELS_UI_GEOMETRY='1', RUST_LOG='warn,mypixels=info')
env.pop('WAYLAND_DISPLAY', None)
env.pop('MYPIXELS_WINDOW', None)
log_path = work / 'app.log'
log = log_path.open('w')
app = sp.Popen([str(binary), '--daemon'], env=env, stdout=log, stderr=log)


def wait(check, message, seconds=15):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        assert app.poll() is None, log_path.read_text()
        value = check()
        if value:
            return value
        time.sleep(.05)
    raise AssertionError(message + '\n' + log_path.read_text())


def launch(*args):
    result = sp.run([str(binary), *args], cwd=work, env=env,
                    capture_output=True, timeout=15)
    assert result.returncode == 0, result.stderr.decode()


def windows():
    # Also exercises MCP queries while there is no window to wake.
    with socket.socket(socket.AF_UNIX) as sock:
        sock.settimeout(5)
        sock.connect(env['MYPIXELS_MCP'])
        sock.sendall(json.dumps({'jsonrpc': '2.0', 'id': 1, 'method': 'tools/call',
                                'params': {'name': 'list_windows'}}).encode() + b'\n')
        reply = json.loads(sock.makefile('rb').readline())
    return json.loads(reply['result']['content'][0]['text'])[0]


def send(key):
    fd = os.open(keys, os.O_WRONLY | os.O_NONBLOCK)
    with os.fdopen(fd, 'w') as out:
        out.write(key + '\n')
    time.sleep(.15)
    if key.endswith('ctrl+shift+w'):
        prefix = key.rsplit(':', 1)[0] + ':' if key.startswith('window:') else ''
        x, y = control_center(log_path, 'close/primary')
        send(prefix + f'click:{x},{y}')


def heartbeat(index):
    path = work / f'alive-{index}'
    before = path.stat().st_mtime_ns
    time.sleep(.2)
    return path.stat().st_mtime_ns > before


def titles(window):
    return [tab['title'] for tab in window['tabs']]




try:
    wait(lambda: (work / 'mcp.sock').exists(), 'daemon did not register')
    assert windows() == [], 'daemon spawned a window at startup'
    assert not list(work.glob('pid-*')), 'daemon spawned a shell at startup'
    visible = sp.run(['xdotool', 'search', '--onlyvisible', '--pid', str(app.pid)],
                     env=env, capture_output=True)
    assert not visible.stdout.strip(), 'daemon mapped a native window'
    duplicate = sp.run([str(binary), '--daemon'], env=env, capture_output=True, timeout=15)
    assert duplicate.returncode == 1, 'second daemon silently ran without session ownership'

    # An empty daemon accepts --new-tab by creating its first window.
    launch('--new-tab')
    wait(lambda: (work / 'alive-0').exists(), 'first terminal missing')
    launch('--new-tab')
    wait(lambda: len(windows()[0]['tabs']) == 2, 'second tab missing')
    send('alt+shift+2')
    wait(lambda: (work / 'alive-2').exists(), 'workspace terminal missing')
    launch('--new-tab')
    wait(lambda: len(windows()[0]['tabs']) == 2, 'workspace second tab missing')
    send('alt+1')  # Remember the first tab within the selected workspace.
    before = windows()[0]
    assert before['active_workspace'] == 2 and before['active_tab'] == 1, before
    pids = sorted(p.read_text() for p in work.glob('pid-*'))

    # Exercise the actual WM close request, including the existing confirmation.
    wid = sp.check_output(['xdotool', 'search', '--onlyvisible', '--pid', str(app.pid)], env=env).splitlines()[-1]
    request_close(wid)
    time.sleep(.4)
    sp.run(['import', '-window', wid, str(work / 'close-confirmation.png')], env=env, check=True)
    x, y = control_center(log_path, 'close/primary')
    send(f'click:{x},{y}')
    wait(lambda: not windows(), 'last window did not close')
    for index in range(4):
        assert heartbeat(index), f'closed window killed terminal {index}'

    # Buried applications still get terminal/graphics protocol replies.
    (work / 'query-2').touch()
    wait(lambda: (work / 'input-2').exists() and b'\x1b[6;' in (work / 'input-2').read_bytes()
         and b'i=123;OK' in (work / 'input-2').read_bytes(), 'windowless protocol replies missing')

    launch('--restore-window')
    wait(lambda: windows() and titles(windows()[0]) == titles(before), 'restored tabs differ')
    after = windows()[0]
    assert after['workspaces'] == before['workspaces'], (before, after)
    assert after['active_tab'] == before['active_tab']
    assert sorted(p.read_text() for p in work.glob('pid-*')) == pids, 'restore respawned a child'
    send('x')
    wait(lambda: b'x' in (work / 'input-2').read_bytes(), 'restored tab does not accept input')

    # Child exits remove only dead terminals; an empty selected workspace
    # falls back to another live workspace on restoration.
    send('ctrl+shift+w')
    wait(lambda: not windows(), 'window did not return to graveyard')
    (work / 'stop-2').touch()
    (work / 'stop-3').touch()
    time.sleep(.4)
    launch('--restore-window')
    wait(lambda: windows() and windows()[0]['active_workspace'] == 1, 'workspace fallback failed')
    assert titles(windows()[0]) == ['terminal-0', 'terminal-1']

    # A whole window is one burial: the limit of one retains both tabs.
    # A newer closed window evicts the older one and terminates its children.
    send('ctrl+shift+w')
    wait(lambda: not windows(), 'window did not close')
    launch()
    wait(lambda: (work / 'alive-4').exists(), 'new window after closing all is missing')
    send('ctrl+shift+w')
    wait(lambda: not windows(), 'second window did not close')
    wait(lambda: not heartbeat(0) and not heartbeat(1), 'oldest window was not evicted')
    assert heartbeat(4), 'newest closed window was evicted'
    launch('--restore-window')
    wait(lambda: windows() and titles(windows()[0]) == ['terminal-4'], 'restored wrong window')

    # Ordinary closed tabs also survive the last window, and child exit while
    # buried clears the entry without ever constructing a replacement window.
    send('ctrl+alt+x')
    wait(lambda: not windows(), 'last-tab close did not remove its window')
    assert heartbeat(4), 'last-tab burial died with the window'
    launch()
    wait(lambda: (work / 'alive-5').exists(), 'fresh terminal missing')
    send('ctrl+shift+z')
    wait(lambda: len(windows()[0]['tabs']) == 2, 'closed tab could not be raised')
    send('ctrl+shift+w')
    wait(lambda: not windows(), 'final window did not close')
    (work / 'stop-4').touch()
    (work / 'stop-5').touch()
    time.sleep(.5)
    launch('--restore-window')
    assert windows() == [], 'exited children were restored'

    # Restored shells keep their original routing identity even when another
    # window now occupies their former position in the window list.
    launch()
    wait(lambda: (work / 'alive-6').exists(), 'routing source missing')
    send('ctrl+shift+w')
    wait(lambda: not windows(), 'routing source did not close')
    launch()
    wait(lambda: (work / 'alive-7').exists(), 'routing neighbor missing')
    send('ctrl+alt+shift+z')
    wait(lambda: len(windows()) == 2, 'restore-window shortcut failed')
    assert titles(windows()[1]) == ['terminal-6']
    (work / 'launch-6').touch()
    wait(lambda: len(windows()[1]['tabs']) == 2, 'restored shell routed to another window')
    assert titles(windows()[0]) == ['terminal-7']
    wait(lambda: titles(windows()[1]) == ['terminal-6', 'terminal-8'], 'routed terminal title missing')
    retained_id = windows()[1]['window']
    for index in range(9, 13):
        send('window:2:ctrl+shift+t')
        wait(lambda: (work / f'alive-{index}').exists(), 'carousel terminal missing')
    send('window:2:ctrl+shift+w')
    wait(lambda: len(windows()) == 1, 'restored window did not close again')
    send('ctrl+shift+g')
    wid = sp.check_output(['xdotool', 'search', '--onlyvisible', '--pid', str(app.pid)], env=env).splitlines()[-1]
    prefix = f'graveyard/window/{retained_id}'

    def click_control(name):
        x, y = control_center(log_path, name)
        sp.run(['xdotool', 'windowraise', wid], env=env, check=True)
        sp.run(['xdotool', 'windowfocus', '--sync', wid], env=env, check=True)
        sp.run(['xdotool', 'mousemove', '--window', wid, str(round(x)), str(round(y)), 'click', '1'], env=env, check=True)
        time.sleep(.3)

    def screenshot(name):
        path = work / f'{name}.png'
        sp.run(['import', '-window', wid, str(path)], env=env, check=True)
        return Image.open(path).convert('RGB')

    time.sleep(2)
    screenshot('window-graveyard')
    sp.run(['xdotool', 'windowsize', '--sync', wid, '560', '480'], env=env, check=True)
    time.sleep(1)
    before = screenshot('window-graveyard-small')
    well = tuple(round(value) for value in control_rect(log_path, prefix + '/tab/6'))
    assert any(hi > 90 for lo, hi in before.crop(well).getextrema()), 'retained window preview has no terminal content'
    (work / 'update-12').touch()
    wait(lambda: ImageChops.difference(before.crop(well), screenshot('window-graveyard-live').crop(well)).getbbox(),
         'retained terminal preview did not refresh')
    assert (work / 'grid-12').read_text() == (work / 'grid-check-12').read_text(), 'preview resized the retained terminal'
    # Browse backward through the shared carousel, without reopening the window.
    click_control(prefix + '/previous')
    click_control(prefix + '/previous')
    assert len(windows()) == 1, 'browsing the carousel restored a window'
    screenshot('window-graveyard-browsed')
    click_control(prefix + '/restore')
    wait(lambda: len(windows()) == 2, 'graveyard Restore window button failed')
    assert windows()[1]['active_tab'] == 4, 'Restore did not use the browsed terminal'
    send('window:2:ctrl+shift+w')
    wait(lambda: len(windows()) == 1, 'panel-restored window did not close')
    time.sleep(.5)
    click_control(prefix + '/kill')
    wait(lambda: all(not heartbeat(index) for index in [6, 8, 9, 10, 11, 12]), 'graveyard Kill window button failed')
    print(f'daemon smoke ok (artifacts in {work})')
finally:
    app.terminate()
    try:
        app.wait(timeout=5)
    except sp.TimeoutExpired:
        app.kill()
        app.wait(timeout=5)
    # SIGTERM ends a manual daemon without systemd's cgroup cleanup.
    for path in work.glob('pid-*'):
        try:
            os.kill(int(path.read_text()), 15)
        except ProcessLookupError:
            pass
    log.close()
