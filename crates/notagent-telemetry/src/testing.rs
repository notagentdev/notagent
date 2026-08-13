//! Runner-unabhängige Konformanzfälle für Telemetrie-Adapter.
//!
//! 1:1-Port von `packages/telemetry/src/testing/` (index.ts 6, types.ts 18,
//! conformance.ts 315).

pub mod conformance;
pub mod types;

pub use conformance::{TelemetryAdapterConformanceCase, create_telemetry_adapter_conformance};
pub use types::{TelemetryAdapterFixture, TelemetryAdapterFixtureFactory};
