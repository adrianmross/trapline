#!/usr/bin/env python3
import hashlib
import importlib.util
import json
from pathlib import Path
import platform
import tempfile
import unittest

SOURCE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('release', SOURCE / 'install-release.py')
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseInstall(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name) / 'release'
        self.home = Path(temp.name) / 'home with spaces'
        files = {'bin/trapline': b'#!/bin/sh\necho trapline 0.2.0\n', 'bin/nudge': b'#!/bin/sh\necho nudge 0.2.0\n',
                 'scripts/install-release.py': (SOURCE / 'install-release.py').read_bytes(),
                 'scripts/install-codex-guidance.py': (SOURCE / 'install-codex-guidance.py').read_bytes(),
                 'examples/codex.toml': b'use = ["devenv-agent"]\n', 'examples/codex-hooks.json': b'{}', 'README.md': b'fixture'}
        for name, data in files.items():
            target = self.root / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
        machine = {'aarch64': 'arm64', 'amd64': 'x86_64'}.get(platform.machine().lower(), platform.machine().lower())
        self.manifest = {'version': '0.2.0', 'system': platform.system().lower(), 'machine': machine,
                         'files': {name: hashlib.sha256(data).hexdigest() for name, data in files.items()}}
        self.save_manifest()

    def save_manifest(self):
        (self.root / 'manifest.json').write_text(json.dumps(self.manifest))

    def test_install_is_additive_idempotent_and_does_not_modify_trust(self):
        hooks = self.home / '.codex/hooks.json'
        hooks.parent.mkdir(parents=True)
        hooks.write_text('{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"existing"}]}]}}')
        config = self.home / '.codex/config.toml'
        config.write_text('[hooks.state]\nexisting = "preserve"\n')
        result = release.install(self.root, self.home)
        before = hooks.read_bytes()
        self.assertEqual(release.install(self.root, self.home)['version'], '0.2.0')
        self.assertEqual(hooks.read_bytes(), before)
        self.assertEqual(config.read_text(), '[hooks.state]\nexisting = "preserve"\n')
        self.assertEqual((self.home / '.local/bin/trapline').stat().st_mode & 0o777, 0o755)
        self.assertEqual(len(json.loads(hooks.read_text())['hooks']['SessionStart']), 2)
        self.assertEqual(result['version'], '0.2.0')

    def test_bad_artifacts_or_custom_profiles_change_no_binary(self):
        (self.root / 'bin/nudge').write_bytes(b'tampered')
        with self.assertRaisesRegex(ValueError, 'checksum'):
            release.install(self.root, self.home)
        self.assertFalse((self.home / '.local/bin').exists())

    def test_platform_and_managed_binary_changes_are_refused(self):
        self.manifest['machine'] = 'other'
        self.save_manifest()
        with self.assertRaisesRegex(ValueError, 'match this host'):
            release.install(self.root, self.home)
        self.manifest['machine'] = {'aarch64': 'arm64', 'amd64': 'x86_64'}.get(platform.machine().lower(), platform.machine().lower())
        self.save_manifest()
        release.install(self.root, self.home)
        target = self.home / '.local/bin/trapline'
        target.write_bytes(b'custom binary')
        with self.assertRaisesRegex(ValueError, 'Managed binary changed'):
            release.install(self.root, self.home, adopt=True)
        self.assertEqual(target.read_bytes(), b'custom binary')

    def test_manual_legacy_install_can_be_adopted_with_backups(self):
        target = self.home / '.local/bin'
        target.mkdir(parents=True)
        (target / 'trapline').write_text('#!/bin/sh\necho trapline 0.1.0\n')
        (target / 'nudge').write_text('#!/bin/sh\necho "usage: nudge packs | nudge show <pack>" >&2\nexit 2\n')
        for name in ('trapline', 'nudge'):
            (target / name).chmod(0o755)
        with self.assertRaisesRegex(ValueError, 'Unmanaged binary differs'):
            release.install(self.root, self.home)
        result = release.install(self.root, self.home, adopt=True)
        self.assertEqual((Path(result['binary_backup']) / 'trapline').read_text(), '#!/bin/sh\necho trapline 0.1.0\n')
        self.assertEqual((target / 'trapline').read_bytes(), (self.root / 'bin/trapline').read_bytes())

    def test_custom_profile_is_rejected_before_any_binary_is_replaced(self):
        profile = self.home / '.config/trapline/codex.toml'
        profile.parent.mkdir(parents=True)
        profile.write_text('custom profile')
        with self.assertRaisesRegex(ValueError, 'profile differs'):
            release.install(self.root, self.home)
        self.assertFalse((self.home / '.local/bin').exists())


if __name__ == '__main__':
    unittest.main()
