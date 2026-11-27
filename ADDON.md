# Connectors

Wire circuit symbols, build flowcharts and link objects on a Goodtype page. Connectors can snap to ports and follow parts when they move.

1. Install Connectors from **Addons** and select **Connect** in the toolbar.
2. Choose straight, curved or orthogonal routing and optionally add arrowheads.
3. Drag between two ports, two free points or a port and a free point.
4. Select an existing connector and pick straight, curved or orthogonal routing in the selection bar.

To turn an existing capped line or open curve into a connector, drag either end dot onto a highlighted port. The drop attaches that end immediately, leaves the other end free and offers Undo. It uses the Connect tool's routing preference. You can also pick a route in the selection bar to adopt the arrow with two free ends. Routing picks follow Goodtype's proposal settings and apply immediately only when both direct-apply settings are enabled.

Drag an existing connector end onto a port to attach it, or release it away from ports to make it free. The other end keeps its attachment while its port is available.

Choose the routing and arrowheads beside Connect on a wide horizontal palette. On a
side dock or in a narrow window, press the active Connect tool again to find those
choices in its settings. The stroke pocket sets the connector's colour and width.
Its tool card sets the pattern, spacing and opacity.

Curved connectors have short straight leads joined by a smooth S-curve. Orthogonal connectors use leads at both ends, usually forming a balanced Z. Aligned free ends stay straight; very short routes use shorter leads.

Deleting a connected part leaves the connector in place and releases that end. Saved connectors remain editable Goodtype shapes. **Detach from Connectors** removes the connection data and keeps the drawing.

Connectors needs permission to edit the objects it owns and react when attached objects change. Routing runs on your device through Goodtype's local addon session.

Connectors 0.3.0 declares `requires: ["end-caps", "path-edit-v0", "scoped-reads-v0"]` in its manifest. It needs a Goodtype host with P3 scoped geometry reads; released v0.3.x hosts reject this field. Legacy heads stay as saved until a successful upgrade. If Goodtype reports that a legacy drawing needs review, you can detach it and edit the saved drawing as an ordinary shape.

Orthogonal connectors route around nearby parts on native release/commit using `scoped-reads-v0`; straight and curved routes stay unchanged. Live Wasm previews have no reads and may differ from the committed route.
