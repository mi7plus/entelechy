<!--
Thanks for contributing to Entelechy. Keep PRs focused; see CONTRIBUTING.md for
dev setup, the full CI gate list, and DCO sign-off requirements.
-->

## Summary

<!-- What does this change and why? Link the PRD requirement id(s) it relates to
     (e.g. RK-2, EV-14, IR-I3) where applicable. -->

## Related

<!-- Issue / discussion links, e.g. Closes #123. -->

## Checklist

- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace` passes (unit + integration + doctests)
- [ ] Touched crates' `//!` docs still cite the relevant PRD section
- [ ] Public API or behaviour changes are noted in `CHANGELOG.md`
- [ ] Every commit is signed off (DCO): `git commit -s`
