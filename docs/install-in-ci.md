# Install Callisto in CI

On GitHub-hosted runners, use the `setup-callisto` action. It downloads the release archive, runs `gh attestation verify` on it, and only then extracts the binary. It needs `gh` and has no fallback: if the release has no asset for the platform, the attestation does not verify, or `gh` is missing, the step fails and links here.

Everywhere else (self-hosted runners, other CI systems, containers), follow these steps.

## Verify and install a release

Pick the asset for your platform:

| Platform | Asset |
| --- | --- |
| Linux x86_64 | `callisto-x86_64-unknown-linux-gnu.tar.gz` |
| Linux x86_64, static (musl) | `callisto-x86_64-unknown-linux-musl.tar.gz` |
| Linux arm64 | `callisto-aarch64-unknown-linux-gnu.tar.gz` |
| macOS arm64 | `callisto-aarch64-apple-darwin.tar.gz` |
| macOS x86_64 | `callisto-x86_64-apple-darwin.tar.gz` |

Then download, verify, and extract. Replace `callisto@X.Y.Z` with the release tag, or use `latest/download` for the newest release:

```bash
asset=callisto-x86_64-unknown-linux-gnu.tar.gz
curl -sSL -f --retry 3 -o "$asset" "https://github.com/orin-dx/callisto/releases/download/callisto@X.Y.Z/$asset"
gh attestation verify "$asset" \
  --repo orin-dx/callisto \
  --signer-workflow orin-dx/callisto/.github/workflows/callisto-release.yml \
  --deny-self-hosted-runners \
  --source-ref refs/heads/main
tar -xzf "$asset" callisto
```

- Verify before you extract. If `gh attestation verify` fails, delete the archive and do not run it.
- `gh` must be authenticated (`GH_TOKEN`) and recent enough to have `gh attestation verify`.
- Put the extracted `callisto` on `PATH`, outside your workspace so a later `git add -A` cannot stage it.

## Build from source

```bash
cargo install callisto-cli --locked
```

Add `--version X.Y.Z` to pin a release. This builds on the runner and skips attestation, so the crate and its dependencies come from crates.io as `Cargo.lock` resolves them.

## Pin the action by SHA

Reference `setup-callisto` (and `callisto-action`) by commit SHA with the release tag in a comment, for example `uses: orin-dx/callisto/.github/actions/setup-callisto@<sha> # callisto@X.Y.Z`; the action then installs that release.
