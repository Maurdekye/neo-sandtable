#!/usr/bin/env python3
"""Check initial deployment and exact reinforcement coverage of both group-one scenarios.
Usage: python tools/units/coverage.py [repo_root] (Python >=3.11).
"""
import sys
import tomllib
from collections import defaultdict
from pathlib import Path

root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parents[2]
errors = []
units, children, sheets, sides = {}, defaultdict(list), defaultdict(list), {}

def read(path):
    with path.open('rb') as stream:
        return tomllib.load(stream)

for path in sorted((root / 'data/units/oa').glob('*/*.toml')):
    data = read(path)
    sid = data['sheet']['id']
    for unit in data.get('unit', []):
        uid = unit['id']
        units[uid] = unit
        sheets[sid].append(uid)
        sides[uid] = data['sheet']['side']
        if 'parent' in unit:
            children[unit['parent']].append(uid)

def descendants(uid, skip, trail=()):
    if uid in trail:
        errors.append(f'cyclic OA parent chain: {trail + (uid,)}')
        return []
    out = []
    for child in children[uid]:
        if child not in skip:
            out.append(child)
            out.extend(descendants(child, skip, trail + (uid,)))
    return out

def deployed(uid):
    return units[uid]['arrives'] == 'D'

def expand(entry, eligible, subtree_default=False):
    skip = set(entry.get('less', [])) | set(entry.get('det', []))
    if 'sheet' in entry:
        ids = sheets[entry['sheet']]
    else:
        uid = entry['unit']
        ids = [uid]
        if not entry.get('hq_only') and entry.get('subtree', subtree_default):
            ids += descendants(uid, skip)
    ids += entry.get('att', [])
    ids += entry.get('assg', [])
    return [uid for uid in ids if uid not in skip and eligible(uid)]

def clock(unit):
    arr = unit.get('arrives')
    return (arr['gt'], arr['opstage']) if isinstance(arr, dict) else None

for scenario_id in ('graziani', 'italian_campaign'):
    folder = root / 'data/scenarios' / scenario_id
    scenario = read(folder / 'scenario.toml')['scenario']
    start = (scenario['start']['gt'], scenario['start']['opstage'])
    end = (scenario['end']['gt'], scenario['end']['opstage'])
    setup_folder = root / 'data/scenarios' / scenario.get('setup_from', scenario_id)
    placed = defaultdict(list)
    for side, filename in (('axis', 'land_axis.toml'), ('commonwealth', 'land_cw.toml')):
        own = folder / filename
        path = own if own.exists() else setup_folder / filename
        for group in read(path)['group']:
            for entry in group.get('unit', []):
                for uid in expand(entry, lambda _: True, subtree_default=True):
                    # Setup expansion includes only D descendants; roots/attachments must also be D.
                    if not deployed(uid):
                        if uid == entry.get('unit') or uid in entry.get('att', []):
                            errors.append(f'{scenario_id}: setup places non-D {uid}')
                        continue
                    if sides[uid] != side:
                        errors.append(f'{scenario_id}: {uid} placed on the wrong side')
                    placed[uid].append(group['id'])
    expected = {uid for uid in units if deployed(uid)}
    for uid in sorted(expected):
        if len(placed[uid]) != 1:
            errors.append(f'{scenario_id}: {uid} placed {len(placed[uid])} times {placed[uid]}')
    for side in ('axis', 'commonwealth'):
        want = {uid for uid in expected if sides[uid] == side}
        got = {uid for uid in want if len(placed[uid]) == 1}
        print(f'{scenario_id} {side}: initial {len(got)}/{len(want)}')

    manifest = read(folder / 'arrivals.toml')
    span = manifest['file']['range']
    if (span['from_gt'], span['to_gt']) != (start[0], end[0]):
        errors.append(f'{scenario_id}: arrivals range does not match scenario clock')
    if scenario_id == 'italian_campaign' and not manifest['file'].get('complete'):
        errors.append(f'{scenario_id}: arrivals manifest is incomplete')
    arrived, withdrawn = defaultdict(list), defaultdict(list)
    seen_sources = set()
    for source in manifest['source']:
        ref = source['file']
        if ref in seen_sources:
            errors.append(f'{scenario_id}: repeated schedule source {ref}')
        seen_sources.add(ref)
        data = read(root / ref)
        covered = data['file']['covers_gt']
        if covered[0] > start[0] or covered[1] < end[0]:
            errors.append(f'{scenario_id}: {ref} only covers {covered}')
        side = data['file']['side']
        for kind in ('arrival', 'replacement', 'withdrawal'):
            for index, row in enumerate(data.get(kind, [])):
                if 'gt' in row:
                    turn = row['gt']
                    # Air rows have no land OpStage; they can be distributed by the player.
                    stamp = (turn, row.get('opstage', 1))
                    active = start <= stamp <= end
                else:
                    first, last = row['gt_from'], row['gt_to']
                    if first > last:
                        errors.append(f'{ref}: reversed monthly interval {first}..{last}')
                    active = first <= end[0] and last >= start[0]
                    stamp = None
                if not active:
                    continue
                for entry in row.get('units', []):
                    if kind == 'withdrawal':
                        eligible = lambda uid: deployed(uid) or (clock(units[uid]) is not None and clock(units[uid]) <= stamp)
                        target = withdrawn
                    else:
                        eligible = lambda uid: clock(units[uid]) == stamp
                        target = arrived
                        uid = entry['unit']
                        if not eligible(uid):
                            errors.append(f'{ref} {stamp}: root {uid} OA arrival differs')
                    for uid in expand(entry, eligible):
                        if sides[uid] != side:
                            errors.append(f'{ref}: {uid} belongs to {sides[uid]}, not {side}')
                        target[uid].append(f'{ref}:{kind}[{index}]')
    for uid in sorted(units):
        stamp = clock(units[uid])
        if stamp is not None and start <= stamp <= end and len(arrived[uid]) != 1:
            errors.append(f'{scenario_id}: {uid} arrival {stamp} covered {len(arrived[uid])} times {arrived[uid]}')
    for uid, refs in withdrawn.items():
        if len(refs) != 1:
            errors.append(f'{scenario_id}: repeated mandatory withdrawal of {uid}: {refs}')
    for side in ('axis', 'commonwealth'):
        want = {uid for uid in units if sides[uid] == side and clock(units[uid]) is not None and start <= clock(units[uid]) <= end}
        got = {uid for uid in want if len(arrived[uid]) == 1}
        print(f'{scenario_id} {side}: arrivals {len(got)}/{len(want)}; mandatory withdrawals {sum(sides[u] == side for u in withdrawn)}')
for error in errors:
    print('ERROR', error)
print(f'coverage errors={len(errors)}')
sys.exit(bool(errors))
