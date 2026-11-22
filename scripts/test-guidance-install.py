#!/usr/bin/env python3
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('install_guidance', Path(__file__).with_name('install-codex-guidance.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class GuidanceInstall(unittest.TestCase):
    def test_additive_idempotent_install_and_custom_profile_refusal(self):
        with tempfile.TemporaryDirectory() as temp:
            home = Path(temp)
            binary = home / 'binary with spaces'
            binary.write_text('#!/bin/sh\nexit 0\n')
            binary.chmod(0o755)
            profile = home / 'example.toml'
            profile.write_text('use = ["devenv-agent"]\n')
            hooks = home / '.codex/hooks.json'
            hooks.parent.mkdir()
            original = {'hooks': {'SessionStart': [{'hooks': [{'type': 'command', 'command': 'existing-hook'}]}]}}
            hooks.write_text(json.dumps(original))
            first = module.install(home, binary, profile)
            self.assertTrue(Path(first['backup']).is_file())
            result = json.loads(hooks.read_text())
            self.assertEqual(result['hooks']['SessionStart'][0], original['hooks']['SessionStart'][0])
            self.assertIn("'" + str(binary.absolute()) + "'", result['hooks']['PreToolUse'][0]['hooks'][0]['command'])
            before = hooks.read_bytes()
            self.assertIsNone(module.install(home, binary, profile)['backup'])
            self.assertEqual(before, hooks.read_bytes())
            (home / '.config/trapline/codex.toml').write_text('custom profile')
            with self.assertRaises(ValueError):
                module.install(home, binary, profile)
            self.assertEqual(before, hooks.read_bytes())
            self.assertFalse((home / '.codex/config.toml').exists())

    def test_package_managed_binary_keeps_stable_hook_path(self):
        with tempfile.TemporaryDirectory() as temp:
            home = Path(temp)
            version = home / 'Cellar/1/bin/trapline'
            version.parent.mkdir(parents=True)
            version.write_text('#!/bin/sh\nexit 0\n')
            version.chmod(0o755)
            stable = home / 'opt/trapline'
            stable.parent.mkdir()
            stable.symlink_to(version)
            profile = home / 'profile.toml'
            profile.write_text('use = ["handoff"]\n')
            module.install(home, stable, profile)
            text = (home / '.codex/hooks.json').read_text()
            self.assertIn(str(stable.absolute()), text)
            self.assertNotIn('Cellar', text)

    def test_repeated_package_profile_upgrades_keep_receipts_and_existing_hook_path(self):
        with tempfile.TemporaryDirectory() as temp:
            home = Path(temp)
            binary = home / 'binary'
            binary.write_text('#!/bin/sh\nexit 0\n'); binary.chmod(0o755)
            alias = home / 'stable-alias'; alias.symlink_to(binary)
            profile = home / 'profile.toml'
            for number in range(3):
                profile.write_text('use = ["handoff"]\n# generation ' + str(number))
                module.install(home, alias if number == 0 else binary, profile)
            hooks = json.loads((home / '.codex/hooks.json').read_text())['hooks']
            self.assertEqual(len(hooks['SessionStart']), 1)
            self.assertEqual(len(hooks['PreToolUse']), 1)
            self.assertIn(str(alias), hooks['SessionStart'][0]['hooks'][0]['command'])
            stored = json.loads((home / '.local/state/trapline/guidance.json').read_text())
            self.assertEqual(stored['profile_sha256'], __import__('hashlib').sha256(profile.read_bytes()).hexdigest())


if __name__ == '__main__':
    unittest.main()
