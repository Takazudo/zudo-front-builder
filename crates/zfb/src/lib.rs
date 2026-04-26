//! zfb — zudo-front-builder library crate.
//!
//! This crate exposes the CLI parser, command dispatchers, configuration loader,
//! and structured output helpers used by the `zfb` binary.
//!
//! Wave 1 (this scaffolding) lands the module skeleton, clap argument structs,
//! and stub command handlers. Wave 2 fills in the actual command logic, the
//! config loader (`config`), and the output helpers (`output`).

pub mod cli;
pub mod commands;
pub mod config;
pub mod output;
