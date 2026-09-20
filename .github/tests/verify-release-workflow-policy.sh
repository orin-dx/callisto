#!/usr/bin/env bash
# Parsed-YAML policy check for the release workflow: secrets, permissions,
# credentials, approval gates, timeouts, and concurrency are asserted on the
# structure, not on line text. `--self-test` proves each rule rejects a mutant.
# Usage: verify-release-workflow-policy.sh [--self-test] [workflow.yml]
set -euo pipefail
exec python3 - "$@" <<'PY'
import json, os, re, shutil, subprocess, sys, tempfile

DEFAULT = ".github/workflows/callisto-release.yml"
TABLE_SRC = "crates/callisto-graph/src/config/resolve.rs"
CONFIG = "callisto.toml"
CI_WORKFLOW = ".github/workflows/callisto-ci.yml"
BUILD_SCRIPT = ".github/scripts/build-release-artifact.sh"
INSTALLER = ".github/actions/setup-callisto/action.yml"
WASM_INSTALLER = ".github/actions/setup-callisto-wasm/action.yml"
COORDINATOR_CONST_SRC = "crates/callisto-model/src/release.rs"
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


def matrix_pairs(path, job):
    wf = load(path)
    return {(e.get("target"), e.get("asset")) for e in wf["jobs"][job]["strategy"]["matrix"]["include"]}


def table_agreement(paths):
    """Every hand-copied target/asset table must equal the Rust table."""
    errs = []
    rust_text = open(paths["rust"]).read()
    block = re.search(r"PRODUCT_ARTIFACT_TARGETS:[^=]*=\s*\[(.*?)\];", rust_text, re.S)
    if not block:
        return [f"{paths['rust']}: PRODUCT_ARTIFACT_TARGETS table not found"]
    rust = set(re.findall(r'\("([^"]+)",\s*"([^"]+)"\)', block.group(1)))
    rust_targets = {t for t, _ in rust}
    rust_assets = {a for _, a in rust}

    def compare(label, found):
        for t, a in sorted(rust - found):
            errs.append(f"{label}: missing ({t}, {a}) present in {paths['rust']}")
        for t, a in sorted(found - rust):
            errs.append(f"{label}: ({t}, {a}) disagrees with {paths['rust']}")

    try:
        import tomllib
        with open(paths["config"], "rb") as f:
            configured = set(tomllib.load(f)["release"]["artifact-targets"])
    except ImportError:  # python < 3.11: read the one array by text
        body = re.search(r"^artifact-targets\s*=\s*\[(.*?)\]", open(paths["config"]).read(), re.S | re.M)
        configured = set(re.findall(r'"([^"]+)"', body.group(1))) if body else set()
    if configured != rust_targets:
        errs.append(f"{paths['config']}: artifact-targets {sorted(configured)} disagrees with {paths['rust']} {sorted(rust_targets)}")
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
            errs.append(f"{paths['installer']}: asset {asset} disagrees with {paths['rust']}")
    wasm = open(paths["wasm_installer"]).read()
    for asset in set(re.findall(r"releases/[^\s\"]*/(callisto-[A-Za-z0-9._-]+)", wasm)):
        if asset not in rust_assets:
            errs.append(f"{paths['wasm_installer']}: asset {asset} disagrees with {paths['rust']}")
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
    for installer in (INSTALLER, WASM_INSTALLER):
        for signer in re.findall(r"--signer-workflow\s+(\S+)", open(installer).read()):
            if not signer.endswith("/" + m.group(1)):
                errs.append(f"{installer}: --signer-workflow {signer} disagrees with {src} coordinator path {m.group(1)}")
    return errs


def default_paths(release=DEFAULT):
    return {"rust": TABLE_SRC, "config": CONFIG, "release": release, "ci": CI_WORKFLOW,
            "script": BUILD_SCRIPT, "installer": INSTALLER, "wasm_installer": WASM_INSTALLER}


def table_mutants(d):
    """Copy each table source into d, mutate one copy at a time."""
    def copy(name, src):
        dst = os.path.join(d, name)
        shutil.copy(src, dst)
        return dst

    def variants():
        base = default_paths()
        for label, key, old, new in (
            ("asset renamed in release matrix", "release", "asset: callisto-moon.wasm", "asset: callisto-moon2.wasm"),
            ("target dropped from release matrix", "release", "          - id: linux-musl\n            runner: ubuntu-latest\n            target: x86_64-unknown-linux-musl\n            asset: callisto-x86_64-unknown-linux-musl.tar.gz\n            kind: cross\n", ""),
            ("asset renamed in ci matrix", "ci", "asset: callisto-x86_64-unknown-linux-gnu.tar.gz", "asset: callisto-x86_64-linux-gnu.tar.gz"),
            ("target dropped from ci matrix", "ci", "          - id: wasm-wasi\n            runner: ubuntu-latest\n            target: wasm32-wasip1\n            asset: callisto-moon.wasm\n            kind: wasm\n", ""),
            ("asset renamed in build script", "script", "cli:aarch64-apple-darwin:callisto-aarch64-apple-darwin.tar.gz", "cli:aarch64-apple-darwin:callisto-arm.tar.gz"),
            ("asset renamed in installer", "installer", 'ASSET_NAME="callisto-aarch64-apple-darwin.tar.gz"', 'ASSET_NAME="callisto-macos.tar.gz"'),
            ("target dropped from callisto.toml", "config", '    "wasm32-wasip1",\n', ""),
        ):
            text = open(base[key]).read()
            if old not in text:
                raise SystemExit(f"self-test mutant anchor missing in {base[key]}: {old!r}")
            paths = dict(base)
            paths[key] = os.path.join(d, f"{key}-mutant")
            open(paths[key], "w").write(text.replace(old, new, 1))
            yield label, paths

    return variants()


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
errs += table_agreement(default_paths(path))
errs += coordinator_path_agreement(DEFAULT, COORDINATOR_CONST_SRC)
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
        for name, paths in table_mutants(d):
            if table_agreement(paths):
                print(f"self-test ok: mutant rejected: {name}")
            else:
                print(f"self-test FAILED: mutant accepted: {name}", file=sys.stderr)
                bad += 1
    if bad:
        sys.exit(1)
print("release workflow policy ok")
PY
