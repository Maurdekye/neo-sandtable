"""Validate seeded line audit provenance and report conditional sample errors.

This never writes map features or coverage. Unresolved labels are excluded and
reported, not silently counted as successes. Stratified counts are not a whole
map accuracy estimate. Python3.10+, standard library only.
"""
import argparse
from collections import Counter
import csv
import hashlib
import json
from pathlib import Path
from propose_lines import KINDS, select_audit

KEY = ('from_hex','to_hex','kind')
PREDICTION = KEY + ('proposal','confidence','score')

def key(row): return tuple(row[f] for f in KEY)

def report(proposals, audit, metadata):
    keyed={key(r):r for r in proposals}
    if len(keyed)!=len(proposals): raise ValueError('Duplicate proposal')
    if any(r['kind'] not in KINDS or r['proposal'] not in ('present','absent','uncertain') for r in proposals):
        raise ValueError('Invalid kind or proposal')
    for r in proposals:
        for f in ('score','confidence'):
            if not 0<=float(r[f])<=1: raise ValueError('Invalid score')
    counts=Counter((r['kind'],r['proposal']) for r in proposals)
    if {f'{k}/{s}':n for (k,s),n in sorted(counts.items())}!=metadata['populations']:
        raise ValueError('Population does not match metadata')
    expected={key(r) for r in select_audit(proposals,metadata['seed'],metadata['sample_per_kind_state'])}
    if len({key(r) for r in audit})!=len(audit): raise ValueError('Duplicate audit row')
    if {key(r) for r in audit}!=expected: raise ValueError('Audit cohort does not match seeded selection')
    for r in audit:
        original=keyed[key(r)]
        if any(str(r[f])!=str(original[f]) for f in PREDICTION): raise ValueError('Audit prediction was changed')
        if r.get('observed') not in ('present','absent','unresolved'): raise ValueError('Every audit row needs an observed label')
        if not r.get('note','').strip(): raise ValueError('Every audit row needs review evidence')
    results={}
    for kind in KINDS:
        entries=[r for r in audit if r['kind']==kind]
        strata={}
        for state in ('present','absent','uncertain'):
            group=[r for r in entries if r['proposal']==state]
            resolved=[r for r in group if r['observed']!='unresolved']
            strata[state]=dict(population=counts[kind,state],sampled=len(group),resolved=len(resolved),
                unresolved=len(group)-len(resolved),observed_present=sum(r['observed']=='present' for r in resolved),
                observed_absent=sum(r['observed']=='absent' for r in resolved),
                errors=None if state=='uncertain' else sum(r['observed']!=state for r in resolved))
        confident=[r for r in entries if r['proposal']!='uncertain' and r['observed']!='unresolved']
        errors=sum(r['proposal']!=r['observed'] for r in confident)
        total=sum(n for (k,_),n in counts.items() if k==kind)
        results[kind]=dict(strata=strata,confident_resolved=len(confident),confident_errors=errors,
            conditional_sample_error=errors/len(confident) if confident else None,
            abstentions=counts[kind,'uncertain'],population=total,
            abstention_fraction=counts[kind,'uncertain']/total if total else None)
    return dict(algorithm=metadata['algorithm'],build_file_sha256=metadata['build_file_sha256'],
        source_image_sha256=metadata['source_image_sha256'],seed=metadata['seed'],selection=metadata['selection'],
        layers=results,scope='single-observer stratified pilot; no independent or full-map error claim',publication='none')

def read_csv(path):
    with path.open(newline='',encoding='utf-8') as f: return list(csv.DictReader(f))

def main(folder):
    output=report(read_csv(folder/'proposals.csv'),read_csv(folder/'audit.csv'),json.loads((folder/'metadata.json').read_text()))
    output['file_sha256']={name:hashlib.sha256((folder/name).read_bytes()).hexdigest() for name in ('proposals.csv','audit.csv','metadata.json')}
    print(json.dumps(output,indent=2))

if __name__=='__main__':
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument('folder',type=Path)
    main(ap.parse_args().folder)
