"""Source-local line proposals and seeded audit sheets; never publish features.

Python3.10+ with Pillow. Scores are heuristic, not measured accuracy. Sampling
compares paired brown strokes, gray dashes and railway tie-width patterns.
"""
import argparse
from collections import Counter
import csv
import hashlib
import json
import math
import time
import os
from pathlib import Path
import random
from generate_grid import extract, IMAGE

ALGORITHM = "line-pattern-v0.4"
KINDS = ("road", "unfinished_road", "track", "railroad")

def brown(c):
    if c is None: return False
    r,g,b=c
    return r<130 and g<120 and b<90 and 0<=r-g<=30 and g-b>12

def gray(c):
    return c is not None and 40<=min(c) and max(c)<150 and max(c)-min(c)<20

def scan(image, a, b):
    dx,dy=b.x-a.x,b.y-a.y;length=math.hypot(dx,dy)
    nx,ny=dx/length,dy/length;tx,ty=-ny,nx
    mx,my=(a.x+b.x)/2,(a.y+b.y)/2
    pixels={};clipped=False
    def pixel(x,y):
        nonlocal clipped
        key=(round(x),round(y))
        if not (0<=key[0]<image.width and 0<=key[1]<image.height):
            clipped=True
            return None
        if key not in pixels: pixels[key]=image.getpixel(key)
        return pixels[key]
    # Search across the full shared side; crossing orientation excludes tangent
    # grid strokes. Offset seeds skip sides with no relevant ink.
    candidates=[]
    for offset in range(-24,25):
        cx,cy=mx+tx*offset,my+ty*offset
        ink=[pixel(cx+nx*s+tx*t,cy+ny*s+ty*t) for s in (-5,0,5) for t in (-3,-2,0,2,3)]
        if not any(brown(c) or gray(c) for c in ink): continue
        for degrees in range(-60,61,5):
            angle=math.radians(degrees)
            ux,uy=nx*math.cos(angle)+tx*math.sin(angle),ny*math.cos(angle)+ty*math.sin(angle)
            vx,vy=-uy,ux
            road_pairs=[];centers=[];outer=[]
            for s in range(-18,19,2):
                x,y=cx+ux*s,cy+uy*s
                # Each stroke can have a one-pixel antialias shift.
                left=any(brown(pixel(x+vx*t,y+vy*t)) for t in (-3,-2,-1))
                right=any(brown(pixel(x+vx*t,y+vy*t)) for t in (1,2,3))
                gap=pixel(x,y)
                road_pairs.append(left and right and gap is not None and min(gap)>170)
                centers.append(any(gray(pixel(x+vx*t,y+vy*t)) for t in (-1,0,1)))
                outer.append(any(gray(pixel(x+vx*t,y+vy*t)) for t in (-5,-4,4,5)))
            paired=sum(road_pairs)/len(road_pairs)
            center=sum(centers)/len(centers);wide=sum(outer)/len(outer)
            transitions=sum(x!=y for x,y in zip(centers,centers[1:]))
            rail=center*min(1.0,wide/.25) if center>=.65 else 0.0
            track=center*max(0.0,1-wide/.25) if 2<=transitions<=10 and .2<=center<=.8 else 0.0
            candidates.append(dict(paired=paired,track=track,rail=rail,center=center,wide=wide,
                                   offset=offset,angle=degrees,transitions=transitions))
    best={name:max((c[name] for c in candidates),default=0.0) for name in ("paired","track","rail")}
    out=[]
    for kind in KINDS:
        score=best["paired"] if kind in {"road","unfinished_road"} else best["rail" if kind=="railroad" else "track"]
        if clipped or kind=="unfinished_road":
            # Gaps can also be occlusion/curvature. Never auto-identify unfinished
            # status from lower support alone; review it separately.
            proposal="uncertain";confidence=0.0
        elif score >= (.75 if kind in {"road","railroad"} else .45):
            proposal="present";confidence=min(.95,.5+.5*score)
        elif score==0:
            proposal="absent";confidence=.8
        else:
            proposal="uncertain";confidence=max(0.05,score)
        out.append(dict(kind=kind,proposal=proposal,confidence=round(confidence,4),score=round(score,4)))
    return out

def select_audit(rows, seed, n):
    if n<1: raise ValueError("Audit sample must be positive")
    rng=random.Random(seed)
    chosen=[]
    for kind in KINDS:
        for state in ("present","absent","uncertain"):
            pool=sorted((r for r in rows if r["kind"]==kind and r["proposal"]==state),
                        key=lambda r:(r["from_hex"],r["to_hex"]))
            chosen.extend(rng.sample(pool,min(n,len(pool))))
    return sorted(chosen,key=lambda r:(r["kind"],r["proposal"],r["from_hex"],r["to_hex"]))

def validate_output(sources, output):
    output=output.resolve();repo=Path(__file__).resolve().parents[2]
    if output.is_relative_to(repo) or output.is_relative_to(sources.resolve()):
        raise ValueError("Inspection/proposal output must be outside repo and sources")
    if output.exists() and any(output.iterdir()):
        raise ValueError("Use a new empty output folder; existing reviews must never be overwritten")
    return output

def propose(sources,output,section,first,second,seed,n):
    from PIL import Image,ImageDraw
    output=validate_output(sources,output)
    if first[0]>first[1] or second[0]>second[1]: raise ValueError("Inverted bounds")
    if n<1: raise ValueError("Audit sample must be positive")
    records,_,_,build_hash,_=extract(sources)
    by_ax={(h.q,h.r):h for h in records}
    by_id={h.hex_id:h for h in records}
    selected=[h for h in records if h.hex_id[0]==section and first[0]<=h.first<=first[1] and second[0]<=h.second<=second[1]]
    if not selected: raise ValueError("Empty pilot")
    edges=set()
    for h in selected:
        for dq,dr in [(1,0),(0,1),(-1,1),(-1,0),(0,-1),(1,-1)]:
            neighbour=by_ax.get((h.q+dq,h.r+dr))
            if neighbour: edges.add(tuple(sorted((h.hex_id,neighbour.hex_id))))
    path=sources/"vassal/extracted/images"/IMAGE
    image=Image.open(path).convert("RGB")
    started=time.perf_counter()
    rows=[]
    for a,b in sorted(edges):
        for sample in scan(image,by_id[a],by_id[b]):
            rows.append(dict(from_hex=a,to_hex=b,**sample))
    audit=select_audit(rows,seed,n)
    output.mkdir(parents=True,exist_ok=True)
    def write(name,rows,fields):
        with (output/name).open("w",newline="",encoding="utf-8") as f:
            writer=csv.DictWriter(f,fields,lineterminator="\n");writer.writeheader();writer.writerows(rows)
    fields=["from_hex","to_hex","kind","proposal","confidence","score"]
    write("proposals.csv",rows,fields)
    write("audit.csv",[dict(r,observed="",note="") for r in audit],fields+["observed","note"])
    summary=Counter((r["kind"],r["proposal"]) for r in rows)
    metadata=dict(algorithm=ALGORITHM,build_file_sha256=build_hash,source_image_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
                  selection=dict(section=section,first=first,second=second),selected_cells=len(selected),edges=len(edges),
                  seed=seed,sample_per_kind_state=n,elapsed_seconds=round(time.perf_counter()-started,3),populations={f"{k}/{s}":v for (k,s),v in sorted(summary.items())},
                  calibration="unmeasured",unknown_kinds=["unfinished_railroad","pipeline"],publication="none")
    (output/"metadata.json").write_text(json.dumps(metadata,indent=2)+"\n",encoding="utf-8")
    # Identical edge can be sampled for several kinds: sheet per kind keeps
    # numerical proposals and independent presence/absence review together.
    for kind in KINDS:
        entries=[r for r in audit if r["kind"]==kind]
        if not entries: continue
        w,h,columns=170,155,4
        sheet=Image.new("RGB",(w*columns,h*math.ceil(len(entries)/columns)),"white")
        for i,r in enumerate(entries):
            a,b=by_id[r["from_hex"]],by_id[r["to_hex"]];mx,my=round((a.x+b.x)/2),round((a.y+b.y)/2)
            cell=Image.new("RGB",(w,h),"white");cell.paste(image.crop((mx-85,my-61,mx+85,my+62)),(0,32))
            draw=ImageDraw.Draw(cell);draw.text((2,0),f"{a.hex_id}/{b.hex_id}",fill="black")
            draw.text((2,14),f"{r['proposal']} {r['confidence']}",fill="black")
            dx,dy=b.x-a.x,b.y-a.y;length=math.hypot(dx,dy)
            tx,ty=-dy/length,dx/length
            draw.line((85-tx*24,93-ty*24,85+tx*24,93+ty*24),fill="#ff00ff",width=1)
            draw.ellipse((82,90,88,96),outline="#ff00ff")
            sheet.paste(cell,(i%columns*w,i//columns*h))
        sheet.save(output/f"audit-{kind}.png")
    print(json.dumps(metadata,indent=2))
    print("Proposals only. All uncertain/low-confidence edges still require review; audit sheets do not publish coverage.")

if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--output",type=Path,required=True)
    ap.add_argument("--section",choices=list("ABCDE"),required=True)
    ap.add_argument("--first",type=int,nargs=2,required=True)
    ap.add_argument("--second",type=int,nargs=2,required=True)
    ap.add_argument("--seed",type=int,default=6022)
    ap.add_argument("--sample",type=int,default=8)
    args=ap.parse_args()
    if not os.environ.get("CNA_SOURCES"): ap.error("Set CNA_SOURCES")
    propose(Path(os.environ["CNA_SOURCES"]),args.output,args.section,args.first,args.second,args.seed,args.sample)
