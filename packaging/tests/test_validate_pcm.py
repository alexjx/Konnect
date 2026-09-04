"""Focused regressions for PCM action-name validation."""

from __future__ import annotations

import ast
import importlib.util
import json
import tempfile
import unittest
import zipfile
from pathlib import Path


PACKAGING_DIR = Path(__file__).resolve().parents[1]
REPOSITORY_ROOT = PACKAGING_DIR.parent
SPEC = importlib.util.spec_from_file_location(
    "validate_pcm", PACKAGING_DIR / "validate-pcm.py"
)
assert SPEC is not None and SPEC.loader is not None
VALIDATE_PCM = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VALIDATE_PCM)


class ActionNameValidationTests(unittest.TestCase):
    def test_rejects_duplicate_exec_and_python_action_names(self):
        plugin = {"actions": [{"name": " Konnect "}]}
        python_source = """
class KonnectPlugin(pcbnew.ActionPlugin):
    def defaults(self):
        self.name = "konnect"
"""

        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "duplicate-actions.zip"
            with zipfile.ZipFile(archive, "w") as package:
                package.writestr("plugins/plugin.json", json.dumps(plugin))
                package.writestr("plugins/__init__.py", python_source)

            errors = VALIDATE_PCM.validate_zip(archive)

        self.assertIn(
            "duplicate-actions.zip: duplicate KiCad action name ' Konnect ' "
            "in plugin.json and __init__.py",
            errors,
        )

    def test_repository_actions_have_distinct_names(self):
        plugin = json.loads(
            (REPOSITORY_ROOT / "plugin" / "plugin.json").read_text(encoding="utf-8")
        )
        python_source = (REPOSITORY_ROOT / "plugin" / "__init__.py").read_text(
            encoding="utf-8"
        )

        self.assertEqual(
            VALIDATE_PCM.duplicate_action_names(plugin, python_source),
            [],
        )

    def test_only_executable_action_requests_a_toolbar_button(self):
        plugin = json.loads(
            (REPOSITORY_ROOT / "plugin" / "plugin.json").read_text(encoding="utf-8")
        )
        python_source = (REPOSITORY_ROOT / "plugin" / "__init__.py").read_text(
            encoding="utf-8"
        )
        tree = ast.parse(python_source)
        python_toolbar_values = [
            node.value.value
            for node in ast.walk(tree)
            if isinstance(node, ast.Assign)
            and isinstance(node.value, ast.Constant)
            and any(
                isinstance(target, ast.Attribute)
                and target.attr == "show_toolbar_button"
                for target in node.targets
            )
        ]

        self.assertEqual(python_toolbar_values, [False])
        self.assertEqual(
            [action.get("show-button") for action in plugin["actions"]],
            [True],
        )


if __name__ == "__main__":
    unittest.main()
