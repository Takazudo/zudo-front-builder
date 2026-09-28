use std::collections::{BTreeMap, BTreeSet};
use zudo_wind::{CandidateIndex, SourceId, SourceKind};

fn rng(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}
fn id(n: u64) -> SourceId {
    SourceId::new("app", format!("src/{}/file{}.tsx", n % 4, n % 9)).unwrap()
}
fn candidates(state: &mut u64) -> BTreeSet<String> {
    (0..(rng(state) % 5))
        .map(|_| format!("c{}", rng(state) % 12))
        .collect()
}

#[test]
fn seeded_operations_equal_fresh_final_state() {
    for seed in [1_u64, 2, 3, 42, 0xabcdef, 0x123456789abcdef] {
        let mut state = seed;
        let mut index = CandidateIndex::default();
        let mut sources = BTreeMap::new();
        let mut manifests = BTreeMap::new();
        let mut safelists = BTreeMap::new();
        for _ in 0..300 {
            match rng(&mut state) % 6 {
                0 | 1 => {
                    let key = id(rng(&mut state));
                    let set = candidates(&mut state);
                    index.upsert(key.clone(), set.iter().cloned());
                    sources.insert(key, set);
                }
                2 => {
                    let key = id(rng(&mut state));
                    index.remove(&key);
                    sources.remove(&key);
                }
                3 => {
                    let prefix =
                        SourceId::new("app", format!("src/{}", rng(&mut state) % 4)).unwrap();
                    index.remove_prefix(&prefix);
                    sources.retain(|key, _| !key.is_under(&prefix));
                }
                4 => {
                    let owner = format!("m{}", rng(&mut state) % 3);
                    if rng(&mut state).is_multiple_of(4) {
                        index.remove_manifest(&owner);
                        manifests.remove(&owner);
                    } else {
                        let set = candidates(&mut state);
                        index.replace_manifest(owner.clone(), set.iter().cloned());
                        manifests.insert(owner, set);
                    }
                }
                _ => {
                    let owner = format!("s{}", rng(&mut state) % 3);
                    if rng(&mut state).is_multiple_of(4) {
                        index.remove_safelist(&owner);
                        safelists.remove(&owner);
                    } else {
                        let set = candidates(&mut state);
                        index.replace_safelist(owner.clone(), set.iter().cloned());
                        safelists.insert(owner, set);
                    }
                }
            }
        }
        let mut fresh = CandidateIndex::default();
        for (key, set) in sources {
            fresh.upsert(key, set);
        }
        for (key, set) in manifests {
            fresh.replace_manifest(key, set);
        }
        for (key, set) in safelists {
            fresh.replace_safelist(key, set);
        }
        assert_eq!(index.live_set(), fresh.live_set(), "seed {seed}");
        assert_eq!(
            index.reference_counts(),
            fresh.reference_counts(),
            "seed {seed}"
        );
    }
}

#[test]
fn ownership_and_reconciliation_edges() {
    let mut index = CandidateIndex::default();
    let a = SourceId::new("app", "src/a/file.tsx").unwrap();
    let ab = SourceId::new("app", "src/ab/file.tsx").unwrap();
    index.upsert(a.clone(), ["shared".into()]);
    index.upsert(ab.clone(), ["shared".into()]);
    index.upsert(a.clone(), ["shared".into()]);
    assert_eq!(index.reference_counts()["shared"], 2);
    index.remove_prefix(&SourceId::new("app", "src/a").unwrap());
    assert_eq!(index.reference_counts()["shared"], 1);
    index.remove(&a);
    index.replace_safelist("owner", ["shared".into()]);
    index.remove(&ab);
    assert!(index.live_set().contains("shared"));
    assert!(!index.reference_counts().contains_key("shared"));
    index.remove_safelist("owner");
    assert!(index.live_set().is_empty());
    index.replace_manifest("producer", ["old".into()]);
    index.replace_manifest("producer", ["new".into()]);
    assert_eq!(index.live_set(), BTreeSet::from(["new".into()]));
    let result = index.reconcile(
        a.clone(),
        Some(br#"<div className="p-1"/>"#),
        SourceKind::Tsx,
    );
    assert!(result.is_some());
    assert!(index.live_set().contains("p-1"));
    index.reconcile(a, None, SourceKind::Tsx);
    assert!(!index.live_set().contains("p-1"));
}
