<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# CI and releases

Pushes and pull requests to `main` run **Lightweight CI**. It runs formatting,
repository hooks, workflow lint, dependency boundaries and licensing/advisory
checks, plus Python regression tests. It does not compile Bevy or build release
packages. The stable **CI required** job fails if any preceding check fails.
The workflow also handles `merge_group` events for a future merge queue.

## Full validation and commit reuse

Run **Actions > Full validation > Run workflow** on the branch to validate, or:

```sh
gh workflow run full-check.yml --ref main
```

Full validation builds and tests the workspace with all features on Windows x64,
Linux x64, macOS Apple Silicon and macOS Intel. Linux also runs Clippy with all
features and targets. Compilation finishes and saves the cache in separate jobs
before the tests execute. No ZIPs are produced. Test jobs hydrate the versioned field through Git LFS;
build jobs and lightweight checks do not download it.

`scripts/validated-commit.py` queries GitHub's run history for a successful manual
Full validation or Release run in this repository with the **exact source commit
SHA**. A successful release dry run qualifies too. PR checks, failed or cancelled
runs, and build-cache hits never qualify. Missing history or API access makes it
run the checks again. A different SHA, including a squash-merge commit, needs its
own validation. Concurrent requests may both validate; neither assumes an
in-progress run will pass. A release that fails after validation may need to
validate again because its overall run did not succeed.

Cheap checks always rerun, including advisory checks whose data can change
without a source change. Select **force** to repeat native validation on an
already validated commit, for example after a runner or Rust toolchain update:

```sh
gh workflow run full-check.yml --ref main -F force=true
```

## Release and dry run

Open **Actions > Release > Run workflow**, select the source branch, and enter a
base version such as `0.2.0`. Choose `stable`, `alpha`, `beta`, or `rc`, plus a
positive prerelease number or an identifier such as `rc1`. For example, `0.2.0`,
`rc`, `2` creates `v0.2.0-rc.2`; `0.0.1`, `alpha`, `rc1` creates
`v0.0.1-alpha.rc1`. Stable ignores the number.

**Dry run defaults to true.** It runs the same validation prerequisite, stamps
the version, builds all four release packages, checks the field through the real
loader, smoke-tests both packaged binaries with `--help`, uploads ZIP artifacts,
and verifies their combined checksums. It creates no tag, draft, or release.
Download its `package-*` artifacts from the run page. Artifacts are retained for
seven days. Set **dry-run=false** to publish:

```sh
# Exercise the complete release path without publishing.
gh workflow run release.yml --ref main -f version=0.2.0 -f channel=alpha -f number=1 -F dry-run=true
# Publish after validation; this creates the tag and public release automatically.
gh workflow run release.yml --ref main -f version=0.2.0 -f channel=alpha -f number=1 -F dry-run=false
```

Release invokes Full validation first, reusing a successful result for the same
SHA when available. Select **force-validation** to disregard that result.
Packaging starts only after validation succeeds. An existing tag is rejected in
both dry and publishing runs. Versions are stamped into workspace manifests and
the lockfile only in the build checkout, so executable version output matches
the release without committing a version bump. Third-party locked versions stay
unchanged. The release tag points to the selected source commit; rebuilding that
tag requires the same version-stamping step.

Publication waits for verification and all four ZIPs. Only the final job has
`contents: write`; its GitHub release commands are skipped in dry-run mode. It
creates a draft, uploads archives and checksums, then publishes. Alpha, beta and
RC releases are marked as prereleases. The workflow uses `GITHUB_TOKEN`, with no
personal token or signing secrets. If upload fails after draft creation, remove
the incomplete draft and tag before retrying. Existing releases are never
overwritten.

## Build caches and debug information

Full validation and packaging share per-platform **release-profile** caches.
There is no second set of native debug-profile CI builds. All native jobs keep
`CARGO_PROFILE_RELEASE_DEBUG=0` and `CARGO_INCREMENTAL=0`; these settings also
cause the CMake-based GNS dependency to use Release rather than RelWithDebInfo.
Workspace artifacts are retained so the test jobs can reuse compiled test
binaries. Cache keys include the compiler/build environment, manifests after
version stamping, and source SHA. A matching build restores exact artifacts;
otherwise a compatible compiler cache supplies reusable dependencies. Builds
save their completed cache before tests run, including version-specific release
binaries. Downloaded crate archives and the Git object database are cached;
unpacked registry sources and Git checkouts are regenerated instead of cached
twice. Version changes can still recompile workspace crates in the build job.

After a Windows build, `scripts/prune-build-cache.py --apply` removes only the
copied vcpkg clone's `.git` and `downloads` directories from GNS's build output.
The locked GNS build script recreates this copy if it runs again. Installed
native libraries, runtime DLLs, Cargo fingerprints and Rust artifacts remain.
Run the script without `--apply` to measure eligible files. It does not modify
Cargo registry sources. Cache eviction can cause a rebuild but cannot bypass
validation. Old cache generations expire normally; they may be deleted from
Actions > Caches once their replacements have been saved.

For local development, third-party crates use `debug=0` while workspace crates
retain debug information. This leaves simulator code debuggable and reduces
third-party library artifacts. Dependency source-level debugging requires
removing that override temporarily. Optimization levels and runtime checks are
unchanged; existing local artifacts are not automatically deleted.

A local Apple Silicon measurement with Rust 1.98.1 compared the third-party
artifacts reported by `cargo build -p rm-simulator-physics --locked
--message-format=json`: 273.4 MiB before the override, 216.7 MiB after, a 20.7%
reduction. These are uncompressed development artifacts, not a prediction of
compressed CI-cache savings. The Windows download pruning needs measurement on
the next native CI build.

## PR merges and branch protection

Work on a branch, open a PR against `main`, wait for **CI required**, resolve
review threads, then squash merge by default. Rebase merge only when each retained
commit is useful and independently passes its relevant checks. Follow the title
and commit-message convention in [CONTRIBUTING.md](../CONTRIBUTING.md). Full native validation is manual,
not a mandatory expensive check on every PR. Run it for platform-sensitive
changes and before release. `CODEOWNERS` requests the maintainer as reviewer.

After this workflow is on `main` and **CI required** has run successfully, inspect
and apply the prepared ruleset with an account that administers the repository:

```sh
python3 scripts/prepare-branch-protection.py
python3 scripts/prepare-branch-protection.py --apply
```

The script creates or updates only its named `main PR workflow` ruleset. It
requires a PR, the up-to-date **CI required** check from GitHub Actions, resolved
review threads and linear history. It blocks force pushes and deletion with no
bypass actors. Mandatory approval count is zero so a solo maintainer can merge
their own PR; increase it when another reviewer is available. The script does
not enable a merge queue or auto-merge. Protection is not enabled merely by
checking these files into Git. Apply it after the new check exists, so the
bootstrap change is not blocked by a check that has never run.

Each ZIP contains `bin/`, `field/`, notices, license texts, dependency lockfile,
and a file checksum manifest. Windows also includes the existing launchers and
MSVC runtime DLLs. macOS includes relocated Homebrew dylibs and ad-hoc signatures.
Linux includes linked non-glibc libraries and relative loader paths, and requires
glibc 2.39 or newer plus graphics/display drivers. `--help` checks catch loader
failures but do not replace gameplay tests on clean machines. There is no code
signing identity, notarization, installer, `.deb`, or `.app` packaging yet.

## Field assets in Git LFS

The approved runtime package lives in `field/`. Its data files use Git LFS;
`CAD-NOTICE.md` remains ordinary Git text and retains the upstream notice
verbatim. These are DJI / RoboMaster assets under their original terms, not
MIT/Apache-licensed simulator code. Original STEP inputs are not imported.

The initial import is byte-for-byte from the former rm-map-tools release archive,
verified against its pinned SHA-256. `scripts/release-field.json` retains that URL
and archive hash as historical provenance; CI no longer downloads that release.
The same file pins every runtime package file by SHA-256. The source commit fixes
both that inventory and the corresponding LFS object IDs.

```sh
git lfs install
git lfs pull --include='field/**'
python3 scripts/stage-release-field.py --verify-only
```

Lightweight CI checks pointer object IDs against the inventory without fetching
LFS content. Test/package checkouts use `lfs: true`. Release staging verifies all
hydrated files, refuses pointer placeholders and symlinks, and copies the package
to `dist/field` before the real Rust loader verifies its manifests. An existing
`dist/field` is not overwritten. Users installing release ZIPs do not need LFS.

For an intentional field update, replace the approved files under `field/`,
preserve the ownership notice and exporter provenance, then run:

```sh
python3 scripts/stage-release-field.py --update-checksums
cargo run --release --locked -p rm-simulator-server --example inspect_assets -- field
```

Commit the changed LFS pointers and `scripts/release-field.json` together. Do not
run text formatters over exported data; hooks exclude `field/` to preserve the
manifest hashes and source notices. The default loader keeps `local-assets/field`
as a local override before the versioned `field/` package.

Git LFS storage and downloads have their own allowance and billing, separate
from Actions caches. Lightweight CI skips downloads; native test/package jobs
fetch the approximately 52 MiB field. See [GitHub's LFS billing documentation](https://docs.github.com/en/billing/concepts/product-billing/git-lfs).

The setup script also enables squash and rebase merges, disables merge commits,
and uses the PR title and body for squash commit messages. CI checks PR titles
and all proposed commit messages before either merge method.
