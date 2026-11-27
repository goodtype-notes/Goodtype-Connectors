# Scoped read conformance cases

`requests.json` pins request params and expected object IDs or error tokens. The Rust
host runs it in `objects::scoped_reads::conformance::shared_wire_cases`.

Fixture: captured revision 7, granted group `symbol` with explicit child `child`,
and unrelated shape `unrelated`. Both shapes are rectangles at (30,40), local size
20x10, scale 2, with local port (0,5). All boxes are (30,40,40,20); page ports are
(30,50). Only `symbol` is a granted group. The region grant expands its box by 144
points and clips it to the page. Private addon data on every fixture object must
never appear in either response.

Copy these cases at a pinned Goodtype commit alongside the geometry above in an
addon conformance runner. See `formats/addon-api-v0.md` for the RPC envelopes,
allowlisted response fields, limits and invocation lifetime rules.

Vendored from Goodtype commit `02d4fac`.
