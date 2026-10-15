//! Isolated upstream-private compatibility for builtin CopyModulePlugin.
//! Pinned source: crates/rolldown_plugin_copy_module/src/lib.rs (24bc2d0).
//! ModuleInfo has no typed copied-asset edge. Never guess a source from a name.
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
const RESERVED: &str = "__ROLLDOWN_COPY_MODULE";
pub(super) fn reference(id: &str) -> Result<Option<&str>> {
    if let Some(reference) = id.strip_prefix("__ROLLDOWN_COPY_MODULE__#") {
        ensure!(!reference.is_empty(), "empty upstream copy reference");
        Ok(Some(reference))
    } else {
        ensure!(
            !id.contains(RESERVED),
            "unknown upstream copy identifier: {id}"
        );
        Ok(None)
    }
}
pub(super) fn output_name<'a>(
    id: &'a str,
    outputs: &'a BTreeMap<String, String>,
) -> Result<&'a str> {
    if reference(id)?.is_some() {
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
            let path = import["path"].as_str().context("copy edge path")?;
            let copied = reference(path)?.is_some();
            ensure!(
                copied == import.get("zfbCopiedOutput").is_some(),
                "copy edge marker/reference mismatch: {path}"
            );
            if copied {
                let filename = import["zfbCopiedOutput"]
                    .as_str()
                    .context("copy output marker must be a filename string")?;
                let sources = copied_sources
                    .get(filename)
                    .context("copy output lacks native provenance")?;
                if sources.len() != 1 || sources[0].is_empty() {
                    bail!("copy output {filename} has ambiguous or empty source provenance");
                }
                import["path"] = json!(sources[0]);
                import.as_object_mut().unwrap().remove("zfbCopiedOutput");
            }
        }
    }
    Ok(())
}

/// Validate identifier fields only; source text and diagnostic strings are not IDs.
pub(super) fn validate_metadata(
    inputs: &BTreeMap<String, Value>,
    outputs: &BTreeMap<String, Value>,
) -> Result<()> {
    fn id(value: &str) -> Result<()> {
        ensure!(
            !value.contains(RESERVED),
            "residual upstream copy identifier: {value}"
        );
        Ok(())
    }
    for (name, entry) in inputs.iter().chain(outputs) {
        id(name)?;
        if let Some(imports) = entry.get("imports") {
            for import in imports.as_array().context("metadata imports")? {
                id(import["path"].as_str().context("metadata import path")?)?;
                ensure!(
                    import.get("zfbCopiedOutput").is_none(),
                    "residual copy output marker"
                );
            }
        }
        if let Some(provenance) = entry.get("inputs") {
            for source in provenance
                .as_object()
                .context("metadata provenance")?
                .keys()
            {
                id(source)?;
            }
        }
        if let Some(entrypoint) = entry.get("entryPoint").and_then(Value::as_str) {
            id(entrypoint)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const COPY: &str = "__ROLLDOWN_COPY_MODULE__#reference";
    fn graph(edge: Value) -> BTreeMap<String, Value> {
        BTreeMap::from([("entry.ts".into(), json!({"imports":[edge]}))])
    }
    #[test]
    fn restores_authoritative_source_and_preserves_ordinary_edges() {
        let mut inputs =
            graph(json!({"path":COPY,"zfbCopiedOutput":"asset.wasm","external":false}));
        restore_sources(
            &mut inputs,
            &BTreeMap::from([("asset.wasm".into(), vec!["/stage/source.wasm".into()])]),
        )
        .unwrap();
        assert_eq!(
            inputs["entry.ts"]["imports"][0],
            json!({"path":"/stage/source.wasm","external":false})
        );
        let mut ordinary = graph(json!({"path":"external-pkg","external":true}));
        let original = ordinary.clone();
        restore_sources(&mut ordinary, &BTreeMap::new()).unwrap();
        assert_eq!(ordinary, original);
    }
    #[test]
    fn rejects_missing_ambiguous_and_empty_provenance() {
        for sources in [
            None,
            Some(vec![]),
            Some(vec!["".into()]),
            Some(vec!["a".into(), "b".into()]),
        ] {
            let mut inputs = graph(json!({"path":COPY,"zfbCopiedOutput":"asset.wasm"}));
            let provenance = sources
                .map(|v| BTreeMap::from([("asset.wasm".into(), v)]))
                .unwrap_or_default();
            assert!(restore_sources(&mut inputs, &provenance).is_err());
        }
        assert!(output_name(COPY, &BTreeMap::new()).is_err());
    }
    #[test]
    fn rejects_malformed_and_drifted_copy_identifiers() {
        for value in [
            "__ROLLDOWN_COPY_MODULE__#",
            "__ROLLDOWN_COPY_MODULE_V2__#ref",
        ] {
            assert!(reference(value).is_err());
            assert!(output_name(value, &BTreeMap::new()).is_err());
        }
        for edge in [
            json!({"path":COPY}),
            json!({"path":COPY,"zfbCopiedOutput":7}),
            json!({"path":"ordinary.ts","zfbCopiedOutput":"asset.wasm"}),
        ] {
            assert!(restore_sources(&mut graph(edge), &BTreeMap::new()).is_err());
        }
    }
    #[test]
    fn rejects_private_identifiers_in_all_published_locations() {
        let empty = BTreeMap::new();
        let cases = [
            BTreeMap::from([(COPY.into(), json!({}))]),
            graph(json!({"path":COPY})),
            BTreeMap::from([("out.mjs".into(), json!({"inputs":{COPY:{}}}))]),
            BTreeMap::from([("out.mjs".into(), json!({"entryPoint":COPY}))]),
            graph(json!({"path":"ordinary.ts","zfbCopiedOutput":"asset"})),
        ];
        for metadata in cases {
            assert!(validate_metadata(&metadata, &empty).is_err());
            assert!(validate_metadata(&empty, &metadata).is_err());
        }
        validate_metadata(
            &BTreeMap::from([(
                "entry.ts".into(),
                json!({"code":COPY,"imports":[{"path":"ordinary.ts","external":true}]}),
            )]),
            &empty,
        )
        .unwrap();
    }
}
