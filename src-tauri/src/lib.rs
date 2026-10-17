//! Library half of the crate.
//!
//! This exists for `core` (shared by the native binary and the WASM bridge)
//! and for the `wasm32` bridge exports below. The desktop entry point lives in
//! `main.rs` as `native_app::run()`, which is what `main()` actually calls —
//! there is deliberately no second entry point here. An earlier `run()` in this
//! file was never called, and its `#[cfg_attr(mobile, tauri::mobile_entry_point)]`
//! annotation made a dead function look like the mobile entry point while the
//! live one lacked the annotation.

pub mod core;

#[cfg(target_arch = "wasm32")]
mod wasm_bridge;

#[cfg(target_arch = "wasm32")]
pub use wasm_bridge::*;

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_bridge_tests;
