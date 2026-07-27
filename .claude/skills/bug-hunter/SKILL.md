---
name: bug-hunter
description: >-
  Outcome-driven adversarial bug-hunting methodology for deep codebase audits, latent defect
  discovery, spec-vs-code drift identification, and empirical root-cause verification. Use when
  asked to audit, adversarially review, or hunt for bugs/security issues/spec drift in this repo.
---

# Bug-Hunter (thin shell)

Canonical source lives outside `.claude/` so it's shared with other tools (e.g. antigravity/gemini)
that also read `.agents/`. Do not duplicate it here — read it fresh every run, since it may change
independently of this shell:

1. Read `.agents/skills/bug-hunter/SKILL.md` in full (taxonomy, mindset, output format, severity
   rubric, verification invariants — all defined there).
2. Read `.agents/skills/bug-hunter/FINDINGS.md` — the running ledger of confirmed findings and
   fix status. Don't re-report what's already logged there.
3. Follow both as written.
