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

Curved routes use a straight lead, a tangent-continuous S-curve and a straight lead at the other end. Orthogonal routes also leave and enter along leads, giving free ends a balanced Z instead of a single L. Aligned ends naturally stay straight. Straight routing is unchanged.

Leads are at most 12 pt and 20% of the endpoint distance. Curve handles are 40% of the distance between lead ends, capped at 160 pt and half the forward projection onto each lead axis to avoid overshoot. Very close away-facing ports (under 24 pt) fall back to a line. Orthogonal obstacle routing can reduce clearance to half or zero when necessary.

Attached ends retain their port normals. Free ends initially choose the dominant span axis (horizontal on a tie), facing the other end. Saved side fields retain that axis until the other axis exceeds it by 15%; direction signs always face the other endpoint. Initial tool previews have no prior saved side and use the dominant axis. Page-edge clamping can shorten leads and handles to keep the drawing valid.

Native and sideload-only Wasm entry points share this routing and shape-building implementation. No permissions, dependencies or addon data version change.

## Host contract

Connectors 0.3.0 declares `requires: ["end-caps", "path-edit-v0", "scoped-reads-v0"]` in its manifest. It needs a Goodtype host with P3 scoped geometry reads; released v0.3.x hosts reject this field. Session negotiation still checks `page-batch-v0`, `path-edit-v0` and `scoped-reads-v0` over `goodtype-api 0.1`. The Windows AppContainer session uses JSON-RPC over stdin and stdout.

The `change-routing` own-object command declares `pathEdit` with the `routing` choice and `ports` snapping. A port drop adopts a capped line or open spline and attaches the dropped end using the Connect tool's routing preference. A routing pick adopts it with two free ends or changes an owned connector's route. The host commits a port drop immediately with Undo. Routing picks follow the host's proposal and direct-apply settings.

An edit returns exactly one `updateObject` for the captured ID inside `batch.operations`, with local centerline geometry and a single addon entry. Creation uses the addon-ID map. Reactions also write single entries and return an empty operations array when there is no change. Saved connector data is version 2. Core style caps render arrowheads and survive rerouting and detaching. An explicit edit or relevant reaction can upgrade a verified legacy wing path. If the saved geometry no longer matches the legacy signature, the request returns an upgrade-review error and preserves the drawing.

The router prefers a few bends and keeps attached-part clearance when possible. A reaction reply carries up to 32 operations, so the host must scope reaction requests accordingly. Reactions still consider only attached parts; release actions can read nearby obstacles. With omitted outline color and width, the host supplies the pen. Its final outline bounds check uses the actual pen width.

Licensed under [Apache 2.0](LICENSE).

Orthogonal connectors route around nearby parts on native release/commit using `scoped-reads-v0`; straight and curved routes stay unchanged. Live Wasm previews have no reads and may differ from the committed route.

A release queries the current route bounding box expanded by 12 pt, intersected with the advertised region. If either side exceeds the advertised limit (at most 1024 pt), it uses the existing router instead of treating a partial corridor as clear. Own path, endpoint IDs and explicit descendants of an endpoint group are excluded; other boxes are inflated by 12 pt. One region read plus at most one group read respects the invocation budgets. Two distinct endpoint groups, over 32 unrelated boxes (the grid-search work cap), any refusal, bad response, stale revision or exhausted budget use the existing attached-part router. Unroutable obstacles also fall back. No obstacle data is persisted. Wasm commits also keep the existing routing because that transport supplies no reads.
