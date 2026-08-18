//! Canonical shared data types for the zfb framework.
//!
//! This crate holds types that are used by multiple crates within the zfb
//! workspace, preventing code duplication and circular dependencies.

pub mod asset_urls;
pub mod audit_eligibility;
pub mod base_prefix;
pub mod client_scripts;
pub mod first_party;
pub mod helpers;
pub mod module_workers;
pub mod page_extensions;
pub mod page_privacy;
pub mod render_region;
pub mod segment;

pub use asset_urls::{
    stable_client_script_filename, stable_client_script_relative_path, stable_client_script_url,
    DIST_ASSETS_DIR, DIST_CLIENT_SCRIPTS_DIR, STABLE_ASSETS_URL_PREFIX,
    STABLE_CLIENT_SCRIPTS_URL_PREFIX, STABLE_CSS_FILENAME, STABLE_CSS_URL, STABLE_ISLANDS_FILENAME,
    STABLE_ISLANDS_URL,
};
pub use audit_eligibility::{stage_escape_audit_eligibility, AuditEligibility};
pub use base_prefix::dev_mount_prefix;
pub use client_scripts::{
    client_script_entry_name, is_client_script_file, CLIENT_SCRIPT_EXTENSIONS, CLIENT_SCRIPT_INFIX,
};
pub use first_party::{claimed_workspace_member_names, first_party_root_for};
pub use helpers::{
    escape_html, has_node_modules_segment, json_string, normalize_path_lexical,
    path_to_posix_string,
};
pub use module_workers::{
    module_worker_content_hash, module_worker_filename, module_worker_filename_scoped,
    module_worker_url_specifier, module_worker_url_specifier_scoped, ModuleWorkerPathError,
    MODULE_WORKER_CSP_GLOB, MODULE_WORKER_FILENAME_PREFIX,
};
pub use page_extensions::{is_page_sidecar_file, ROUTABLE_PAGE_EXTENSIONS, SCRIPT_PAGE_EXTENSIONS};
pub use page_privacy::path_has_private_prefix_component;
pub use render_region::{
    render_region_marker, RenderRegionEdge, MARKER_HEAD, MARKER_ID_ATTR, MARKER_KIND_END,
    MARKER_KIND_START, MARKER_TAIL, REGION_ID_ATTR, RENDER_REGION_ATTR,
    RENDER_REGION_MARKER_PARITY_FIXTURE,
};
pub use segment::Segment;
