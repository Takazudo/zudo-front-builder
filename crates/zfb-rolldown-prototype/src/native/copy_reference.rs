//! Isolated upstream-private compatibility for builtin CopyModulePlugin.
//! Pinned source: crates/rolldown_plugin_copy_module/src/lib.rs (24bc2d0).
//! ModuleInfo has no typed copied-asset edge. Never guess a source from a name.
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
pub(super) fn reference(id: &str) -> Option<&str> {
    id.strip_prefix("__ROLLDOWN_COPY_MODULE__#")
}
pub(super) fn output_name<'a>(
    id: &'a str,
    outputs: &'a BTreeMap<String, String>,
) -> Result<&'a str> {
    if reference(id).is_some() {
        Ok(outputs
            .get(id)
            .context("copy chunk import lacks emitted filename")?
            .as_str())
    } else {
        Ok(id)
    }
}
pub(super) fn restore_sources(
    inputs: &mut BTreeMap<String, Value>,
    copied_sources: &BTreeMap<String, Vec<String>>,
) -> Result<()> {
    for input in inputs.values_mut() {
        for import in input["imports"]
            .as_array_mut()
            .context("resolved imports")?
        {
            if let Some(filename) = import
                .get("zfbCopiedOutput")
                .and_then(Value::as_str)
                .map(str::to_string)
            {
                let sources = copied_sources
                    .get(&filename)
                    .context("copy output lacks native provenance")?;
                if sources.len() != 1 {
                    bail!("copy output {filename} has ambiguous source provenance");
                }
                import["path"] = json!(sources[0]);
                import.as_object_mut().unwrap().remove("zfbCopiedOutput");
            }
        }
    }
    Ok(())
}
