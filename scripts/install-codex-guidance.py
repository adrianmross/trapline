#!/usr/bin/env python3
"""Install additive Codex guidance definitions; hook trust remains a user action."""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import shlex
import tempfile


def atomic(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(dir=path.parent, prefix='.' + path.name + '-')
    try:
        with os.fdopen(fd, 'wb') as output:
            output.write(data)
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)


def install(home, binary, profile_source):
    binary = binary.resolve(strict=True)
    if not os.access(binary, os.X_OK):
        raise ValueError('Trapline binary must be executable')
    profile = home / '.config/trapline/codex.toml'
    hooks = home / '.codex/hooks.json'
    data = profile_source.read_bytes()
    if profile.exists() and (profile.is_symlink() or profile.read_bytes() != data):
        raise ValueError('Existing Codex guidance profile differs; review it before changing it')
    if hooks.is_symlink():
        raise ValueError('Hook definitions are source-managed; merge the sample in their owning worktree')
    document = json.loads(hooks.read_text()) if hooks.exists() else {}
    groups = document.setdefault('hooks', {})
    command = 'TRAPLINE_CONFIG=' + shlex.quote(str(profile)) + ' ' + shlex.join([str(binary), 'agent', 'codex'])
    handler = {'type': 'command', 'command': command, 'timeout': 3, 'additionalContextLimit': 1000}
    changed = False
    for name in ('SessionStart', 'PreToolUse'):
        existing = groups.setdefault(name, [])
        if any(item.get('command') == command for group in existing for item in group.get('hooks', [])):
            continue
        group = {'hooks': [handler]}
        if name == 'PreToolUse':
            group['matcher'] = 'Bash|exec_command|shell_command'
        existing.append(group)
        changed = True
    backup = None
    if changed and hooks.exists():
        stamp = datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%S%fZ')
        backup = hooks.with_name('hooks.json.before-trapline-' + stamp)
        atomic(backup, hooks.read_bytes())
    atomic(profile, data)
    if changed:
        atomic(hooks, (json.dumps(document, indent=2) + '\n').encode())
    return {'hooks': str(hooks), 'profile': str(profile), 'profile_sha256': hashlib.sha256(data).hexdigest(),
            'backup': str(backup) if backup else None, 'review': 'Open /hooks in a new Codex session and review the two Trapline definitions. Trust was not modified.'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--home', type=Path, default=Path.home())
    parser.add_argument('--trapline', type=Path, required=True)
    parser.add_argument('--profile', type=Path, default=Path(__file__).resolve().parents[1] / 'examples/codex.toml')
    args = parser.parse_args()
    os.umask(0o077)
    try:
        print(json.dumps(install(args.home, args.trapline, args.profile), indent=2))
    except (OSError, ValueError, TypeError, KeyError) as error:
        parser.exit(1, str(error) + '\n')


if __name__ == '__main__':
    main()
