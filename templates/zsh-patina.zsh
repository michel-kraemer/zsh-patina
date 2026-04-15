# WARNING: Do not cache or source this file manually. Its contents are generated
# automatically when zsh-patina is started via the `activate` command. To set up
# zsh-patina, add the following to your .zshrc:
#
#   eval "$(zsh-patina activate)"
#
# For more details, see the README:
# https://github.com/michel-kraemer/zsh-patina#how-to-install

# this variable needs to be exported so `zsh-patina check` can find it
export _ZSH_PATINA_PATH="<{zsh_patina_path}>"

zsh-patina() {
    "$_ZSH_PATINA_PATH" "$@"
}

_zsh_patina_resolve_callable() {
    local word=$1
    local -A seen=()

    local IFS=' '$'\t'$'\n' # make sure `read` splits at whitespace

    # resolve alias to real command
    while (( $+aliases[(e)$word] || $+galiases[(e)$word] )); do
        # check for cycles
        (( $+seen[$word] )) && break

        seen[$word]=1

        # extract the first term (n.b. don't use `read -r` as it would not
        # resolve escaped characters)
        if (( $+aliases[(e)$word] )); then
            read word _ <<< "$aliases[$word]"
        else
            read word _ <<< "$galiases[$word]"
        fi
    done

    if (( ${#seen} )); then
        # We've resolved an alias. Check whether it is a function, builtin, or
        # command. Otherwise, check if it is executable (an executable file or
        # directory). This is consistent with how the daemon checks for existing
        # callables. Paths without slashes don't need to be checked as they
        # should already be contained in $commands)
        if (( $+functions[(e)$word] || $+builtins[(e)$word] || $+commands[(e)$word] )) ||
            [[ "$word" == */* && -x "$word" ]]; then
            REPLY=a
        else
            REPLY=m
        fi
    elif (( $+functions[(e)$word] )); then
        REPLY=f
    elif (( $+builtins[(e)$word] )); then
        REPLY=b
    elif (( $+commands[(e)$word] )); then
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

# Define a _zsh_highlight plugin for compatibility with other plugins that look
# for a syntax highlighter. See https://github.com/michel-kraemer/zsh-patina/issues/10
# for example.
_zsh_highlight() {
    _zsh_patina
}

_zsh_patina() {
    # start=$EPOCHREALTIME

    # remove tokens we have set earlier - do not clear the whole array as this
    # might reset syntax highlighting from other plugins (e.g. auto suggestions)
    region_highlight=( ${region_highlight:#*memo=zsh_patina} )

    # return immediately if both pre-buffer and buffer are empty
    [[ -z "$PREBUFFER" && -z "$BUFFER" ]] && return

    local socket_path="<{zsh_patina_runtime_dir}>/daemon.sock"
    if [[ ! -S "$socket_path" ]]; then
        # socket does not exist - daemon is not running
        return
    fi

    # Count lines in pre-buffer. In a multi-line input at the secondary prompt,
    # the pre-buffer contains the lines before the one the cursor is currently
    # in.
    local pre_count=0
    if [[ -n "$PREBUFFER" ]]; then
        # remove every character instead of '\n' and then get string length
        pre_count=$(( ${#${PREBUFFER//[^$'\n']/}} + 1 ))
    fi

    # Count lines in buffer
    local count=0
    if [[ -n "$BUFFER" ]]; then
        count=$(( ${#${BUFFER//[^$'\n']/}} + 1 ))
    fi

    if ! zsocket "$socket_path" 2>/dev/null; then
        # this is a real error that should not happen - so better print an error
        # message than being silent
        zle -M "zsh-patina: failed to connect to socket at $socket_path. Please restart your shell and/or the zsh-patina daemon with 'zsh-patina restart'."
        return
    fi
    local fd=$REPLY

    if [[ -z "$_ZSH_PATINA_ENCODED_PWD" ]]; then
        # Lazily set _ZSH_PATINA_ENCODED_PWD if it's empty. Doing this here
        # rather than right at activation, makes sure we get the actual
        # directory the user has started in and not the one from which
        # `zsh-patina activate` was called.
        _zsh_patina_encode_string $PWD
        _ZSH_PATINA_ENCODED_PWD=$REPLY
    fi

    {
        # build header
        local header="ver=<{version}> term_cols=$COLUMNS term_rows=$LINES cursor=$CURSOR pre_buffer_line_count=$pre_count buffer_line_count=$count pwd=$_ZSH_PATINA_ENCODED_PWD"

        if (( $+REGION_ACTIVE )) && (( REGION_ACTIVE != 0 )); then
            _zsh_patina_encode_string "${${zle_highlight[(r)region:*]-}#*:}"
            header="${header} region_active=$REGION_ACTIVE mark=$MARK zle_highlight_region=$REPLY"
        fi
        if (( $+SUFFIX_ACTIVE )) && (( SUFFIX_ACTIVE != 0 )); then
            _zsh_patina_encode_string "${${zle_highlight[(r)suffix:*]-}#*:}"
            header="${header} suffix_active=$SUFFIX_ACTIVE suffix_start=$SUFFIX_START suffix_end=$SUFFIX_END zle_highlight_suffix=$REPLY"
        fi
        if (( $+ISEARCHMATCH_ACTIVE )) && (( ISEARCHMATCH_ACTIVE != 0 )); then
            _zsh_patina_encode_string "${${zle_highlight[(r)isearch:*]-}#*:}"
            header="${header} isearch_active=$ISEARCHMATCH_ACTIVE isearch_start=$ISEARCHMATCH_START isearch_end=$ISEARCHMATCH_END zle_highlight_isearch=$REPLY"
        fi
        if (( $+YANK_ACTIVE )) && (( YANK_ACTIVE != 0 )); then
            _zsh_patina_encode_string "${${zle_highlight[(r)paste:*]-}#*:}"
            header="${header} yank_active=$YANK_ACTIVE yank_start=$YANK_START yank_end=$YANK_END zle_highlight_paste=$REPLY"
        fi

        # send header
        print -r -- $header

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

    # declaring all other variables outside the while loop (outside the hot
    # path), too, slightly increases performance
    local -A choices
    local parsed_callable args choices_raw remainder key value

    local new_regions=("${region_highlight[@]}") # preserve existing highlighting
    local line
    while IFS= read -r -u $fd line; do
        [[ -z "$line" ]] && continue

        if [[ "$line" == "-DY"* ]]; then
            # Strip "-DY" prefix and split by whitespace
            remainder="${line#-DY}"
            args=(${(@s/ /)remainder})

            range_start=$args[1]
            range_end=$args[2]
            _zsh_patina_decode_string $args[3]
            parsed_callable=$REPLY
            _zsh_patina_decode_string $args[4]
            choices_raw=$REPLY

            # Parse choices_raw ("key:val;key:val;...") into associative array.
            # Split keys into individual characters.
            choices=()
            for entry in "${(@s/;/)choices_raw}"; do
                key="${entry%%:*}"
                value="${entry#*:}"
                for ch in "${(@s::)key}"; do
                    choices[$ch]="$value"
                done
            done

            _zsh_patina_resolve_callable $parsed_callable

            if (( $+choices[$REPLY] )); then
                new_regions+=("$range_start $range_end ${choices[$REPLY]} memo=zsh_patina")
            elif (( $+choices[e] )); then
                new_regions+=("$range_start $range_end ${choices[e]} memo=zsh_patina")
            fi
        else
            new_regions+=("$line memo=zsh_patina")
        fi
    done

    # performance: set region_highlight once at the end rather than updating it
    # for every region
    region_highlight=("${new_regions[@]}")

    # close socket connection
    exec {fd}>&-

    # end=$EPOCHREALTIME
    # elapsed_ms=$(( (end - start) * 1000 ))
    # zle -M $elapsed_ms
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

# Add hook for the current working directory but don't call `_zsh_patina_chpwd`
# right now. We will lazily initialize _ZSH_PATINA_ENCODED_PWD later.
add-zsh-hook chpwd _zsh_patina_chpwd
