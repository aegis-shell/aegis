# Aegis and Optics Cross-Repository Development

Use one long-lived linked Git worktree for Aegis and Optics development. The
primary Aegis worktree remains on `main` with the canonical remote dependency
graph. The development worktree normally remains on the long-lived local
`dev` branch and resolves the live sibling Optics sources through Cargo
`[patch]`.

## Dependency Modes

| Concern | Primary worktree | Aegis development worktree |
|---------|------------------|-----------------------------|
| Rust bindings | Tagged Optics Git source | Sibling `../optics` paths through `[patch]` |
| `Cargo.lock` | Canonical and committed | Local resolution, never committed |
| Cargo configuration | `.cargo/config.toml` absent | Generated from `.cargo/optics-local.toml` |
| Build cache | Primary `target/` | Worktree-local `target/` |
| Native libraries | Matching installed Optics release | Sibling Optics Meson build |

Do not set a shared `CARGO_TARGET_DIR` for these worktrees. Separate target
directories prevent the canonical and patched dependency graphs from reusing
the same incremental artifacts.

## Create the Development Worktree

Create the linked worktree once from the primary Aegis worktree:

```bash
git worktree add -b dev ../aegis-dev main
```

The expected directory layout is:

```text
projects/
├── aegis/
├── aegis-dev/
└── optics/
```

The primary `aegis/` worktree keeps `main` checked out. The `aegis-dev/`
worktree permanently keeps the local `dev` branch checked out.

Enter the development worktree, install the local patch configuration, and
enable the repository hooks once:

```bash
cd ../aegis-dev
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
Do not use `--no-verify`. Local patch state is not part of an Aegis commit.

Commits created on `dev` already belong to the shared Git repository. No
file-copy, push, or pull step is required for a local merge. Uncommitted files
in the development worktree do not enter `main`; Git merges commits, not
worktree state.

## Synchronize with Aegis Main

Update `main` in the primary worktree first. This may be a local merge or a
pull when remote changes exist:

```bash
cd ../aegis
git switch main
git pull --ff-only
```

Skip the pull when `main` is already current locally. Then restore the
disposable local lockfile and rebase the development branch onto the shared
local `main` branch:

```bash
cd ../aegis-dev
git restore Cargo.lock
git rebase main
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
```

The local patch configuration is absent at this point, so the hook permits
the canonical `Cargo.lock` update.

## Merge Locally and Reuse the Worktree

Fast-forward `main` to the completed `dev` commits from the primary worktree:

```bash
cd ../aegis
git switch main
git merge --ff-only dev
```

This operation uses commits already stored in the shared local repository. It
does not contact a remote server.

Both branches now point at the same commit. Continue with the next change in
the existing development worktree:

```bash
cd ../aegis-dev
git restore Cargo.lock
cp .cargo/optics-local.toml .cargo/config.toml
cargo check -p aegis
```

The copy command is harmless when local mode remained enabled. It also
restores local mode when the previous change removed `.cargo/config.toml`
during Optics release promotion.

## Optional Pull Request Workflow

Use a remote pull request only when CI, review, backup, or collaboration
requires it. Start that change on a temporary feature branch from canonical
`main` instead of advancing `dev`:

```bash
cd ../aegis-dev
git restore Cargo.lock
git switch -c feat/<topic> main
cargo check -p aegis

# After committing the reviewed change:
git push -u origin feat/<topic>

# After the remote pull request is merged:
cd ../aegis
git switch main
git pull --ff-only

cd ../aegis-dev
git restore Cargo.lock
git switch dev
git merge --ff-only main
cp .cargo/optics-local.toml .cargo/config.toml
cargo check -p aegis
```

The development worktree remains in place and returns to `dev` after the pull
request. Delete the temporary branch when it is no longer needed.

## Remove the Development Worktree

Remove the worktree only when cross-repository development is no longer
needed:

```bash
cd ../aegis-dev
mv .cargo/config.toml /tmp/aegis-optics-local.toml
git restore Cargo.lock

cd ../aegis
git worktree remove ../aegis-dev
```

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
