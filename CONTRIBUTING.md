<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Contributing

Keep changes focused and develop them through pull requests. Follow the crate
ownership described in `AGENTS.md` and [the architecture guide](docs/architecture-refactor.md).

## Issues and security reports

Use the bug report or feature request template for public issues. Include only
sanitized logs and screenshots; keep private addresses, credentials, CAD packages
and the Steamworks SDK out of issues and pull requests. Report vulnerabilities
privately using [the security policy](SECURITY.md).

`.editorconfig` supplies editor defaults; Rust formatting is controlled by
`rustfmt.toml`. Git and the hooks use LF line endings on every platform.

## Set up the hooks

This repository uses `prek`:

```sh
cargo install prek --version 0.4.14 --locked
prek install --hook-type pre-commit --hook-type commit-msg
```

## Commits and pull requests

Use `type(scope): summary` for commit subjects and PR titles. Scope is optional;
use a short lowercase name such as `server`, `app`, `physics`, or `deps`.
Allowed types are `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`,
`build`, `ci`, `chore`, and `revert`. Add `!` before the colon for breaking changes.
Keep the complete subject within 72 characters, use an imperative summary, and
omit the trailing period. Separate an optional body with a blank line. Explain
why the change is needed or list a few concrete changes; avoid repeating the title.

```text
fix(server): preserve confirmation snapshot order

Keep confirmations ahead of periodic snapshots.

- Preserve worker submission order
- Add a regression test for queued confirmations
```

Squash merge ordinary PRs. GitHub uses the PR title and body as the suggested
commit message; review them before merging. Rebase merge only when the individual
commits form useful, independently passing steps. Verify those steps locally;
CI checks the final PR state, not every intermediate snapshot. Merge commits are
disabled and main requires linear history.

CI validates both PR titles and every proposed commit message so either allowed
merge method preserves the convention. Clean up temporary `fixup!` or `squash!`
commits before merging. The local commit-message hook catches mistakes early.
To check a branch manually, run:

```sh
python3 scripts/check-commit-message.py --range origin/main..HEAD
```

## Verify a change

```sh
just verify
```

The individual commands are available as `just fmt`, `just check`, `just lint`,
`just test`, `just deny`, `just module-deps`, `just mpl`, and `just hooks`.

## License of contributions

`rm-simulator` is dual-licensed `MIT OR Apache-2.0`; see [NOTICE.md](NOTICE.md)
for third-party terms and the MPL-2.0 source offer. By opening a pull request
you agree to license your contribution under those same terms, unless you state
otherwise in the pull request.

Every file you add starts with its SPDX identifier and the copyright line:

```rust
// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
```

Use the comment syntax of the file's language. Do not add that header to
third-party artwork, recorded measurements, license texts or generated data.
`NOTICE.md` explains where those come from, and `just mpl` checks that MPL-2.0
dependencies stay documented.

## Documentation and targeted checks

Use the [documentation index](docs/README.md) to distinguish current guides from
historical plans. Update the README option/control tables and live-rule digest
when behavior changes. Keep dated measurements tied to their recorded revision
and package hashes; mark superseded designs instead of presenting them as current.
Record user-visible changes under `Unreleased` in the changelog.

`just world-test` covers the world facade and live rules. For extracted dynamics
and geometry, run `cargo test -p rm-simulator-physics --locked`. `just verify`
covers both crates, the rest of the workspace, Python harness checks and dependency
policy. Run timing probes after compilation has finished. The asynchronous network
trace tests can also miss their comparison-count threshold under heavy competing
CPU load; rerun in isolation before diagnosing a behavior regression.

## Release pause

Release packaging and publishing are paused during development. Commit and verify
source changes; create no release archives or published releases until explicitly
requested.
