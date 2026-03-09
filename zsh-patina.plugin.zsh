# ensure the daemon is running
_zsh_patina_ensure_running() {
    local daemon_path="$_zsh_patina_path/target/release/zsh-patina"

    if [[ ! -x "$daemon_path" ]]; then
        echo "zsh-patina: daemon not found or not executable: $daemon_path" >&2
        return 1
    fi

    # `start` is a no-op when the daemon is already up, so this is always safe
    "$daemon_path" start
}

_zsh_patina() {
    # start=$EPOCHREALTIME

    if (( ! _zsh_patina_zsh_net_socket_available )); then
        print -u2 "zsh-patina: failed to load zsh/net/socket module"
        return
    fi

    # remove tokens we have set earlier - do not clear the whole array as this
    # might reset syntax highlighting from other plugins (e.g. auto suggestions)
    region_highlight=( ${region_highlight:#*memo=zsh_patina} )

    local socket_path
    socket_path="$HOME/.local/share/zsh-patina/daemon.sock"

    # if the socket is gone the daemon has crashed – try to restart it once
    if [[ ! -S "$socket_path" ]]; then
        _zsh_patina_ensure_running || return
        # give it a moment to create the socket
        sleep 0.1
        [[ ! -S "$socket_path" ]] && return
    fi

    # Split buffer into lines
    local -a lines
    lines=("${(@f)BUFFER}")
    local count=${#lines}

    if ! zsocket "$socket_path" 2>/dev/null; then
        print -u2 "zsh-patina: failed to connect to socket at $socket_path"
        return
    fi
    local fd=$REPLY

    {
        # send header
        print -r -- "term_cols=$COLUMNS term_rows=$LINES cursor=$CURSOR line_count=$count"

        # send lines
        print -r -- "$BUFFER"
    } >&$fd || {
        print -u2 "zsh-patina: Write to socket failed"
        exec {fd}>&-
        return
    }

    local line
    while IFS= read -r -u $fd line; do
        [[ -n "$line" ]] && region_highlight+=("$line memo=zsh_patina")
    done

    exec {fd}>&-

    # alternative but spawns an additional process (i.e. nc):
    # printf '%s\n' "$1" | nc -U "$sock" 2>/dev/null

    # end=$EPOCHREALTIME
    # elapsed_ms=$(( (end - start) * 1000 ))
    # printf "%.3f ms\n" $elapsed_ms
}

if ! zmodload zsh/net/socket 2>/dev/null; then
    _zsh_patina_zsh_net_socket_available=0
else
    _zsh_patina_zsh_net_socket_available=1
fi

_zsh_patina_path="${0:A:h}"

autoload -U add-zle-hook-widget
add-zle-hook-widget line-pre-redraw _zsh_patina

# ensure the daemon is running
_zsh_patina_ensure_running
