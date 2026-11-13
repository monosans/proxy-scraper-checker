<!--
Pull requests are squash-merged, so write the title in the Conventional Commits
style: `fix: ...`, `feat: ...`, `docs: ...`, `refactor: ...`, `test: ...`, `chore: ...`.
-->

## Summary

<!-- What does this change, and why? -->

## Related issues

<!-- `Closes #123` closes the issue on merge. Remove this section if there are none. -->

## Testing

<!-- How did you check that this works? -->

## Checklist

- [ ] `uvx prek run --all-files` passes.
- [ ] `cargo +nightly fmt --check` passes.
- [ ] `cargo +beta clippy --all-targets --features tui --` and `cargo +beta clippy --all-targets --` pass.
- [ ] `cargo +beta test --features tui --locked` and `cargo +beta test --locked` pass, and new behavior has tests.
- [ ] `config.toml`, `config-schema.json` and `README.md` match any change to the options or the output.
