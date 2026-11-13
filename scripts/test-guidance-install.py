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
            self.assertIn("'" + str(binary.resolve()) + "'", result['hooks']['PreToolUse'][0]['hooks'][0]['command'])
            before = hooks.read_bytes()
            self.assertIsNone(module.install(home, binary, profile)['backup'])
            self.assertEqual(before, hooks.read_bytes())
            (home / '.config/trapline/codex.toml').write_text('custom profile')
            with self.assertRaises(ValueError):
                module.install(home, binary, profile)
            self.assertEqual(before, hooks.read_bytes())
            self.assertFalse((home / '.codex/config.toml').exists())


if __name__ == '__main__':
    unittest.main()
