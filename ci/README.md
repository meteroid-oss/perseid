# GitHub Actions workflow

After the generator in PR #1 is merged, move `ci/github-actions.yml` to
`.github/workflows/ci.yml` to enable the Rust and all-language generation checks.

This file is staged here for manual activation because the publishing credential
cannot create files under `.github/workflows/`. GitHub Actions does not execute
workflows from this directory.
