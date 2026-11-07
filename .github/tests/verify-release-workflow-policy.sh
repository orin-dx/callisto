#!/usr/bin/env bash
# Policy check for the release workflow: the rules that stop a wrong publish or a leaked credential, asserted on the
# parsed YAML, plus the artifact target tables that must agree across the config, CI, build script and installer.
# Usage: verify-release-workflow-policy.sh [workflow.yml]
set -euo pipefail
exec python3 - "$@" <<'PY'
import json, os, re, shutil, subprocess, sys

DEFAULT = ".github/workflows/callisto-release.yml"
CONFIG = "callisto.toml"
CI_WORKFLOW = ".github/workflows/callisto-ci.yml"
BUILD_SCRIPT = ".github/scripts/build-release-artifact.sh"
INSTALLER = ".github/actions/setup-callisto/scripts/install-callisto.sh"
COORDINATOR_CONST_SRC = "crates/callisto-model/src/release/github.rs"
# Deny-by-default: every `write` scope a job may hold, named explicitly, so a new write permission cannot be added
# unnoticed.
ALLOWED_WRITES = {
    "version-pr": {"contents", "pull-requests"},
    "build-artifact": {"attestations", "id-token"},
    "execute": {"contents"},
    "drain": {"actions"},
}
# The only secrets the publishing job may reference.
EXECUTE_SECRETS = {"CARGO_REGISTRY_TOKEN"}


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
    wf = load(path)
    jobs = wf["jobs"]
    if "secrets." in json.dumps({k: v for k, v in wf.items() if k != "jobs"}):
        errs.append("secrets referenced outside jobs")
    if wf.get("permissions") != {}:
        errs.append("workflow-level permissions must be {}")
    for name, job in jobs.items():
        used = set(re.findall(r"secrets\.([A-Za-z0-9_]+)", json.dumps(job)))
        if name != "execute" and used:
            errs.append(f"{name}: secrets referenced outside execute")
        if name == "execute" and used != EXECUTE_SECRETS:
            errs.append(f"execute: secrets must be exactly {sorted(EXECUTE_SECRETS)}, found {sorted(used)}")
        perms = job.get("permissions")
        if not isinstance(perms, dict) or not perms:
            errs.append(f"{name}: permissions must be declared per job")
            perms = {}
        for scope, level in perms.items():
            if level == "write-all":
                errs.append(f"{name}: write-all")
            if level == "write" and scope not in ALLOWED_WRITES.get(name, set()):
                errs.append(f"{name}: {scope}: write is not in that job's allowed write scopes")
        if any(isinstance(n, dict) and "environment" in n for n in walk(job)):
            errs.append(f"{name}: environment key forbidden (merge is the approval)")
        for step in job.get("steps", []):
            if str(step.get("uses", "")).startswith("actions/checkout@"):
                w = step.get("with") or {}
                keep = name == "execute" and w.get("path") == "release-source"
                if w.get("persist-credentials") is not keep:
                    errs.append(f"{name}: checkout persist-credentials must be {str(keep).lower()}")

    # Only a GitHub-verified merge commit of the managed release PR on main is released.
    rc = jobs.get("release-candidate", {})
    if "github.ref == 'refs/heads/main'" not in str(rc.get("if", "")):
        errs.append("release-candidate must require refs/heads/main")
    script = next((s.get("run", "") for s in rc.get("steps", []) if s.get("id") == "candidate"), "")
    for needle, why in (
        ("^[0-9a-f]{40}$", "a full 40-hex release_source_sha"),
        (".merge_commit_sha == $sha", "the PR's exact merge commit"),
        ('"$verified" == true', "a verified commit"),
    ):
        if needle not in script:
            errs.append(f"release-candidate must require {why}")

    # execute is the one irreversible job: cancelling the run must stop it, and it releases the verified source.
    ex = jobs.get("execute", {})
    ex_if = re.sub(r"\s+", " ", str(ex.get("if", "")))
    if "!cancelled()" not in ex_if or re.search(r"\balways\(\)", ex_if):
        errs.append("execute must gate on !cancelled(), not always()")
    src = [
        s for s in ex.get("steps", [])
        if str(s.get("uses", "")).startswith("actions/checkout@") and (s.get("with") or {}).get("path") == "release-source"
    ]
    if len(src) != 1 or src[0]["with"].get("ref") != "${{ needs.release-candidate.outputs.release_source_sha }}":
        errs.append("execute must check out release_source_sha")
    return errs


def matrix_pairs(path, job):
    wf = load(path)
    return {(e.get("target"), e.get("asset")) for e in wf["jobs"][job]["strategy"]["matrix"]["include"]}


def table_agreement(paths):
    """Every hand-copied target/asset table must equal callisto.toml's [[release.artifact]]."""
    errs = []
    try:
        import tomllib
        with open(paths["config"], "rb") as f:
            declared = tomllib.load(f).get("release", {}).get("artifact", [])
        rust = {(a["target"], a["asset-name"]) for a in declared}
    except ImportError:  # python < 3.11: read the artifact tables by text
        text = open(paths["config"]).read()
        rust = set(re.findall(r'target\s*=\s*"([^"]+)"\s*\nasset-name\s*=\s*"([^"]+)"', text))
    if not rust:
        return [f"{paths['config']}: no [[release.artifact]] target/asset-name pairs found"]
    rust_assets = {a for _, a in rust}

    def compare(label, found):
        for t, a in sorted(rust - found):
            errs.append(f"{label}: missing ({t}, {a}) declared in {paths['config']}")
        for t, a in sorted(found - rust):
            errs.append(f"{label}: ({t}, {a}) disagrees with {paths['config']}")

    compare(f"{paths['release']} build-artifact matrix", matrix_pairs(paths["release"], "build-artifact"))
    ci = load(paths["ci"])
    ci_pairs = set()
    for job in ci["jobs"].values():
        for e in (job.get("strategy") or {}).get("matrix", {}).get("include", []) or []:
            if "target" in e and "asset" in e:
                ci_pairs.add((e["target"], e["asset"]))
    compare(f"{paths['ci']} preflight matrix", ci_pairs)
    script = open(paths["script"]).read()
    compare(f"{paths['script']} tuples", set(re.findall(r"^\s*\w+:([^:\s]+):([^\s|\\)]+)[\s|\\)]*$", script, re.M)))
    installer = open(paths["installer"]).read()
    for asset in set(re.findall(r'ASSET_NAME="([^"]+)"', installer)):
        if asset not in rust_assets:
            errs.append(f"{paths['installer']}: asset {asset} disagrees with {paths['config']}")
    return errs


def coordinator_path_agreement(path, src):
    """The Rust constant is the one spelling; the workflow file must exist there."""
    m = re.search(r'RELEASE_COORDINATOR_WORKFLOW_PATH:\s*&str\s*=\s*"([^"]+)"', open(src).read())
    if not m:
        return [f"{src}: RELEASE_COORDINATOR_WORKFLOW_PATH not found"]
    errs = []
    if m.group(1) != path:
        errs.append(f"{src}: coordinator path {m.group(1)} disagrees with the checked workflow {path}")
    if not os.path.isfile(m.group(1)):
        errs.append(f"{src}: coordinator workflow {m.group(1)} does not exist")
    for signer in re.findall(r"--signer-workflow\s+(\S+)", open(INSTALLER).read()):
        if not signer.endswith("/" + m.group(1)):
            errs.append(f"{INSTALLER}: --signer-workflow {signer} disagrees with {src} coordinator path {m.group(1)}")
    return errs


path = sys.argv[1] if len(sys.argv) > 1 else DEFAULT
paths = {"config": CONFIG, "release": path, "ci": CI_WORKFLOW, "script": BUILD_SCRIPT, "installer": INSTALLER}
errs = check(path) + table_agreement(paths) + coordinator_path_agreement(DEFAULT, COORDINATOR_CONST_SRC)
if errs:
    print("release workflow policy failed:\n  " + "\n  ".join(errs), file=sys.stderr)
    sys.exit(1)
print("release workflow policy ok")
PY
