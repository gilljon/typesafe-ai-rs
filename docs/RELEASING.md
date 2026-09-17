# Releasing

The GitHub repository is `gilljon/typesafe-ai-rs`, and the package is named
`typesafe-ai-rs` (imported as `typesafe_ai_rs`).

For a registry release:

1. Update the package version and confirm that version is unused on crates.io.
   For the first release, confirm the package name remains available.
2. Authenticate locally with `cargo login`, or configure a scoped `CARGO_REGISTRY_TOKEN`
   secret for the release workflow. Ensure the publishing account has a verified email
   address. Do not put tokens in repository files.
3. Run the checks in CI, including package verification. Check the parity notes against
   the pinned upstream versions and update the changelog.
4. Run `cargo publish --dry-run`, then `cargo publish`, or dispatch the Publish workflow
   from the reviewed main-branch commit with its matching version.
5. Verify the published package and docs.rs build. Create the matching Git tag and
   GitHub release, and update the README's alternative Git installation tag.

Publication uses a manually dispatched workflow, restricted to `main`, with formatting,
linting, tests, package verification, and a version confirmation before upload. The workflow
does not publish automatically on pushes or tags.
