#!/usr/bin/env python3
"""Install a verified native release; preserve existing hooks and normal review."""
import argparse
import datetime
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import subprocess


def install(root, home, adopt=False):
    manifest = json.loads((root / 'manifest.json').read_text())
    machine = {'aarch64': 'arm64', 'amd64': 'x86_64'}.get(platform.machine().lower(), platform.machine().lower())
    if manifest['system'] != platform.system().lower() or manifest['machine'] != machine:
        raise ValueError('Release does not match this host')
    expected = {'bin/trapline', 'bin/nudge', 'scripts/install-release.py', 'scripts/install-codex-guidance.py',
                'examples/codex.toml', 'examples/codex-hooks.json', 'README.md'}
    if set(manifest['files']) != expected:
        raise ValueError('Invalid release file list')
    for name, digest in manifest['files'].items():
        path = root / name
        if path.is_symlink() or not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            raise ValueError('Release checksum mismatch')
    spec = importlib.util.spec_from_file_location('guidance', root / 'scripts/install-codex-guidance.py')
    guidance = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(guidance)
    guidance.preflight(home, root / 'examples/codex.toml')
    receipt = home / '.local/state/trapline/install.json'
    previous = json.loads(receipt.read_text()) if receipt.exists() else {}
    targets = {name: home / '.local/bin' / name for name in ('trapline', 'nudge')}
    for name, target in targets.items():
        if target.is_symlink():
            raise ValueError('Existing binary is package-managed or linked; use its owning installer')
        if target.exists():
            digest = hashlib.sha256(target.read_bytes()).hexdigest()
            if digest in (manifest['files']['bin/' + name], previous.get('binaries', {}).get(name)):
                continue
            if name in previous.get('binaries', {}):
                raise ValueError('Managed binary changed; inspect it before upgrading')
            if not adopt:
                raise ValueError('Unmanaged binary differs; review it and use --adopt-existing for a known installation')
            identity = subprocess.run([str(target), '--version'], capture_output=True, text=True, timeout=5)
            recognized = identity.returncode == 0 and re.fullmatch(re.escape(name) + r' \d+\.\d+\.\d+', identity.stdout.strip())
            legacy = name == 'nudge' and identity.returncode == 2 and identity.stderr.strip() == 'usage: nudge packs | nudge show <pack>'
            if not recognized and not legacy:
                raise ValueError('Unmanaged command is not a recognized Trapline installation')
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%S%fZ')
    backup = home / '.local/state/trapline/backups' / stamp
    backed_up = False
    for name, target in targets.items():
        if target.exists() and hashlib.sha256(target.read_bytes()).hexdigest() == manifest['files']['bin/' + name]:
            continue
        if target.exists():
            guidance.atomic(backup / name, target.read_bytes(), 0o755)
            backed_up = True
        guidance.atomic(target, (root / 'bin' / name).read_bytes(), 0o755)
    result = guidance.install(home, targets['trapline'], root / 'examples/codex.toml')
    result.update(version=manifest['version'], binaries={n: manifest['files']['bin/' + n] for n in targets}, binary_backup=str(backup) if backed_up else None)
    guidance.atomic(receipt, (json.dumps(result, indent=2) + '\n').encode())
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--home', type=Path, default=Path.home())
    parser.add_argument('--adopt-existing', action='store_true')
    args = parser.parse_args()
    os.umask(0o077)
    try:
        print(json.dumps(install(Path(__file__).resolve().parents[1], args.home, args.adopt_existing), indent=2))
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        parser.exit(1, str(error) + '\n')


if __name__ == '__main__':
    main()
