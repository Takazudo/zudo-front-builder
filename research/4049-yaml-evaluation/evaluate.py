#!/usr/bin/env python3
"""Cheap evaluation controls only. Never runs Cargo or installs a toolchain."""
import argparse
import datetime
import hashlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tomllib
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
FROZEN = json.loads((HERE / 'provenance.json').read_text())
PAIR = ('noyalib', 'noyalib-serde-yaml')
PROTECTED = [
    'Cargo.toml', 'Cargo.lock', 'deny.toml',
    'crates/zfb-content/tests/yaml_differential_harness.rs',
    'crates/zfb-content/tests/fixtures/yaml/serde_yaml_corpus.json',
    'crates/zfb-content/tests/fixtures/yaml/serde_yaml_baseline.json',
    'crates/zfb-content/tests/error_messages.rs',
    'crates/zfb-md-wasm/tests/api.rs', 'crates/zfb-md-wasm/tests/parse_to_ast.rs',
    'crates/zfb-md-wasm/shipped-sizes.json',
    'scripts/yaml-candidate-baseline.json',
    'crates/zfb-content/src/frontmatter.rs',
    'crates/zfb/src/diagnostics.rs', 'crates/zfb-md-wasm/src/lib.rs',
]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def fetch(url):
    return urllib.request.urlopen(urllib.request.Request(
        url, headers={'User-Agent': 'zfb-4049-evaluation'}), timeout=60).read()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['snapshot', 'pin', 'lock', 'restore', 'verify', 'disk', 'registry'])
    parser.add_argument('--evidence', type=Path, required=True)
    args = parser.parse_args()
    evidence = args.evidence.resolve()
    assert not evidence.is_relative_to(ROOT), 'keep raw evidence outside the worktree'
    if args.action == 'snapshot':
        evidence.mkdir(parents=True, exist_ok=True)
        assert not (evidence / 'snapshot.json').exists(), 'never overwrite initial evidence'
        state = {path: digest(ROOT / path) for path in PROTECTED}
        for path in ['Cargo.toml', 'Cargo.lock', 'crates/zfb-md-wasm/shipped-sizes.json']:
            dest = evidence / path
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / path, dest)
        corpus = json.loads((ROOT / PROTECTED[4]).read_text())
        baseline = json.loads((ROOT / PROTECTED[5]).read_text())
        assert len(corpus['cases']) == len(baseline['cases']) == 18
        assert [c['name'] for c in corpus['cases']] == [c['name'] for c in baseline['cases']]
        assert '=0.0.44' in (ROOT / 'Cargo.toml').read_text()
        (evidence / 'snapshot.json').write_text(json.dumps({
            'sha': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
            'hashes': state, 'caseNames': [c['name'] for c in corpus['cases']],
        }, indent=2) + '\n')
    elif args.action == 'pin':
        initial = json.loads((evidence / 'snapshot.json').read_text())
        assert digest(ROOT / 'Cargo.toml') == initial['hashes']['Cargo.toml']
        path = ROOT / 'Cargo.toml'
        text = path.read_text()
        old = 'serde_yaml = { package = "noyalib-serde-yaml", version = "=0.0.44" }'
        assert text.count(old) == 1
        path.write_text(text.replace(old, old.replace('0.0.44', '0.0.55')))
    elif args.action == 'lock':
        before = tomllib.loads((evidence / 'Cargo.lock').read_text())['package']
        after = tomllib.loads((ROOT / 'Cargo.lock').read_text())['package']
        assert len(before) == len(after), 'package count changed; review before compiling'
        assert [p for p in before if p['name'] not in PAIR] == [p for p in after if p['name'] not in PAIR], 'unrelated lock delta'
        for name in PAIR:
            old = next(p for p in before if p['name'] == name)
            new = next(p for p in after if p['name'] == name)
            assert new['version'] == '0.0.55'
            assert new['checksum'] == FROZEN['crates'][name]['checksum']
            assert {k: v for k, v in old.items() if k not in ['version', 'checksum']} == {k: v for k, v in new.items() if k not in ['version', 'checksum']}
        print('PASS: frozen checksums before build; exact two-entry delta; packages=', len(after))
    elif args.action == 'restore':
        for path in ['Cargo.toml', 'Cargo.lock', 'crates/zfb-md-wasm/shipped-sizes.json']:
            shutil.copyfile(evidence / path, ROOT / path)
    elif args.action == 'verify':
        state = json.loads((evidence / 'snapshot.json').read_text())
        for path, expected in state['hashes'].items():
            assert digest(ROOT / path) == expected, f'not restored: {path}'
        print('PASS: all protected files byte-identical')
    elif args.action == 'disk':
        free = shutil.disk_usage(ROOT).free
        print(json.dumps({'checkedAt': datetime.datetime.now(datetime.timezone.utc).isoformat(), 'freeBytes': free, 'minimumBytes': 30 * 1024**3}))
        assert free >= 30 * 1024**3, 'Phase 2 blocked: under unchanged 30 GiB disk gate'
    elif args.action == 'registry':
        result = {'checkedAt': datetime.datetime.now(datetime.timezone.utc).isoformat(), 'crates': {}}
        for name in PAIR:
            records = json.loads(fetch('https://crates.io/api/v1/crates/' + name))['versions']
            record = next(v for v in records if v['num'] == '0.0.55')
            assert not record['yanked']
            assert record['checksum'] == FROZEN['crates'][name]['checksum']
            archive = fetch(f'https://static.crates.io/crates/{name}/{name}-0.0.55.crate')
            assert hashlib.sha256(archive).hexdigest() == record['checksum']
            with tarfile.open(fileobj=io.BytesIO(archive), mode='r:gz') as tar:
                manifest = tomllib.loads(tar.extractfile(f'{name}-0.0.55/Cargo.toml').read().decode())
                if name == PAIR[1]:
                    assert manifest['dependencies'] == {'noyalib': {'version': '=0.0.55', 'features': ['std', 'compat-serde-yaml'], 'default-features': False}}
            newer = [v['num'] for v in records if not v['yanked'] and '-' not in v['num'] and tuple(map(int, v['num'].split('.'))) > (0, 0, 55)]
            result['crates'][name] = {'record': record, 'newerNonYanked': newer, 'archiveSha256': hashlib.sha256(archive).hexdigest(), 'skipped': [{k: v[k] for k in ['num', 'created_at', 'yanked']} for v in records if '-' not in v['num'] and (0, 0, 44) < tuple(map(int, v['num'].split('.'))) < (0, 0, 55)]}
            adopted = next(v for v in records if v['num'] == '0.0.44')
            assert not adopted['yanked'], 'adopted baseline yanked: reconcile trigger'
        complete = sorted(set(result['crates'][PAIR[0]]['newerNonYanked']) & set(result['crates'][PAIR[1]]['newerNonYanked']))
        result['newerCompletePairs'] = complete
        print(json.dumps(result, indent=2))
        assert not complete, 'newer complete pair: reselect before verdict; record incomplete half-pairs separately'


if __name__ == '__main__':
    main()
