---
callisto-cli: minor
---

New `callisto release pending` lists merged release commits whose release never finished (a missing tag or an unpublished GitHub release), oldest first, so a release that stopped partway can be found and recovered. It reads the committed decision of each release commit and publishes nothing.
