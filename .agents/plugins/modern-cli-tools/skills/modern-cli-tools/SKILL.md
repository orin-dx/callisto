---
name: modern-cli-tools
description: >-
  Global modern CLI tool preference directive for AI coding agents across all projects, repositories, and workspaces. Mandates the use of ripgrep (rg), fd, fzf, bat, delta, eza, jq, and gh CLI over legacy POSIX/Unix shell builtins.
---

# Global Preferred Modern CLI Tools Directive

This directive is **Mandatory & Rigid** across all projects, monorepos, and agentic workspaces.

Whenever executing shell operations, searching for files, inspecting code, viewing text, parsing structured data, or interacting with GitHub, AI agents **MUST** prioritize modern CLI tools over legacy POSIX/Unix shell builtins:

---

## Tool Substitution Table

| Task / Operation | Preferred Modern Tool | Legacy Tool to Avoid |
| :--- | :--- | :--- |
| **Code & Pattern Search** | `ripgrep` (`rg`) | `grep` / `egrep` |
| **File & Directory Discovery** | `fd` | `find` |
| **Interactive Choice Filtering** | `fzf` | Manual interactive selection |
| **File Viewing & Syntax Highlighting** | `bat` | `cat` / `less` |
| **Git Diffs & Code Inspection** | `delta` | `diff` / raw `git diff` |
| **Directory Formatting & Tree View** | `eza` (or `exa`) | `ls` / `tree` |
| **JSON Querying & Transformations** | `jq` | `python -m json.tool` / `sed` |
| **GitHub Workflow & Operations** | `gh` CLI | Manual curl GitHub API calls |

---

## Agentic Execution Directives

1. **Ripgrep Search (`rg`)**:
   - Use `rg` (or `grep_search` tool) for fast text/pattern searches across codebases.
   - Example: `rg "fn handle_request" src/`

2. **File Discovery (`fd`)**:
   - Use `fd` for fast file and directory finding.
   - Example: `fd -e rs -e toml`

3. **File Viewing (`bat`)**:
   - Use `bat` when displaying file contents with line numbers and syntax highlighting.
   - Example: `bat -n src/main.rs`

4. **Git Diffs (`delta`)**:
   - Use `delta` when presenting visual code diffs in terminal output.
   - Example: `git diff | delta`

5. **Directory View (`eza`)**:
   - Use `eza --tree` or `eza -la` for modern directory listings.
   - Example: `eza --tree --level=2`

6. **JSON Transformations (`jq`)**:
   - Use `jq` for extracting, filtering, and formatting JSON structures.
   - Example: `cat package.json | jq '.dependencies'`

7. **GitHub CLI (`gh`)**:
   - Use `gh` for GitHub release, pull request, issue, and workflow management.
   - Example: `gh pr status` or `gh release list`
