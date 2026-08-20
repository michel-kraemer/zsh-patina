# This file is sourced automatically by plugin managers that follow the
# "<name>.plugin.zsh" convention (e.g. oh-my-zsh, antigen). It only handles
# activation. The zsh-patina binary itself must already be installed and
# available on PATH (see the README for installation instructions).

if (( ${+commands[zsh-patina]} )); then
    eval "$(zsh-patina activate)"

    # Load completions but only if they haven't been registered yet, i.e. if
    # they haven't already been installed into the site-functions directory
    if (( ! $+functions[_zsh-patina] )); then
        eval "$(zsh-patina completion)"
    fi
else
    print -u2 "zsh-patina binary not found in PATH. Please install it first:"
    print -u2 "https://github.com/michel-kraemer/zsh-patina#how-to-install"
fi
