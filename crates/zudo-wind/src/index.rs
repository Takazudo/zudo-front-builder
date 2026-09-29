//! Reference-counted candidate ownership across files, manifests and safelist owners.
use crate::{extract_candidates, ExpandedFile, ExtractionResult, SourceId, SourceKind};
use std::collections::{BTreeMap, BTreeSet};
use std::io;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CandidateIndex {
    sources: BTreeMap<SourceId, BTreeSet<String>>,
    manifests: BTreeMap<String, BTreeSet<String>>,
    safelist: BTreeMap<String, BTreeSet<String>>,
    counts: BTreeMap<String, usize>,
}

impl CandidateIndex {
    fn subtract(&mut self, old: BTreeSet<String>) {
        for candidate in old {
            if let Some(count) = self.counts.get_mut(&candidate) {
                *count -= 1;
                if *count == 0 {
                    self.counts.remove(&candidate);
                }
            }
        }
    }
    fn add(&mut self, new: &BTreeSet<String>) {
        for candidate in new {
            *self.counts.entry(candidate.clone()).or_default() += 1;
        }
    }
    pub fn upsert(&mut self, source: SourceId, candidates: impl IntoIterator<Item = String>) {
        let new: BTreeSet<_> = candidates.into_iter().collect();
        if self.sources.get(&source) == Some(&new) {
            return;
        }
        if let Some(old) = self.sources.insert(source, new.clone()) {
            self.subtract(old);
        }
        self.add(&new);
    }
    pub fn remove(&mut self, source: &SourceId) {
        if let Some(old) = self.sources.remove(source) {
            self.subtract(old);
        }
    }
    pub fn remove_prefix(&mut self, directory: &SourceId) {
        let ids: Vec<_> = self
            .sources
            .keys()
            .filter(|id| id.is_under(directory))
            .cloned()
            .collect();
        for id in ids {
            self.remove(&id);
        }
    }
    pub fn replace_manifest(
        &mut self,
        producer: impl Into<String>,
        candidates: impl IntoIterator<Item = String>,
    ) {
        let producer = producer.into();
        let new: BTreeSet<_> = candidates.into_iter().collect();
        if self.manifests.get(&producer) == Some(&new) {
            return;
        }
        if let Some(old) = self.manifests.insert(producer, new.clone()) {
            self.subtract(old);
        }
        self.add(&new);
    }
    pub fn remove_manifest(&mut self, producer: &str) {
        if let Some(old) = self.manifests.remove(producer) {
            self.subtract(old);
        }
    }
    pub fn replace_safelist(
        &mut self,
        owner: impl Into<String>,
        candidates: impl IntoIterator<Item = String>,
    ) {
        let owner = owner.into();
        let new: BTreeSet<_> = candidates.into_iter().collect();
        self.safelist.insert(owner, new);
    }
    pub fn remove_safelist(&mut self, owner: &str) {
        self.safelist.remove(owner);
    }
    /// Reconcile from observed existence, regardless of watcher event kind.
    pub fn reconcile(
        &mut self,
        source: SourceId,
        bytes: Option<&[u8]>,
        kind: SourceKind,
    ) -> Option<ExtractionResult> {
        match bytes {
            Some(bytes) => {
                let result = extract_candidates(bytes, kind);
                self.upsert(source, result.candidates.iter().map(|c| c.text.clone()));
                Some(result)
            }
            None => {
                self.remove(&source);
                None
            }
        }
    }

    /// Read and extract a file returned by `expand_file_set`, using the same
    /// existence-keyed path as an incremental update.
    pub fn index_file(&mut self, file: &ExpandedFile) -> io::Result<ExtractionResult> {
        let kind = match file.path.extension().and_then(|part| part.to_str()) {
            Some("tsx") => SourceKind::Tsx,
            Some("ts") => SourceKind::Ts,
            Some("jsx") => SourceKind::Jsx,
            Some("js") => SourceKind::Js,
            Some("mjs") => SourceKind::Mjs,
            Some("mdx") => SourceKind::Mdx,
            Some("md") => SourceKind::Md,
            Some("html") => SourceKind::Html,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "unsupported source extension",
                ))
            }
        };
        let bytes = std::fs::read(&file.path)?;
        Ok(self
            .reconcile(file.id.clone(), Some(&bytes), kind)
            .expect("present bytes extract"))
    }
    pub fn reference_counts(&self) -> &BTreeMap<String, usize> {
        &self.counts
    }
    pub fn live_set(&self) -> BTreeSet<String> {
        self.counts
            .keys()
            .cloned()
            .chain(self.safelist.values().flat_map(|set| set.iter().cloned()))
            .collect()
    }
}
