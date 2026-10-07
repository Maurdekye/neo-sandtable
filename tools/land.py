#!/usr/bin/env python3
"""Serialize pushes to main with a lock held on the remote: the `landing-lock` branch.

Many agents push to `main` directly (owner decision). A push races whenever `main` moves while
someone is rebasing and running checks. Whoever holds the lock is the only one who pushes to
`main`; everyone else keeps working locally and waits their turn.

    python tools/land.py acquire --who <name> [--wait 3600]  # queue, then take the lock
    git pull --rebase origin main && <run the checks> && git push origin HEAD:main
    python tools/land.py release --who <name>                # always, even if the push failed
    python tools/land.py status                              # holder and queue
    python tools/land.py refresh --who <name>                # holder: restart the stale clock
    python tools/land.py leave --who <name>                  # give up your place in the queue

The lock is a commit on `refs/heads/landing-lock` whose message names the holder and the time.
Taking it is an atomic create-if-absent (`--force-with-lease` with an empty expected value), and
releasing deletes it only if it is still the holder's own commit, so nobody can drop someone
else's lock by mistake. A lock older than STALE_MINUTES is treated as abandoned and broken by the
next `acquire`; a holder whose checks run long calls `refresh`. Hold it only for rebase, checks
and push, never while editing.

Waiters queue fairly. `acquire` first places a ticket, `refs/heads/landing-queue/<UTC time>-<name>`,
and only the oldest ticket may take a free lock. Re-running `acquire` keeps your ticket, so a
wait that times out or is killed does not lose your place. A ticket whose owner does not take a
free lock within GRACE_SECONDS is dropped by the next waiter, so a dead waiter never blocks the
queue.
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
QUEUE = "refs/heads/landing-queue/"
STALE_MINUTES = 25
GRACE_SECONDS = 90
POLL_SECONDS = 10
PREFIX = "landing-lock held by "
EMPTY_TREE = "4b825dc642cb6eb9a060e54bf8d69288fbee4904"

Ticket = tuple[str, str, str]  # (ref, sha, who), oldest first when sorted


def git(*args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(["git", *args], capture_output=True, text=True, check=check)


def now() -> dt.datetime:
    return dt.datetime.now(dt.timezone.utc)


def remote() -> tuple[str | None, list[Ticket]]:
    """The lock's sha (None when free) and the queue, oldest ticket first."""
    out = git("ls-remote", "origin", REF, f"{QUEUE}*").stdout
    lock, queue = None, []
    for line in out.splitlines():
        sha, ref = line.split("\t")
        if ref == REF:
            lock = sha
        elif ref.startswith(QUEUE):
            queue.append((ref, sha, ref[len(QUEUE):].partition("-")[2]))
    return lock, sorted(queue)


_holders: dict[str, tuple[str, dt.datetime]] = {}


def describe(sha: str) -> tuple[str, dt.datetime] | None:
    """(who, since) of the lock commit `sha`, or None if it vanished meanwhile."""
    if sha not in _holders:
        if git("fetch", "-q", "origin", REF, check=False).returncode != 0:
            return None
        fetched = git("rev-parse", "FETCH_HEAD").stdout.strip()
        subject = git("log", "-1", "--format=%s", fetched).stdout.strip()
        stamp = git("log", "-1", "--format=%cI", fetched).stdout.strip()
        who = subject[len(PREFIX):].split(" ", 1)[0] if subject.startswith(PREFIX) else "?"
        _holders[fetched] = (who, dt.datetime.fromisoformat(stamp).astimezone(dt.timezone.utc))
        if fetched != sha:
            return None  # the lock changed hands between the two calls; look again
    return _holders[sha]


def holder() -> tuple[str, str, dt.datetime] | None:
    """(sha, who, since) of the current lock, or None when nobody holds it."""
    sha, _ = remote()
    info = describe(sha) if sha else None
    return (sha, *info) if sha and info else None


def age_minutes(since: dt.datetime) -> float:
    return (now() - since).total_seconds() / 60


def lock_commit(who: str) -> str:
    stamp = now().isoformat(timespec="seconds")
    return git("commit-tree", EMPTY_TREE, "-m", f"{PREFIX}{who} since {stamp}").stdout.strip()


def push_ref(ref: str, new: str, expected: str) -> bool:
    """Point `ref` at `new` ("" deletes it) only if it is still `expected` ("" = absent)."""
    push = git("push", f"--force-with-lease={ref}:{expected}", "origin", f"{new}:{ref}", check=False)
    return push.returncode == 0


def take_ticket(who: str) -> str:
    ref = f"{QUEUE}{now():%Y%m%dT%H%M%S%f}-{who}"
    sha = git("commit-tree", EMPTY_TREE, "-m", f"landing-queue ticket for {who}").stdout.strip()
    if not push_ref(ref, sha, ""):
        raise SystemExit(f"could not place a queue ticket for {who}; check the remote")
    return ref


def drop_tickets(who: str, queue: list[Ticket]) -> None:
    for ref, sha, owner in queue:
        if owner == who:
            push_ref(ref, "", sha)


def acquire(who: str, wait: int) -> int:
    deadline = time.monotonic() + wait
    _, queue = remote()
    mine = [t for t in queue if t[2] == who]
    ticket = mine[0][0] if mine else take_ticket(who)
    drop_tickets(who, [t for t in mine[1:]])
    announced = None
    idle_head: tuple[str, float] | None = None  # (ticket, since when it could have taken the lock)
    while True:
        lock, queue = remote()
        info = describe(lock) if lock else None
        if lock and not info:
            continue  # changed hands while we looked
        if info and info[0] == who:
            drop_tickets(who, queue)
            print(f"{who} already holds the lock (since {info[1]:%H:%M:%SZ})")
            return 0
        if info and age_minutes(info[1]) > STALE_MINUTES:
            print(f"breaking a stale lock of {info[0]} ({age_minutes(info[1]):.0f} min old)")
            push_ref(REF, "", lock)
            continue
        if not any(t[0] == ticket for t in queue):
            ticket = take_ticket(who)  # dropped as idle while this process was not running
            print("our ticket had been dropped as idle; queued again at the back")
            continue
        head = queue[0]
        if not lock and head[0] == ticket:
            if push_ref(REF, lock_commit(who), ""):
                drop_tickets(who, queue)
                print(f"lock acquired by {who}")
                return 0
            continue  # someone without a ticket (an older land.py) took it first
        if not lock:
            if idle_head and idle_head[0] == head[0]:
                if time.monotonic() - idle_head[1] > GRACE_SECONDS:
                    print(f"dropping the idle ticket of {head[2]}")
                    push_ref(head[0], "", head[1])
                    idle_head = None
                    continue
            else:
                idle_head = (head[0], time.monotonic())
        else:
            idle_head = None
        position = next(i for i, t in enumerate(queue) if t[0] == ticket) + 1
        state = (position, info[0] if info else None)
        if announced != state:
            held = f"held by {info[0]} for {age_minutes(info[1]):.1f} min" if info else "free"
            print(f"position {position} of {len(queue)} in the queue; lock {held}; waiting")
            announced = state
        if time.monotonic() > deadline:
            drop_tickets(who, queue)
            print("gave up waiting and left the queue; nothing was pushed", file=sys.stderr)
            return 1
        time.sleep(POLL_SECONDS)


def release(who: str) -> int:
    lock, queue = remote()
    drop_tickets(who, queue)
    info = describe(lock) if lock else None
    if not info:
        print("no lock held")
        return 0
    if info[0] != who:
        print(f"the lock belongs to {info[0]}, not {who}; not releasing", file=sys.stderr)
        return 1
    if push_ref(REF, "", lock):
        print(f"lock released by {who}")
        return 0
    print("release failed (the lock changed); run status", file=sys.stderr)
    return 1


def refresh(who: str) -> int:
    lock, _ = remote()
    info = describe(lock) if lock else None
    if not info or info[0] != who:
        print(f"{who} does not hold the lock; nothing to refresh", file=sys.stderr)
        return 1
    if push_ref(REF, lock_commit(who), lock):
        print(f"lock refreshed for {who}")
        return 0
    print("refresh failed (the lock changed); run status", file=sys.stderr)
    return 1


def leave(who: str) -> int:
    _, queue = remote()
    drop_tickets(who, queue)
    print(f"{who} left the queue")
    return 0


def status() -> int:
    lock, queue = remote()
    info = describe(lock) if lock else None
    if not info:
        print("free")
    else:
        print(f"held by {info[0]} since {info[1]:%Y-%m-%d %H:%M:%SZ} ({age_minutes(info[1]):.1f} min)")
    for i, (ref, _, who) in enumerate(queue, 1):
        print(f"  {i}. {who} (queued {ref[len(QUEUE):].partition('-')[0]})")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    a = sub.add_parser("acquire")
    a.add_argument("--who", required=True)
    a.add_argument("--wait", type=int, default=3600, help="seconds to wait in the queue")
    for name in ("release", "refresh", "leave"):
        sub.add_parser(name).add_argument("--who", required=True)
    sub.add_parser("status")
    args = ap.parse_args()
    if args.cmd == "acquire":
        return acquire(args.who, args.wait)
    if args.cmd == "status":
        return status()
    return {"release": release, "refresh": refresh, "leave": leave}[args.cmd](args.who)


if __name__ == "__main__":
    sys.exit(main())
