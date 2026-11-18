"""Read the native-pixel bounds actually allocated by egui controls."""
import json


def control_rect(log_path, name):
    prefix = f'ui control {name}: '
    for line in reversed(log_path.read_text().splitlines()):
        if prefix in line:
            return tuple(json.loads(line.split(prefix, 1)[1]))
    raise AssertionError(f'Control was not laid out: {name}')


def control_center(log_path, name):
    left, top, right, bottom = control_rect(log_path, name)
    return (left + right) / 2, (top + bottom) / 2
