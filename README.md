# zsh-patina [![Actions Status](https://github.com/michel-kraemer/zsh-patina/workflows/CI/badge.svg)](https://github.com/michel-kraemer/zsh-patina/actions) [![MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**$ A blazingly fast Zsh plugin performing syntax highlighting of your command line while you type 🌈**

The plugin spawns a small background daemon written in Rust. The daemon is shared between Zsh sessions and caches the syntax definition and color theme. Typical commands are highlighted in **less than a millisecond**. Extremely long commands only take a few milliseconds.

Internally, the plugin relies on [syntect](https://github.com/trishume/syntect/), which provides **high-quality syntax highlighting** based on [Sublime Text](https://www.sublimetext.com/) syntax definitions. With this, you get a similar experience to editing code in your IDE. The default built-in [theme](#theming) uses the eight ANSI colors and is compatible with all terminal emulators.

Besides normal static highlighting, zsh-patina is able to dynamically detect whether a command you entered is valid and can be executed and if the arguments refer to existing files or directories. By default, **invalid commands are shown in red** and **existing files/directories are underlined**. If you want, you can disable dynamic highlighting in the [configuration](#configuration) or change the styling in a [custom theme](#creating-a-custom-theme).

## Examples

<img src="https://raw.githubusercontent.com/michel-kraemer/zsh-patina/4052ba10fcd79c2c7b1d558bad8037affc8077c0/.github/screenshot.png" alt="Screenshot" />

## Table of contents

* [How to install](#how-to-install)
  * [Homebrew (for macOS)](#homebrew-for-macos)
  * [Cargo (for Rust developers)](#cargo-for-rust-developers)
  * [Zinit (for Zinit users)](#zinit-for-zinit-users)
  * [AUR (for Arch Linux users)](#aur-for-arch-linux-users)
  * [flake.nix (for Nix users)](#flakenix-for-nix-users)
  * [Pre-compiled binaries (for everyone)](#pre-compiled-binaries-for-everyone)
  * [Build from source (for the brave ones)](#build-from-source-for-the-brave-ones)
* [Configuration](#configuration)
* [Theming](#theming)
* [Benchmarks](#benchmarks)
* [Troubleshooting](#troubleshooting)
* [How to remove the plugin](#how-to-remove-the-plugin)
* [Contributing](#contributing)
* [License](#license)

## How to install

### Homebrew (for macOS)

1. Install zsh-patina:

   ```shell
   brew tap michel-kraemer/zsh-patina
   brew install zsh-patina
   ```

2. Initialize the plugin at the end of your `.zshrc` file:

   ```shell
   echo "eval \"\$($(brew --prefix)/bin/zsh-patina activate)\"" >> ~/.zshrc
   ```

3. Restart your terminal, or run:

   ```shell
   exec zsh
   ```

### Cargo (for Rust developers)

1. Install zsh-patina:

   ```shell
   cargo install --locked zsh-patina
   ```

2. Initialize the plugin at the end of your `.zshrc` file:

   ```shell
   echo 'eval "$(~/.cargo/bin/zsh-patina activate)"' >> ~/.zshrc
   ```

3. Restart your terminal, or run:

   ```shell
   exec zsh
   ```

### Zinit (for Zinit users)

Just add the following two lines to your `.zshrc` file:

```shell
zinit ice as"program" from"gh-r" pick"zsh-patina-*/zsh-patina" atload'eval "$(zsh-patina activate)"'
zinit light michel-kraemer/zsh-patina
```

### AUR (for Arch Linux users)

1. Install zsh-patina:

    ```shell
    yay -S zsh-patina-git
    # or
    paru -S zsh-patina-git
    ```

2. Initialize the plugin at the end of your `.zshrc` file:

   ```shell
   echo 'eval "$(zsh-patina activate)"' >> ~/.zshrc
   ```

3. Restart your terminal, or run:

   ```shell
   exec zsh
   ```

### flake.nix (for Nix users)

A flake is provided to make the executable the plugin requires available in `/nix/store`.

1. Add this flake to your flake inputs:

   ```nix
   inputs = {
     nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
     zsh-patina = {
       url = "github:michel-kraemer/zsh-patina";
       inputs.nixpkgs.follows = "nixpkgs";
     };
   }
   ```

2. Add the executable to your `systemPackages`, making it available in your PATH (optional):

   ```nix
   environment.systemPackages = [
     inputs.zsh-patina.packages.${pkgs.stdenv.hostPlatform.system}.default
   ];
   ```

3. Activate the plugin in your `.zshrc` file:

   ```shell
   # If you've added zsh-patina to systemPackages
   eval "$(zsh-patina activate)"

   # Reference the executable direcly
   eval "$(${inputs.zsh-patina.packages.${pkgs.stdenv.hostPlatform.system}.default}/bin/zsh-patina activate)"
   ```

4. Restart your terminal, or run:

   ```shell
   exec zsh
   ```

### Pre-compiled binaries (for everyone)

1. Visit https://github.com/michel-kraemer/zsh-patina/releases and download the appropriate archive for your system. There are binaries for Linux and macOS.

2. Extract the archive to an arbitrary directory. For example, if you want to extract it to `~/.zsh-patina`:

   ```shell
   mkdir ~/.zsh-patina
   tar xfz zsh-patina-v1.2.0-aarch64-apple-darwin.tar.gz -C ~/.zsh-patina --strip-components 1
   ```

3. Initialize the plugin at the end of your `.zshrc` file:

   ```shell
   echo 'eval "$(~/.zsh-patina/zsh-patina activate)"' >> ~/.zshrc
   ```

4. Restart your terminal, or run:

   ```shell
   exec zsh
   ```

### Build from source (for the brave ones)

**Prerequisites:** To build the plugin, you need to have [Rust](https://rust-lang.org/) 1.88.0 or higher on your system. The easiest way to install Rust is through [rustup](https://rustup.rs/).

1. Clone the repository:

   ```shell
   git clone https://github.com/michel-kraemer/zsh-patina.git ~/.zsh-patina
   ```

2. Build the plugin:

   ```shell
   cd ~/.zsh-patina
   cargo build --release
   ```

3. Initialize the plugin at the end of your `.zshrc` file:

   ```shell
   echo 'eval "$(~/.zsh-patina/target/release/zsh-patina activate)"' >> ~/.zshrc
   ```

4. Restart your terminal, or run:

   ```shell
   exec zsh
   ```

## Configuration

zsh-patina can be configured through an optional configuration file at `~/.config/zsh-patina/config.toml`. If the file doesn't exist, the plugin uses the default settings shown below.

**Example configuration:**

```toml
[highlighting]
# Either the name of a built-in theme (e.g. `"simple"`, `"patina"`) or a string
# in the form `"file:/path/mytheme.toml"` pointing to a custom theme toml file.
theme = "patina"

# Enable or disable dynamic highlighting. Can be `true` or `false` ...
dynamic = true

# ... or a table with the keys `callables` and `paths`.
# dynamic = { callables = true, paths = true }

# For performance reasons, highlighting is disabled for very long lines. This
# option specifies the maximum length of a line (in bytes) up to which
# highlighting is applied.
max_line_length = 20000

# The maximum time (in milliseconds) to spend on highlighting a command. If
# highlighting takes longer, it will be aborted and the command will be
# partially highlighted.
#
# Note that the timeout only applies to multi-line commands. Highlighting cannot
# be aborted in the middle of a line. If you often deal with long lines that
# take longer to highlight than the timeout, consider reducing `max_line_length`.
timeout_ms = 500
```

After changing the configuration, restart the daemon with:

```shell
zsh-patina restart
```

## Theming

zsh-patina supports custom syntax highlighting themes. You can choose from one of the built-in themes or create your own.

Note that after changing the `theme` setting or editing your custom theme file, as described [above](#configuration), you need to restart the daemon so the new colors are applied.

### Built-in themes

Set the `theme` option in your configuration file (`~/.config/zsh-patina/config.toml`):

```toml
[highlighting]
theme = "patina"
```

The following built-in themes are available:

| Theme | Description |
|-------|-------------|
| `patina` | The **default** theme with a balanced color palette. |
| `catppuccin-frappe` | A soothing dark theme with muted tones. Based on [Catppuccin Frappé](https://catppuccin.com/palette#frappe). |
| `catppuccin-latte` | A soothing light theme with pastel tones. Based on [Catppuccin Latte](https://catppuccin.com/palette#latte). |
| `catppuccin-macchiato` | A soothing dark theme with vibrant tones. Based on [Catppuccin Macchiato](https://catppuccin.com/palette#macchiato). |
| `catppuccin-mocha` | A soothing warm-toned dark theme. Based on [Catppuccin Mocha](https://catppuccin.com/palette#mocha). |
| `classic` | ANSI color theme inspired by [fast-syntax-highlighting's default theme](https://github.com/zdharma-continuum/fast-syntax-highlighting/blob/master/themes/default.ini).
| `lavender` | A variant with magenta/lavender tones. |
| `nord` | An arctic, north-bluish color palette. Based on [Nord](https://www.nordtheme.com/). |
| `simple` | A minimal theme with fewer colors. |
| `solarized` | Precision colors for machines and people. Originally by [Ethan Schoonover](https://ethanschoonover.com/solarized/).
| `tokyonight` | Celebrates the lights of downtown Tokyo at night. Originally by [enkia](https://github.com/tokyo-night/tokyo-night-vscode-theme). |

To load a custom theme from a file, use the `file:` prefix:

```toml
[highlighting]
theme = "file:/path/to/mytheme.toml"
```

The path must be absolute. It can start with a tilde `~` (for your home directory), and you can use environment variables such as `$HOME`.

If you want a quick preview of all available themes with highlighted example commands, just run:

```shell
zsh-patina list-themes
```

### Creating a custom theme

A theme is a [TOML](https://toml.io/) file that maps **scopes** to **styles**. Each key is a scope name (note the quotation marks!) and each value is either a string denoting a foreground [color](#colors) or a [style](#styles). For example:

```toml
# comments
"comment" = "#a0a0a0"

# strings
"string" = "green"

# escape characters
"constant.character.escape" = "yellow"

# environment variables
"variable.other" = "yellow"
"punctuation.definition.variable" = "yellow"

# commands
"variable.function" = "cyan"

# keywords
"keyword" = { foreground = "blue", background = "red" }
```

### Colors

Colors can be specified as:

- One of the eight **ANSI colors**: `black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `white`
- A **hex color** in the format `#RRGGBB` (e.g. `"#a0a0a0"`) or `#RGB` (e.g. `"#f00"`)
- An integer in the range `0-255` specifying an **8-bit ANSI color code**.

ANSI color names use your terminal's color scheme, so the actual appearance depends on your terminal configuration. Hex colors are displayed as true colors (24-bit) if your terminal supports them.

### Styles

A style is a struct with a foreground color and a background color (both are optional). In addition, you can specify if text should be shown in bold or underlined.

For example:

```toml
"keyword" = { foreground = "blue", underline = true }
"variable.function" = { foreground = "cyan", background = "red", bold = true }
```

Or in TOML's table syntax:

```toml
["keyword"]
foreground = "blue"
underline = true

["variable.function"]
foreground = "cyan"
background = "red"
bold = true
```

Note that as of Zsh 5.9, it's unfortunately not possible to show text on the command line in italics as the ZLE (Zsh Line Editor) only supports [bold and underlined](https://zsh.sourceforge.io/Doc/Release/Zsh-Line-Editor.html#Character-Highlighting).

### Scopes

Scopes follow the [Sublime Text scope naming convention](https://www.sublimetext.com/docs/scope_naming.html). A scope like `keyword` matches all keyword-related tokens (e.g. `keyword.control.for.shell`, `keyword.operator`). More specific scopes take precedence over general ones.

To list all available scopes, run:

```shell
zsh-patina list-scopes
```

You can also use the `tokenize` subcommand to inspect which scopes are assigned to parts of a command:

```shell
echo 'for i in 1 2 3; do echo $i; done' | zsh-patina tokenize
```

### Dynamic scopes

zsh-patina dynamically highlights commands, files, and directories based on whether they exist and are accessible. The styles used for this can be controlled with the following scopes:

```toml
# General scope for everything this is callable
"dynamic.callable" = "cyan"

# This style will be applied to aliases, commands, etc. that are not executable
"dynamic.callable.missing" = "red"

# This style will be applied to existing files/directories
"dynamic.path" = { underline = true }

# Optional fine-grained scopes for each individual callable
"dynamic.callable.alias" = "cyan"
"dynamic.callable.builtin" = "cyan"
"dynamic.callable.command" = "cyan"
"dynamic.callable.function" = "cyan"
```

The styles of the dynamic scopes are *mixed into* the normal styles, which means, first the normal styles are applied, and then every attribute of the dynamic style overwrites the normal style's attribute with the same name. For example, if `variable.function.shell` (the normal style for callables if dynamic highlighting is disabled) specifies that a callable should be highlighted in blue, and `dynamic.callable.command.shell` specifies `underline = true`, then any command that exists and can be executed will be highlighted in blue *and* underlined.

### Extending another theme

If you want your custom theme to extend an existing one (either a built-in theme or another custom file), you can use the `extends` property in the `metadata` table. Please note that due to the way TOML files are structured, the metadata table must be placed the end of the file. For example:

```toml
# Override just the scopes you want to change:
"comment" = "red"

[metadata]
# extend the built-in nord theme
extends = "nord"

# ... or extend another custom theme
# extends = "file:/path/to/another/theme.toml"
```

## Benchmarks

Here are the results from benchmarks I ran to compare the performance of zsh-patina with that of [zsh-syntax-highlighting](https://github.com/zsh-users/zsh-syntax-highlighting) and [fast-syntax-highlighting](https://github.com/zdharma-continuum/fast-syntax-highlighting). The benchmarks were executed with [zsh-bench](https://github.com/romkatv/zsh-bench).

&nbsp; | zsh-patina | zsh-syntax-highlighting | fast-syntax-highlighting
-|-|-|-
first_prompt_lag_ms | **17.680** | 23.389 | 26.164
first_command_lag_ms | **26.090** | 31.771 | 28.601
command_lag_ms |  **0.197** | 0.528 | 0.240
input_lag_ms | **1.394** | 8.385 | 3.643
exit_time_ms | **17.725** | 19.934 | 24.095

Fastest times are displayed in **bold**.

**Setup:** I ran the benchmarks with a clean Zsh configuration. The only thing that was included in the `.zshrc` file was the code required to initialize the individual plugins. I ran all benchmarks 5 times to make sure the numbers are consistent. I have copied the results from the fastest run for each plugin here. The benchmarks were executed on a MacBook Pro 16″ 2023.

**Disclaimer:** Benchmarks are hard and the numbers may be different on other systems, so take these results with a grain of salt. You may want to run zsh-bench on your own system and with your own setup.

## Troubleshooting

If the plugin doesn't work as expected, please first check if you've followed the [install instructions](#how-to-install) correctly. The plugin must be activated in your `.zshrc` file, and this must happen **at the end of the file** (after all other instructions).

You may also run zsh-patina's self-check:

```shell
zsh-patina check
```

If the self-check doesn't find any errors, it will print `Everything is OK`. Otherwise, you will get hints about what might be wrong.

### Plugin has no effect on startup, but works after manual `source`

This can happen in minimal environments (e.g. Docker containers or some Linux distributions) where ZLE (Zsh Line Editor) is not fully initialized when the `.zshrc` file is loaded. Make sure your `.zshrc` file calls `compinit` or `bindkey` before the `eval` line:

```zsh
autoload -Uz compinit && compinit
eval "$(zsh-patina activate)"
```

### None of the above

If none of the above has helped, and you cannot solve the issue yourself, please [open an issue](https://github.com/michel-kraemer/zsh-patina/issues).

## How to remove the plugin

In the unlikely case you don't like zsh-patina ☹️, you can remove it as follows (note that these instructions assume you've installed the plugin in `~/.zsh-patina`):

1. Remove the `eval "$(~/.zsh-patina/zsh-patina activate)"` line from your `.zshrc`.
2. Restart the terminal
3. Stop the daemon:

   ```shell
   ~/.zsh-patina/zsh-patina stop
   ```

4. Delete the directory where `zsh-patina` is installed:

   ```shell
   rm -rf ~/.zsh-patina
   ```

5. Delete the plugin's data directory:

   ```shell
   rm -rf ~/.local/share/zsh-patina/
   ```

6. If you have created a [configuration](#configuration) file, you may also want to delete the configuration directory:

   ```shell
   rm -rf ~/.config/zsh-patina/
   ```

## Contributing

I mostly built the plugin for myself because I wasn't satisfied with existing solutions (in terms of accuracy and performance). zsh-patina does one job, and it does it well IMHO.

If you like the plugin as much as I do and want to add a feature or found a bug, feel free to contribute. **Issue reports and pull requests are more than welcome!**

## License

zsh-patina is released under the **MIT license**. See the [LICENSE](LICENSE) file for more information.
