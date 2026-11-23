import json
from pathlib import Path
import tempfile
import unittest

from manifest import validate


class ManifestTests(unittest.TestCase):
    def test_connector_tool_declares_both_settings(self):
        manifest = validate()
        tool, = manifest["tools"]
        self.assertEqual(tool["snap"], "ports")
        self.assertEqual(tool["settings"], ["routing", "arrow"])
        self.assertEqual(set(manifest["permissions"]), {"own-objects", "react-to-changes"})
        self.assertEqual(manifest["commands"][0]["surface"], "own-object")

    def test_tool_requires_declared_settings_and_ownership(self):
        for field, value in [("permissions", []), ("settings", [])]:
            with self.subTest(field=field), tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "manifest.json"
                path.write_text(json.dumps(validate() | {field: value}), encoding="utf-8")
                with self.assertRaises(ValueError):
                    validate(path)
