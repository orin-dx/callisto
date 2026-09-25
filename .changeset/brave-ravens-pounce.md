---
callisto-graph: patch
---

Release reruns no longer reject a local tag whose message differs from the current run's, as long as it targets the right commit and is annotated; annotation text was never comparable on the remote side, so the two disagreed depending on fetch state.
