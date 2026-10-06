#!/usr/bin/env python3
"""Serialize pushes to main with a lock held on the remote: the `landing-lock` branch.

Many agents push to `main` directly (owner decision). A push races whenever `main` moves while
someone is rebasing and running checks. Whoever holds the lock is the only one who pushes to
`main`; everyone else keeps working locally and waits.

    python tools/land.py acquire --who <name> [--wait 900]   # take the lock (retries while held)
    git pull --rebase origin main && <run the checks> && git push origin HEAD:main
    python tools/land.py release --who <name>                 # always, even if the push failed
    python tools/land.py status                               # who holds it, and since when

The lock is a commit on `refs/heads/landing-lock` whose message names the holder and the time.
Taking it is an atomic create-if-absent (`--force-with-lease` with an empty expected value), and
releasing deletes it only if it is still the holder's own commit, so nobody can drop someone
else's lock by mistake. A lock older than STALE_MINUTES is treated as abandoned and broken by the
next `acquire`. Hold it only for rebase, checks and push, never while editing.
"""

from __future__ import annotations

import argparse
import datetime as dt
import subprocess
import sys
import time
from functools import partial

print = partial(print, flush=True)  # noqa: A001 - keep stdout and stderr in order

REF = "refs/heads/landing-lock"
STALE_MINUTES = 15
PREFIX = "landing-lock held by "
EMPTY_TREE = "4b825dc642cb6eb9a060e54bf8d69288fbee4904"


def git(*args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(["git", *args], capture_output=True, text=True, check=check)


def holder() -> tuple[str, str, dt.datetime] | None:
    """(sha, who, since) of the current lock, or None when nobody holds it."""
    out = git("ls-remote", "origin", REF).stdout.split()
    if not out:
        return None
    sha = out[0]
    if git("fetch", "-q", "origin", REF, check=False).returncode != 0:
        return None  # deleted between the two calls
    subject = git("log", "-1", "--format=%s", "FETCH_HEAD").stdout.strip()
    stamp = git("log", "-1", "--format=%cI", "FETCH_HEAD").stdout.strip()
    who = subject[len(PREFIX):].split(" ", 1)[0] if subject.startswith(PREFIX) else "?"
    return sha, who, dt.datetime.fromisoformat(stamp).astimezone(dt.timezone.utc)


def age_minutes(since: dt.datetime) -> float:
    return (dt.datetime.now(dt.timezone.utc) - since).total_seconds() / 60


def try_create(who: str) -> bool:
    now = dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds")
    sha = git("commit-tree", EMPTY_TREE, "-m", f"{PREFIX}{who} since {now}").stdout.strip()
    push = git("push", f"--force-with-lease={REF}:", "origin", f"{sha}:{REF}", check=False)
    return push.returncode == 0


def delete(sha: str) -> bool:
    push = git("push", f"--force-with-lease={REF}:{sha}", "origin", f":{REF}", check=False)
    return push.returncode == 0


def acquire(who: str, wait: int) -> int:
    deadline = time.monotonic() + wait
    announced = None
    while True:
        if try_create(who):
            print(f"lock acquired by {who}")
            return 0
        current = holder()
        if current:
            sha, owner, since = current
            if owner == who:
                print(f"{who} already holds the lock (since {since:%H:%M:%SZ})")
                return 0
            if age_minutes(since) > STALE_MINUTES:
                print(f"breaking a stale lock of {owner} ({age_minutes(since):.0f} min old)")
                delete(sha)
                continue
            if announced != sha:
                print(f"held by {owner} for {age_minutes(since):.1f} min; waiting")
                announced = sha
        if time.monotonic() > deadline:
            print("gave up waiting; nothing was pushed", file=sys.stderr)
            return 1
        time.sleep(20)


def release(who: str) -> int:
    current = holder()
    if not current:
        print("no lock held")
        return 0
    sha, owner, _ = current
    if owner != who:
        print(f"the lock belongs to {owner}, not {who}; not releasing", file=sys.stderr)
        return 1
    if delete(sha):
        print(f"lock released by {who}")
        return 0
    print("release failed (the lock changed); run status", file=sys.stderr)
    return 1


def status() -> int:
    current = holder()
    if not current:
        print("free")
    else:
        _, owner, since = current
        print(f"held by {owner} since {since:%Y-%m-%d %H:%M:%SZ} ({age_minutes(since):.1f} min)")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    a = sub.add_parser("acquire")
    a.add_argument("--who", required=True)
    a.add_argument("--wait", type=int, default=900, help="seconds to keep retrying")
    r = sub.add_parser("release")
    r.add_argument("--who", required=True)
    sub.add_parser("status")
    args = ap.parse_args()
    if args.cmd == "acquire":
        return acquire(args.who, args.wait)
    if args.cmd == "release":
        return release(args.who)
    return status()


if __name__ == "__main__":
    sys.exit(main())
