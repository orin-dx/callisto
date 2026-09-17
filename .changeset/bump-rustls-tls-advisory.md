---
callisto-moon: patch
---

**Restore the dependency audit baseline**

`callisto-moon`'s dependency tree pinned `rustls 0.23.43`, affected by [RUSTSEC-2026-0285](https://rustsec.org/advisories/RUSTSEC-2026-0285), and the yanked `chacha20 0.10.1`. Update both transitive dependencies to audited releases without moving unrelated packages.
