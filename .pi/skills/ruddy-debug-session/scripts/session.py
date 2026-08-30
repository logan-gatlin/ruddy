#!/usr/bin/env python3
"""Inspect and edit the browser's live Ruddy debugger session."""

import json
import os
import sys
import urllib.error
import urllib.request

BASE = os.environ.get("RUDDY_DEBUG_URL", "http://127.0.0.1:7878").rstrip("/")


def request(path="/session", method="GET", body=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(
        BASE + path,
        data=data,
        method=method,
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req) as response:
            raw = response.read()
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as error:
        message = error.read().decode(errors="replace").strip()
        print(f"debugger API: HTTP {error.code}: {message}", file=sys.stderr)
        raise SystemExit(2 if error.code == 409 else 1)
    except urllib.error.URLError as error:
        print(f"debugger API: cannot connect to {BASE}: {error.reason}", file=sys.stderr)
        raise SystemExit(1)


def session():
    return request()


def source_file(shared, wanted=None):
    wanted = wanted or shared["view"].get("active_file")
    for file in shared["request"]["files"]:
        if file["path"] == wanted:
            return file
    print(f"file is not in the live bundle: {wanted}", file=sys.stderr)
    raise SystemExit(1)


def line_col(shared, file_path, byte):
    info = next((f for f in shared["snapshot"]["files"] if f["path"] == file_path), None)
    if not info:
        return "?"
    starts = info["line_starts"]
    line = 0
    for index, start in enumerate(starts):
        if start > byte:
            break
        line = index
    return f"{line + 1}:{byte - starts[line] + 1}"


def context(shared):
    view = shared["view"]
    snap = shared["snapshot"]
    active = view.get("active_file") or "?"
    caret = view.get("caret", 0)
    print(
        f"session r{shared['session_revision']} | "
        f"{shared['request']['document']}/{active}:{line_col(shared, active, caret)} | "
        f"build {snap['build']} compile r{snap['revision']}"
    )
    panes = view.get("panes", [])
    if panes:
        rendered = []
        for pane in panes:
            detail = f"{pane.get('stage')}:{pane.get('view')}"
            if pane.get("filter"):
                detail += f" filter={pane['filter']!r}"
            if pane.get("step") is not None:
                detail += f" step={pane['step']}"
            detail += f" visible_nodes={len(pane.get('visible_nodes', []))}"
            rendered.append(detail)
    else:
        tabs = view.get("tabs", [])
        visible = tabs[: 2 if view.get("split") else 1]
        rendered = [f"{stage}:{view.get('views', {}).get(stage, 'default')}" for stage in visible]
    print("visible: " + (", ".join(rendered) or "unknown"))
    selection = view.get("selection")
    if selection:
        print("selection: " + json.dumps(selection, separators=(",", ":")))

    diagnostics = snap.get("diagnostics", [])
    print(f"diagnostics: {len(diagnostics)}")
    for diag in diagnostics:
        where = "generated"
        span = diag.get("span")
        if span:
            info = snap["files"][span["file"]]
            where = f"{info['path']}:{line_col(shared, info['path'], span['range'][0])}"
        print(f"  {diag['severity']} {diag['code']} {where}: {diag['message']}")
        if diag.get("label"):
            print(f"    {diag['label']}")

    print("stages:")
    for stage in snap.get("stages", []):
        timing = "" if stage.get("micros") is None else f" {stage['micros']}us"
        print(f"  {stage['id']:<12} {stage['status']:<8}{timing}  {stage['summary']}")
    if snap.get("panic"):
        panic = snap["panic"]
        print(f"panic in {panic['stage']}: {panic['message']} at {panic['location']}")


def usage():
    print("usage: session.py context|dump|file [PATH]|stage ID|write PATH", file=sys.stderr)
    raise SystemExit(2)


def main():
    if len(sys.argv) < 2:
        usage()
    command = sys.argv[1]
    shared = session()

    if command == "context" and len(sys.argv) == 2:
        context(shared)
    elif command == "dump" and len(sys.argv) == 2:
        json.dump(shared, sys.stdout, indent=2)
        print()
    elif command == "file" and len(sys.argv) <= 3:
        sys.stdout.write(source_file(shared, sys.argv[2] if len(sys.argv) == 3 else None)["source"])
    elif command == "stage" and len(sys.argv) == 3:
        stage = next((stage for stage in shared["snapshot"]["stages"] if stage["id"] == sys.argv[2]), None)
        if stage is None:
            print(f"unknown stage: {sys.argv[2]}", file=sys.stderr)
            raise SystemExit(1)
        json.dump(stage, sys.stdout, indent=2)
        print()
    elif command == "write" and len(sys.argv) == 3:
        # Validate the path against the version just read before consuming
        # stdin, so a typo cannot look like a successful empty replacement.
        source_file(shared, sys.argv[2])
        updated = request(
            method="PUT",
            body={
                "base_revision": shared["session_revision"],
                "document": shared["request"]["document"],
                "path": sys.argv[2],
                "source": sys.stdin.read(),
            },
        )
        context(updated)
    else:
        usage()


if __name__ == "__main__":
    main()
