---
callisto-cli: patch
---

**Repositories with many foreign release tags on one commit are much faster.** When a package took over a name that another package once held, `status`, `version` and `release` no longer repeat the same history lookups for every tag on that commit.

- At 50 such tags, `status` drops from 304 helper processes and about 3.0 s to 17 processes and 0.4 s. The count no longer grows with the number of tags.
- No configuration change is needed. The tags are still reported as owned by another package, and the choice of last tag is unchanged.
