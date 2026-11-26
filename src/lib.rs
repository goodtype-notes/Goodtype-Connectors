//! Goodtype Connectors: lines that stay attached to parts, as an addon process.
pub mod addon;
pub mod connector;
pub mod geometry;
pub mod route;
pub mod session;

#[cfg(target_arch = "wasm32")]
mod wasm;
