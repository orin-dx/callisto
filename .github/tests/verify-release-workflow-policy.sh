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


def _tokens(expr):
    pat = re.compile(r"\s*(?:(&&|\|\||==|!=|!|\(|\))|'((?:[^']|'')*)'|([A-Za-z_][\w.\-]*))")
    pos, out = 0, []
    expr = expr.strip()
    while pos < len(expr):
        m = pat.match(expr, pos)
        if not m:
            raise ValueError(f"unsupported expression syntax near {expr[pos:]!r}")
        pos = m.end()
        out.append(("op", m.group(1)) if m.group(1) else ("str", m.group(2).replace("''", "'")) if m.group(2) is not None else ("id", m.group(3)))
    return out


def _truthy(v):
    return v not in (None, "", False, 0)


def _loose_eq(a, b):
    norm = lambda v: "" if v is None else str(v).lower() if not isinstance(v, bool) else str(v).lower()
    return norm(a) == norm(b)


def eval_if(expr, ctx):
    """GitHub expression subset: && || == != ! ( ), string literals, contexts, always()."""
    toks, i = _tokens(expr), 0

    def peek():
        return toks[i] if i < len(toks) else (None, None)

    def take():
        nonlocal i
        i += 1
        return toks[i - 1]

    def primary():
        kind, val = take()
        if (kind, val) == ("op", "("):
            v = orx()
            if take() != ("op", ")"):
                raise ValueError("unbalanced parenthesis")
            return v
        if kind == "str":
            return val
        if kind == "id":
            if peek() == ("op", "("):
                take()
                if take() != ("op", ")"):
                    raise ValueError("function arguments unsupported")
                if val == "always":
                    return True
                raise ValueError(f"status function {val}() unsupported by the scheduler check")
            return ctx(val)
        raise ValueError(f"unexpected token {val!r}")

    def unary():
        if peek() == ("op", "!"):
            take()
            return not _truthy(unary())
        return primary()

    def eq():
        v = unary()
        while peek() in (("op", "=="), ("op", "!=")):
            op = take()[1]
            r = unary()
            v = _loose_eq(v, r) if op == "==" else not _loose_eq(v, r)
        return v

    def andx():
        v = eq()
        while peek() == ("op", "&&"):
            take()
            r = eq()
            v = r if _truthy(v) else v
        return v

    def orx():
        v = andx()
        while peek() == ("op", "||"):
            take()
            r = andx()
            v = v if _truthy(v) else r
        return v

    v = orx()
    if i != len(toks):
        raise ValueError("trailing tokens in expression")
    return v


def _needs(job):
    n = job.get("needs") or []
    return [n] if isinstance(n, str) else list(n)


def schedule(jobs, event, ref, inputs, results, outputs):
    """Which jobs run under GitHub's rules: no status function means an implicit
    success() over every transitive ancestor, and a skipped ancestor fails it."""
    ran, res, outs = {}, {}, {}

    def ancestors(name, seen=None):
        seen = set() if seen is None else seen
        for n in _needs(jobs[name]):
            if n not in seen:
                seen.add(n)
                ancestors(n, seen)
        return seen

    def order():
        done, seq = set(), []
        while len(seq) < len(jobs):
            for n, j in jobs.items():
                if n not in done and all(d in done for d in _needs(j)):
                    done.add(n)
                    seq.append(n)
        return seq

    for name in order():
        job = jobs[name]
        cond = job.get("if")
        cond = re.sub(r"\s+", " ", str(cond)).strip() if cond is not None else None

        def ctx(path):
            parts = path.split(".")
            if path == "github.event_name":
                return event
            if path == "github.ref":
                return ref
            if parts[0] == "inputs":
                return inputs.get(parts[1])
            if parts[0] == "needs" and parts[2] == "result":
                return res.get(parts[1], "skipped")
            if parts[0] == "needs" and parts[2] == "outputs":
                return outs.get(parts[1], {}).get(parts[3], "") if ran.get(parts[1]) else ""
            raise ValueError(f"unsupported context {path}")

        has_status = cond is not None and re.search(r"\b(always|success|failure|cancelled)\(\)", cond)
        if has_status:
            run = _truthy(eval_if(cond, ctx))
        else:
            run = all(res.get(a) == "success" for a in ancestors(name)) and (cond is None or _truthy(eval_if(cond, ctx)))
        ran[name] = run
        res[name] = results.get(name, "success") if run else "skipped"
        outs[name] = outputs.get(name, {}) if run else {}
    return {n for n, r in ran.items() if r}


MAIN = "refs/heads/main"
_SHA = "a" * 40
_GATES = {"verify", "recovery-checks", "release-candidate"}
# (scenario, event, ref, inputs, results, outputs, expected jobs that run)
SCENARIOS = [
    ("push: release PR merge with artifacts", "push", MAIN, {}, {},
     {"release-candidate": {"is_release_pr": "true"}, "plan": {"has_artifacts": "true"}},
     _GATES - {"recovery-checks"} | {"plan", "build-artifact", "build", "execute"}),
    ("push: release PR merge without artifacts", "push", MAIN, {}, {},
     {"release-candidate": {"is_release_pr": "true"}, "plan": {"has_artifacts": "false"}},
     _GATES - {"recovery-checks"} | {"plan", "execute"}),
    ("push: ordinary commit opens the release PR", "push", MAIN, {}, {},
     {"release-candidate": {"is_release_pr": "false"}},
     _GATES - {"recovery-checks"} | {"version-pr"}),
    ("dispatch: recovery of a merged release", "workflow_dispatch", MAIN, {"release_source_sha": _SHA}, {},
     {"release-candidate": {"is_release_pr": "true"}, "plan": {"has_artifacts": "true"}},
     _GATES - {"verify"} | {"plan", "build-artifact", "build", "execute"}),
    ("dispatch: recovery of an unmanaged commit", "workflow_dispatch", MAIN, {"release_source_sha": _SHA}, {},
     {"release-candidate": {"is_release_pr": "false"}},
     _GATES - {"verify"}),
    ("dispatch without a source sha acts like a push", "workflow_dispatch", MAIN, {}, {},
     {"release-candidate": {"is_release_pr": "false"}},
     _GATES - {"recovery-checks"} | {"version-pr"}),
    ("dispatch from a non-main ref", "workflow_dispatch", "refs/heads/topic", {"release_source_sha": _SHA}, {}, {},
     {"recovery-checks"}),
    ("push: verify fails", "push", MAIN, {}, {"verify": "failure"}, {}, {"verify"}),
    ("push: plan fails", "push", MAIN, {}, {"plan": "failure"},
     {"release-candidate": {"is_release_pr": "true"}},
     _GATES - {"recovery-checks"} | {"plan"}),
    ("push: an artifact build fails", "push", MAIN, {}, {"build-artifact": "failure"},
     {"release-candidate": {"is_release_pr": "true"}, "plan": {"has_artifacts": "true"}},
     _GATES - {"recovery-checks"} | {"plan", "build-artifact"}),
    ("push: assembly fails", "push", MAIN, {}, {"build": "failure"},
     {"release-candidate": {"is_release_pr": "true"}, "plan": {"has_artifacts": "true"}},
     _GATES - {"recovery-checks"} | {"plan", "build-artifact", "build"}),
]


def schedule_errors(jobs):
    errs = []
    for name, event, ref, inputs, results, outputs, expected in SCENARIOS:
        try:
            got = schedule(jobs, event, ref, inputs, results, outputs)
        except (ValueError, KeyError) as e:
            errs.append(f"schedule scenario {name!r} could not be evaluated: {e}")
            continue
        if got != expected:
            errs.append(f"schedule scenario {name!r}: expected {sorted(expected)}, would run {sorted(got)}")
    return errs


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
    script = next((st.get("run", "") for st in rc.get("steps", []) if st.get("id") == "candidate"), "")
    if "^[0-9a-f]{40}$" not in script or '[[ ! "$release_source_sha" =~ ^[0-9a-f]{40}$ ]]' not in script:
        errs.append("release-candidate must require a full 40-lowercase-hex release_source_sha")
    if not re.search(r'\[\[ "\$resolved_sha" != "\$release_source_sha" \]\]; then\s*echo [^\n]*\n\s*exit 1', script):
        errs.append("release-candidate must reject a release_source_sha whose resolved sha differs")
    prs_line = next((l for l in script.splitlines() if l.strip().startswith("prs=$(")), "")
    for frag in ('.merged_at != null', '.base.ref == "main"', '.head.repo.full_name == "', 'callisto/version-packages'):
        if frag not in prs_line:
            errs.append(f"release-candidate managed-PR association check lost {frag!r}")
    if not re.search(r"^\s*verified=\$\(gh api [^\n]*--jq '\.commit\.verification\.verified'\)$", script, re.M):
        errs.append("release-candidate must compute the commit verification from .commit.verification.verified")
    if not re.search(r"if \[\[ \"\$prs\" == 1 && \"\$verified\" == true \]\]; then\s*\{\s*echo 'is_release_pr=true'", script):
        errs.append("is_release_pr=true must require exactly one managed PR and a verified commit")
    if len(re.findall(r"is_release_pr=true", script)) != 1:
        errs.append("is_release_pr=true must be set in exactly one place")
    plan = jobs.get("plan", {})
    cond = re.sub(r"\s+", " ", str(plan.get("if", ""))).strip()
    if cond != "always() && needs.release-candidate.result == 'success' && needs.release-candidate.outputs.is_release_pr == 'true'" or plan.get("needs") != "release-candidate":
        errs.append("plan must run only when release-candidate succeeded and says is_release_pr == 'true'")
    # release-candidate descends from two mutually exclusive gates, one always
    # skipped. GitHub skips any job whose transitive ancestor was skipped unless
    # its own `if` has a status function, so every dependent job needs one.
    for name, job in jobs.items():
        if job.get("needs") and not re.search(r"\balways\(\)", str(job.get("if", ""))):
            errs.append(f"job {name} depends on a gated job and must use always() in its if")
    errs += schedule_errors(jobs)
    ba = re.sub(r"\s+", " ", str(jobs.get("build-artifact", {}).get("if", "")))
    if "needs.plan.result == 'success'" not in ba:
        errs.append("build-artifact must require needs.plan.result == 'success'")
    ex = jobs.get("execute", {})
    if not re.search(r"needs\.plan\.result == 'success'", str(ex.get("if", ""))):
        errs.append("execute must require needs.plan.result == 'success'")
    if set(ex.get("needs", [])) != {"plan", "build", "release-candidate"}:
        errs.append("execute must need plan, build, and release-candidate")
    steps = ex.get("steps", [])
    src = [s for s in steps if str(s.get("uses", "")).startswith("actions/checkout@") and (s.get("with") or {}).get("path") == "release-source"]
    if len(src) != 1 or src[0]["with"].get("ref") != "${{ needs.release-candidate.outputs.release_source_sha }}":
        errs.append("execute release-source checkout ref must be needs.release-candidate.outputs.release_source_sha")
    hand = [i for i, s in enumerate(steps) if s.get("name") == "Verify same-run handoff before credentials"]
    sec = [i for i, s in enumerate(steps) if "secrets." in json.dumps(s)]
    if len(hand) != 1:
        errs.append("execute must have exactly one 'Verify same-run handoff before credentials' step")
    else:
        h = steps[hand[0]]
        if "if" in h or "continue-on-error" in h or "cmp " not in h.get("run", "") or "-a 256 -c" not in h.get("run", ""):
            errs.append("handoff verification step must be unconditional, fail-closed, and run the sha256 and cmp checks")
        if not sec or hand[0] > min(sec):
            errs.append("handoff verification must precede the first step that references secrets")
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
        "verified requirement dropped": sub('if [[ "$prs" == 1 && "$verified" == true ]]', 'if [[ "$prs" == 1 ]]'),
        "dispatch sha regex weakened": sub("^[0-9a-f]{40}$", "^.+$"),
        "resolved-sha equality dropped": sub('if [[ "$resolved_sha" != "$release_source_sha" ]]', 'if false'),
        "execute loses plan success": sub("always() && needs.plan.result == 'success' &&", "always() &&"),
        "plan runs always": sub("      always() && needs.release-candidate.result == 'success' &&\n      needs.release-candidate.outputs.is_release_pr == 'true'\n    timeout-minutes: 20",
            "      always()\n    timeout-minutes: 20"),
        "plan loses always": sub("      always() && needs.release-candidate.result == 'success' &&\n      needs.release-candidate.outputs.is_release_pr == 'true'\n    timeout-minutes: 20",
            "      needs.release-candidate.outputs.is_release_pr == 'true'\n    timeout-minutes: 20"),
        "build-artifact loses always": sub("if: always() && needs.plan.result == 'success' && needs.plan.outputs.has_artifacts == 'true'",
            "if: needs.plan.outputs.has_artifacts == 'true'"),
        "build-artifact loses plan success": sub("if: always() && needs.plan.result == 'success' && needs.plan.outputs.has_artifacts == 'true'",
            "if: always() && needs.plan.outputs.has_artifacts == 'true'"),
        "execute checks out moving ref": after("\n  execute:\n", "ref: ${{ needs.release-candidate.outputs.release_source_sha }}", "ref: main"),
        "handoff step disabled": sub("      - name: Verify same-run handoff before credentials\n", "      - name: Verify same-run handoff before credentials\n        if: false\n"),
        "handoff step renamed away": sub("Verify same-run handoff before credentials", "Handoff"),
        "handoff continue-on-error": sub("      - name: Verify same-run handoff before credentials\n        env:", "      - name: Verify same-run handoff before credentials\n        continue-on-error: true\n        env:"),
        "verified query dropped": sub("--jq '.commit.verification.verified'", "--jq '.sha'"),
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
