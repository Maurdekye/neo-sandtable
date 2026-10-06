"""Replay explicitly accepted line-audit rows into features and per-kind masks.

Source hashes, recorded proposal/audit hashes, canonical adjacency and seeded
cohort are checked. Unresolved rows never create coverage. No image is copied.
"""
import hashlib
import json
from pathlib import Path
from audit_lines import read_csv, report
from geometry import DIRECTIONS
from layers import LINE_KINDS


def csv_hash(path):
    # Git may materialize LF records as CRLF on Windows checkouts.
    text=path.read_text(encoding="utf-8")
    return hashlib.sha256(text.encode("utf-8")).hexdigest()

def load_line_reviews(folder, grid, image_hash, build_hash):
    features=[];coverage=[];seen=set()
    for path in sorted(folder.glob('*/metadata.json')):
        meta=json.loads(path.read_text(encoding='utf-8'))
        if meta.get('publish_reviewed_labels') is not True:
            raise ValueError('Line batch has not explicitly accepted its review labels')
        if meta.get('cohort')!='individually_reviewed_audit_rows_only' or not meta.get('review_basis','').strip():
            raise ValueError('Line batch needs explicit cohort and review basis')
        if meta.get('review_id')!=path.parent.name:
            raise ValueError('Line batch identity differs from folder')
        if meta['source_image_sha256']!=image_hash or meta['build_file_sha256']!=build_hash:
            raise ValueError('Line review source identity changed')
        for name in ('proposals.csv','audit.csv'):
            if csv_hash(path.parent/name)!=meta['files_sha256'].get(name):
                raise ValueError('Line review file hash changed')
        proposals=read_csv(path.parent/'proposals.csv');audit=read_csv(path.parent/'audit.csv')
        report(proposals,audit,meta)
        for r in proposals:
            a,b=r['from_hex'],r['to_hex']
            if a>=b or a not in grid.hexes or b not in grid.hexes:
                raise ValueError('Line endpoints must be sorted canonical hex ids')
            if b not in [grid.neighbour(a,d) for d in DIRECTIONS]:
                raise ValueError('Line endpoints must be adjacent')
        for r in audit:
            if r['observed']=='unresolved': continue
            a,b,kind=r['from_hex'],r['to_hex'],r['kind']
            if kind not in LINE_KINDS: raise ValueError('Unknown line kind')
            identity=(a,b,kind)
            if identity in seen: raise ValueError('Duplicate surveyed edge across line batches; explicit amendment required')
            seen.add(identity)
            src=meta['src_by_kind'].get(kind)
            if not src or not all(isinstance(s,str) and ':' in s for s in src):
                raise ValueError('Line review needs source cases per kind')
            common=dict(src=';'.join(src),review_batch=meta['review_id'])
            coverage.append(dict(layer='line:'+kind,hex_id=a,neighbour_id=b,**common))
            if r['observed']=='present':
                features.append(dict(from_hex=a,to_hex=b,kind=kind,**common))
    return sorted(features,key=lambda r:(r['from_hex'],r['to_hex'],r['kind'])),sorted(coverage,key=lambda r:(r['layer'],r['hex_id'],r['neighbour_id']))
