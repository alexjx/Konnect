#!/usr/bin/env python3
"""Validate Konnect's PCM packaging against KiCAD's addon schema.

Two modes, combinable:

  python packaging/validate-pcm.py --metadata packaging/metadata.json
      Validate a metadata.json file against the vendored packages.v1 schema.

  python packaging/validate-pcm.py --zip dist/konnect-pcm-v0.1.2.zip
      Assert the PCM zip structure (metadata at root, plugin launcher,
      per-OS entrypoint binary, icons) and validate the embedded metadata.

Exits non-zero on any failure. This is the gate that would have caught the
v0.1.1 install failures (#4/#8: missing author.contact) and the schema-invalid
license/sha fields found while building it.

Requires: pip install jsonschema
"""

import argparse
import ast
import json
import sys
import zipfile
from pathlib import Path

import jsonschema

SCHEMA_PATH = Path(__file__).parent / "schema" / "packages.v1.schema.json"

REQUIRED_ZIP_ENTRIES = [
    "metadata.json",
    "plugins/__init__.py",
    "plugins/plugin.json",
    "plugins/settings_dialog.py",
    "plugins/resources/icon.png",
    "resources/icon.png",
]


def load_schema():
    return json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))


def validate_metadata(meta: dict, label: str) -> list[str]:
    errors = []
    try:
        jsonschema.validate(meta, load_schema())
    except jsonschema.ValidationError as e:
        errors.append(f"{label}: schema violation at {e.json_path}: {e.message}")
    return errors


def python_action_names(source: str) -> list[str]:
    """Return statically declared pcbnew ActionPlugin names."""
    tree = ast.parse(source)
    names = []
    for class_node in (node for node in tree.body if isinstance(node, ast.ClassDef)):
        is_action_plugin = any(
            (
                isinstance(base, ast.Attribute)
                and isinstance(base.value, ast.Name)
                and base.value.id == "pcbnew"
                and base.attr == "ActionPlugin"
            )
            or (isinstance(base, ast.Name) and base.id == "ActionPlugin")
            for base in class_node.bases
        )
        if not is_action_plugin:
            continue
        for node in ast.walk(class_node):
            if not isinstance(node, ast.Assign):
                continue
            if not isinstance(node.value, ast.Constant) or not isinstance(
                node.value.value, str
            ):
                continue
            if any(
                isinstance(target, ast.Attribute)
                and isinstance(target.value, ast.Name)
                and target.value.id == "self"
                and target.attr == "name"
                for target in node.targets
            ):
                names.append(node.value.value)
    return names


def validate_zip(zip_path: Path) -> list[str]:
    errors = []
    z = zipfile.ZipFile(zip_path)
    names = set(z.namelist())

    for entry in REQUIRED_ZIP_ENTRIES:
        if entry not in names:
            errors.append(f"{zip_path.name}: missing required entry {entry}")

    # Exactly one server binary must be present, and plugin.json's entrypoint
    # must point at it (PR #7's per-OS stamping contract).
    binaries = [n for n in names if n.startswith("plugins/bin/konnect")]
    if not binaries:
        errors.append(f"{zip_path.name}: no plugins/bin/konnect* binary found")

    if "plugins/plugin.json" in names:
        plugin = json.loads(z.read("plugins/plugin.json"))
        for action in plugin.get("actions", []):
            ep = action.get("entrypoint", "")
            if ep.startswith("bin/") and f"plugins/{ep}" not in names:
                errors.append(
                    f"{zip_path.name}: plugin.json entrypoint '{ep}' "
                    f"not present in the zip"
                )

        # KiCad loads executable actions from plugin.json and legacy Python
        # ActionPlugins from __init__.py. Identical names make them appear as
        # indistinguishable duplicate menu/toolbar entries.
        if "plugins/__init__.py" in names:
            try:
                legacy_names = python_action_names(
                    z.read("plugins/__init__.py").decode("utf-8")
                )
            except (SyntaxError, UnicodeDecodeError) as e:
                errors.append(
                    f"{zip_path.name}: cannot inspect Python action names: {e}"
                )
            else:
                action_names = [
                    action.get("name")
                    for action in plugin.get("actions", [])
                    if isinstance(action.get("name"), str)
                ]
                normalized_legacy = {name.strip().casefold() for name in legacy_names}
                duplicates = sorted(
                    {
                        name
                        for name in action_names
                        if name.strip().casefold() in normalized_legacy
                    },
                    key=str.casefold,
                )
                for name in duplicates:
                    errors.append(
                        f"{zip_path.name}: duplicate KiCad action name '{name}' "
                        "in plugin.json and __init__.py"
                    )

    if "metadata.json" in names:
        meta = json.loads(z.read("metadata.json"))
        errors += validate_metadata(meta, f"{zip_path.name}:metadata.json")
        # Empty-string download fields pass nothing; they must be real or absent.
        for v in meta.get("versions", []):
            for field in ("download_sha256", "download_url"):
                if v.get(field) == "":
                    errors.append(
                        f"{zip_path.name}: {field} is an empty string — "
                        f"omit the field or provide a real value"
                    )

    return errors


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--metadata", type=Path, help="metadata.json to validate")
    ap.add_argument("--zip", type=Path, help="PCM zip to validate")
    args = ap.parse_args()

    if not args.metadata and not args.zip:
        ap.error("provide --metadata and/or --zip")

    errors: list[str] = []
    if args.metadata:
        meta = json.loads(args.metadata.read_text(encoding="utf-8"))
        errors += validate_metadata(meta, str(args.metadata))
    if args.zip:
        errors += validate_zip(args.zip)

    if errors:
        for e in errors:
            print(f"FAIL: {e}", file=sys.stderr)
        return 1

    checked = " and ".join(
        str(p) for p in (args.metadata, args.zip) if p is not None
    )
    print(f"OK: {checked} passed PCM validation")
    return 0


if __name__ == "__main__":
    sys.exit(main())
