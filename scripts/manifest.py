"""Validate goodtype-addon.json against manifest v1 plus the Step 3c tool additions (Python 3.12+)."""
import json
from pathlib import Path
import re
import tomllib

ROOT = Path(__file__).resolve().parents[1]
ID = re.compile(r"[a-z0-9]+(?:-[a-z0-9]+)*")
ICON = re.compile(r"[MmLlHhVvCcSsQqAaZz0-9.,\- ]+")
TEXT = re.compile(r"[^<>\x00-\x1f\x7f-\x9f]{1,256}")
PERMISSIONS = {"picked-import-file", "derived-import-artwork", "publish-import-batch", "selected-ink",
               "add-page-objects", "read-pages", "own-objects", "react-to-changes"}
SURFACES = {"library-import", "ink-selection", "insert-form", "page-generator", "export", "own-object"}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def unique_pairs(pairs):
    result = dict(pairs)
    require(len(result) == len(pairs), "Duplicate JSON object key")
    return result


def keys(value, required, optional=""):
    required, optional = set(required.split()), set(optional.split())
    require(isinstance(value, dict) and required <= value.keys() <= required | optional,
            f"Expected keys {sorted(required)} with optional {sorted(optional)}: {value!r}")


def identifier(value):
    require(isinstance(value, str) and len(value) <= 48 and ID.fullmatch(value), f"Invalid identifier: {value!r}")
    return value


def unique_ids(items):
    ids = [identifier(item["id"]) for item in items]
    require(len(set(ids)) == len(ids), "Duplicate declaration ID")
    return ids


def icon(value):
    keys(value, "paths")
    paths = value["paths"]
    require(isinstance(paths, list) and 1 <= len(paths) <= 8, "An icon has 1–8 paths")
    for path in paths:
        require(isinstance(path, str) and 0 < len(path) <= 512 and ICON.fullmatch(path),
                f"Invalid icon path: {path!r}")
    require(sum(len(p) for p in paths) <= 2048, "Icon exceeds 2 KiB")


def setting(value, labels):
    kind = value.get("type")
    if kind == "toggle":
        keys(value, "id label type default", "hint search")
        require(type(value["default"]) is bool, "Toggle default must be a Boolean")
    else:
        require(kind == "choice", f"Unsupported setting type: {kind}")
        keys(value, "id label type default choices", "hint search")
        choices = value["choices"]
        require(isinstance(choices, list) and 2 <= len(choices) <= 8, "A choice needs 2–8 entries")
        values = []
        for choice in choices:
            keys(choice, "value label", "icon")
            if "icon" in choice:
                icon(choice["icon"])
            values.append(identifier(choice["value"]))
            labels.append(identifier(choice["label"]))
        require(len(set(values)) == len(values), "Duplicate choice value")
        require(value["default"] in values, "Choice default is not one of its values")
    labels.append(identifier(value["label"]))
    labels.extend(identifier(value[key]) for key in ("hint", "search") if key in value)


def validate(manifest_path=ROOT / "goodtype-addon.json", cargo_path=ROOT / "Cargo.toml"):
    data = manifest_path.read_bytes()
    require(len(data) <= 64 * 1024, "Manifest exceeds 64 KiB")
    m = json.loads(data.decode("utf-8"), object_pairs_hook=unique_pairs)
    keys(m, "manifestVersion id version apiVersion target entrypoint transport permissions commands settings strings",
         "tools")
    require(type(m["manifestVersion"]) is int and m["manifestVersion"] == 1, "Expected manifest version 1")
    require(m["id"] == "connectors", "Unexpected addon ID")
    version = m["version"]
    require(isinstance(version, str) and len(version) <= 32 and re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version),
            "Invalid version")
    cargo = tomllib.loads(cargo_path.read_text(encoding="utf-8"))["package"]["version"]
    require(version == cargo, f"Manifest version {version} differs from Cargo.toml {cargo}")
    require(m["apiVersion"] == "goodtype-api 0.1" and m["target"] == "x86_64-pc-windows-msvc",
            "Wrong API or target")
    require(m["entrypoint"] == "goodtype-connectors.exe" and m["transport"] == "stdio-jsonrpc",
            "Wrong entrypoint or transport")
    permissions = m["permissions"]
    require(isinstance(permissions, list) and len(set(permissions)) == len(permissions)
            and set(permissions) <= PERMISSIONS, "Invalid permissions")

    labels = []
    settings = m["settings"]
    require(isinstance(settings, list) and len(settings) <= 16, "At most 16 settings")
    setting_ids = unique_ids(settings)
    for value in settings:
        setting(value, labels)

    tools = m.get("tools", [])
    require(isinstance(tools, list) and len(tools) <= 8, "At most 8 tools")
    unique_ids(tools)
    for tool in tools:
        keys(tool, "id label icon kind snap settings", "stroke")
        labels.append(identifier(tool["label"]))
        require(tool["kind"] in ("place", "connect", "frame") and tool["snap"] in ("ports", "none"),
                "Invalid tool kind or snap")
        icon(tool["icon"])
        require(type(tool.get("stroke", False)) is bool, "Tool stroke must be a Boolean")
        require(all(s in setting_ids for s in tool["settings"]), "A tool names an undeclared setting")
    require(not tools or "own-objects" in permissions, "Tools require own-objects")

    commands = m["commands"]
    require(isinstance(commands, list) and 1 <= len(commands) <= 16, "Process addons declare 1–16 commands")
    unique_ids(commands)
    for command in commands:
        keys(command, "id label surface")
        labels.append(identifier(command["label"]))
        require(command["surface"] in SURFACES, "Unknown command surface")
        if command["surface"] == "own-object":
            require("own-objects" in permissions, "Own-object commands require own-objects")

    strings = m["strings"]
    keys(strings, "en de")
    require(strings["en"].keys() == strings["de"].keys(), "Locale keys differ")
    for table in strings.values():
        require("name" in table and len(table) <= 128, "Invalid localization table")
        for key, text in table.items():
            identifier(key)
            require(isinstance(text, str) and TEXT.fullmatch(text) and "{{" not in text and "}}" not in text,
                    f"Invalid plain-text translation for {key}")
    missing = set(labels) - strings["en"].keys()
    require(not missing, f"Missing localized strings: {sorted(missing)}")
    unused = strings["en"].keys() - set(labels) - {"name"}
    require(not unused, f"Unused localized strings: {sorted(unused)}")
    require(all(strings["en"][k] != strings["de"][k] for k in strings["en"]), "German must be translated")
    return m


if __name__ == "__main__":
    manifest = validate()
    print(f"{manifest['id']} {manifest['version']} manifest valid")
