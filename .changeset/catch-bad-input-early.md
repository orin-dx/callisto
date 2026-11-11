---
callisto-cli: minor
---

Mistyped config keys and pre-release tags are caught where you make them.

- An unknown key inside a `[[fixed-group]]` or `[[linked-group]]` block now fails with an error naming the key. Before, it was silently ignored.
- `callisto pre enter` rejects a tag with a space, `!`, `/`, an empty part or a number with a leading zero such as `01` (E324) and writes nothing. Before, it wrote `pre.json` and `version` failed later with a confusing version error. Dotted tags such as `beta.1` still work.

Action: if a group block in `callisto.toml` carries a key Callisto does not recognise, remove it.
