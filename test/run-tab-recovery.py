#!/usr/bin/env python3
"""Run tab recovery integration test against a disposable SSH server on localhost."""
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time

repo = Path(__file__).resolve().parent.parent
sshd = shutil.which('sshd') or '/usr/sbin/sshd'
with tempfile.TemporaryDirectory(prefix='buoy-tab-recovery-') as directory:
    root = Path(directory)
    def run(*command):
        subprocess.run(command, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for name in ('host', 'client'):
        run('ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-f', str(root / name))
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        port = listener.getsockname()[1]
    config = root / 'sshd_config'
    config.write_text(f'''Port {port}
ListenAddress 127.0.0.1
HostKey {root}/host
PidFile {root}/pid
AuthorizedKeysFile {root}/client.pub
StrictModes no
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
AllowTcpForwarding yes
LogLevel ERROR
SetEnv XDG_CACHE_HOME={root}/cache SHELL=/bin/sh
''')
    with (root / 'sshd.log').open('w+') as log:
        server = subprocess.Popen([sshd, '-D', '-e', '-f', str(config)], stdout=log, stderr=log)
        try:
            for attempt in range(50):
                if server.poll() is not None:
                    log.seek(0)
                    raise RuntimeError('Disposable sshd failed: ' + log.read())
                try:
                    with socket.create_connection(('127.0.0.1', port), timeout=.1):
                        break
                except OSError:
                    time.sleep(.1)
            else:
                raise RuntimeError('Disposable sshd did not start')
            env = dict(os.environ, BUOY_RECOVERY_SSH_PORT=str(port),
                       BUOY_RECOVERY_SSH_KEY=str(root / 'client'))
            subprocess.run(['cargo', 'test', '-p', 'buoy', '--test',
                            'live_tab_recovery', '--', '--nocapture'], cwd=repo, env=env, check=True)
        finally:
            server.terminate()
            server.wait(timeout=5)
