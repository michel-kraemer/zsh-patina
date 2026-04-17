---
name: changelog-conventions
description: Conventions for writing and updating CHANGELOG.md and GitHub release notes for zsh-patina. Use this skill when editing the changelog, adding new release entries, or updating GitHub release descriptions.
---

# CHANGELOG conventions for zsh-patina

## Section structure

The project follows [Semantic Versioning](https://semver.org/). Each release uses up to four sections, formatted as **bold text** (not Markdown headings), in this order:

- `**New features**` — backwards-compatible additions; their presence requires at least a MINOR version bump
- `**Breaking changes**` — backwards-incompatible changes; their presence requires a MAJOR version bump
- `**Bug fixes**` — backwards-compatible fixes; present in PATCH and higher releases
- `**Maintenance**` — internal changes with no user-facing impact (e.g. dependency updates, refactoring, test improvements); present in any release

Omit sections that have no entries for a given release.

## Version header format

Each release entry starts with a level-2 heading in this exact format:

```
## [X.Y.Z] - YYYY-MM-DD
```

The version number is always a reference link (e.g. `[1.4.0]`) pointing to the corresponding GitHub release. The reference link definition goes at the bottom of the file with the other version links.

## Reference link ordering

Reference links at the bottom of the file are ordered as follows:

1. Version links, newest to oldest (e.g. `[1.4.0]`, `[1.3.1]`, …)
2. Issue and PR links (e.g. `[#10]`)
3. Named links, alphabetically (e.g. `[Catppuccin]`, `[Nord]`, `[solarized]`, …)

## Writing style

Begin each bullet point with a verb in the **imperative mood** (base form, no subject): *Add*, *Fix*, *Improve*, *Update*, etc. This follows the same convention as Git commit messages, where the implied subject is "this release".

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

Contributor credits use the format `(contributed by @username <emoji>)`, where the emoji is a fun or party-related one to praise the contributor (e.g. 🎉 🥳 🎊 🙌). The choice of emoji is informal and intentionally varied.
