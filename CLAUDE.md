# Uniform: notes for Claude

- Interfaces: `docs/SPEC.md` and `shared/`. Design: `docs/plans/2026-11-06-uniform-design.md`.
- Work in the normal checkout on `dev`. Do not use the Superpowers `using-git-worktrees` skill, and do not create worktrees or extra clones.
- Each commit is one meaningful unit of work: not a big mixed commit, and not a string of tiny follow-up tweaks (fold those into the commit they belong to).
- No `Co-Authored-By` or other AI attribution lines in commits or PRs.
- Ask before every push; committing locally is fine.
- Stay inside your owner's folders (see "Who owns what" in SPEC.md). Change `shared/`, `docs/SPEC.md` or `server/Cargo.toml` only when both owners agree.
- Never print, commit or ask for private keys. Deploys use the Foundry keystore (`--account deployer`); the user types the password.
- Testnet funds are limited: test on a local fork first and spend real gas only for the final check.
