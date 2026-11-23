# Connectors for Goodtype

Connectors adds port snapping, straight, curved and orthogonal routing, and optional arrowheads to Goodtype. See [how to use it](ADDON.md).

![Straight, curved and orthogonal connectors on a page](docs/preview.png)

## Build and verify

Use Windows x64, the pinned Rust toolchain and Python 3.12.

```powershell
cargo xtask verify
cargo build --release --locked
python scripts/package.py
```

The script verifies the extracted executable and writes the ZIP and SHA-256 checksum to `target/package`. To regenerate the preview after building, install PyMuPDF and run `python scripts/preview.py`.

## Release

CI packages and tests the executable on Windows. A tag matching the manifest version on the main branch publishes through the protected release environment. Configure its `ADDON_SIGNING_KEY` secret and commit the matching Ed25519 public key as `release-public-key.pem` before publishing.

## Host contract

The Windows AppContainer session uses JSON-RPC over stdin and stdout. Replies use the published `batch.operations` envelope, including an empty array for a reaction that changes nothing. The Step 3c example for an empty reaction spells this as `ops`; the host and addon must agree on the envelope.

Arrowheads are open segments of the same editable spline on this API. Creating a group is supported by the host batch format, but Step 3c has no operation to replace its child geometries and the command payload supplies no group-child snapshots. Separate grouped arrowheads need a host contract for rerouting those children.

The router prefers a few bends and keeps attached-part clearance when possible. A reaction reply carries up to 32 operations, so the host must scope reaction requests accordingly. Only the attached parts are considered because the session receives their snapshots. With omitted outline color and width, the host supplies the pen. Its final outline bounds check uses the actual pen width.

Licensed under [Apache 2.0](LICENSE).
