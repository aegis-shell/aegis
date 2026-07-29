# Aegis and Optics Cross-Repository Development

Use a linked Git worktree when one change edits both Aegis and Optics. The
primary Aegis worktree remains the canonical remote-dependency checkout; the
linked worktree resolves the live sibling Optics sources through Cargo
`[patch]`.

## Dependency Modes

| Concern | Primary worktree | Optics development worktree |
|---------|------------------|-----------------------------|
| Rust bindings | Tagged Optics Git source | Sibling `../optics` paths through `[patch]` |
| `Cargo.lock` | Canonical and committed | Local resolution, never committed |
| Cargo configuration | `.cargo/config.toml` absent | Generated from `.cargo/optics-local.toml` |
| Build cache | Primary `target/` | Worktree-local `target/` |
| Native libraries | Matching installed Optics release | Sibling Optics Meson build |

Do not set a shared `CARGO_TARGET_DIR` for these worktrees. Separate target
directories prevent the canonical and patched dependency graphs from reusing
the same incremental artifacts.

## Create the Worktree

Run the following commands from the primary Aegis worktree:

```bash
git fetch origin
git worktree add \
  ../aegis-optics-dev \
  -b feat/<topic> \
  origin/main
```

The expected directory layout is:

```text
projects/
├── aegis/
├── aegis-optics-dev/
└── optics/
```

Enter the linked Aegis worktree, install the local patch configuration, and
enable the repository hooks:

```bash
cd ../aegis-optics-dev
cp .cargo/optics-local.toml .cargo/config.toml
git config core.hooksPath .githooks
```

These commands:

1. Copy the reviewed local `[patch]` template to the ignored
   `.cargo/config.toml`.
2. Enable the repository-owned pre-commit hook.

Do not create `.cargo/config.toml` in the primary Aegis worktree. Confirm that
the linked worktree has an Optics sibling before continuing:

```bash
test -f ../optics/meson.build
```

Build the native libraries, then resolve the worktree-local Cargo graph:

```bash
meson compile -C ../optics/build
cargo check -p aegis
```

The first Cargo command intentionally omits `--locked` because `[patch]`
changes the local lockfile from Git package identities to path package
identities. After that resolution succeeds, ordinary commands may use
`--locked` until an Optics manifest changes.

Verify the selected source:

```bash
cargo tree -i flux
cargo tree -i lens
```

Both trees must show paths below the sibling `optics` checkout.

## Daily Development

Compile Optics before building an Aegis consumer:

```bash
meson compile -C ../optics/build
cargo check --locked -p aegis
cargo test --locked --workspace
```

If an Optics package version or dependency changes, resolve once without
`--locked`:

```bash
cargo check -p aegis
```

The Cargo patch controls the Rust crates only. The `-sys` crates still find
the native `flux`, `flux-scene-graph`, `lens`, and `iris` libraries through
`pkg-config`. Keep the sibling Rust bindings and Meson build at the same
Optics commit.

Do not run concurrent Meson or Ninja commands against the same
`../optics/build` directory. Cargo worktrees have separate build directories,
but the Optics Meson directory remains a shared write location.

## Commit Aegis Changes

Stage changes normally:

```bash
git add .
git commit -m "feat: integrate the Optics change"
```

While `.cargo/config.toml` contains the local Optics patch, the tracked
pre-commit hook automatically unstages:

- `Cargo.lock`; and
- `.cargo/config.toml`, if it was force-added.

The hook prints the excluded paths and lets the remaining commit proceed.
Do not use `--no-verify`. Local patch state is not part of an Aegis feature
commit.

Commits created in the linked worktree already belong to the shared Git
repository. No file-copy step is required. Push the feature branch for a
pull request:

```bash
git push -u origin feat/<topic>
```

Uncommitted files in the linked worktree do not enter `main`; Git merges
commits, not worktree state.

## Rebase onto Aegis Main

The local lockfile is disposable. Restore it before rebasing, then resolve
the patch again:

```bash
git restore Cargo.lock
git fetch origin
git rebase origin/main
cargo check -p aegis
```

The ignored `.cargo/config.toml` remains in the linked worktree across the
rebase.

## Promote an Optics Release

Land and tag Optics before making the Aegis branch canonical. Use one
immutable tag for every Optics crate.

Confirm that the sibling checkout is at the release tag:

```bash
test "$(git -C ../optics rev-parse HEAD)" = \
  "$(git -C ../optics rev-list -n 1 vX.Y.Z)"
meson compile -C ../optics/build
```

Disable local mode and restore the canonical Aegis lockfile:

```bash
mv .cargo/config.toml /tmp/aegis-optics-local.toml
git restore Cargo.lock
```

Update every Optics dependency in the workspace `Cargo.toml` to `vX.Y.Z`.
Verify that they all use one tag:

```bash
scripts/optics-release-ref.sh
```

Resolve and validate the remote graph:

```bash
cargo check -p aegis
cargo check --locked --workspace
cargo test --locked --workspace
cargo build --locked -p aegis
```

Confirm that `cargo tree -i flux` and `cargo tree -i lens` now report the
tagged Git source. Review `Cargo.lock`, then commit the canonical dependency
update:

```bash
git add .
git commit -m "build: adopt Optics vX.Y.Z"
git push
```

The local patch configuration is absent at this point, so the hook permits
the canonical `Cargo.lock` update.

## Merge and Remove the Worktree

Merge through the pull request, then update the primary worktree:

```bash
cd ../aegis
git switch main
git pull --ff-only
```

After the feature branch is merged and the linked worktree is clean, remove
it:

```bash
git worktree remove ../aegis-optics-dev
git branch -d feat/<topic>
```

For a local-only integration, merge the committed branch directly from the
primary worktree:

```bash
git switch main
git merge --ff-only feat/<topic>
```

The ignored local configuration and any uncommitted local lockfile are not
part of that merge.

## Test an Unreleased Optics Commit in CI

When Aegis CI must run before an Optics tag exists, temporarily point every
Optics Git dependency at the same fixed `rev`. Do not use a moving branch.
Replace the fixed revision with the final release tag before merging Aegis.

## Recover the Canonical Mode

If a local patch was enabled in the wrong worktree, move it out of the way
and restore the committed lockfile:

```bash
mv .cargo/config.toml /tmp/aegis-optics-local.toml
git restore Cargo.lock
cargo check --locked --workspace
```

The final command verifies the tagged remote dependency graph.
