#!/usr/bin/env python3
"""Package native binaries and the additive installer without host configuration."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import subprocess
import tarfile
import tempfile
import tomllib


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tag')
    parser.add_argument('--output', type=Path, default=Path('dist'))
    parser.add_argument('--binary-dir', type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    version = tomllib.loads((root / 'Cargo.toml').read_text())['workspace']['package']['version']
    if args.tag and args.tag != 'v' + version:
        parser.error('Tag must match workspace version')
    system = platform.system().lower()
    machine = {'aarch64': 'arm64', 'amd64': 'x86_64'}.get(platform.machine().lower(), platform.machine().lower())
    if system not in ('darwin', 'linux') or machine not in ('arm64', 'x86_64'):
        parser.error('Unsupported release platform')
    name = 'trapline-' + version + '-' + system + '-' + machine
    args.output.mkdir(parents=True, exist_ok=True)
    files = {**{'bin/' + n: (args.binary_dir or root / 'target/release') / n for n in ('trapline', 'nudge')},
             **{'scripts/' + n: root / 'scripts' / n for n in ('install-release.py', 'install-codex-guidance.py')},
             **{'examples/' + n: root / 'examples' / n for n in ('codex.toml', 'codex-hooks.json')}, 'README.md': root / 'README.md'}
    for binary in ('trapline', 'nudge'):
        if subprocess.check_output([str(files['bin/' + binary]), '--version'], text=True).strip() != binary + ' ' + version:
            raise ValueError('Release binary does not match source version')
        if system == 'linux' and 'INTERP' in subprocess.check_output(['readelf', '-l', str(files['bin/' + binary])], text=True):
            raise ValueError('Linux releases must be static; build the musl target')
    manifest = {'version': version, 'system': system, 'machine': machine,
                'files': {n: hashlib.sha256(p.read_bytes()).hexdigest() for n, p in files.items()}}
    with tempfile.TemporaryDirectory() as temp:
        metadata = Path(temp) / 'manifest.json'
        metadata.write_text(json.dumps(manifest, indent=2) + '\n')
        with tarfile.open(args.output / (name + '.tar.gz'), 'w:gz') as archive:
            for relative, source in {**files, 'manifest.json': metadata}.items():
                archive.add(source, arcname=name + '/' + relative, recursive=False)
    target = args.output / (name + '.tar.gz')
    (args.output / (name + '.sha256')).write_text(hashlib.sha256(target.read_bytes()).hexdigest() + '  ' + target.name + '\n')
    print(target)


if __name__ == '__main__':
    main()
