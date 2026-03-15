# zsh-patina [![Actions Status](https://github.com/michel-kraemer/zsh-patina/workflows/CI/badge.svg)](https://github.com/michel-kraemer/zsh-patina/actions) [![MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**$ A blazingly fast Zsh plugin performing syntax highlighting of your command line while you type 🌈**

The plugin spawns a small background daemon written in Rust. The daemon is shared between Zsh sessions and caches the syntax definition and color theme. Typical commands are highlighted in **less than a millisecond**. Extremely long commands only take a few milliseconds.

Internally, the plugin relies on [syntect](https://github.com/trishume/syntect/), which provides **high-quality syntax highlighting** based on [Sublime Text](https://www.sublimetext.com/) syntax definitions. The default built-in [theme](#theming) uses the eight ANSI colors and is compatible with all terminal emulators.

In contrast to other Zsh syntax highlighters (e.g. [zsh-syntax-highlighting](https://github.com/zsh-users/zsh-syntax-highlighting/) or [fast-syntax-highlighting](https://github.com/zdharma-continuum/fast-syntax-highlighting)), which use different colors to indicate whether a command or a directory/file exists, zsh-patina performs **static highlighting that solely depends on the characters you enter**. This way, you get a similar experience to editing code in your IDE.

## Examples

<img src="https://raw.githubusercontent.com/michel-kraemer/zsh-patina/19d811a1f8af94161871b9cf1e0d8302a032c873/.github/screenshot.png" alt="Screenshot" />

## How to install

### Homebrew (for macOS)

1. Install zsh-patina:

   ```shell
   brew tap michel-kraemer/zsh-patina
   brew install zsh-patina
   ```

2. Initialize the plugin at the end of your `.zshrc` file:

   ```shell
   echo "eval \"\$($(brew --prefix)/bin/zsh-patina activate)\"" >> $HOME/.zshrc
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
   echo 'eval "$(~/.cargo/bin/zsh-patina activate)"' >> $HOME/.zshrc
   ```

3. Restart your terminal, or run:

   ```shell
   exec zsh
   ```

### Pre-compiled binaries (for everyone)

1. Visit https://github.com/michel-kraemer/zsh-patina/releases and download the appropriate archive for your system. There are binaries for Linux and macOS.

2. Extract the archive to an arbitrary directory. For example, if you want to extract it to `~/.zsh-patina`:

   ```shell
   mkdir ~/.zsh-patina
   tar xfz zsh-patina-v1.0.0-aarch64-apple-darwin.tar.gz -C ~/.zsh-patina --strip-components 1
   ```

3. Initialize the plugin at the end of your `.zshrc` file:

   ```shell
   echo 'eval "$(~/.zsh-patina/zsh-patina activate)"' >> $HOME/.zshrc
   ```

4. Restart your terminal, or run:

   ```shell
   exec zsh
   ```

### Build from source (for the brave ones)

**Prerequisites:** To build the plugin, you need to have [Rust](https://rust-lang.org/) 1.88.0 or higher on your system. The easiest way to install Rust is through [rustup](https://rustup.rs/).

1. Clone the repository:

   ```shell
   git clone https://github.com/michel-kraemer/zsh-patina.git $HOME/.zsh-patina
   ```

2. Build the plugin:

   ```shell
   cd $HOME/.zsh-patina
   cargo build --release
   ```

3. Initialize the plugin at the end of your `.zshrc` file:

   ```shell
   echo 'eval "$(~/.zsh-patina/target/release/zsh-patina activate)"' >> $HOME/.zshrc
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

zsh-patina supports custom syntax highlighting themes. You can choose one of the built-in themes or create your own.

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
| `patina` | The **default** theme with a balanced color palette |
| `lavender` | A variant with magenta/lavender tones |
| `simple` | A minimal theme with fewer colors |
| `tokyonight` | Celebrates the lights of downtown Tokyo at night. Originally by [enkia](https://github.com/tokyo-night/tokyo-night-vscode-theme). |

To load a custom theme from a file, use the `file:` prefix:

```toml
[highlighting]
theme = "file:/path/to/mytheme.toml"
```

The path must be absolute. It can start with a tilde `~` (for your home directory), and you can use environment variables such as `$HOME`.

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

ANSI color names use your terminal's color scheme, so the actual appearance depends on your terminal configuration. Hex colors are displayed as true colors (24-bit) if your terminal supports them.

### Styles

A style is a struct with a foreground color and an optional background color. In addition, you can specify if text should be shown in bold or underlined.

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

## How to remove the plugin

In the unlikely case you don't like zsh-patina ☹️, you can remove it as follows (note that these instructions assume you've installed the plugin in `~/.zsh-patina`):

1. Remove the `eval "$(~/zsh-patina activate)"` line from your `.zshrc`.
2. Restart the terminal
3. Stop the daemon:

   ```shell
   ~/zsh-patina stop
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

## Contribute

I mostly built the plugin for myself because I wasn't satisfied with existing solutions (in terms of accuracy and performance). zsh-patina does one job, and it does it well IMHO.

If you like the plugin as much as I do and want to add a feature or found a bug, feel free to contribute. **Issue reports and pull requests are more than welcome!**

## License

zsh-patina is released under the **MIT license**. See the [LICENSE](LICENSE) file for more information.
