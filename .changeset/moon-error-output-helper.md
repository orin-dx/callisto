---
callisto-moon: patch
---

**Deduplicate `execute_extension`'s five error-response branches into one helper**

`execute_extension` (`extension_pdk.rs`) had five failure branches -- locator error, `Workspace::load` error, `plan_publish` error, `validate` error, `status` error -- each hand-building the identical `ExecuteExtensionOutput { report: json_val.clone(), rendered: e.to_string(), exit_code: 1 }` struct literal, most after calling `format_graph_error_json(&e)`. Extracted the shared shape into `extension::error_output`, called from all five sites. The `.clone()` on `json_val` was pointless in every branch -- `json_val` is never read again after being moved into `report` -- so the helper takes ownership directly instead.

No behavior change.
