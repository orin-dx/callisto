#!/usr/bin/env bash
# Parsed-YAML policy check for the release workflow: secrets, permissions,
# credentials, approval gates, timeouts, and concurrency are asserted on the
# structure, not on line text. `--self-test` proves each rule rejects a mutant.
# Usage: verify-release-workflow-policy.sh [--self-test] [workflow.yml]
set -euo pipefail
exec python3 - "$@" <<'PY'
import json, os, re, shutil, subprocess, sys, tempfile

DEFAULT = ".github/workflows/callisto-release.yml"
CONTENTS_WRITE = {"execute", "version-pr"}  # version-pr commits the managed branch via the forge API
OIDC_JOBS = {"build-artifact"}


def load(path):
    if shutil.which("yq"):
        out = subprocess.run(["yq", "-o=json", ".", path], check=True, capture_output=True, text=True).stdout
        return json.loads(out)
    try:
        import yaml
    except ImportError:
        sys.exit("policy check needs yq or python3 PyYAML")
    with open(path) as f:
        return yaml.safe_load(f)


def walk(node):
    yield node
    if isinstance(node, dict):
        for v in node.values():
            yield from walk(v)
    elif isinstance(node, list):
        for v in node:
            yield from walk(v)


def check(path):
    errs = []
    text = open(path).read()
    wf = load(path)
    # PyYAML parses the bare key `on` as True; yq keeps it as a string.
    jobs = wf["jobs"]
    top = {k: v for k, v in wf.items() if k != "jobs"}
    if "secrets." in json.dumps(top):
        errs.append("secrets referenced outside jobs")
    if wf.get("permissions") != {}:
        errs.append("workflow-level permissions must be {}")
    conc = wf.get("concurrency") or {}
    if conc.get("group") != "release-${{ github.sha }}" or conc.get("cancel-in-progress") is not False:
        errs.append("workflow concurrency must be per-SHA with cancel-in-progress false")
    for name, job in jobs.items():
        blob = json.dumps(job)
        if name != "execute" and "secrets." in blob:
            errs.append(f"{name}: secrets referenced outside execute")
        if name == "execute":
            used = set(re.findall(r"secrets\.([A-Za-z0-9_]+)", blob))
            if used != {"CARGO_REGISTRY_TOKEN"}:
                errs.append(f"execute: secrets must be exactly CARGO_REGISTRY_TOKEN, found {sorted(used)}")
        perms = job.get("permissions")
        if not isinstance(perms, dict) or not perms:
            errs.append(f"{name}: permissions must be declared per job")
            perms = {}
        for scope, level in perms.items():
            if level == "write" and scope == "contents" and name not in CONTENTS_WRITE:
                errs.append(f"{name}: contents: write not allowed")
            if level == "write" and scope in ("id-token", "attestations") and name not in OIDC_JOBS:
                errs.append(f"{name}: {scope}: write not allowed")
            if level == "write-all":
                errs.append(f"{name}: write-all")
        if any(isinstance(n, dict) and "environment" in n for n in walk(job)):
            errs.append(f"{name}: environment key forbidden (merge is the approval)")
        t = job.get("timeout-minutes")
        if not isinstance(t, int) or isinstance(t, bool) or t <= 0:
            errs.append(f"{name}: timeout-minutes missing")
        if "concurrency" in job and job["concurrency"].get("cancel-in-progress") is not False:
            errs.append(f"{name}: cancel-in-progress must be false")
        for step in job.get("steps", []):
            if str(step.get("uses", "")).startswith("actions/checkout@"):
                w = step.get("with") or {}
                keep = name == "execute" and w.get("path") == "release-source"
                pc = w.get("persist-credentials")
                if keep and pc is not True:
                    errs.append("execute release-source checkout must persist credentials")
                if not keep and pc is not False:
                    errs.append(f"{name}: checkout must set persist-credentials false")
    for job, group in (("version-pr", "version-pr"), ("execute", "release-execute")):
        c = jobs.get(job, {}).get("concurrency") or {}
        if c.get("group") != group or c.get("cancel-in-progress") is not False:
            errs.append(f"{job}: job concurrency must be group {group} without cancellation")
    rc = jobs.get("release-candidate", {})
    if "github.ref == 'refs/heads/main'" not in str(rc.get("if", "")):
        errs.append("release-candidate must require refs/heads/main")
    if re.search(r"commits/main\b|orchestration_sha=\$\(", text):
        errs.append("orchestration_sha must not come from a moving ref")
    assigns = re.findall(r"^\s*orchestration_sha=.*$", text, re.M)
    if [a.strip() for a in assigns] != ['orchestration_sha="$TRIGGERING_SHA"']:
        errs.append("orchestration_sha must be assigned once, from TRIGGERING_SHA")
    env = next((s.get("env", {}) for s in rc.get("steps", []) if s.get("id") == "candidate"), {})
    if env.get("TRIGGERING_SHA") != "${{ github.sha }}":
        errs.append("TRIGGERING_SHA must be github.sha")
    return errs


def mutants(text):
    def sub(a, b, count=1):
        if a not in text:
            raise SystemExit(f"self-test mutant anchor missing: {a!r}")
        return text.replace(a, b, count)

    def after(anchor, old, new):
        i = text.index(anchor)
        j = text.index(old, i)
        return text[:j] + new + text[j + len(old):]

    return {
        "secret in build-artifact": sub("          CALLISTO_RELEASE_ARTIFACT_KIND: ${{ matrix.kind }}\n",
            "          CALLISTO_RELEASE_ARTIFACT_KIND: ${{ matrix.kind }}\n          LEAK: ${{ secrets.LEAK }}\n"),
        "contents: write in plan": after("\n  plan:\n", "      contents: read", "      contents: write"),
        "environment on execute": sub("    name: Execute approved release\n",
            "    name: Execute approved release\n    environment: release\n"),
        "timeout removed": sub("    timeout-minutes: 60\n", ""),
        "cancel-in-progress true": sub("cancel-in-progress: false", "cancel-in-progress: true"),
        "hardcoded orchestration_sha": sub('orchestration_sha="$TRIGGERING_SHA"', "orchestration_sha=deadbeef"),
        "id-token on execute": after("\n  execute:\n", "      attestations: read\n",
            "      attestations: read\n      id-token: write\n"),
        "NPM_TOKEN on execute": sub("          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}\n",
            "          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}\n          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}\n"),
    }


args = [a for a in sys.argv[1:] if a != "--self-test"]
path = args[0] if args else DEFAULT
errs = check(path)
if errs:
    print("release workflow policy failed:\n  " + "\n  ".join(errs), file=sys.stderr)
    sys.exit(1)
if "--self-test" in sys.argv:
    text = open(path).read()
    bad = 0
    with tempfile.TemporaryDirectory() as d:
        for name, mutated in mutants(text).items():
            p = os.path.join(d, "wf.yml")
            open(p, "w").write(mutated)
            if check(p):
                print(f"self-test ok: mutant rejected: {name}")
            else:
                print(f"self-test FAILED: mutant accepted: {name}", file=sys.stderr)
                bad += 1
    if bad:
        sys.exit(1)
print("release workflow policy ok")
PY
