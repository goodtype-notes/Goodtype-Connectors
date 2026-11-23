"""Package and smoke-test the Windows addon executable built with Cargo."""
from hashlib import sha256
import json
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo

from manifest import ROOT, require, validate

PACKAGE_NAME = "goodtype-connectors"


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def exchange(binary, frames):
    result = subprocess.run(
        [binary, "--session"], input="".join(json.dumps(row) + "\n" for row in frames),
        text=True, capture_output=True, check=True, timeout=10,
    )
    return [json.loads(row) for row in result.stdout.splitlines()]


def fixture(name):
    return json.loads((ROOT / f"tests/fixtures/{name}.json").read_text(encoding="utf-8"))


def smoke_test(binary, version):
    output = subprocess.check_output([binary, "--version", "--json"], text=True, timeout=10)
    require(json.loads(output)["version"] == version, "Packaged version differs from the manifest")
    output = subprocess.check_output([binary, "--check"], text=True, timeout=10)
    require(json.loads(output)["ok"], "Packaged self-check failed")
    handshake, applied = exchange(binary, [fixture("initialize"), fixture("tool-apply")])
    require(handshake["result"]["addon"]["id"] == "connectors", "Wrong session identity")
    op, = applied["result"]["batch"]["operations"]
    require(op["op"] == "addShape" and "connectors" in op["addonData"], "Wrong operation")


def package():
    manifest = validate()
    name = f"{PACKAGE_NAME}-{manifest['version']}-windows-x64"
    output = ROOT / "target/package"
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f"{name}.zip"
    binary = ROOT / "target/release" / manifest["entrypoint"]
    files = {path.name: path.read_bytes() for path in (
        binary, ROOT / "LICENSE", ROOT / "goodtype-addon.json", ROOT / "ADDON.md",
    )}
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", manifest["target"]],
        cwd=ROOT,
    ))
    for dependency in metadata["packages"]:
        if dependency["source"] is None:
            continue
        licenses = sorted(Path(dependency["manifest_path"]).parent.glob("LICENSE*"))
        require(licenses, f"Missing license for {dependency['name']}")
        for path in licenses:
            files[f"licenses/{dependency['name']}-{dependency['version']}/{path.name}"] = path.read_bytes()
    provenance = {
        "version": manifest["version"], "target": manifest["target"], "protocolVersion": 1,
        "sourceCommit": git("rev-parse", "HEAD"), "sourceDirty": bool(git("status", "--porcelain")),
        "files": [{"path": path, "sizeBytes": len(data), "sha256": sha256(data).hexdigest()}
                  for path, data in sorted(files.items())],
    }
    files["importer-manifest.json"] = (json.dumps(provenance, indent=2) + "\n").encode("utf-8")
    pending = archive.with_suffix(".zip.tmp")
    try:
        with ZipFile(pending, "w") as zipped:
            for path, data in sorted(files.items()):
                zipped.writestr(ZipInfo(f"{name}/{path}"), data, compress_type=ZIP_DEFLATED)
        with TemporaryDirectory() as directory, ZipFile(pending) as zipped:
            require(zipped.testzip() is None, "ZIP integrity check failed")
            zipped.extractall(directory)
            root = Path(directory) / name
            for record in provenance["files"]:
                require(sha256((root / record["path"]).read_bytes()).hexdigest() == record["sha256"],
                        f"Extracted file differs: {record['path']}")
            smoke_test(root / manifest["entrypoint"], manifest["version"])
        pending.replace(archive)
    finally:
        pending.unlink(missing_ok=True)
    digest = sha256(archive.read_bytes()).hexdigest()
    (output / "SHA256SUMS").write_bytes(f"{digest}  {archive.name}\n".encode("ascii"))
    print(f"{archive}  sha256 {digest}")


if __name__ == "__main__":
    package()
