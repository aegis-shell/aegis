Before writing, modifying, or archiving any documentation, please read and follow `docs/dev/documentation/index.md` in the project root. AI assistants may read `docs/dev/documentation/` and suggest changes, but must not directly modify files in that directory.

Do not bypass Git hooks. When local Optics mode is active, the pre-commit hook
keeps the worktree-local `Cargo.lock` and `.cargo/config.toml` out of commits.
