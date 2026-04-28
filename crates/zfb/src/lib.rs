//! zfb — zudo-front-builder library crate.
//!
//! This crate exposes the CLI parser, command dispatchers, configuration
//! loader, and structured output helpers used by the `zfb` binary.

pub mod cli;
pub mod commands;
pub mod config;
pub mod diagnostics;
pub mod output;
pub mod render_pipeline;
