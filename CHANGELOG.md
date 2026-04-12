# Changelog

## [1.4.0] - 2026-04-11

### Added

- Add support for highlighting history expansions
- Add shell completions
- Add possibility to provide path to configuration file with the `$ZSH_PATINA_CONFIG_PATH` environment variable (contributed by @antinomie8)
- Release `.deb` packages for Debian/Ubuntu
- Release Windows build for [MSYS2] and [Cygwin]

### Fixed

- Fall back to getting `$ZDOTDIR` by spawning a `zsh` process if necessary

## [1.3.1] - 2026-04-05

### Fixed

- Store PID file and Unix domain socket in `$XDG_RUNTIME_DIR` instead of `$XDG_DATA_HOME`
- **Check command:** Try to resolve `.zshrc` file using `$ZDOTDIR`

## [1.3.0] - 2026-04-04

### Changed

- Optimize cold start: `zsh-patina activate` will now run faster if the daemon is not running yet
- Improve support for the `time` keyword
- Apply minor performance optimizations

### Added

- Add [Catppuccin] theme variants (contributed by @carlmlane)
- Add support for Zsh keywords `foreach`, `nocorrect`, `repeat`, and `select`
- Respect `$XDG_CONFIG_HOME` and `$XDG_DATA_HOME` if set
- Add AUR package for Arch Linux users (contributed by @levinion)
- Add logging (start zsh-patina manually with `RUST_LOG=debug zsh-patina start --no-daemon` to get verbose output; valid log levels are `tracing`, `debug`, `info`, `warn`, and `error`)

### Fixed

- Lazily get current working directory to fix dynamic highlighting for the first command when zsh-patina is loaded through zinit
- Correctly highlight a callable followed by a comment
- Configure timeouts for the communication between client and daemon to prevent the shell from becoming unresponsive

## [1.2.0] - 2026-03-28

### Changed

- Consider directories executable if they contain a slash (and not just if they end with a slash)
- Add more output to the `check` command to test for various error sources and display help when zsh-patina doesn't work as expected
- Improve output of `tokenize` command
- Disallow unknown fields in the configuration file to make debugging easier
- Set `region_highlight` only once at the end of the highlighting process to improve overall highlighting performance, especially for long commands
- Don't process empty command lines to slightly reduce the time it takes for a new command prompt to appear
- Don't process dynamic styles outside the terminal window to improve highlighting performance for long commands
- Don't store or highlight lines outside the terminal window to improve highlighting performance for long commands

### Added

- Dynamically highlight redirection targets (such as `>/dev/null`)
- Add `list-themes` command showing all available themes including small examples for preview
- Add Nix flake (contributed by @carlblomqvist)
- Add `classic` theme: an ANSI color theme inspired by [fast-syntax-highlighting's default theme][fsh-default-theme] (contributed by @aaronbruiz)
- Add `solarized` theme: precision colors for machines and people, originally by [Ethan Schoonover][solarized]
- Add support for 8-bit ANSI color codes in custom themes

### Fixed

- Correctly resolve tilde `~` to the user's home directory during dynamic highlighting
- Correctly highlight aliases pointing to missing commands
- Apply `zle_highlight` styles so text in copy&paste mode or reverse search is highlighted correctly
- Improve compatibility with other ZSH plugins such as [zsh-history-substring-search] ([#10])

## [1.1.0] - 2026-03-22

### Added

- Add dynamic highlighting of callables (aliases, builtins, functions, and commands) based on whether they actually exist and are executable; missing callables are shown in a distinct "missing" style (red by default)
- Add dynamic highlighting of paths: arguments that exist and are accessible are underlined by default
- Add option to disable dynamic highlighting in the configuration file
- Add `nord` theme: an arctic, north-bluish color palette based on [Nord]
- Add `tokyonight` theme: celebrates the lights of downtown Tokyo at night, originally by [enkia][tokyonight-origin]
- Add theme inheritance: theme TOML files can now specify a `[metadata]` table with an `extends` key to inherit scopes from another theme (built-in or custom)
- Allow omitting the `foreground` color in theme styles (e.g. `"dynamic.path" = { underline = true }`)
- Auto-restart the daemon after an update of zsh-patina on the next shell start
- Add `--no-daemon` flag to `zsh-patina start` to run the highlighter in the foreground for debugging

## [1.0.0] - 2026-03-13

_First release._

[1.4.0]: https://github.com/michel-kraemer/zsh-patina/releases/tag/1.4.0
[1.3.1]: https://github.com/michel-kraemer/zsh-patina/releases/tag/1.3.1
[1.3.0]: https://github.com/michel-kraemer/zsh-patina/releases/tag/1.3.0
[1.2.0]: https://github.com/michel-kraemer/zsh-patina/releases/tag/1.2.0
[1.1.0]: https://github.com/michel-kraemer/zsh-patina/releases/tag/1.1.0
[1.0.0]: https://github.com/michel-kraemer/zsh-patina/releases/tag/1.0.0
[#10]: https://github.com/michel-kraemer/zsh-patina/issues/10
[Catppuccin]: https://catppuccin.com/
[Cygwin]: https://cygwin.com/
[enkia]: https://github.com/enkia
[fsh-default-theme]: https://github.com/zdharma-continuum/fast-syntax-highlighting/blob/master/themes/default.ini
[MSYS2]: https://www.msys2.org/
[Nord]: https://www.nordtheme.com/
[solarized]: https://ethanschoonover.com/solarized/
[tokyonight-origin]: https://github.com/enkia/tokyo-night-vscode-theme
[zsh-history-substring-search]: https://github.com/zsh-users/zsh-history-substring-search
