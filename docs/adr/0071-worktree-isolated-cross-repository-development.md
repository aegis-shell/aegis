# ADR-0071: Worktree-isolated Aegis and Optics development

- Status: Accepted
- Date: 2026-07-29

## Context

[ADR-0067](0067-remote-optics-dependencies-and-local-overrides.md)
established tagged Optics Git dependencies as the canonical Aegis contract
and an ignored Cargo configuration for sibling development. Its local
configuration used Cargo `paths` overrides so the committed remote
`Cargo.lock` could remain unchanged.

Cargo `paths` overrides cannot change the structure or source identity of a
package's dependencies. The Optics monorepo contains transitive relationships
such as `flux` to `flux-sys`, `lens` to `flux`, and `iris` to `lens`. Replacing
the Git packages with individual path packages changes those dependency
identities. Cargo therefore warns that the graph can produce spurious
recompiles and that the warning may become an error.

Cargo `[patch]` is the supported mechanism for replacing a Git source with
local packages, but it intentionally participates in dependency resolution.
Enabling it changes the local `Cargo.lock`. Switching the canonical and local
graphs in one working directory also makes both modes share incremental build
artifacts and creates a persistent risk that the local lockfile is committed.

## Decision

Aegis keeps tagged Optics Git dependencies and their resolved commit in the
canonical `Cargo.toml` and `Cargo.lock`. The primary worktree, CI, release
builds, and distribution packaging do not contain `.cargo/config.toml`.

Cross-repository development takes place in a linked Git worktree next to the
Optics checkout. The ignored `.cargo/config.toml` in that worktree uses
`[patch."https://github.com/ming2k/optics"]` entries for every Optics binding
crate. The worktree owns its local `Cargo.lock` contents and `target`
directory. The first Cargo command after enabling or changing the patch may
update the local lockfile; subsequent commands may use `--locked`.

A tracked pre-commit hook automatically removes `Cargo.lock` and
`.cargo/config.toml` from the staged set while the local Optics patch is
active. Contributors and automated tools do not bypass repository hooks.
This preserves ordinary `git add .` usage without allowing local dependency
state into feature commits.

An Optics-dependent Aegis change is promoted only after Optics has an
immutable release tag or fixed commit. Promotion removes the local Cargo
configuration, restores the canonical lockfile, updates every Optics Git
dependency together, resolves the new remote graph, and validates it with
`--locked`. Only that canonical `Cargo.toml` and `Cargo.lock` update enters
the Aegis development branch.

CI derives the Optics native-library checkout tag from the workspace
manifest. The Rust bindings, native libraries, and lockfile therefore use one
Optics release.

This decision supersedes
[ADR-0067](0067-remote-optics-dependencies-and-local-overrides.md).

## Alternatives

- **Continue using Cargo `paths`.** This keeps the canonical lockfile clean,
  but relies on a dependency-graph change that Cargo identifies as buggy and
  plans to reject.
- **Use `[patch]` in the primary worktree.** This is supported by Cargo, but
  makes the canonical checkout persistently dirty and mixes remote and local
  artifacts in one build directory.
- **Commit a permanent local patch and lockfile branch.** This produces a
  clean local index, but every feature must strip or cherry-pick around the
  local-only base commit before review.
- **Share one target directory across worktrees.** This saves some disk
  space, but defeats dependency-mode isolation and reintroduces incremental
  artifact churn.

## Consequences

- Canonical Aegis checkouts remain reproducible without a sibling Optics
  repository.
- Local Aegis and Optics edits use Cargo's supported Git-source patch
  mechanism without path-override warnings.
- Local and canonical dependency graphs use different worktrees, lockfile
  contents, and build caches.
- Contributors can continue to stage with `git add .`; the repository hook
  removes local dependency state before the commit is created.
- Cross-repository changes land Optics first, then promote and land Aegis
  against an immutable Optics reference.
- A local worktree must regenerate its patch-resolved lockfile after rebasing
  across canonical dependency changes.
- Native Optics libraries and Rust bindings must still be built from the same
  Optics commit.
