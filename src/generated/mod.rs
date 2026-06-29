//! Build-time-regenerated PDK config bindings.
//!
//! `make build` (which runs `cargo anypoint config-gen`) overwrites
//! `config.rs` from `definition/gcl.yaml`. The placeholder shipped in
//! this repo lets `cargo check` succeed before the first build.
pub mod config;
