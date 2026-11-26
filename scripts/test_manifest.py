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
        self.assertIs(tool["stroke"], True)
        for setting in manifest["settings"]:
            self.assertTrue(all(choice["icon"]["paths"] for choice in setting["choices"]))
        self.assertEqual(tool["settings"], ["routing", "arrow"])
        self.assertEqual(set(manifest["permissions"]), {"own-objects", "react-to-changes"})
        self.assertEqual(manifest["commands"][0]["surface"], "own-object")

    def test_rejects_non_path_option_icon(self):
        manifest = validate()
        manifest["settings"][0]["choices"][0]["icon"]["paths"] = ["<script>"]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "manifest.json"
            path.write_text(json.dumps(manifest), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "Invalid icon path"):
                validate(path)

    def test_tool_requires_declared_settings_and_ownership(self):
        for field, value in [("permissions", []), ("settings", [])]:
            with self.subTest(field=field), tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "manifest.json"
                path.write_text(json.dumps(validate() | {field: value}), encoding="utf-8")
                with self.assertRaises(ValueError):
                    validate(path)

    def test_path_edit_requires_a_declared_choice_and_port_snap(self):
        manifest = validate()
        self.assertEqual(manifest["commands"][0]["pathEdit"], {"choice": "routing", "snap": "ports"})
        for descriptor in [{"choice": "missing", "snap": "ports"}, {"choice": "routing", "snap": "none"}]:
            with self.subTest(descriptor=descriptor), tempfile.TemporaryDirectory() as directory:
                manifest["commands"][0]["pathEdit"] = descriptor
                path = Path(directory) / "manifest.json"
                path.write_text(json.dumps(manifest), encoding="utf-8")
                with self.assertRaisesRegex(ValueError, "Invalid path edit descriptor"):
                    validate(path)
