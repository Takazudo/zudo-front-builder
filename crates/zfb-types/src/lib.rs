//! Canonical shared data types for the zfb framework.
//!
//! This crate holds types that are used by multiple crates within the zfb
//! workspace, preventing code duplication and circular dependencies.

pub mod asset_urls;
pub mod base_prefix;
pub mod client_scripts;
pub mod helpers;
pub mod segment;

pub use asset_urls::{
    stable_client_script_filename, stable_client_script_relative_path, stable_client_script_url,
    DIST_ASSETS_DIR, DIST_CLIENT_SCRIPTS_DIR, STABLE_ASSETS_URL_PREFIX,
    STABLE_CLIENT_SCRIPTS_URL_PREFIX, STABLE_CSS_FILENAME, STABLE_CSS_URL, STABLE_ISLANDS_FILENAME,
    STABLE_ISLANDS_URL,
};
pub use base_prefix::dev_mount_prefix;
pub use client_scripts::{
    client_script_entry_name, is_client_script_file, CLIENT_SCRIPT_EXTENSIONS, CLIENT_SCRIPT_INFIX,
};
pub use helpers::{escape_html, json_string, normalize_path_lexical, path_to_posix_string};
pub use segment::Segment;
