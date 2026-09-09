---
callisto-model: patch
---

`GroupName` and `RegistryKey` (`identity.rs`) each hand-wrote the identical shape: same derive list, `#[schemars(with = "String")]`, `#[serde(transparent)]`, an `as_str(&self) -> &str { &self.0 }`, and a `Display` impl that just wrote the inner string. Extracted a `string_newtype!` macro that expands to that whole shape from a name (plus an inner `string_newtype_display!` for just the `as_str`/`Display` pair); both types now expand from one invocation each. `RegistryKey`'s well-known-key associated consts stay in their own hand-written `impl` block, unaffected.

`TagName` (`tag.rs`) looked identical at a glance but isn't: it validates on construction/deserialize (private field, `parse`/`new_unchecked`, hand-written `Serialize`/`Deserialize` routed through `parse`), the same intentionally-different shape as `CommitSha`. Only its duplicated `as_str`/`Display` pair -- genuinely identical text to the other two -- was switched to `string_newtype_display!`; its struct definition, derives, and validating serde impls are untouched. No behavior change: public API, trait impls, and serde wire format are identical before and after for all three types.
