#!/usr/bin/env bash
# audit-harness/duplicate-gate.sh — CI gate for the F-3 class (crate resolved twice).
#
# VERSION MARKER: duplicate-gate v3 — reads `cargo metadata` and BFS-walks from the
# workspace members along NON-DEV edges. If you see "DUPLICATE <name>" without the
# "(runtime)" marker, you are running v1 (lock-file based) from an old copy: v1
# cannot tell a dev-only duplicate from one that reaches the binary.
#
# Why not "fail on any duplicate": a healthy Rust tree legitimately carries
#   syn 2.x + 3.x (proc-macro ABI), unicode-width 0.1 + 0.2, hashbrown 0.15 + 0.17,
#   windows-sys 0.59 + 0.61 (non-Linux targets).
# A gate on *any* duplicate fails on day one. This one checks the crates whose
# duplication is a real defect and, BY CONSTRUCTION, excludes dev-only
# duplicates: a dev-dependency subtree is not linked into the shipped binary, so
# a second major there (e.g. thiserror 1.x via portable-pty next to the app's
# 2.x) is a warning, never a failure.
#
# Run from the crate root (needs Cargo.lock / Cargo.toml). Exit 0 = no runtime dupes.
set -uo pipefail

echo "duplicate-gate v3 (cargo metadata + non-dev BFS)" >&2

[ -f Cargo.lock ] || { echo "duplicate-gate: run from the crate root (Cargo.lock missing)" >&2; exit 2; }
command -v python3 >/dev/null 2>&1 || { echo "duplicate-gate: python3 required" >&2; exit 2; }

python3 - <<'PY'
import collections, json, subprocess, sys

# Crate families where a duplicate is a defect for THIS app. Extend deliberately:
# every entry must be one the code links directly or through a single API boundary.
CRITICAL = {
    "crossterm", "ratatui", "tokio", "serde", "serde_json",
    "clap", "tracing", "tracing-subscriber", "anyhow", "thiserror",
    "directories", "pulldown-cmark",
}

try:
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked"],
        check=True, capture_output=True, text=True,
    ).stdout)
except (subprocess.CalledProcessError, FileNotFoundError) as exc:
    detail = getattr(exc, "stderr", "") or str(exc)
    print(f"duplicate-gate: cargo metadata failed: {detail.strip()[:400]}", file=sys.stderr)
    sys.exit(2)

name_of = {p["id"]: p["name"] for p in meta["packages"]}
ver_of = {p["id"]: p["version"].split("+")[0] for p in meta["packages"]}

# A package is "runtime" only if it is reachable from a workspace member along
# edges that are not dev-only. Per-EDGE kind is not enough: portable-pty (a
# dev-dependency) reaches thiserror 1.x through a NORMAL edge, but the whole
# subtree is dev-only because nothing in the shipped graph leads there. BFS it.
adj: dict[str, list[tuple[str, set[str]]]] = {}
for node in (meta.get("resolve") or {}).get("nodes", []):
    for dep in node.get("deps", []):
        kinds = {k.get("kind") or "normal" for k in dep.get("dep_kinds", [])}
        adj.setdefault(node["id"], []).append((dep["pkg"], kinds))

roots = [p["id"] for p in meta["packages"] if p["id"] in set(meta.get("workspace_members", []))]
runtime_ids: set[str] = set()
stack = list(roots)
while stack:
    cur = stack.pop()
    for dep, kinds in adj.get(cur, []):
        if kinds - {"dev"} and dep not in runtime_ids:
            runtime_ids.add(dep)
            stack.append(dep)

runtime: dict[str, set[str]] = collections.defaultdict(set)
dev: dict[str, set[str]] = collections.defaultdict(set)
for pid, name in name_of.items():
    if name not in CRITICAL:
        continue
    (runtime if pid in runtime_ids else dev)[name].add(ver_of[pid])

bad_runtime = {n: sorted(v) for n, v in runtime.items() if len(v) > 1}
# Dev-only duplicates: a crate with 2+ versions overall whose copies are NOT all
# reachable at runtime (e.g. thiserror 1.x via portable-pty, 2.x in the binary).
all_versions: dict[str, set[str]] = collections.defaultdict(set)
for bucket in (runtime, dev):
    for name, vers in bucket.items():
        all_versions[name] |= vers
warn_dev = {n: sorted(v) for n, v in all_versions.items() if len(v) > 1 and n not in bad_runtime}

for name, vers in sorted(bad_runtime.items()):
    print(f"DUPLICATE(runtime) {name}: {' '.join(vers)}", file=sys.stderr)
for name, vers in sorted(warn_dev.items()):
    print(f"warning(dev-only) {name}: {' '.join(vers)}", file=sys.stderr)

if bad_runtime:
    print(f"duplicate-gate: {len(bad_runtime)} critical crate(s) linked twice at runtime", file=sys.stderr)
    sys.exit(1)
print(f"duplicate-gate: ok — {len(runtime)} critical crates, one runtime version each")
if warn_dev:
    print(f"duplicate-gate: {len(warn_dev)} dev-only duplicate(s) above (test tooling, not shipped)")
PY
