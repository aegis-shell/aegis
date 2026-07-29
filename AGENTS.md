Before writing, modifying, or archiving any documentation, please read and follow `docs/dev/documentation/index.md` in the project root. AI assistants may read `docs/dev/documentation/` and suggest changes, but must not directly modify files in that directory.

Do not bypass Git hooks. When local Optics mode is active, the pre-commit hook
keeps the worktree-local `Cargo.lock` and `.cargo/config.toml` out of commits.

Local Optics mode is active when `.cargo/config.toml` contains
`[patch."https://github.com/ming2k/optics"]`. In that mode:

- Treat `.cargo/config.toml` and the path-resolved `Cargo.lock` as local
  worktree state. They must not be staged or committed.
- Leave `Cargo.lock` in the state produced by the local `../optics` patch.
  Do not restore the Git `source` fields merely to make `git status` clean;
  Cargo will remove them again, and doing so breaks the intended joint
  development workflow.
- Do not use `--no-verify`, force-add either file, or otherwise defeat the
  pre-commit hook. Check the staged diff and let the hook unstage either file
  if necessary.
- Update the canonical committed lockfile only after intentionally disabling
  local Optics mode to promote a released Optics revision.
