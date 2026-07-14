//! Content-read provenance for dependency-graph `Content` edges.
//!
//! ## Contract
//!
//! This module is the pure boundary between the future render/route-table
//! tracker and the dependency graph. Its primary input is an ordered-free set
//! of [`TrackedContentRead`] observations made while rendering or evaluating
//! `paths()`. A per-entry observation produces an edge to that entry. A
//! collection observation represents an aggregate reader and expands to every
//! member of that collection. That covers index, tag, and pagination routes,
//! plus page- or layout-level reads.
//!
//! Collection membership is supporting input only: it enumerates the entries
//! for an already-tracked aggregate collection read. It is never used to infer
//! that a route read a collection. Persisted dependency-graph state is
//! reconciliation-only and is deliberately not an input to this core; a
//! current render/route-table observation must win over restored state.
//!
//! Conditional reads are represented by the branch actually observed. A
//! conditional branch that did not call a tracked collection API emits no
//! [`TrackedContentRead`] and therefore no edge. Reads through arbitrary user
//! JavaScript or any untracked API are out of scope. Callers must not install
//! partial provenance when [`ContentProvenanceError`] is returned or a read is
//! untracked, and must not use persisted `Content` edges to fill that gap:
//! keep the dependency graph's conservative `All` fallback instead.
//! Over-rendering is acceptable; silently under-rendering is not.
//!
//! Groups are keyed by `(consumer, collection)`, rather than flattened by
//! consumer alone. A refresh can therefore replace one collection's complete
//! edge set while leaving another collection read by the same route intact.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use zfb_graph::PageId;

/// Stable identifier of one configured content collection.
///
/// The value is normally the configured collection name. It is deliberately
/// distinct from a filesystem root because one root can participate in more
/// than one collection configuration with different filters.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentCollectionId(String);

impl ContentCollectionId {
    /// Construct an identifier from a configured collection name.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the configured collection name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for ContentCollectionId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for ContentCollectionId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

/// A complete, explicit membership snapshot for configured collections.
///
/// An inserted collection with no paths is a known empty collection. A
/// collection absent from this map is unavailable, which is materially
/// different: aggregate provenance cannot be safely expanded in that case.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContentCollectionMembership {
    entries: BTreeMap<ContentCollectionId, BTreeSet<PathBuf>>,
}

impl ContentCollectionMembership {
    /// Start an empty membership snapshot.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the known members of one configured collection.
    ///
    /// Supplying an empty iterator records that the collection is known to be
    /// empty; it does not remove the collection from the snapshot.
    pub fn insert(
        &mut self,
        collection: impl Into<ContentCollectionId>,
        entries: impl IntoIterator<Item = PathBuf>,
    ) -> Option<BTreeSet<PathBuf>> {
        self.entries
            .insert(collection.into(), entries.into_iter().collect())
    }

    /// Return the known members of `collection`, or `None` when the
    /// membership walker did not produce a complete snapshot for it.
    pub fn entries(&self, collection: &ContentCollectionId) -> Option<&BTreeSet<PathBuf>> {
        self.entries.get(collection)
    }
}

/// One tracked content API read observed by the render pipeline or route
/// table.
///
/// The `consumer` is the source [`PageId`] that should be dirtied when the
/// observed content changes. It intentionally remains a source id even when a
/// dynamic route expands into many concrete URLs: the dependency graph is
/// source-keyed and the render layer handles URL fan-out separately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackedContentRead {
    /// A route read one concrete entry. This yields an edge to just `entry`.
    Entry {
        /// Source route that consumed the entry.
        consumer: PageId,
        /// Configured collection that owns `entry`.
        collection: ContentCollectionId,
        /// Canonical entry path emitted by the tracked API.
        entry: PathBuf,
    },
    /// A route enumerated a collection. This yields edges to every member.
    ///
    /// The variant applies equally to a post index, tag page, pagination
    /// source, layout, or page-level collection enumeration.
    Collection {
        /// Source route that enumerated the collection.
        consumer: PageId,
        /// Configured collection that was enumerated.
        collection: ContentCollectionId,
    },
}

impl TrackedContentRead {
    /// Construct a tracked per-entry read.
    pub fn entry(
        consumer: impl Into<PageId>,
        collection: impl Into<ContentCollectionId>,
        entry: impl Into<PathBuf>,
    ) -> Self {
        Self::Entry {
            consumer: consumer.into(),
            collection: collection.into(),
            entry: entry.into(),
        }
    }

    /// Construct a tracked collection enumeration.
    pub fn collection(
        consumer: impl Into<PageId>,
        collection: impl Into<ContentCollectionId>,
    ) -> Self {
        Self::Collection {
            consumer: consumer.into(),
            collection: collection.into(),
        }
    }
}

// The tracked read intent for one `(consumer, collection)` pair. It stays
// separate from the expanded entry set so a membership refresh can recompute
// the whole group without mistaking direct-entry reads for aggregate reads.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ContentReadGroup {
    consumer: PageId,
    collection: ContentCollectionId,
    entry_reads: BTreeSet<PathBuf>,
    reads_collection: bool,
}

/// Expanded `DepKind::Content` edge set for one `(consumer, collection)`
/// group.
///
/// An empty `entries` set is meaningful: it records a known-empty aggregate
/// read so a later membership refresh can add its entries without losing the
/// group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentEdgeGroup {
    /// Source route that should receive the content dependencies.
    pub consumer: PageId,
    /// Collection whose membership produced this edge group.
    pub collection: ContentCollectionId,
    /// Entry paths to write as `DepKind::Content` dependencies.
    pub entries: BTreeSet<PathBuf>,
}

/// Aggregated tracked content reads, grouped for collection-scoped refresh.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContentProvenance {
    groups: BTreeMap<(PageId, ContentCollectionId), ContentReadGroup>,
}

impl ContentProvenance {
    /// Group observed reads by `(consumer, collection)`.
    ///
    /// This performs no membership expansion, so the primary read evidence
    /// stays distinct from the supporting membership snapshot.
    pub fn from_reads(reads: impl IntoIterator<Item = TrackedContentRead>) -> Self {
        let mut provenance = Self::default();
        for read in reads {
            match read {
                TrackedContentRead::Entry {
                    consumer,
                    collection,
                    entry,
                } => {
                    let group = provenance.group_mut(consumer, collection);
                    group.entry_reads.insert(entry);
                }
                TrackedContentRead::Collection {
                    consumer,
                    collection,
                } => {
                    let group = provenance.group_mut(consumer, collection);
                    group.reads_collection = true;
                }
            }
        }
        provenance
    }

    /// Whether no tracked content API reads were observed.
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// Expand every tracked group into collection-scoped content edges.
    ///
    /// The result is all-or-nothing. Missing membership or a direct entry that
    /// does not belong to its declared collection returns an error rather than
    /// exposing a partial edge set for a caller to accidentally install.
    pub fn edge_groups(
        &self,
        membership: &ContentCollectionMembership,
    ) -> Result<Vec<ContentEdgeGroup>, ContentProvenanceError> {
        self.groups
            .values()
            .map(|group| group.expand(membership))
            .collect()
    }

    /// Expand only one collection's groups for a collection-scoped refresh.
    ///
    /// A caller can replace the returned groups wholesale after a membership
    /// walk without touching groups for other collections.
    pub fn edge_groups_for_collection(
        &self,
        collection: &ContentCollectionId,
        membership: &ContentCollectionMembership,
    ) -> Result<Vec<ContentEdgeGroup>, ContentProvenanceError> {
        self.groups
            .values()
            .filter(|group| &group.collection == collection)
            .map(|group| group.expand(membership))
            .collect()
    }

    fn group_mut(
        &mut self,
        consumer: PageId,
        collection: ContentCollectionId,
    ) -> &mut ContentReadGroup {
        self.groups
            .entry((consumer.clone(), collection.clone()))
            .or_insert_with(|| ContentReadGroup {
                consumer,
                collection,
                entry_reads: BTreeSet::new(),
                reads_collection: false,
            })
    }
}

impl ContentReadGroup {
    fn expand(
        &self,
        membership: &ContentCollectionMembership,
    ) -> Result<ContentEdgeGroup, ContentProvenanceError> {
        let members = membership.entries(&self.collection).ok_or_else(|| {
            ContentProvenanceError::MissingCollectionMembership {
                consumer: self.consumer.clone(),
                collection: self.collection.clone(),
            }
        })?;

        for entry in &self.entry_reads {
            if !members.contains(entry) {
                return Err(ContentProvenanceError::EntryOutsideCollectionMembership {
                    consumer: self.consumer.clone(),
                    collection: self.collection.clone(),
                    entry: entry.clone(),
                });
            }
        }

        let mut entries = self.entry_reads.clone();
        if self.reads_collection {
            entries.extend(members.iter().cloned());
        }

        Ok(ContentEdgeGroup {
            consumer: self.consumer.clone(),
            collection: self.collection.clone(),
            entries,
        })
    }
}

/// A provenance input was incomplete or internally inconsistent.
///
/// Callers must preserve the conservative dependency-graph fallback instead
/// of installing any subset of the corresponding content edges. A persisted
/// `Content` edge is not a substitute for a failed current derivation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContentProvenanceError {
    /// An observed read named a collection that the membership walk did not
    /// explicitly report. This is distinct from a known empty collection.
    #[error(
        "content consumer {consumer:?} read collection {collection:?}, but its membership is unavailable"
    )]
    MissingCollectionMembership {
        /// Source route that made the read.
        consumer: PageId,
        /// Collection whose membership was unavailable.
        collection: ContentCollectionId,
    },
    /// A direct-entry observation did not agree with the current membership
    /// snapshot for its declared collection.
    #[error(
        "content consumer {consumer:?} read entry {entry:?} outside collection {collection:?}"
    )]
    EntryOutsideCollectionMembership {
        /// Source route that made the read.
        consumer: PageId,
        /// Collection declared by the tracked API.
        collection: ContentCollectionId,
        /// Entry that was absent from the collection membership snapshot.
        entry: PathBuf,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(path: &str) -> PageId {
        PageId::new(path)
    }

    fn path(value: &str) -> PathBuf {
        PathBuf::from(value)
    }

    fn collection(value: &str) -> ContentCollectionId {
        ContentCollectionId::new(value)
    }

    fn members(
        collections: impl IntoIterator<Item = (&'static str, Vec<&'static str>)>,
    ) -> ContentCollectionMembership {
        let mut membership = ContentCollectionMembership::new();
        for (name, entries) in collections {
            membership.insert(collection(name), entries.into_iter().map(path));
        }
        membership
    }

    #[test]
    fn per_entry_reader_edges_only_its_observed_entry() {
        let alpha = path("/project/content/posts/alpha.mdx");
        let beta = path("/project/content/posts/beta.mdx");
        let membership = members([(
            "posts",
            vec![
                "/project/content/posts/alpha.mdx",
                "/project/content/posts/beta.mdx",
            ],
        )]);
        let provenance = ContentProvenance::from_reads([TrackedContentRead::entry(
            page("pages/posts/[slug].tsx"),
            collection("posts"),
            alpha.clone(),
        )]);

        assert_eq!(
            provenance.edge_groups(&membership).unwrap(),
            vec![ContentEdgeGroup {
                consumer: page("pages/posts/[slug].tsx"),
                collection: collection("posts"),
                entries: BTreeSet::from([alpha]),
            }]
        );
        assert!(!provenance.edge_groups(&membership).unwrap()[0]
            .entries
            .contains(&beta));
    }

    #[test]
    fn aggregate_reader_expands_each_collection_it_reads() {
        let membership = members([
            (
                "posts",
                vec![
                    "/project/content/posts/alpha.mdx",
                    "/project/content/posts/beta.mdx",
                ],
            ),
            ("docs", vec!["/project/content/docs/intro.mdx"]),
        ]);
        let provenance = ContentProvenance::from_reads([
            // A route may read one entry while also enumerating the same
            // collection; both observations must coalesce into one group.
            TrackedContentRead::entry(
                page("pages/index.tsx"),
                collection("posts"),
                path("/project/content/posts/alpha.mdx"),
            ),
            TrackedContentRead::collection(page("pages/index.tsx"), collection("posts")),
            TrackedContentRead::collection(page("pages/index.tsx"), collection("docs")),
            TrackedContentRead::collection(page("pages/tags/[tag].tsx"), collection("posts")),
        ]);

        assert_eq!(
            provenance.edge_groups(&membership).unwrap(),
            vec![
                ContentEdgeGroup {
                    consumer: page("pages/index.tsx"),
                    collection: collection("docs"),
                    entries: BTreeSet::from([path("/project/content/docs/intro.mdx")]),
                },
                ContentEdgeGroup {
                    consumer: page("pages/index.tsx"),
                    collection: collection("posts"),
                    entries: BTreeSet::from([
                        path("/project/content/posts/alpha.mdx"),
                        path("/project/content/posts/beta.mdx"),
                    ]),
                },
                ContentEdgeGroup {
                    consumer: page("pages/tags/[tag].tsx"),
                    collection: collection("posts"),
                    entries: BTreeSet::from([
                        path("/project/content/posts/alpha.mdx"),
                        path("/project/content/posts/beta.mdx"),
                    ]),
                },
            ]
        );
    }

    #[test]
    fn pagination_reader_edges_every_entry_not_only_its_current_page() {
        let membership = members([(
            "posts",
            vec![
                "/project/content/posts/alpha.mdx",
                "/project/content/posts/beta.mdx",
                "/project/content/posts/gamma.mdx",
            ],
        )]);
        let provenance = ContentProvenance::from_reads([TrackedContentRead::collection(
            page("pages/posts/page/[page].tsx"),
            collection("posts"),
        )]);

        assert_eq!(
            provenance.edge_groups(&membership).unwrap()[0].entries,
            BTreeSet::from([
                path("/project/content/posts/alpha.mdx"),
                path("/project/content/posts/beta.mdx"),
                path("/project/content/posts/gamma.mdx"),
            ]),
            "pagination may render a slice, but its route source must be invalidated by every collection member"
        );
    }

    #[test]
    fn conditional_collection_reads_only_exist_for_the_observed_branch() {
        let membership = members([("posts", vec!["/project/content/posts/alpha.mdx"])]);
        let read_enabled = false;
        let reads = if read_enabled {
            vec![TrackedContentRead::collection(
                page("pages/conditional.tsx"),
                collection("posts"),
            )]
        } else {
            Vec::new()
        };
        let provenance = ContentProvenance::from_reads(reads);

        assert!(provenance.is_empty());
        assert!(provenance.edge_groups(&membership).unwrap().is_empty());

        let observed = ContentProvenance::from_reads([TrackedContentRead::collection(
            page("pages/conditional.tsx"),
            collection("posts"),
        )]);
        assert_eq!(
            observed.edge_groups(&membership).unwrap()[0].entries,
            BTreeSet::from([path("/project/content/posts/alpha.mdx")])
        );
    }

    #[test]
    fn known_empty_collection_retains_group_for_later_wholesale_refresh() {
        let provenance = ContentProvenance::from_reads([TrackedContentRead::collection(
            page("pages/posts/index.tsx"),
            collection("posts"),
        )]);
        let empty = members([("posts", vec![])]);

        assert_eq!(
            provenance.edge_groups(&empty).unwrap(),
            vec![ContentEdgeGroup {
                consumer: page("pages/posts/index.tsx"),
                collection: collection("posts"),
                entries: BTreeSet::new(),
            }],
            "an aggregate read of a known-empty collection must survive for a later membership refresh"
        );

        let refreshed = members([("posts", vec!["/project/content/posts/new.mdx"])]);
        assert_eq!(
            provenance
                .edge_groups_for_collection(&collection("posts"), &refreshed)
                .unwrap()[0]
                .entries,
            BTreeSet::from([path("/project/content/posts/new.mdx")])
        );
    }

    #[test]
    fn collection_refresh_is_scoped_without_disturbing_other_groups() {
        let provenance = ContentProvenance::from_reads([
            TrackedContentRead::collection(page("pages/index.tsx"), collection("posts")),
            TrackedContentRead::collection(page("pages/index.tsx"), collection("docs")),
        ]);
        let membership = members([
            ("posts", vec!["/project/content/posts/next.mdx"]),
            ("docs", vec!["/project/content/docs/intro.mdx"]),
        ]);

        assert_eq!(
            provenance
                .edge_groups_for_collection(&collection("posts"), &membership)
                .unwrap(),
            vec![ContentEdgeGroup {
                consumer: page("pages/index.tsx"),
                collection: collection("posts"),
                entries: BTreeSet::from([path("/project/content/posts/next.mdx")]),
            }]
        );
        assert_eq!(
            provenance.edge_groups(&membership).unwrap()[0],
            ContentEdgeGroup {
                consumer: page("pages/index.tsx"),
                collection: collection("docs"),
                entries: BTreeSet::from([path("/project/content/docs/intro.mdx")]),
            },
            "refreshing posts must retain the independent docs group"
        );
    }

    #[test]
    fn incomplete_membership_returns_an_error_without_partial_edges() {
        let membership = members([("posts", vec!["/project/content/posts/alpha.mdx"])]);
        let consumer = page("pages/index.tsx");
        let provenance = ContentProvenance::from_reads([
            TrackedContentRead::collection(consumer.clone(), collection("posts")),
            TrackedContentRead::collection(consumer.clone(), collection("drafts")),
        ]);

        assert_eq!(
            provenance.edge_groups(&membership),
            Err(ContentProvenanceError::MissingCollectionMembership {
                consumer,
                collection: collection("drafts"),
            })
        );
    }

    #[test]
    fn direct_entry_outside_membership_is_rejected() {
        let membership = members([("posts", vec!["/project/content/posts/alpha.mdx"])]);
        let consumer = page("pages/posts/[slug].tsx");
        let entry = path("/project/content/posts/untracked.mdx");
        let provenance = ContentProvenance::from_reads([TrackedContentRead::entry(
            consumer.clone(),
            collection("posts"),
            entry.clone(),
        )]);

        assert_eq!(
            provenance.edge_groups(&membership),
            Err(ContentProvenanceError::EntryOutsideCollectionMembership {
                consumer,
                collection: collection("posts"),
                entry,
            })
        );
    }
}
