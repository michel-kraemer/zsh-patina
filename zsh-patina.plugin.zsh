# This file is sourced automatically by plugin managers that follow the
# "<name>.plugin.zsh" convention (e.g. oh-my-zsh, antigen). It only handles
# activation. The zsh-patina binary itself must already be installed and
# available on PATH (see the README for installation instructions).

if (( ${+commands[zsh-patina]} )); then
    eval "$(zsh-patina activate)"
else
    print -u2 "zsh-patina binary not found in PATH. Please install it first:"
    print -u2 "https://github.com/michel-kraemer/zsh-patina#how-to-install"
fi
