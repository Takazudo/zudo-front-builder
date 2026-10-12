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
# The owner's #4161 waiver (1,024 B threshold, zero render growth) names this
# exact pair only; any other candidate is judged without it.
OWNER_EXCEPTION_CANDIDATE = '0.0.55'

# Probe cases whose change between the pins is announced by the 0.0.55
# release (verbatim tags, >1024-char implicit flow-collection keys, alias
# depth now charged against max_depth). A diff anywhere else is unexpected.
DOCUMENTED_BOUNDARY_CASES = {
    'verbatim-int', 'verbatim-core-uri-int', 'verbatim-core-uri-str',
    'verbatim-local', 'verbatim-uri', 'flow-seq-key-1025-chars',
    'flow-seq-key-30000-chars', 'flow-pair-in-seq-1050', 'alias-depth-over-max',
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
    'wasm-packed': 'wasm',
    'wasm-release': 'wasm',
}
# Opt-in lanes (compare.sh WITH_ZFB_DIAGNOSTICS=1): absent from both lanes is
# not missing evidence.
OPTIONAL_STEPS = {'diagnostics'}
# Host-byte budget runs: informational exits, judged through sizes.json.
# `wasm-budget-strict` compares Mac bytes with the CI manifest and is
# expected to report manifest-mismatch off CI; `wasm-budget-ceilings` checks
# host bytes against the unchanged ceilings.
INFORMATIONAL_STEPS = ['wasm-budget-strict', 'wasm-budget-ceilings']
# Exit codes that mean the environment, not the product, failed: missing
# tool, heavy-guard contention/expiry, SIGKILL (OOM).
ENV_EXITS = {75, 124, 126, 127, 137}
ENV_SUSPECT = 'heavy-guard: verdict=ENV_SUSPECT'
TOOLCHAIN_KEYS = ('rustc', 'wasm-rustc', 'wasm-bindgen', 'wasm-opt', 'node', 'pnpm')


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


def lock_errors(control_text, candidate_text):
    before = lock_packages(control_text)
    after = lock_packages(candidate_text)
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
    return len(after), errors


def cmd_lock(args):
    count, errors = lock_errors(Path(args.control).read_text(), Path(args.candidate).read_text())
    print(json.dumps({'packages': count, 'errors': errors}))
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


def measure_rows(args, shape, counts, reps, expect_refusal):
    """Time `counts` in order; stop at the first size that cannot finish inside the limits."""
    rows = []
    semantic_ok = True
    for count in counts:
        samples = []
        for _ in range(reps):
            sample = run_sample([args.probe, 'flow', shape, str(count)], args.timeout,
                                args.cpu, args.address_bytes)
            samples.append(sample)
            if 'elapsedNanos' not in sample:
                break  # one sample over the limits answers the question for this size
        completed = [s for s in samples if 'elapsedNanos' in s]
        for sample in completed:
            if expect_refusal:
                # Over max_nodes both pins must refuse; only the time to refuse differs.
                semantic_ok &= not sample.get('ok') and bool(sample.get('error'))
            else:
                semantic_ok &= bool(sample.get('ok') and sample.get('shapeOk'))
        nanos = sorted(s['elapsedNanos'] for s in completed)
        rows.append({
            'scalars': count,
            'inputBytes': completed[0]['inputBytes'] if completed else None,
            'completed': len(completed),
            'attempted': len(samples),
            'minNanos': nanos[0] if nanos else None,
            'medianNanos': nanos[len(nanos) // 2] if nanos else None,
            'maxRssBytes': max(s['maxRssBytes'] for s in samples),
            'error': completed[0].get('error') if completed else None,
            'samples': samples,
        })
        if len(completed) < reps:
            break  # a larger input cannot finish inside the same limits
    return rows, semantic_ok


def cmd_flow(args):
    scalars = [int(value) for value in args.scalars.split(',')]
    refusal = [int(value) for value in args.refusal_scalars.split(',') if value]
    result = {'checkedAt': now(), 'probe': args.probe, 'repetitions': args.reps,
              'refusalRepetitions': args.refusal_reps,
              'timeoutSeconds': args.timeout, 'cpuLimitSeconds': args.cpu,
              'addressLimitBytes': args.address_bytes, 'platform': sys.platform,
              'shapes': {}}
    semantic_ok = True
    for shape in ('mapping-value', 'document-start'):
        rows, ok = measure_rows(args, shape, scalars, args.reps, False)
        refusal_rows, refusal_ok = measure_rows(args, shape, refusal, args.refusal_reps, True)
        semantic_ok &= ok and refusal_ok
        timed = [(row['scalars'], row['minNanos']) for row in rows if row['minNanos']]
        result['shapes'][shape] = {
            'rows': rows,
            'refusalRows': refusal_rows,
            'logLogSlopeAll': log_log_slope(timed),
            'logLogSlopeLargestFour': log_log_slope(timed[-4:]),
            'allCompleted': all(row['completed'] == args.reps for row in rows) and len(rows) == len(scalars),
            'refusalAllCompleted': all(row['completed'] == args.refusal_reps for row in refusal_rows)
            and len(refusal_rows) == len(refusal),
        }
    result['semanticOk'] = semantic_ok
    Path(args.out).write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({shape: {k: v for k, v in data.items() if k not in ('rows', 'refusalRows')}
                      for shape, data in result['shapes'].items()}))
    return 0 if semantic_ok else 1


BUDGET_LINE = re.compile(
    r'^(default|highlight-only|render-only|parse-only): cdylib=([\d,]+) bytes '
    r'bindgen=([\d,]+) bytes final-wasm=([\d,]+) bytes gzip-9=([\d,]+) bytes '
    r'glue=([\d,]+) bytes glue-gzip-9=([\d,]+) bytes', re.M)
TARBALL_LINE = re.compile(r'^complete packed tarball: ([\d,]+) bytes', re.M)


def parse_budget_log(text):
    # GitHub logs prefix each line with an ISO timestamp; `gh run view --log`
    # adds `<job>\t<step>\t` before it.
    text = re.sub(r'^[^\n]*?\d{4}-\d\d-\d\dT[\d:.]+Z ', '', text, flags=re.M)
    sizes = {}
    for match in BUDGET_LINE.finditer(text):
        values = [int(group.replace(',', '')) for group in match.groups()[1:]]
        sizes[match.group(1)] = dict(zip(
            ['rawCdylib', 'bindgen', 'finalWasm', 'gzip9', 'glue', 'glueGzip9'], values))
    tarball = TARBALL_LINE.search(text)
    if tarball:
        sizes['tarball'] = {'bytes': int(tarball.group(1).replace(',', ''))}
    return sizes


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def cmd_sizes(args):
    sizes = parse_budget_log(Path(args.budget_log).read_text())
    sizes.pop('tarball', None)
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


def step_state(lane_dir, step):
    exit_code = read_exit(lane_dir, step)
    if exit_code is None:
        return 'NOT_RUN'
    if exit_code == 0:
        return 'PASS'
    log = lane_dir / (step + '.log')
    if exit_code in ENV_EXITS or (log.is_file() and ENV_SUSPECT in log.read_text(errors='replace')):
        return 'ENV_FAIL'
    return 'FAIL'


def classify_diff(control, candidate):
    if candidate is None:
        return 'missing'
    kinds = (control['kind'], candidate['kind'])
    return {('error', 'error'): 'error-text-only', ('success', 'success'): 'value-change',
            ('success', 'error'): 'newly-refused', ('error', 'success'): 'newly-accepted'}[kinds]


def cmd_summarize(args):
    evidence = Path(args.evidence)
    lanes = {label: evidence / label for label in ('control', 'candidate')}
    report = {'schema': 'zfb-4172-result/v1', 'issue': FROZEN['issue'], 'summarizedAt': now(),
              'control': CONTROL, 'candidate': CANDIDATE,
              'environment': read_json(evidence / 'environment.json'),
              'registry': read_json(evidence / 'registry.json'), 'steps': {}, 'gates': {},
              'incompleteReasons': [], 'pendingReasons': [], 'keepReasons': [], 'notes': []}
    incomplete = report['incompleteReasons'].append
    pending = report['pendingReasons'].append
    keep = report['keepReasons'].append
    note = report['notes'].append

    environment = report['environment'] or {}
    for key in TOOLCHAIN_KEYS:
        values = {lane: (environment.get(lane) or {}).get(key)
                  for lane in ('toolchainControl', 'toolchainCandidate')}
        if None in values.values() or len(set(values.values())) != 1:
            incomplete('toolchain %s differs or is unrecorded between lanes: %s' % (key, values))

    for step in ('preflight', 'registry', 'lock-check', 'branch-untouched'):
        code = read_exit(evidence, step)
        if code != 0:
            incomplete('%s exit=%s' % (step, code))

    # The harness's adapter label is a constant; `cargo tree` proves which
    # pair each lane actually compiled.
    for label, version in (('control', CONTROL), ('candidate', CANDIDATE)):
        tree = lanes[label] / 'tree-normal.log'
        text = tree.read_text() if tree.is_file() else ''
        if not re.search(r'^noyalib v%s\b' % re.escape(version), text, re.M) \
                or 'noyalib-serde-yaml v%s' % version not in text:
            incomplete('%s lane: cargo tree does not show the noyalib %s pair' % (label, version))

    for step, gate in LANE_STEPS.items():
        states = {label: step_state(lane, step) for label, lane in lanes.items()}
        report['steps'][step] = states
        report['gates'].setdefault(gate, []).append(step)
        if step in OPTIONAL_STEPS and set(states.values()) == {'NOT_RUN'}:
            continue
        if 'NOT_RUN' in states.values() or 'ENV_FAIL' in states.values():
            incomplete('%s: %s' % (step, states))
        elif states['control'] != 'PASS':
            incomplete('%s: control lane is red, so the baseline is not established' % step)
        elif states['candidate'] != 'PASS':
            keep('%s: candidate fails where control passes' % step)
    report['informationalSteps'] = {
        step: {label: read_exit(lane, step) for label, lane in lanes.items()} for step in INFORMATIONAL_STEPS}

    dispositions = read_json(args.dispositions) if args.dispositions else {}
    if args.dispositions and dispositions is None:
        incomplete('dispositions file %s is missing' % args.dispositions)
        dispositions = {}
    observations = {label: read_jsonl(lane / 'probe-observe.jsonl') for label, lane in lanes.items()}
    if all(observations.values()):
        diffs = []
        for name, control in observations['control'].items():
            candidate = observations['candidate'].get(name)
            if candidate is not None and candidate['observation'] == control['observation']:
                continue
            candidate_obs = candidate and candidate['observation']
            diff = {'name': name, 'documented': name in DOCUMENTED_BOUNDARY_CASES,
                    'class': classify_diff(control['observation'], candidate_obs),
                    'control': control['observation'], 'candidate': candidate_obs}
            disposition = (dispositions or {}).get(name)
            if disposition:
                diff['disposition'] = disposition
            diffs.append(diff)
            verdict = (disposition or {}).get('disposition')
            if verdict == 'regression':
                keep('frontmatter behavior regression recorded for %s: %s'
                     % (name, disposition.get('reason', '')))
            elif verdict == 'accepted':
                continue
            elif diff['documented'] and diff['class'] == 'error-text-only':
                note('%s: documented boundary, both pins refuse; only the error text differs' % name)
            else:
                pending('%s: %s%s frontmatter diff needs a recorded disposition'
                        % (name, '' if diff['documented'] else 'UNEXPECTED ', diff['class']))
        report['probeDiffs'] = diffs
        report['probeCases'] = len(observations['control'])
    else:
        incomplete('probe observations missing for a lane')

    flows = {label: read_json(lane / 'flow.json') for label, lane in lanes.items()}
    if all(flows.values()):
        summary = {}
        for shape in ('mapping-value', 'document-start'):
            per_lane = {label: flow['shapes'][shape] for label, flow in flows.items()}
            row_view = lambda rows: [[r['scalars'], r['inputBytes'], r['completed'], r['minNanos']] for r in rows]
            summary[shape] = {
                label: {'logLogSlopeAll': data['logLogSlopeAll'],
                        'logLogSlopeLargestFour': data['logLogSlopeLargestFour'],
                        'allCompleted': data['allCompleted'],
                        'refusalAllCompleted': data.get('refusalAllCompleted'),
                        'rowsScalarsBytesCompletedMinNanos': row_view(data['rows']),
                        'refusalRows': row_view(data.get('refusalRows', []))}
                for label, data in per_lane.items()}
            control, candidate = per_lane['control'], per_lane['candidate']
            largest = {label: max((r for r in data['rows'] if r['minNanos']), key=lambda r: r['scalars'],
                                  default=None) for label, data in per_lane.items()}
            ratio = None
            if largest['control'] and largest['candidate'] \
                    and largest['control']['scalars'] == largest['candidate']['scalars']:
                ratio = round(largest['control']['minNanos'] / largest['candidate']['minNanos'], 2)
            slope_c, slope_n = control['logLogSlopeLargestFour'], candidate['logLogSlopeLargestFour']
            # Descriptive only: timing never gates the decision (#4172 keeps
            # noisy thresholds out of the verdict); the evidence is reported.
            demonstrated = bool(candidate['allCompleted'] and (
                not control['allCompleted']
                or (slope_c is not None and slope_n is not None and slope_c >= 1.6 and slope_n <= 1.3)))
            summary[shape]['largestCommonControlOverCandidate'] = ratio
            summary[shape]['improvementDemonstrated'] = demonstrated
            if not candidate['allCompleted']:
                keep('long-flow-line %s: candidate cannot finish an accepted size inside the limits' % shape)
        report['longFlowLine'] = summary
        if not all(summary[shape]['improvementDemonstrated'] for shape in summary):
            note('long-flow-line improvement is NOT demonstrated for every shape on this host; '
                 'report the data to the owner, who accepted the growth for this fix')
        for label, flow in flows.items():
            if not flow['semanticOk']:
                (keep if label == 'candidate' else incomplete)(
                    'long-flow-line %s lane: wrong value shape or refusal result' % label)
    else:
        incomplete('long-flow-line data missing for a lane')

    sizes = {label: read_json(lane / 'sizes.json') for label, lane in lanes.items()}
    ci_candidate = None
    if args.ci_candidate_budget_log:
        ci_candidate = parse_budget_log(Path(args.ci_candidate_budget_log).read_text())
        report['ciCandidate'] = {'run': args.ci_candidate_run, 'parsed': ci_candidate}
        if not args.ci_candidate_run:
            pending('CI candidate budget log has no recorded run URL/SHA (--ci-candidate-run)')
        missing = [name for name in ARTIFACTS if name not in ci_candidate]
        if missing:
            incomplete('CI candidate budget log lacks %s' % ', '.join(missing))
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
                row['ciCandidateHeadroom'] = CEILINGS[name] - ci_candidate[name]['gzip9']
                row['ciCandidateUnderCeiling'] = ci_candidate[name]['gzip9'] <= CEILINGS[name]
            rows[name] = row
        tarballs = {label: (sizes[label].get('tarball') or {}).get('bytes') for label in lanes}
        exception = CANDIDATE == OWNER_EXCEPTION_CANDIDATE
        render_growth = rows.get('render-only', {}).get('delta', {}).get('gzip9')
        report['sizes'] = {'artifacts': rows, 'tarballBytes': tarballs,
                           'tarballCeiling': CEILINGS['tarball'],
                           'ciTarballBytes': (ci_candidate or {}).get('tarball', {}).get('bytes'),
                           'distTotalBytes': {label: sizes[label]['distTotalBytes'] for label in lanes},
                           'renderZeroGrowth': render_growth is not None and render_growth <= 0,
                           'ownerExceptionApplies': exception}
        for name, row in rows.items():
            if row['hostControlUnderCeiling'] and not row['hostCandidateUnderCeiling']:
                keep('%s: candidate breaches the unchanged %d B ceiling on this host where control fits'
                     % (name, CEILINGS[name]))
            if row.get('ciCandidateUnderCeiling') is False:
                keep('%s: CI candidate gzip-9 breaches the unchanged %d B ceiling' % (name, CEILINGS[name]))
        if None in tarballs.values():
            incomplete('packed tarball missing for a lane')
        elif tarballs['candidate'] > CEILINGS['tarball']:
            keep('tarball breaches the unchanged %d B ceiling' % CEILINGS['tarball'])
        if report['sizes']['ciTarballBytes'] is not None and report['sizes']['ciTarballBytes'] > CEILINGS['tarball']:
            keep('CI candidate tarball breaches the unchanged %d B ceiling' % CEILINGS['tarball'])
        if not rows or any('ciCandidate' not in row for row in rows.values()):
            pending('hard ceilings are only projected from host deltas; a CI wasm-md (default) '
                    'measurement at the candidate pin is required (Mac bytes are not CI bytes)')
        grew = any(row['exceedsDiscretionaryGrowth'] for row in rows.values()) or not report['sizes']['renderZeroGrowth']
        if grew and exception:
            note('growth exceeds the 1,024 B discretionary threshold and/or render grows; waived for the '
                 'exact 0.0.55 pair by the owner (#4161, 2026-10-12), not a failure')
        elif grew:
            keep('discretionary growth/zero-render-growth criteria fail and no owner exception covers %s'
                 % CANDIDATE)
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
                      ('decision', 'incompleteReasons', 'keepReasons', 'pendingReasons', 'notes')}, indent=2))
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
    # 240,000 one-byte scalars plus their sequences stay under the compat
    # profile's 250,000 max_nodes; the refusal sizes are over it.
    p.add_argument('--scalars', default='10000,20000,40000,80000,160000,240000')
    p.add_argument('--refusal-scalars', default='400000,800000')
    p.add_argument('--reps', type=int, default=5)
    p.add_argument('--refusal-reps', type=int, default=3)
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
    p.add_argument('--ci-candidate-run')
    p.add_argument('--dispositions')
    args = parser.parse_args()
    return globals()['cmd_' + args.action](args)


if __name__ == '__main__':
    sys.exit(main())
