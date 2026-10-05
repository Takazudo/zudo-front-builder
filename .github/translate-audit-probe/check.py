import hashlib,json,re,sys
from pathlib import Path
root=Path(sys.argv[1])
assert (root/'source-before.txt').read_text().strip()=='20afaf16eb6dfcdf556ea579184f09bee65edbf1'
assert (root/'source-after.txt').read_text().strip()=='43f2924b9c12c55da1b6e464be8d81bbf68301b7'
assert (root/'inputs-before.txt').read_bytes()==(root/'inputs-after.txt').read_bytes()
assert re.search(r'test result: ok\. 2 passed; 0 failed; 0 ignored;', (root/'tests.log').read_text())
before=json.loads((root/'before/composition/report.json').read_text())
after=json.loads((root/'after/composition/report.json').read_text())
for report in [before,after]:
 assert report['hasErrors'] is False and report['diagnostics']==[]
 assert report['audit']['outcome']=='complete'
for key in ['rules','explanations','ordinaryClasses','authoredClasses']:
 assert before[key]==after[key], key
css_before=(root/'before/composition/wind.css').read_bytes()
css_after=(root/'after/composition/wind.css').read_bytes()
assert css_before and css_before==css_after, 'generated CSS changed'
def conflicts(report):
 rows=report['audit']['conflicts']
 result={tuple(sorted([r['firstCandidate'],r['secondCandidate']])):r for r in rows}
 assert len(rows)==len(result)
 return result
old,new=conflicts(before),conflicts(after)
expected={tuple(sorted(pair)) for pair in [
 ['translate-x-px','translate-x-full'],['translate-y-0','translate-y-px'],['w-0','w-full']]}
removed={tuple(sorted(pair)) for pair in [
 ['translate-x-1/2','translate-y-1/2'],['-translate-x-px','translate-y-px'],
 ['hover:translate-x-full','hover:-translate-y-full'],['sm:translate-x-1/2','sm:translate-y-1/2']]}
assert set(new)==expected and set(old)==expected|removed
for key in new:assert new[key]==old[key], ('control changed',key)
for key in removed:assert old[key]['overlappingProperties']==['translate']
for report,count in [(before,7),(after,3)]:
 diagnostics=report['audit']['diagnostics']
 assert len(diagnostics)==count
 assert all(d['code']=='ZW013' and d['severity']=='auditInfo' for d in diagnostics)
summary={'parent':(root/'source-before.txt').read_text().strip(),'candidate':(root/'source-after.txt').read_text().strip(),'css_bytes':len(css_after),'css_sha256':hashlib.sha256(css_after).hexdigest(),'css_byte_identical':True,'before_conflicts':len(old),'after_conflicts':len(new),'removed_compatible_pairs':[list(k) for k in sorted(removed)],'retained_controls':[list(k) for k in sorted(expected)],'native_regressions':2,'result':'passed'}
(root/'comparison.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps(summary,indent=2))
