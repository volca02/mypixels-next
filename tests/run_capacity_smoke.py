"""Verify that a dense terminal renders beyond the former 4,096-run pool cap.

Run under a private display: dbus-run-session -- xvfb-run -a -s
"-screen 0 1600x1200x24" python3 tests/run_capacity_smoke.py.
"""
import json
import os
from pathlib import Path
import re
import subprocess as sp
import tempfile
import time

from PIL import Image

root = Path(__file__).resolve().parents[1]
work = Path(tempfile.mkdtemp(prefix='mypixels-run-capacity-'))
config = work / 'config.yml'
config.write_text("""wal_palette: ''
term_font: DejaVu Sans Mono 12
default_width: 1200
default_height: 1000
default_maximize: false
hide_single_tab: false
background_color: '#000000'
foreground_color: '#ffffff'
""")
fixture = work / 'fixture.py'
fixture.write_text(r'''
import fcntl, json, os, struct, sys, termios, time, tty
from pathlib import Path
tty.setraw(0)
time.sleep(1)
rows, cols, _, _ = struct.unpack('HHHH', fcntl.ioctl(0, termios.TIOCGWINSZ, b'\0' * 8))
Path(sys.argv[1]).write_text(json.dumps([rows, cols]))
output = bytearray(b'\x1b[?25l\x1b[?7l\x1b[2J')
for row in range(rows):
    output.extend(f'\x1b[{row + 1};1H'.encode())
    for col in range(cols):
        output.extend(b'\x1b[91mX' if col % 2 == 0 else b'\x1b[92mX')
os.write(1, output)
while os.read(0, 1024):
    pass
''')
env = dict(os.environ, MYPIXELS_CONFIG=str(config), MYPIXELS_FULL_REDRAW='1', RUST_LOG='info')
env.pop('WAYLAND_DISPLAY', None)
env.pop('MYPIXELS_KEYS', None)
binary = os.environ.get('MYPIXELS_BIN', str(root / 'target/debug/mypixels'))
log_path = work / 'app.log'
log = log_path.open('w')
app = sp.Popen([binary, '-s', 'python3', str(fixture), str(work / 'grid.json')],
               env=env, stdout=log, stderr=log)
try:
    deadline = time.monotonic() + 20
    wid = None
    while time.monotonic() < deadline:
        assert app.poll() is None, log_path.read_text()
        windows = sp.run(['xdotool', 'search', '--onlyvisible', '--pid', str(app.pid)],
                         env=env, stdout=sp.PIPE).stdout.decode().splitlines()
        if windows and (work / 'grid.json').exists():
            wid = windows[-1]
            break
        time.sleep(.1)
    assert wid, 'window or terminal fixture missing'
    time.sleep(3)
    screenshot = work / 'dense-terminal.png'
    sp.run(['import', '-window', wid, str(screenshot)], env=env, check=True, timeout=10)
    image = Image.open(screenshot).convert('RGB')
    info = log_path.read_text()
    # Use the effective metrics, including any startup scale-factor adjustment.
    cell_w, cell_h = map(float, re.findall(r'cell = ([0-9.]+) x ([0-9.]+)', info)[-1])
    strip = float(re.findall(r'strip(?: =)? ([0-9.]+)', info)[-1])
    rows, cols = json.loads((work / 'grid.json').read_text())
    assert rows * cols > 4096, 'fixture no longer exceeds the former pool limit'
    pixels = image.load()
    visible = 0
    for row in range(rows):
        for col in range(cols):
            found = False
            for y in range(round(strip + row * cell_h), min(image.height, round(strip + (row + 1) * cell_h))):
                for x in range(round(col * cell_w), min(image.width, round((col + 1) * cell_w))):
                    r, g, b = pixels[x, y]
                    if max(r, g) > 90 and b < max(r, g) * .7:
                        found = True
                        break
                if found:
                    break
            visible += found
    assert visible == rows * cols, f'Only {visible}/{rows * cols} characters rendered; artifacts: {work}'
    print(f'PASS: all {visible} characters rendered; artifacts: {work}', flush=True)
finally:
    app.terminate()
    try:
        app.wait(timeout=3)
    except sp.TimeoutExpired:
        app.kill()
        app.wait()
    log.close()
