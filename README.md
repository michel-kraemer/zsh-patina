# zsh-patina

**$ A blazingly fast ZSH plugin performing syntax highlighting of your command line while you type 🌈**

The plugin spawns a small background daemon written in Rust. The daemon is shared between ZSH sessions and caches the syntax definition and color theme. Commands of typical length are highlighted in **less than a millisecond**. Extremely long commands only take a few milliseconds.

Internally, the plugin uses [syntect](https://github.com/trishume/syntect/), which provides **high-quality syntax highlighting** based on [Sublime Text](https://www.sublimetext.com/) syntax definitions. The built-in theme uses the eight ANSI colors and is compatible with all terminal emulators.

In contrast to other ZSH syntax highlighters (e.g. [zsh-syntax-highlighting](https://github.com/zsh-users/zsh-syntax-highlighting/) or [fast-syntax-highlighting](https://github.com/zdharma-continuum/fast-syntax-highlighting)), which use different colors to indicate whether a command or a directory/file exists, zsh-patina performs **static highlighting that does not change while you type**. This way, you get a similar experience to editing code in your IDE.

## Examples

<img src="./.github/screenshot.png" alt="Screenshot" />

## How to install

**Prerequisites:** At the moment, there are no pre-compiled binaries. You have to build the plugin yourself. For this, you require [Rust](https://rust-lang.org/) 1.94.0 or higher. The easiest way to install Rust is through [rustup](https://rustup.rs/).

1. Clone the repository:

   ```shell
   git clone https://github.com/michel-kraemer/zsh-patina.git $HOME/.zsh-patina
   ```

2. Build the plugin:

   ```shell
   cd $HOME/.zsh-patina
   cargo build --release
   ```

3. Add the plugin to the end of your `.zshrc` file:

   ```shell
   echo "source ~/.zsh-patina/zsh-patina.plugin.zsh" >> $HOME/.zshrc
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
~/.zsh-patina/target/release/zsh-patina stop
~/.zsh-patina/target/release/zsh-patina start
```

## How to remove the plugin

In the unlikely case you don't like zsh-patina ☹️, you can remove it as follows:

1. Remove the `source ~/.zsh-patina/...` line from your `.zshrc`.
2. Restart the terminal
3. Delete the directory where `zsh-patina` is installed:

   ```shell
   rm -rf $HOME/.zsh-patina
   ```

4. Delete the plugin's data directory:

   ```shell
   rm -rf $HOME/.local/share/zsh-patina/
   ```

## Contribute

I mostly built the plugin for myself because I wasn't satisfied with existing solutions (in terms of accuracy and performance). It doesn't have many features and is not particularly [configurable](#configuration) yet. It does one job, and it does it well IMHO.

If you like the plugin and want to add a feature or found a bug, feel free to contribute. **Issue reports and pull requests are more than welcome!**

## License

zsh-patina is released under the **MIT license**. See the [LICENSE](LICENSE) file
for more information.
