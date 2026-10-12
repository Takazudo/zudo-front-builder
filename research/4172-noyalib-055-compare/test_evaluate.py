"""Offline tests for evaluate.py: python3 -I -m unittest discover -s <this dir>."""
import argparse
import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path

import evaluate

REPO = Path(__file__).resolve().parents[2]
CONTROL_LOCK = (REPO / 'Cargo.lock').read_text()


def candidate_lock(text):
    for name in evaluate.PAIR:
        old = evaluate.FROZEN['pairs'][evaluate.CONTROL][name]['checksum']
        new = evaluate.FROZEN['pairs'][evaluate.CANDIDATE][name]['checksum']
        block = 'name = "%s"\nversion = "%s"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "%s"'
        text = text.replace(block % (name, evaluate.CONTROL, old), block % (name, evaluate.CANDIDATE, new))
    return text


def budget_line(label, gzip9, final=2000000):
    return ('%s: cdylib=3,000,000 bytes bindgen=2,500,000 bytes final-wasm=%s bytes gzip-9=%s bytes '
            'glue=8,772 bytes glue-gzip-9=2,661 bytes ceiling=1,100,000 bytes'
            % (label, format(final, ','), format(gzip9, ',')))


HOST_GZIP = {'default': 1550000, 'highlight-only': 825000, 'render-only': 1113000, 'parse-only': 297000}


class LockTests(unittest.TestCase):
    def test_exact_two_entry_delta_passes(self):
        count, errors = evaluate.lock_errors(CONTROL_LOCK, candidate_lock(CONTROL_LOCK))
        self.assertEqual(errors, [])
        self.assertGreater(count, 100)

    def test_unrelated_delta_is_refused(self):
        changed = candidate_lock(CONTROL_LOCK).replace('name = "indexmap"', 'name = "indexmap2"', 1)
        _, errors = evaluate.lock_errors(CONTROL_LOCK, changed)
        self.assertIn('unrelated Cargo.lock delta', errors)

    def test_unchanged_lock_is_refused(self):
        _, errors = evaluate.lock_errors(CONTROL_LOCK, CONTROL_LOCK)
        self.assertTrue(any('versions' in error for error in errors))


class ParseTests(unittest.TestCase):
    def test_budget_log_with_github_timestamps(self):
        prefix = 'wasm-md (default)\tEnforce four-artifact gzip budgets and report metrics\t'
        text = '\n'.join(prefix + '2026-10-13T01:02:03.4567890Z ' + budget_line(label, gzip)
                         for label, gzip in HOST_GZIP.items())
        text += '\n2026-10-13T01:02:04.0000000Z complete packed tarball: 3,456,789 bytes ceiling=3,900,000 bytes'
        sizes = evaluate.parse_budget_log(text)
        self.assertEqual(sizes['render-only']['gzip9'], 1113000)
        self.assertEqual(sizes['default']['rawCdylib'], 3000000)
        self.assertEqual(sizes['tarball']['bytes'], 3456789)

    def test_classify(self):
        error, success = {'kind': 'error'}, {'kind': 'success'}
        self.assertEqual(evaluate.classify_diff(error, error), 'error-text-only')
        self.assertEqual(evaluate.classify_diff(success, error), 'newly-refused')
        self.assertEqual(evaluate.classify_diff(success, None), 'missing')


def write_lane(lane, label, gzip_delta=0, flow_ok=True, observation=None):
    lane.mkdir(parents=True)
    version = evaluate.CONTROL if label == 'control' else evaluate.CANDIDATE
    (lane / 'tree-normal.log').write_text(
        'noyalib v%s\n└── noyalib-serde-yaml v%s\n    └── zfb-content v0.0.0\n' % (version, version))
    for step in evaluate.LANE_STEPS:
        if step not in evaluate.OPTIONAL_STEPS:
            (lane / (step + '.exit')).write_text('0\n')
    observation = observation or {'kind': 'success', 'value': {'v': 1}}
    (lane / 'probe-observe.jsonl').write_text(
        json.dumps({'name': 'verbatim-int', 'observation': observation}) + '\n'
        + json.dumps({'name': 'duplicate-nested', 'observation': {'kind': 'success', 'value': {}}}) + '\n')
    rows = [{'scalars': n, 'inputBytes': 2 * n, 'completed': 5, 'minNanos': n * (n if label == 'control' else 1000)}
            for n in (10000, 20000, 40000, 80000)]
    shape = {'rows': rows, 'refusalRows': [], 'logLogSlopeAll': 2.0 if label == 'control' else 1.0,
             'logLogSlopeLargestFour': 2.0 if label == 'control' else 1.0,
             'allCompleted': True, 'refusalAllCompleted': True}
    (lane / 'flow.json').write_text(json.dumps(
        {'semanticOk': flow_ok, 'shapes': {'mapping-value': shape, 'document-start': shape}}))
    artifacts = {name: {'rawCdylib': 1, 'bindgen': 1, 'finalWasm': 2000000 + gzip_delta, 'gzip9': gzip + gzip_delta,
                        'glue': 1, 'glueGzip9': 1} for name, gzip in HOST_GZIP.items()}
    (lane / 'sizes.json').write_text(json.dumps(
        {'artifacts': artifacts, 'distTotalBytes': 10, 'tarball': {'bytes': 3400000 + gzip_delta}}))


def summarize(tmp, ci_log=None, ci_run=None, dispositions=None, **candidate):
    evidence = Path(tmp)
    toolchain = {key: 'same' for key in evaluate.TOOLCHAIN_KEYS}
    (evidence / 'environment.json').write_text(json.dumps(
        {'toolchainControl': toolchain, 'toolchainCandidate': toolchain}))
    for step in ('preflight', 'registry', 'lock-check', 'branch-untouched'):
        (evidence / (step + '.exit')).write_text('0\n')
    write_lane(evidence / 'control', 'control')
    write_lane(evidence / 'candidate', 'candidate', **candidate)
    out = evidence / 'result.json'
    args = argparse.Namespace(evidence=str(evidence), out=str(out), ci_candidate_budget_log=ci_log,
                              ci_candidate_run=ci_run, dispositions=dispositions)
    with contextlib.redirect_stdout(io.StringIO()):
        evaluate.cmd_summarize(args)
    return json.loads(out.read_text())


def ci_log(tmp, render_gzip):
    gzips = dict(evaluate.CI_CONTROL_GZIP9, **{'render-only': render_gzip})
    path = Path(tmp) / 'ci.log'
    path.write_text('\n'.join(budget_line(label, gzip) for label, gzip in gzips.items()) + '\n')
    return str(path)


class DecisionTests(unittest.TestCase):
    def test_without_ci_measurement_is_hold(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = summarize(tmp, gzip_delta=1043)
        self.assertEqual(result['decision'], 'HOLD')
        self.assertTrue(any('CI wasm-md' in reason for reason in result['pendingReasons']))
        self.assertTrue(result['longFlowLine']['mapping-value']['improvementDemonstrated'])

    def test_ci_under_ceilings_with_growth_is_migrate_eligible(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = summarize(tmp, ci_log(tmp, 1095252), 'run/1 sha', gzip_delta=1043)
        self.assertEqual(result['decision'], 'MIGRATE-ELIGIBLE', result)
        self.assertTrue(any('waived' in note for note in result['notes']))

    def test_ci_ceiling_breach_is_keep(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = summarize(tmp, ci_log(tmp, 1100001), 'run/1 sha', gzip_delta=1043)
        self.assertEqual(result['decision'], 'KEEP')

    def test_candidate_red_step_is_keep(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = summarize(tmp, ci_log(tmp, 1095252), 'run/1 sha', flow_ok=False)
        self.assertEqual(result['decision'], 'KEEP')

    def test_missing_step_is_hold_even_beside_keep(self):
        with tempfile.TemporaryDirectory() as tmp:
            summarize(tmp, ci_log(tmp, 1100001), 'run/1 sha')
            (Path(tmp) / 'candidate' / 'wasm-build.exit').unlink()
            args = argparse.Namespace(evidence=tmp, out=str(Path(tmp) / 'r.json'), ci_candidate_budget_log=None,
                                      ci_candidate_run=None, dispositions=None)
            with contextlib.redirect_stdout(io.StringIO()):
                evaluate.cmd_summarize(args)
            result = json.loads((Path(tmp) / 'r.json').read_text())
        self.assertEqual(result['decision'], 'HOLD')

    def test_wrong_compiled_pair_is_hold(self):
        with tempfile.TemporaryDirectory() as tmp:
            summarize(tmp, ci_log(tmp, 1095252), 'run/1 sha')
            tree = Path(tmp) / 'candidate' / 'tree-normal.log'
            tree.write_text(tree.read_text().replace(evaluate.CANDIDATE, evaluate.CONTROL))
            args = argparse.Namespace(evidence=tmp, out=str(Path(tmp) / 'r.json'), ci_candidate_budget_log=None,
                                      ci_candidate_run=None, dispositions=None)
            with contextlib.redirect_stdout(io.StringIO()):
                evaluate.cmd_summarize(args)
            result = json.loads((Path(tmp) / 'r.json').read_text())
        self.assertEqual(result['decision'], 'HOLD')
        self.assertTrue(any('cargo tree' in reason for reason in result['incompleteReasons']))

    def test_env_suspect_guard_verdict_is_hold(self):
        with tempfile.TemporaryDirectory() as tmp:
            summarize(tmp)
            lane = Path(tmp) / 'candidate'
            (lane / 'content.exit').write_text('1\n')
            (lane / 'content.log').write_text('heavy-guard: verdict=ENV_SUSPECT exit=1 secs=9\n')
            self.assertEqual(evaluate.step_state(lane, 'content'), 'ENV_FAIL')

    def test_documented_value_change_needs_disposition(self):
        changed = {'kind': 'success', 'value': {'v': '1'}}
        with tempfile.TemporaryDirectory() as tmp:
            held = summarize(tmp, ci_log(tmp, 1095252), 'run/1 sha', observation=changed)
        self.assertEqual(held['decision'], 'HOLD')
        self.assertEqual(held['probeDiffs'][0]['class'], 'value-change')
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'dispositions.json'
            path.write_text(json.dumps({'verbatim-int': {'disposition': 'accepted', 'reason': 'spec'}}))
            accepted = summarize(tmp, ci_log(tmp, 1095252), 'run/1 sha', str(path), observation=changed)
        self.assertEqual(accepted['decision'], 'MIGRATE-ELIGIBLE')


if __name__ == '__main__':
    unittest.main()
