"""Disposable CI-only instrumentation; never merge into the product branch."""
from pathlib import Path
import hashlib, json, sys
root = Path(sys.argv[1]).resolve()
evidence = Path(sys.argv[2]).resolve()
evidence.mkdir(parents=True, exist_ok=True)
p = root/'crates/zfb/tests/client_router_autoinclude_build.rs'
original_bytes = p.read_bytes()
original = original_bytes.decode('utf-8')
anchor='''        truncate(&html_blob, 1500),
    );
}

fn truncate'''
assert original.count(anchor)==1, 'router test shape changed; review instrumentation'
replacement='''        truncate(&html_blob, 1500),
    );
    let destination = std::env::var_os("ZFB_ROUTER_PROBE_OUT")
        .expect("ZFB_ROUTER_PROBE_OUT must name the original-output evidence directory");
    let destination = PathBuf::from(destination);
    assert!(!destination.exists(), "refuse to overwrite prior router evidence");
    fn copy_dist(source: &Path, destination: &Path) {
        fs::create_dir_all(destination).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let target = destination.join(entry.file_name());
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                copy_dist(&entry.path(), &target);
            } else {
                assert!(kind.is_file(), "unexpected non-file in emitted dist");
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    copy_dist(&dist, &destination);
    fs::write(destination.join("../build-stdout.log"), output.stdout).unwrap();
    fs::write(destination.join("../build-stderr.log"), output.stderr).unwrap();
}

fn truncate'''
p.write_text(original.replace(anchor,replacement))
instrumented_bytes = p.read_bytes()
(evidence/'source-test-original.rs').write_bytes(original_bytes)
(evidence/'source-test-instrumented.rs').write_bytes(instrumented_bytes)
(evidence/'instrumentation.json').write_text(json.dumps({'path':str(p.relative_to(root)),'originalSha256':hashlib.sha256(original_bytes).hexdigest(),'instrumentedSha256':hashlib.sha256(instrumented_bytes).hexdigest()},indent=2)+'\n')
