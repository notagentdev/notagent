# notagent-telemetry

Vendor-neutral telemetry contracts: spans, events, attributes, and typed
schema definitions.

The crate defines the `TelemetryContext` and `TelemetrySpan` traits plus two
implementations: an in-memory recorder (`memory.rs`, used by tests and the
conformance suite) and a no-op context (`noop.rs`). There is no exporter and
no global current-span — parent contexts are passed explicitly, and without a
wired-in context all spans are no-ops.

Nothing in this crate performs network I/O.
