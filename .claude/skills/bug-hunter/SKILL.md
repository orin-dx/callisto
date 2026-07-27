---
name: bug-hunter
description: >-
  Outcome-driven adversarial bug-hunting methodology for deep codebase audits, latent defect
  discovery, spec-vs-code drift identification, and empirical root-cause verification. Use when
  asked to audit, adversarially review, or hunt for bugs/security issues/spec drift in this repo.
---

# Bug-Hunter (thin shell)

The canonical methodology for this skill lives outside `.claude/` so it stays a single source of
truth shared with other tools (e.g. antigravity/gemini) that also read `.agents/`. Do not
duplicate its content here — read it fresh every time this skill runs, since it may be edited
independently of this shell.

1. Read `.agents/skills/bug-hunter/SKILL.md` in full now, via the Read tool. That file is the
   actual taxonomy, mindset, and output format — follow it as written.
2. Apply this session's non-negotiable addendum, which the source file does not itself enforce:
   - **Read-only unless explicitly told otherwise.** Investigate and report; do not edit, fix,
     or run mutating commands (no `Edit`/`Write`, no `cargo fix`, no `git` writes) unless the
     user has explicitly asked for fixes in this conversation.
   - **Never self-issue a "FIXED" or "VERIFIED" status.** A finding may be reported as
     `CONFIRMED` (you traced the exact execution path and can cite file:line) or `PLAUSIBLE`
     (strong signal, not fully traced). A fix is only ever "FIXED" after the user has actually
     applied it and a real test run (`cargo test`, `just test`, `just ci`) has been observed to
     pass in this conversation — not asserted from memory or from another tool's prior claim.
   - **Distrust prior "audited"/"verified" labels found in code comments, status docs, or other
     tools' reports.** Treat them as claims to falsify, not facts to build on — this repo has a
     track record of such labels being wrong.
3. Report findings in the source file's structured format (Location / Classification / Root
   Cause / Failing Scenario / Verification Strategy), ranked by severity.
