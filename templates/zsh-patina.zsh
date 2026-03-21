# WARNING: Do not cache or source this file manually. Its contents are generated
# automatically when zsh-patina is started via the `activate` command. To set up
# zsh-patina, add the following to your .zshrc:
#
#   eval "$(zsh-patina activate)"
#
# For more details, see the README:
# https://github.com/michel-kraemer/zsh-patina#how-to-install

zsh-patina() {
    "<{zsh_patina_path}>" "$@"
}

_zsh_patina_resolve_callable() {
    if (( $+aliases[(e)$1] || $+galiases[(e)$1] )); then
        REPLY=a
    elif (( $+functions[(e)$1] )); then
        REPLY=f
    elif (( $+builtins[(e)$1] )); then
        REPLY=b
    elif (( $+commands[(e)$1] )); then
        REPLY=c
    else
        REPLY=m
    fi
}

_zsh_patina_encode_string() {
    # fast path
    [[ $1 != *[%$'\t\n\r\f ']* ]] && { REPLY="$1"; return }

    # only encode characters recognized by Rust's split_ascii_whitespace()
    local s="${1//'%'/%25}"
    s="${s//' '/%20}"
    s="${s//$'\t'/%09}"
    s="${s//$'\n'/%0A}"
    s="${s//$'\r'/%0D}"
    s="${s//$'\f'/%0C}"

    REPLY="$s"
}

_zsh_patina_decode_string() {
    # fast path
    [[ $1 != *%* ]] && { REPLY="$1"; return }

    local s="${1//'%0C'/$'\f'}"
    s="${s//'%0D'/$'\r'}"
    s="${s//'%0A'/$'\n'}"
    s="${s//'%09'/$'\t'}"
    s="${s//'%20'/ }"
    s="${s//'%25'/%}"

    REPLY="$s"
}

_zsh_patina() {
    # start=$EPOCHREALTIME

    # remove tokens we have set earlier - do not clear the whole array as this
    # might reset syntax highlighting from other plugins (e.g. auto suggestions)
    region_highlight=( ${region_highlight:#*memo=zsh_patina} )

    local socket_path
    socket_path="$HOME/.local/share/zsh-patina/daemon.sock"

    if [[ ! -S "$socket_path" ]]; then
        # socket does not exist - daemon is not running
        return
    fi

    # Split pre-buffer into lines. In a multi-line input at the secondary
    # prompt, the pre-buffer contains the lines before the one the cursor is
    # currently in.
    local pre_count
    local -a pre_lines
    if [[ -n "$PREBUFFER" ]]; then
        pre_lines=("${(@f)PREBUFFER}")
        pre_count=${#pre_lines}
    else
        pre_lines=()
        pre_count=0
    fi

    # Split edit buffer into lines
    local count
    local -a lines
    if [[ -n "$BUFFER" ]]; then
        lines=("${(@f)BUFFER}")
        count=${#lines}
    else
        lines=()
        count=0
    fi

    if ! zsocket "$socket_path" 2>/dev/null; then
        # this is a real error that should not happen - so better print an error
        # message than being silent
        print -u2 "zsh-patina: failed to connect to socket at $socket_path"
        return
    fi
    local fd=$REPLY

    {
        # send header
        print -r -- "term_cols=$COLUMNS term_rows=$LINES cursor=$CURSOR pre_buffer_line_count=$pre_count buffer_line_count=$count pwd=$_ZSH_PATINA_ENCODED_PWD"

        # send pre-buffer lines
        if (( pre_count != 0 )); then
            print -r -- "$PREBUFFER"
        fi

        # send lines
        if (( count != 0 )); then
            print -r -- "$BUFFER"
        fi
    } >&$fd || {
        print -u2 "zsh-patina: Write to socket failed"
        exec {fd}>&-
        return
    }

    # Must be declared here because we reuse them in the while loop. Otherwise,
    # their contents will be printed in the second loop iteration (strange Zsh
    # behaviour).
    local entry range_start range_end ch

    local line
    while IFS= read -r -u $fd line; do
        [[ -z "$line" ]] && continue

        if [[ "$line" == "-DY"* ]]; then
            # Strip "-DY" prefix and split by whitespace
            local remainder="${line#-DY}"
            local args=(${(@s/ /)remainder})

            _zsh_patina_decode_string $args[1]
            local range_start=$REPLY
            _zsh_patina_decode_string $args[2]
            local range_end=$REPLY
            _zsh_patina_decode_string $args[3]
            local parsed_callable=$REPLY
            _zsh_patina_decode_string $args[4]
            local choices_raw=$REPLY

            # Parse choices_raw ("key:val;key:val;...") into associative array.
            # Split keys into individual characters.
            local -A choices=()
            for entry in "${(@s/;/)choices_raw}"; do
                local key="${entry%%:*}"
                local value="${entry#*:}"
                for ch in "${(@s::)key}"; do
                    choices[$ch]="$value"
                done
            done

            _zsh_patina_resolve_callable $parsed_callable

            if (( $+choices[$REPLY] )); then
                region_highlight+=("$range_start $range_end ${choices[$REPLY]} memo=zsh_patina")
            elif (( $+choices[e] )); then
                region_highlight+=("$range_start $range_end ${choices[e]} memo=zsh_patina")
            fi
        else
            region_highlight+=("$line memo=zsh_patina")
        fi
    done

    # close socket connection
    exec {fd}>&-

    # alternative but spawns an additional process (i.e. nc):
    # printf '%s\n' "$1" | nc -U "$sock" 2>/dev/null

    # end=$EPOCHREALTIME
    # elapsed_ms=$(( (end - start) * 1000 ))
    # printf "%.3f ms\n" $elapsed_ms
}

# store and update the current working directory in an encoded form
_zsh_patina_chpwd() {
    _zsh_patina_encode_string $PWD
    _ZSH_PATINA_ENCODED_PWD=$REPLY
}

if ! zmodload zsh/net/socket 2>/dev/null; then
    print -u2 "zsh-patina: failed to load zsh/net/socket module"
fi

autoload -U add-zle-hook-widget add-zsh-hook
add-zle-hook-widget line-pre-redraw _zsh_patina
add-zsh-hook chpwd _zsh_patina_chpwd

_zsh_patina_chpwd
