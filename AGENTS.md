# Callisto AI Agent Guidelines (`AGENTS.md`)

This guide provides instructions, architectural rules, engineering invariants, and task runner workflows for AI coding agents (Antigravity, Claude, Cursor, Copilot, Codex) working on the Callisto codebase.

---

## 1. Repository Architecture & Crate Layers

Callisto is a fast, polyglot monorepo versioning and release management engine written in Rust. It is divided into 10 workspace crates structured in strict architectural layers:

```text
┌────────────────────────────────────────────────────────────────────────┐
│                        CALLISTO CRATE LAYERS                           │
├───────────────────────────────────┬────────────────────────────────────┤
│ LAYER & CRATE                     │ LICENSE & PURPOSE                  │
├───────────────────────────────────┼────────────────────────────────────┤
│ Layer 1: callisto-model           │ MIT / Apache-2.0                   │
│          callisto-format          │ Domain primitives, SemVer grammars,│
│          callisto-conventional    │ changeset markdown parser/writer   │
│          callisto-changelog       │                                    │
├───────────────────────────────────┼────────────────────────────────────┤
│ Layer 2: callisto-manifests       │ AGPL-3.0-only                      │
│          callisto-vcs             │ AST manifest editors & native Git  │
├───────────────────────────────────┼────────────────────────────────────┤
│ Layer 3: callisto-graph           │ AGPL-3.0-only                      │
│                                   │ Dependency DAG solver & cascades   │
├───────────────────────────────────┼────────────────────────────────────┤
│ Layer 4: callisto-cli             │ AGPL-3.0-only                      │
│          callisto-moon            │ Standalone CLI & Moon WASM plugin  │
├───────────────────────────────────┼────────────────────────────────────┤
│ Dev:     callisto-fixtures        │ Dev-only byte-compat test corpus   │
└───────────────────────────────────┴────────────────────────────────────┘
```

> **CRITICAL RULE (Layer Licensing Boundaries)**: Layer 1 crates (`callisto-model`, `callisto-format`) MUST NOT depend on Layer 2, 3, or 4 crates (`callisto-graph`, `callisto-cli`, `callisto-manifests`). Layer 1 crates must remain permissive (`MIT OR Apache-2.0`) and standalone.

---

## 2. Primary Task Runners & Verification Pipelines

Always use `just` or `moon` task runners for building, testing, linting, and formatting. Do not invent manual cargo command combinations when task runners exist.

### Primary Command Reference

| Action | Primary Task Runner Command | Moon Engine Command |
| :--- | :--- | :--- |
| **Run Full Verification CI** | `just ci` | `moon run :format-check && moon run :lint && moon run :test` |
| **Run Test Suite** | `just test` | `moon run :test` |
| **Check Clippy Lints** | `just lint` | `moon run :lint` |
| **Check Formatting** | `just fmt-check` | `moon run :format-check` |
| **Format Code** | `just fmt` | `moon run :format` |
| **Verify WASM Cross-Compilation** | `just wasm-check` | `cargo check -p callisto-moon --target wasm32-wasip1 --features pdk` |

---

## 3. Strict Engineering Invariants

Agents modifying Callisto code MUST enforce the following 5 engineering invariants:

### 1. Safe Rust Only (`unsafe_code = "forbid"`)
- `unsafe` code blocks are strictly forbidden across all 10 workspace crates.
- Memory and thread safety must be guaranteed by safe Rust abstractions.

### 2. Concrete Syntax Tree (CST) Format Preservation
- NEVER use regular expressions or line-based string replace for manifest editing (`Cargo.toml`, `package.json`, `pyproject.toml`).
- **TOML Editing**: Must use `toml_edit` to manipulate CST elements, preserving user comments, key order, and whitespace.
- **JSON Editing**: Must fingerprint indentation style (`IndentStyle::Tabs` vs `IndentStyle::Spaces(N)`) and preserve key insertion order using `serde_json`.

### 3. Crash-Safe Atomic Disk Writes
- All manifest and configuration edits MUST go through `callisto_manifests::atomic::atomic_write`.
- Writes create a `NamedTempFile` in the target file's parent directory, flush data to disk, and atomically replace the target file via `fs::rename`.

### 4. Rich Diagnostic Cards (`miette`)
- User-facing CLI errors MUST derive `miette::Diagnostic` with explicit error codes, clear error cards, and actionable remediation suggestions.

### 5. No Emoji Directive in Documentation & Code
- Documentation (`README.md`, `CONTRIBUTING.md`, `ARCHITECTURE.md`) and code comments MUST remain clean, technical, scannable, and devoid of emojis or AI bot filler phrases.

---

## 4. Changeset CLI Execution Modes

When generating changesets using Callisto CLI (`callisto add`):

- **Interactive Human Mode (Terminal TTY)**: Launches the 5-step interactive `dialoguer` wizard (Package Selection -> Major Bump Selection -> Minor Bump Selection -> Summary Input -> Confirmation Preview).
- **Non-Interactive Agent/CI Mode**: Bypasses the wizard completely using explicit CLI flags:
  ```bash
  callisto add --package callisto-cli:minor --summary "Add feature description"
  ```

---

## 5. Preferred Modern CLI Tools Directive

Agents executing shell operations or terminal commands MUST prioritize modern CLI tools over legacy POSIX/Unix shell builtins:

- **Code & Pattern Search**: `ripgrep` (`rg`) over `grep`
- **File Discovery**: `fd` over `find`
- **Interactive Filtering**: `fzf` for selection menus
- **File & Code Viewing**: `bat` over `cat`
- **Diff Inspection**: `delta` over `diff` / `git diff`
- **Directory Formatting**: `eza` over `ls`
- **JSON Processing**: `jq` for stream & file transformations
- **GitHub Workflow & API**: `gh` CLI for GitHub release, PR, and repo management
