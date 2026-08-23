# Changelog

_Note: This project follows [Semantic Versioning]._

## [1.10.0] - 2026-08-23

**New features**

- Highlight named directories (contributed by @ccjmne 🎊)
- Add `zsh-patina highlight` subcommand (thanks to @iamkroot for this very useful feature request 👍). The subcommand can be used to highlight a command line from an input file or stdin ad-hoc within the current shell session.
- Deprecate flake.nix (contributed by @Lubsch 🎉). The package is now available in nixpkgs-26.05 and nixpkgs-unstable.
- Remove check for `zsh-patina activate` invoked last in the `.zshrc` (contributed by @ccjmne 😎). Placing zsh-patina at the end of the file is now just a recommendation mentioned in the [troubleshooting section][troubleshooting] of the [README].
- Add install instructions for various plugin managers and add `zsh-patina.plugin.zsh` (thanks to @jasonm23 for the idea 💡)
- Bump up client/daemon protocol to v3

**Bug fixes**

- Do not highlight directories as executable unless `autocd` is enabled (contributed by @ccjmne 🥳)
- Fix file descriptor leak that could happen when pressing Ctrl+C during a long-running operation
- Prevent daemon from keeping inherited file descriptors open (thanks to @taciturnaxolotl for the input 💬)

**Maintenance**

- Add [CONTRIBUTING] guide
- Update dependencies
- Update GitHub action dependencies

## [1.9.0] - 2026-07-18

**New features**

- Add zsh-patina to [homebrew-core] 🎉🥳

**Bug fixes**

- Use file lock to prevent race condition when starting the daemon. This prevents multiple daemons from being started at the same time.

**Maintenance**

- Update dependencies
- Raise MSRV to 1.89

## [1.8.0] - 2026-06-14

**New features**

- Add support for the Zsh `AUTO_CD` option. If this option is enabled, directories in callable position will now be correctly highlighted.
- Improve performance by skipping highlighting if there are bytes pending for input. This can happen when pasting from the clipboard or when positioning the cursor with Alt+Click/Option+Click, for example.
- Improve performance by merging similar regular expressions in the Zsh syntax definition
- Refactor client-daemon protocol for increased performance and maintainability

**Bug fixes**

- Quote parameters to protect against the `SH_WORD_SPLIT` option being enabled

**Maintenance**

- Update dependencies
- Only run build script if syntax definition has changed

## [1.7.0] - 2026-05-17

**New features**

- Add the possibility to configure [custom precommands]. A precommand is a command that prefixes another command or a list of arguments, causing zsh-patina to highlight what follows accordingly.

**Bug fixes**

- Add missing `--edit` option to sudo precommand
- Recognize new commands installed after `zsh-patina activate` (contributed by @ccjmne 🚀). Previously, programs newly added to `PATH` would not be highlighted as available until the next shell session.

**Maintenance**

- Migrate highlighter tests to snapshot testing

## [1.6.0] - 2026-05-02

**New features**

- Highlight external precommands `doas`, `env`, `nice`, `nohup`, `sudo`, and `sudoedit`

**Bug fixes**

- Don't assume all percent signs represent a background job (contributed by @ccjmne 🎊)

**Maintenance**

- Check if the installed Zsh version is at least 5.9
- Update dependencies
- Improve unit tests

## [1.5.1] - 2026-04-25

**Bug fixes**

- Respect `BANGHIST` option
- Fix crash caused by heredoc
- Correctly highlight aliases even if they consist of complex commands or command chains, or if they spawn a subshell (support for self-recursive aliases contributed by @ccjmne 🚀)
- Deny unknown fields in theme styles
- Reset command substitution base highlighting to theme defaults (contributed by @ccjmne 🥳)
- Document default highlighting style mechanism (contributed by @ccjmne 😎)

**Maintenance**

- Add Docker-based integration tests
- Cache build artifacts during CI

## [1.5.0] - 2026-04-18

**New features**

- Add kanagawa theme (contributed by @antinomie8 🥳)
- Add install instructions for the Scoop package manager on Windows (contributed by @marovira 🎊)
- Add support for dynamic highlighting of partial paths. This feature is disabled by default and can be activated in zsh-patina's [configuration].
- Add the possibility to highlight partial paths differently than complete ones
- Add highlighting of the built-in precommands `-`, `builtin`, `command`, `exec`, and `noglob`
- Deny unknown fields for the configuration of dynamic highlighting
- Check if zsh-patina is active in the current shell session
- Display more detailed warnings in the check command

**Bug fixes**

- Dynamically highlight directories ending with `/` or starting with `/`, `./`, `../` for consistency. This is similar to how the fish shell highlights directories.
- Always highlight `..` as callable for consistency. This is similar to how `.` is highlighted.

**Maintenance**

- Add CHANGELOG.md
- Deprecate scopes `dynamic.path.file.shell` and `dynamic.path.directory.shell`. Use `dynamic.path.file.partial`, `dynamic.path.file.complete` and `dynamic.path.directory.partial`, `dynamic.path.directory.complete` instead, or simply `dynamic.path.file` and `dynamic.path.directory`.

## [1.4.0] - 2026-04-11

**New features**

- Add support for highlighting history expansions
- Add shell completions
- Add possibility to provide path to configuration file with the `$ZSH_PATINA_CONFIG_PATH` environment variable (contributed by @antinomie8 🎉)
- Release `.deb` packages for Debian/Ubuntu
- Release Windows build for [MSYS2] and [Cygwin]

**Bug fixes**

- Fall back to getting `$ZDOTDIR` by spawning a `zsh` process if necessary

## [1.3.1] - 2026-04-05

**Bug fixes**

- Store PID file and Unix domain socket in `$XDG_RUNTIME_DIR` instead of `$XDG_DATA_HOME`
- Check command: Try to resolve `.zshrc` file using `$ZDOTDIR`

## [1.3.0] - 2026-04-04

**New features**

- Optimize cold start. `zsh-patina activate` will now run faster if the daemon is not running yet.
- Add [Catppuccin] theme variants (contributed by @carlmlane 🥳)
- Add support for Zsh keywords `foreach`, `nocorrect`, `repeat`, and `select`
- Improve support for the `time` keyword
- Respect `$XDG_CONFIG_HOME` and `$XDG_DATA_HOME` if set
- Add AUR package for Arch Linux users (contributed by @levinion 🎉)
- Add logging. Start zsh-patina manually with `RUST_LOG=debug zsh-patina start --no-daemon` to get verbose output. Valid log levels are `tracing`, `debug`, `info`, `warn`, and `error`.
- Apply minor performance optimizations

**Bug fixes**

- Lazily get current working directory. This makes sure we get the actual directory the user has started in and not the one from which `zsh-patina activate` was called. This fixes dynamic highlighting for the first command when zsh-patina is loaded through zinit.
- Correctly highlight a callable followed by a comment
- Configure timeouts for the communication between client and daemon. This prevents the shell from becoming unresponsive in case there is a bug in the communication protocol.

**Maintenance**

- Update dependencies

## [1.2.0] - 2026-03-28

**New features**

- Dynamically highlight redirection targets (such as `>/dev/null`)
- Consider directories executable if they contain a slash (and not just if they end with a slash)
- Add `list-themes` command showing all available themes including small examples for preview
- Add more output to the `check` command. The command now tests for various error sources and displays help when zsh-patina doesn't work as expected.
- Improve output of `tokenize` command
- Disallow unknown fields in the configuration file to make debugging easier
- Add Nix flake (contributed by @carlblomqvist 🎉)
- Add `classic` theme: an ANSI color theme inspired by [fast-syntax-highlighting's default theme][fsh-default-theme] (contributed by @aaronbruiz 🥳)
- Add `solarized` theme: precision colors for machines and people, originally by [Ethan Schoonover][solarized]
- Add support for 8-bit ANSI color codes in custom themes
- Set `region_highlight` only once at the end of the highlighting process. This improves the overall highlighting performance, especially for long commands.
- Don't process empty command lines to slightly reduce the time it takes for a new command prompt to appear
- Don't process dynamic styles outside the terminal window to improve highlighting performance for long commands
- Don't store or highlight lines outside the terminal window to improve highlighting performance for long commands

**Bug fixes**

- Correctly resolve tilde `~` to the user's home directory during dynamic highlighting
- Correctly highlight aliases pointing to missing commands
- Apply `zle_highlight` styles so text in copy&paste mode or reverse search is highlighted correctly
- Improve compatibility with other ZSH plugins such as [zsh-history-substring-search] (see [#10])

**Maintenance**

- Simplify unit tests
- Use temporary home directory in unit tests
- Reorganize code related to CLI commands into modules
- Add table of contents to `README`
- Add troubleshooting section to `README`
- Slightly optimize build process

## [1.1.0] - 2026-03-22

**New features**

- Add dynamic highlighting of callables. Aliases, builtins, functions, and commands are highlighted dynamically based on whether they actually exist and are executable. Missing callables are shown in a distinct "missing" style (red by default).
- Add dynamic highlighting of paths. Files and directories that exist and are accessible are underlined by default.
- Add option to disable dynamic highlighting in the configuration file
- Add `nord` theme: an arctic, north-bluish color palette based on [Nord]
- Add `tokyonight` theme: celebrates the lights of downtown Tokyo at night, originally by [enkia][tokyo-night-vscode-theme]
- Add theme inheritance: theme TOML files can now specify a `[metadata]` table with an `extends` key to inherit scopes from another theme (built-in or custom)
- Allow omitting the `foreground` color in theme styles (e.g. `"dynamic.path" = { underline = true }`)
- Auto-restart the daemon after an update of zsh-patina on the next shell start
- Add `--no-daemon` flag to `zsh-patina start` to run the highlighter in the foreground. This can be useful for debugging.

## [1.0.0] - 2026-03-13

_First release._

[1.10.0]: https://github.com/michel-kraemer/zsh-patina/releases/tag/1.10.0
[1.9.0]: https://github.com/michel-kraemer/zsh-patina/releases/tag/1.9.0
[1.8.0]: https://github.com/michel-kraemer/zsh-patina/releases/tag/1.8.0
[1.7.0]: https://github.com/michel-kraemer/zsh-patina/releases/tag/1.7.0
[1.6.0]: https://github.com/michel-kraemer/zsh-patina/releases/tag/1.6.0
[1.5.1]: https://github.com/michel-kraemer/zsh-patina/releases/tag/1.5.1
[1.5.0]: https://github.com/michel-kraemer/zsh-patina/releases/tag/1.5.0
[1.4.0]: https://github.com/michel-kraemer/zsh-patina/releases/tag/1.4.0
[1.3.1]: https://github.com/michel-kraemer/zsh-patina/releases/tag/1.3.1
[1.3.0]: https://github.com/michel-kraemer/zsh-patina/releases/tag/1.3.0
[1.2.0]: https://github.com/michel-kraemer/zsh-patina/releases/tag/1.2.0
[1.1.0]: https://github.com/michel-kraemer/zsh-patina/releases/tag/1.1.0
[1.0.0]: https://github.com/michel-kraemer/zsh-patina/releases/tag/1.0.0
[#10]: https://github.com/michel-kraemer/zsh-patina/issues/10
[Catppuccin]: https://catppuccin.com/
[configuration]: https://github.com/michel-kraemer/zsh-patina#configuration
[CONTRIBUTING]: https://github.com/michel-kraemer/zsh-patina/blob/main/CONTRIBUTING.md
[custom precommands]: https://github.com/michel-kraemer/zsh-patina#custom-precommands
[Cygwin]: https://cygwin.com/
[fsh-default-theme]: https://github.com/zdharma-continuum/fast-syntax-highlighting/blob/master/themes/default.ini
[homebrew-core]: https://github.com/Homebrew/homebrew-core
[MSYS2]: https://www.msys2.org/
[Nord]: https://www.nordtheme.com/
[README]: https://github.com/michel-kraemer/zsh-patina#readme
[Semantic Versioning]: https://semver.org/
[solarized]: https://ethanschoonover.com/solarized/
[tokyo-night-vscode-theme]: https://github.com/enkia/tokyo-night-vscode-theme
[troubleshooting]: https://github.com/michel-kraemer/zsh-patina#troubleshooting
[zsh-history-substring-search]: https://github.com/zsh-users/zsh-history-substring-search
