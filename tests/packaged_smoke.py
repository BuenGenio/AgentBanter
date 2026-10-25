#!/usr/bin/env python3
"""Exercise a standalone binary from outside the checkout without opening a real browser."""
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request

binary = Path(sys.argv[1]).resolve()
with tempfile.TemporaryDirectory(prefix='agentbanter installed test ') as temp:
    root = Path(temp)
    installed = root / 'bin with spaces' / 'agentbanter'
    installed.parent.mkdir()
    import shutil
    shutil.copy2(binary, installed)
    installed.chmod(0o755)
    env = os.environ.copy()
    env.update(HOME=str(root), XDG_CONFIG_HOME=str(root / 'config'), XDG_DATA_HOME=str(root / 'data'))
    # A launcher is external UI: record its request instead of opening a real window.
    fake = root / 'tools'
    fake.mkdir()
    browser_log = root / 'browser-url'
    env['AGENTBANTER_TEST_BROWSER_LOG'] = str(browser_log)
    for name in ('open', 'xdg-open'):
        launcher = fake / name
        launcher.write_text('#!/bin/sh\nprintf "%s" "$1" > "$AGENTBANTER_TEST_BROWSER_LOG"\n')
        launcher.chmod(0o755)
    env['PATH'] = str(fake) + os.pathsep + env.get('PATH', '')
    config = root / 'settings with spaces.json'
    empty = root / 'empty'
    empty.mkdir()
    config.write_text(json.dumps({'port': 1, 'bind': ['127.0.0.1'], 'projects_dir': str(empty), 'codex_dir': str(empty), 'chatgpt_dir': str(empty)}))
    # Implicit cwd assets must not affect an installed executable.
    (root / 'static').mkdir()
    (root / 'static/app.css').write_text('UNTRUSTED_CWD_ASSET')
    (root / 'templates').mkdir()
    (root / 'templates/index.html').write_text('UNTRUSTED_CWD_TEMPLATE')
    with socket.socket() as probe:
        probe.bind(('127.0.0.1', 0))
        port = probe.getsockname()[1]
    url = f'http://127.0.0.1:{port}'
    command = [str(installed), '--config', str(config), 'open', '--port', str(port)]
    pid = None
    def get(path):
        with urllib.request.urlopen(url + path, timeout=3) as response:
            return response.read().decode()
    try:
        result = subprocess.run(command, cwd=root, env=env, capture_output=True, text=True, timeout=25)
        assert result.returncode == 0, result.stderr
        health = json.loads(get('/healthz'))
        assert health['app'] == 'agentbanter'
        pid = health['pid']
        assert browser_log.read_text() == url
        assert 'UNTRUSTED_CWD_TEMPLATE' not in get('/')
        assert 'UNTRUSTED_CWD_ASSET' not in get('/static/app.css')
        again = subprocess.run(command + ['--no-browser'], cwd=root, env=env, capture_output=True, text=True, timeout=10)
        assert again.returncode == 0, again.stderr
        assert json.loads(get('/healthz'))['pid'] == pid, 'Repeated open must reuse the server'
        assert list(root.rglob('server.log')), 'Detached server needs a log file'
        if sys.platform.startswith('linux'):
            install = subprocess.run([str(installed), '--config', str(config), 'desktop', 'install'], cwd=root, env=env, capture_output=True, text=True)
            assert install.returncode == 0, install.stderr
            desktop = root / 'data/applications/agentbanter.desktop'
            subprocess.run(['desktop-file-validate', str(desktop)], check=True)
            assert str(installed) in desktop.read_text()
            assert (root / 'data/icons/hicolor/scalable/apps/agentbanter.svg').exists()
            subprocess.run([str(installed), '--config', str(config), 'desktop', 'uninstall'], cwd=root, env=env, check=True)
            assert not desktop.exists()
            # Exercise service-file generation and CLI dispatch without needing a
            # systemd user bus or changing any real service on the CI host.
            control = fake / 'systemctl'
            control.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$AGENTBANTER_TEST_SYSTEMCTL_LOG"\n')
            control.chmod(0o755)
            env['AGENTBANTER_TEST_SYSTEMCTL_LOG'] = str(root / 'systemctl-requests')
            base = [str(installed), '--config', str(config), 'service']
            subprocess.run(base + ['install'], cwd=root, env=env, check=True)
            unit = root / 'config/systemd/user/agentbanter.service'
            assert 'Type=exec' in unit.read_text()
            assert f'"{installed}" serve --config "{config}"' in unit.read_text()
            for action in ('start', 'status', 'stop', 'uninstall'):
                subprocess.run(base + [action], cwd=root, env=env, check=True)
            assert not unit.exists()
            requests = (root / 'systemctl-requests').read_text()
            assert '--user daemon-reload' in requests
            assert '--user start agentbanter.service' in requests

        print('Packaged smoke: CLI config precedence, browser launcher, detached startup, reuse, embedded assets passed.')
    finally:
        if pid is not None:
            os.kill(pid, signal.SIGTERM)
