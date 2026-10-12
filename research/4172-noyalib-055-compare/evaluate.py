#!/usr/bin/env python3
"""#4172 evaluation controls. Never runs Cargo; compare.sh owns builds.

Python 3.9 compatible (macOS system python): no tomllib, no match.
"""
import argparse
import datetime
import hashlib
import io
import json
import math
import os
import re
import resource
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
FROZEN = json.loads((HERE / 'provenance.json').read_text())
PAIR = ('noyalib', 'noyalib-serde-yaml')
CONTROL = FROZEN['control']
CANDIDATE = FROZEN['candidate']
PIN_LINE = 'serde_yaml = { package = "noyalib-serde-yaml", version = "=%s" }'

# Unchanged hard ceilings and the CI-measured 0.0.53 row
# (crates/zfb-md-wasm/shipped-sizes.json at badb6836, PR #4160 reconciliation).
CEILINGS = {'default': 1600000, 'highlight-only': 880000, 'render-only': 1100000,
            'parse-only': 325000, 'tarball': 3900000}
CI_CONTROL_GZIP9 = {'default': 1519893, 'highlight-only': 817815,
                    'render-only': 1094209, 'parse-only': 284532}
DISCRETIONARY_GROWTH_BYTES = 1024
ARTIFACTS = ['default', 'highlight-only', 'render-only', 'parse-only']

# Probe cases whose change between the pins is announced by the 0.0.55
# release ("Breaking parse behaviour": verbatim tags, >1024-char implicit
# flow-collection keys). A diff anywhere else is unexpected.
DOCUMENTED_BOUNDARY_CASES = {
    'verbatim-int', 'verbatim-core-uri-int', 'verbatim-core-uri-str',
    'verbatim-local', 'verbatim-uri', 'flow-seq-key-1025-chars',
    'flow-seq-key-30000-chars', 'flow-pair-in-seq-1050',
}

# Steps compare.sh runs in each lane, and the gate each belongs to.
LANE_STEPS = {
    'harness': 'compat-corpus',
    'budgets': 'resource-budgets',
    'error-messages': 'content-api',
    'content': 'content-api',
    'md-api-ast': 'content-api',
    'diagnostics': 'content-api',
    'probe-build': 'probe',
    'probe-observe': 'probe',
    'flow': 'long-flow-line',
    'wasm-build': 'wasm',
    'wasm-npm-test': 'content-api',
    'wasm-exports': 'content-api',
    'wasm-pack': 'wasm',
}
# Host-byte budget runs: informational exits, judged through sizes.json.
# `wasm-budget-strict` compares Mac bytes with the CI manifest and is
# expected to report manifest-mismatch off CI; `wasm-budget-ceilings` checks
# host bytes against the unchanged ceilings.
INFORMATIONAL_STEPS = ['wasm-budget-strict', 'wasm-budget-ceilings']
# Exit codes that mean the environment, not the product, failed: missing
# tool, heavy-guard contention/expiry, SIGKILL (OOM).
ENV_EXITS = {75, 124, 126, 127, 137}


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def fetch(url):
    request = urllib.request.Request(url, headers={'User-Agent': 'zfb-4172-evaluation'})
    return urllib.request.urlopen(request, timeout=60).read()


def version_tuple(num):
    return tuple(int(part) for part in num.split('.'))


def cmd_registry(args):
    result = {'checkedAt': now(), 'crates': {}, 'errors': []}
    for name in PAIR:
        records = json.loads(fetch('https://crates.io/api/v1/crates/' + name))['versions']
        by_num = {record['num']: record for record in records}
        entry = {}
        for version in (CONTROL, CANDIDATE):
            frozen = FROZEN['pairs'][version][name]
            record = by_num.get(version)
            if record is None:
                result['errors'].append('%s %s missing from crates.io' % (name, version))
                continue
            archive = fetch('https://static.crates.io/crates/%s/%s-%s.crate' % (name, name, version))
            digest = hashlib.sha256(archive).hexdigest()
            row = {'yanked': record['yanked'], 'checksum': record['checksum'],
                   'archiveSha256': digest, 'archiveBytes': len(archive),
                   'rustVersion': record.get('rust_version'), 'license': record.get('license')}
            if record['yanked']:
                result['errors'].append('%s %s is yanked' % (name, version))
            if record['checksum'] != frozen['checksum'] or digest != frozen['checksum']:
                result['errors'].append('%s %s checksum differs from provenance.json' % (name, version))
            if len(archive) != frozen['archiveBytes']:
                result['errors'].append('%s %s archive size differs' % (name, version))
            if name == PAIR[1]:
                with tarfile.open(fileobj=io.BytesIO(archive), mode='r:gz') as tar:
                    manifest = tar.extractfile('%s-%s/Cargo.toml' % (name, version)).read().decode()
                block = re.search(r'^\[dependencies\.noyalib\]\n(.*?)(?:\n\[|\Z)', manifest, re.S | re.M)
                pin = re.search(r'^version = "([^"]+)"', block.group(1), re.M) if block else None
                row['exactCorePin'] = pin.group(1) if pin else None
                if row['exactCorePin'] != '=' + version:
                    result['errors'].append('%s %s does not pin noyalib =%s' % (name, version, version))
            entry[version] = row
        entry['newerNonYanked'] = sorted(
            (r['num'] for r in records if not r['yanked'] and '-' not in r['num']
             and version_tuple(r['num']) > version_tuple(CANDIDATE)), key=version_tuple)
        result['crates'][name] = entry
    # Newer pairs are recorded only: the owner scoped #4172 to the exact 0.0.55 pair.
    result['newerPairsOutOfScope'] = sorted(
        set(result['crates'][PAIR[0]]['newerNonYanked']) & set(result['crates'][PAIR[1]]['newerNonYanked']),
        key=version_tuple)
    Path(args.out).write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'errors': result['errors'], 'newerPairsOutOfScope': result['newerPairsOutOfScope']}))
    return 1 if result['errors'] else 0


def cmd_pin(args):
    path = Path(args.checkout) / 'Cargo.toml'
    text = path.read_text()
    old, new = PIN_LINE % CONTROL, PIN_LINE % CANDIDATE
    if text.count(old) != 1:
        print('expected exactly one control pin line in %s' % path, file=sys.stderr)
        return 1
    path.write_text(text.replace(old, new))
    print('pinned %s -> %s in %s' % (CONTROL, CANDIDATE, path))
    return 0


def lock_packages(text):
    packages = []
    for block in text.split('[[package]]\n')[1:]:
        fields = {}
        for line in block.strip().splitlines():
            match = re.match(r'^(\w+) = (.*)$', line)
            if match:
                key, value = match.groups()
                fields[key] = value
            elif fields:
                last = list(fields)[-1]
                fields[last] += '\n' + line
        packages.append(fields)
    return packages


def unquote(value):
    return value.strip('"')


def cmd_lock(args):
    before = lock_packages(Path(args.control).read_text())
    after = lock_packages(Path(args.candidate).read_text())
    errors = []
    if len(before) != len(after):
        errors.append('package count %d -> %d' % (len(before), len(after)))
    others = lambda packages: [p for p in packages if unquote(p['name']) not in PAIR]
    if others(before) != others(after):
        errors.append('unrelated Cargo.lock delta')
    for name in PAIR:
        old = [p for p in before if unquote(p['name']) == name]
        new = [p for p in after if unquote(p['name']) == name]
        if len(old) != 1 or len(new) != 1:
            errors.append('%s must appear exactly once in both locks' % name)
            continue
        old, new = old[0], new[0]
        if unquote(old['version']) != CONTROL or unquote(new['version']) != CANDIDATE:
            errors.append('%s versions %s -> %s' % (name, old['version'], new['version']))
        if unquote(old.get('checksum', '')) != FROZEN['pairs'][CONTROL][name]['checksum']:
            errors.append('%s control checksum differs from provenance.json' % name)
        if unquote(new.get('checksum', '')) != FROZEN['pairs'][CANDIDATE][name]['checksum']:
            errors.append('%s candidate checksum differs from provenance.json' % name)
        strip = lambda p: {k: v for k, v in p.items() if k not in ('version', 'checksum')}
        if strip(old) != strip(new):
            errors.append('%s dependency list changed' % name)
    print(json.dumps({'packages': len(after), 'errors': errors}))
    return 1 if errors else 0


def limit_child(cpu_seconds, address_bytes):
    def apply():
        resource.setrlimit(resource.RLIMIT_CPU, (cpu_seconds, cpu_seconds))
        try:
            resource.setrlimit(resource.RLIMIT_AS, (address_bytes, address_bytes))
        except (ValueError, OSError):
            pass  # macOS does not enforce RLIMIT_AS; the CPU limit and wall timeout still apply.
    return apply


def run_sample(argv, timeout, cpu_seconds, address_bytes):
    with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as err:
        started = time.monotonic()
        child = subprocess.Popen(argv, stdout=out, stderr=err,
                                 preexec_fn=limit_child(cpu_seconds, address_bytes))
        status = rusage = None
        while True:
            pid, status, rusage = os.wait4(child.pid, os.WNOHANG)
            if pid:
                break
            if time.monotonic() - started > timeout:
                child.kill()
                _, status, rusage = os.wait4(child.pid, 0)
                break
            time.sleep(0.005)
        wall = time.monotonic() - started
        out.seek(0)
        err.seek(0)
        stdout, stderr = out.read().decode(), err.read().decode()
    # ru_maxrss is bytes on Darwin and KiB on Linux.
    rss = rusage.ru_maxrss if sys.platform == 'darwin' else rusage.ru_maxrss * 1024
    sample = {'wallSeconds': round(wall, 4), 'maxRssBytes': rss,
              'cpuSeconds': round(rusage.ru_utime + rusage.ru_stime, 4)}
    if os.WIFSIGNALED(status):
        sample['signal'] = os.WTERMSIG(status)
        sample['timedOut'] = wall > timeout
    else:
        sample['exit'] = os.WEXITSTATUS(status)
    try:
        sample.update(json.loads(stdout.strip().splitlines()[-1]))
    except (IndexError, ValueError):
        sample['stderrTail'] = stderr[-2000:]
    return sample


def log_log_slope(points):
    pts = [(math.log(n), math.log(t)) for n, t in points if t > 0]
    if len(pts) < 2:
        return None
    mean_x = sum(x for x, _ in pts) / len(pts)
    mean_y = sum(y for _, y in pts) / len(pts)
    denom = sum((x - mean_x) ** 2 for x, _ in pts)
    return round(sum((x - mean_x) * (y - mean_y) for x, y in pts) / denom, 3) if denom else None


def cmd_flow(args):
    scalars = [int(value) for value in args.scalars.split(',')]
    result = {'checkedAt': now(), 'probe': args.probe, 'repetitions': args.reps,
              'timeoutSeconds': args.timeout, 'cpuLimitSeconds': args.cpu,
              'addressLimitBytes': args.address_bytes, 'platform': sys.platform,
              'shapes': {}}
    semantic_ok = True
    for shape in ('mapping-value', 'document-start'):
        rows = []
        for count in scalars:
            samples = [run_sample([args.probe, 'flow', shape, str(count)], args.timeout,
                                  args.cpu, args.address_bytes) for _ in range(args.reps)]
            completed = [s for s in samples if 'elapsedNanos' in s]
            for sample in completed:
                if not (sample.get('ok') and sample.get('shapeOk')):
                    semantic_ok = False
            nanos = sorted(s['elapsedNanos'] for s in completed)
            rows.append({
                'scalars': count,
                'inputBytes': completed[0]['inputBytes'] if completed else None,
                'completed': len(completed),
                'minNanos': nanos[0] if nanos else None,
                'medianNanos': nanos[len(nanos) // 2] if nanos else None,
                'maxRssBytes': max(s['maxRssBytes'] for s in samples),
                'samples': samples,
            })
            if len(completed) < len(samples):
                break  # a larger input cannot finish inside the same limits
        timed = [(row['scalars'], row['minNanos']) for row in rows if row['minNanos']]
        result['shapes'][shape] = {
            'rows': rows,
            'logLogSlopeAll': log_log_slope(timed),
            'logLogSlopeLargestFour': log_log_slope(timed[-4:]),
            'allCompleted': all(row['completed'] == args.reps for row in rows) and len(rows) == len(scalars),
        }
    result['semanticOk'] = semantic_ok
    Path(args.out).write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({shape: {k: v for k, v in data.items() if k != 'rows'}
                      for shape, data in result['shapes'].items()}))
    return 0 if semantic_ok else 1


BUDGET_LINE = re.compile(
    r'^(default|highlight-only|render-only|parse-only): cdylib=([\d,]+) bytes '
    r'bindgen=([\d,]+) bytes final-wasm=([\d,]+) bytes gzip-9=([\d,]+) bytes '
    r'glue=([\d,]+) bytes glue-gzip-9=([\d,]+) bytes', re.M)


def parse_budget_log(text):
    sizes = {}
    for match in BUDGET_LINE.finditer(text):
        values = [int(group.replace(',', '')) for group in match.groups()[1:]]
        sizes[match.group(1)] = dict(zip(
            ['rawCdylib', 'bindgen', 'finalWasm', 'gzip9', 'glue', 'glueGzip9'], values))
    return sizes


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def cmd_sizes(args):
    sizes = parse_budget_log(Path(args.budget_log).read_text())
    dist = Path(args.dist)
    files = {}
    total = 0
    for path in sorted(p for p in dist.rglob('*') if p.is_file()):
        size = path.stat().st_size
        total += size
        if path.suffix in ('.wasm', '.mjs') and ('_bg' in path.name or '_glue' in path.name):
            files[str(path.relative_to(dist))] = {'bytes': size, 'sha256': sha256(path)}
    result = {'label': args.label, 'artifacts': sizes, 'distTotalBytes': total, 'files': files}
    tarball = Path(args.tarball) if args.tarball else None
    if tarball and tarball.is_file():
        result['tarball'] = {'bytes': tarball.stat().st_size, 'sha256': sha256(tarball)}
    Path(args.out).write_text(json.dumps(result, indent=2) + '\n')
    missing = [name for name in ARTIFACTS if name not in sizes]
    print(json.dumps({'missingArtifacts': missing, 'distTotalBytes': total,
                      'tarball': result.get('tarball')}))
    return 1 if missing else 0


def read_exit(lane_dir, step):
    path = lane_dir / (step + '.exit')
    return int(path.read_text().strip()) if path.is_file() else None


def read_json(path):
    path = Path(path)
    return json.loads(path.read_text()) if path.is_file() else None


def read_jsonl(path):
    path = Path(path)
    if not path.is_file():
        return None
    return {row['name']: row for row in (json.loads(line) for line in path.read_text().splitlines() if line.strip())}


def step_state(exit_code):
    if exit_code is None:
        return 'NOT_RUN'
    if exit_code == 0:
        return 'PASS'
    return 'ENV_FAIL' if exit_code in ENV_EXITS else 'FAIL'


def cmd_summarize(args):
    evidence = Path(args.evidence)
    lanes = {label: evidence / label for label in ('control', 'candidate')}
    report = {'issue': FROZEN['issue'], 'summarizedAt': now(), 'control': CONTROL,
              'candidate': CANDIDATE, 'environment': read_json(evidence / 'environment.json'),
              'registry': read_json(evidence / 'registry.json'), 'steps': {}, 'gates': {},
              'incompleteReasons': [], 'pendingReasons': [], 'keepReasons': [], 'notes': []}
    incomplete = report['incompleteReasons'].append
    pending = report['pendingReasons'].append
    keep = report['keepReasons'].append

    environment = report['environment'] or {}
    for key in ('rustc', 'wasm-bindgen', 'node', 'pnpm'):
        values = {lane: (environment.get(lane) or {}).get(key)
                  for lane in ('toolchainControl', 'toolchainCandidate')}
        if None in values.values() or len(set(values.values())) != 1:
            incomplete('toolchain %s differs or is unrecorded between lanes: %s' % (key, values))

    for step in ('preflight', 'registry', 'lock-check'):
        code = read_exit(evidence, step)
        if code != 0:
            incomplete('%s exit=%s' % (step, code))

    for step, gate in LANE_STEPS.items():
        states = {label: step_state(read_exit(lane, step)) for label, lane in lanes.items()}
        report['steps'][step] = states
        report['gates'].setdefault(gate, []).append(step)
        if 'NOT_RUN' in states.values() or 'ENV_FAIL' in states.values():
            incomplete('%s: %s' % (step, states))
        elif states['control'] != 'PASS':
            incomplete('%s: control lane is red, so the baseline is not established' % step)
        elif states['candidate'] != 'PASS':
            keep('%s: candidate fails where control passes' % step)
    report['informationalSteps'] = {
        step: {label: read_exit(lane, step) for label, lane in lanes.items()} for step in INFORMATIONAL_STEPS}

    observations = {label: read_jsonl(lane / 'probe-observe.jsonl') for label, lane in lanes.items()}
    if all(observations.values()):
        diffs = []
        for name, control in observations['control'].items():
            candidate = observations['candidate'].get(name)
            if candidate is None or candidate['observation'] != control['observation']:
                diffs.append({'name': name, 'documented': name in DOCUMENTED_BOUNDARY_CASES,
                              'control': control['observation'],
                              'candidate': candidate and candidate['observation']})
        report['probeDiffs'] = diffs
        unexpected = [d['name'] for d in diffs if not d['documented']]
        if unexpected:
            pending('unexpected frontmatter behavior diffs need review: %s' % ', '.join(unexpected))
        if any(d['documented'] for d in diffs):
            report['notes'].append('documented 0.0.55 boundary diffs reached the frontmatter path; '
                                   'record a disposition and add deterministic coverage in #4173')
    else:
        incomplete('probe observations missing for a lane')

    flows = {label: read_json(lane / 'flow.json') for label, lane in lanes.items()}
    if all(flows.values()):
        report['longFlowLine'] = {
            label: {shape: {k: v for k, v in data.items() if k != 'rows'} for shape, data in flow['shapes'].items()}
            for label, flow in flows.items()}
        report['longFlowLine']['rowsScalarsBytesMinNanos'] = {
            label: {shape: [[row['scalars'], row['inputBytes'], row['minNanos']] for row in data['rows']]
                    for shape, data in flow['shapes'].items()}
            for label, flow in flows.items()}
    else:
        incomplete('long-flow-line data missing for a lane')

    sizes = {label: read_json(lane / 'sizes.json') for label, lane in lanes.items()}
    ci_candidate = None
    if args.ci_candidate_budget_log:
        ci_candidate = parse_budget_log(Path(args.ci_candidate_budget_log).read_text())
    if all(sizes.values()):
        rows = {}
        for name in ARTIFACTS:
            control = sizes['control']['artifacts'].get(name)
            candidate = sizes['candidate']['artifacts'].get(name)
            if not control or not candidate:
                incomplete('%s missing from size evidence' % name)
                continue
            delta = {key: candidate[key] - control[key] for key in control}
            projected = CI_CONTROL_GZIP9[name] + delta['gzip9']
            row = {'control': control, 'candidate': candidate, 'delta': delta,
                   'ceiling': CEILINGS[name],
                   'hostControlUnderCeiling': control['gzip9'] <= CEILINGS[name],
                   'hostCandidateUnderCeiling': candidate['gzip9'] <= CEILINGS[name],
                   'projectedCiGzip9': projected,
                   'projectedCiHeadroom': CEILINGS[name] - projected,
                   'exceedsDiscretionaryGrowth': delta['finalWasm'] > DISCRETIONARY_GROWTH_BYTES
                   or delta['gzip9'] > DISCRETIONARY_GROWTH_BYTES}
            if ci_candidate and name in ci_candidate:
                row['ciCandidate'] = ci_candidate[name]
                row['ciCandidateUnderCeiling'] = ci_candidate[name]['gzip9'] <= CEILINGS[name]
            rows[name] = row
        tarballs = {label: (sizes[label].get('tarball') or {}).get('bytes') for label in lanes}
        report['sizes'] = {'artifacts': rows, 'tarballBytes': tarballs,
                           'tarballCeiling': CEILINGS['tarball'],
                           'distTotalBytes': {label: sizes[label]['distTotalBytes'] for label in lanes},
                           'renderZeroGrowth': rows.get('render-only', {}).get('delta', {}).get('gzip9', 1) <= 0,
                           'ownerExceptionApplies': True}
        for name, row in rows.items():
            if row['hostControlUnderCeiling'] and not row['hostCandidateUnderCeiling']:
                keep('%s: candidate breaches the unchanged %d B ceiling on this host where control fits'
                     % (name, CEILINGS[name]))
            if row.get('ciCandidateUnderCeiling') is False:
                keep('%s: CI candidate gzip-9 breaches the unchanged ceiling' % name)
        if None in tarballs.values():
            incomplete('packed tarball missing for a lane')
        elif tarballs['candidate'] > CEILINGS['tarball']:
            keep('tarball breaches the unchanged %d B ceiling' % CEILINGS['tarball'])
        if not rows or any('ciCandidate' not in row for row in rows.values()):
            pending('hard ceilings are only projected from host deltas; a CI wasm-md (default) '
                    'measurement at the candidate pin is required (Mac bytes are not CI bytes)')
        if any(row['exceedsDiscretionaryGrowth'] for row in rows.values()):
            report['notes'].append('growth exceeds the 1,024 B discretionary threshold; waived for the exact '
                                   '0.0.55 pair by the owner (#4161 comment 2026-10-12), not a failure')
    else:
        incomplete('size evidence missing for a lane')

    # Missing or interrupted evidence is HOLD even beside an early KEEP signal;
    # KEEP needs a completed evaluation that fails a substantive gate.
    if report['incompleteReasons']:
        report['decision'] = 'HOLD'
    elif report['keepReasons']:
        report['decision'] = 'KEEP'
    elif report['pendingReasons']:
        report['decision'] = 'HOLD'
    else:
        report['decision'] = 'MIGRATE-ELIGIBLE'
    Path(args.out).write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({key: report[key] for key in
                      ('decision', 'incompleteReasons', 'keepReasons', 'pendingReasons')}, indent=2))
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='action', required=True)
    p = sub.add_parser('registry')
    p.add_argument('--out', required=True)
    p = sub.add_parser('pin')
    p.add_argument('--checkout', required=True)
    p = sub.add_parser('lock')
    p.add_argument('--control', required=True)
    p.add_argument('--candidate', required=True)
    p = sub.add_parser('flow')
    p.add_argument('--probe', required=True)
    p.add_argument('--out', required=True)
    p.add_argument('--scalars', default='10000,20000,40000,80000,160000,240000')
    p.add_argument('--reps', type=int, default=5)
    p.add_argument('--timeout', type=float, default=120.0)
    p.add_argument('--cpu', type=int, default=120)
    p.add_argument('--address-bytes', type=int, default=4 * 1024 ** 3)
    p = sub.add_parser('sizes')
    p.add_argument('--label', required=True)
    p.add_argument('--budget-log', required=True)
    p.add_argument('--dist', required=True)
    p.add_argument('--tarball')
    p.add_argument('--out', required=True)
    p = sub.add_parser('summarize')
    p.add_argument('--evidence', required=True)
    p.add_argument('--out', required=True)
    p.add_argument('--ci-candidate-budget-log')
    args = parser.parse_args()
    return globals()['cmd_' + args.action](args)


if __name__ == '__main__':
    sys.exit(main())
