---
callisto-cli: minor
---

npm prereleases now publish under the tag their version names: `1.0.0-beta.2` goes to `beta`, `2.0.0-rc.1` to `rc`, and `next` remains the fallback, instead of every prerelease landing on `next`. `release` now refuses a version that `callisto snapshot` wrote (E169), so a snapshot is never tagged or published.
