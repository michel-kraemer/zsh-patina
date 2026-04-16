---
name: changelog-conventions
description: Conventions for writing and updating CHANGELOG.md and GitHub release notes for zsh-patina. Use this skill when editing the changelog, adding new release entries, or updating GitHub release descriptions.
---

# CHANGELOG conventions for zsh-patina

## Section structure

Each release uses three sections, formatted as **bold text** (not Markdown headings):

- `**New features**`
- `**Bug fixes**`
- `**Maintenance**`

Omit sections that have no entries for a given release.

## Full stop rule

- Single-sentence bullet points: no trailing full stop
- Multi-sentence bullet points: use full stops on all sentences

## Reference links

Use reference-style links at the bottom of the file (e.g. `[Nord]`, `[Catppuccin]`).

## GitHub release notes

Release notes on GitHub mirror the CHANGELOG entries exactly (same wording, same structure). When writing release notes via the GitHub CLI, expand reference-style links to inline links since the reference table at the bottom of the CHANGELOG is not available in that context.

Update a release with:

```
gh release edit <tag> --repo michel-kraemer/zsh-patina --notes-file <file>
```

Contributor credits use the format `(contributed by @username 🎉)` or `(contributed by @username 🥳)`.
