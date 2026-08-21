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
    # handle `highlight` subcommand
    if [[ $1 = "highlight" ]]; then
        _zsh_patina_highlight_subcommand "${@:2}" || return 1
    else
        "$_ZSH_PATINA_PATH" "$@"
    fi
}

_zsh_patina_highlight_subcommand() {
    local -a positional=()
    local has_option=0
    local seen_dashdash=0
    local input_file
    local contents

    for a in "$@"; do
        if (( seen_dashdash )); then
            # everything after `--` is a positional argument
            positional+=("$a")
        elif [[ "$a" == "--" ]]; then
            seen_dashdash=1
        elif [[ "$a" == -* ]]; then
            has_option=1
        else
            positional+=("$a")
        fi
    done

    if (( has_option || ${#positional[@]} > 1 )); then
        "$_ZSH_PATINA_PATH" highlight "$@" || return 1
    else
        # read from file or from stdin
        if (( ${#positional[@]} == 1 )); then
            input_file="${positional[1]}"
            if ! { [[ -r "$input_file" ]] && [[ -f "$input_file" ]] && contents=$(<"$input_file") }; then
                print -u2 -- "\e[31;1mzsh-patina:\e[0m Failed to read file: '$input_file'"
                return 1
            fi
        else
            contents=$(cat)
        fi

        # perform the highlighting
        _zsh_patina_highlight_string "$contents"
        _zsh_patina_highlighting_to_formatted_string "$contents" "${reply[@]}"
        print -rP -- "$REPLY"
    fi
}

# Takes a command line as first argument and creates highlighting instructions
# for `$region_highlight` within the current shell session. Assumes that the
# cursor is at the end of the command line.
#
# Example:
#
#     _zsh_patina_highlight_string "echo hello"
#
# This sets `$reply` to:
#
#     ("0 4 fg=cyan memo=zsh_patina")
#
# This function is part of the `highlight` subcommand
_zsh_patina_highlight_string() {
    emulate -L zsh

    local -ah region_highlight=()
    local -h PREBUFFER=""
    local -h BUFFER="$1"
    local -h CURSOR=${#BUFFER}

    # pretend our terminal is really large
    local -h COLUMNS=2147483647
    local -h LINES=2147483647

    _zsh_patina

    reply=("${region_highlight[@]}")
}

# escape a string so it can be printed verbatim using `print -P`
_zsh_patina_escape_highlighting_segment() {
    local input="$1"
    input="${input//\\/\\\\}" # \  -> \\
    input="${input//\`/\\\`}" # `  -> \`
    input="${input//\$/\\\$}" # $  -> \$
    input="${input//\%/%%}"   # %  -> %%
    REPLY="$input"
}

# Takes a command line as first argument and highlighting instructions from
# `$region_highlight` as subsequent arguments. Escapes the command line and then
# formats it using prompt expansion sequences.
#
# Example:
#
#     _zsh_patina_highlighting_to_formatted_string "echo hello" "0 4 fg=cyan"
#
# This sets `$REPLY` to:
#
#     %F{cyan}echo%f hello
#
# Note: You may print the stylized command line with `print -rP`:
#
#     print -rP -- $REPLY
#
# This function is part of the `highlight` subcommand
_zsh_patina_highlighting_to_formatted_string() {
    emulate -L zsh

    local -a parts subparts
    local token start end result style fgset bgset boldset underlineset
    local last=1

    local input="$1"
    (( $# > 0 )) && shift

    for style in "$@"; do
        parts=("${(s: :)style}")
        (( ${#parts} > 1 )) || continue

        start=$(( parts[1] + 1 ))
        end=$(( parts[2] ))

        # append any unformatted substring
        if (( $start > $last )); then
            _zsh_patina_escape_highlighting_segment "${input[$last,$start - 1]}"
            result+="$REPLY"
        fi
        last=$(( end + 1 ))

        # generate formatting sequences
        fgset=0
        bgset=0
        boldset=0
        underlineset=0
        for token in "${parts[@]}"; do
            case "$token" in
                fg=*)
                    subparts=("${(s:=:)token}")
                    result+="%F{$subparts[2]}"
                    fgset=1
                    ;;

                bg=*)
                    subparts=("${(s:=:)token}")
                    result+="%K{$subparts[2]}"
                    bgset=1
                    ;;

                bold)
                    result+="%B"
                    boldset=1
                    ;;

                underline)
                    result+="%U"
                    underlineset=1
                    ;;
            esac
        done

        # append formatted substrings and reset formatting
        _zsh_patina_escape_highlighting_segment "${input[$start,$end]}"
        result+="$REPLY"
        (( $underlineset )) && result+="%u"
        (( $boldset )) && result+="%b"
        (( $bgset )) && result+="%k"
        (( $fgset )) && result+="%f"
    done

    # append unformatted remainder
    if (( ${#input} >= $last )); then
        _zsh_patina_escape_highlighting_segment "${input[$last,${#input}]}"
        result+="$REPLY"
    fi

    REPLY="$result"
}

_zsh_patina_resolve_callable() {
    local word=$1
    local lookup_aliases=${2:-1}

    if (( lookup_aliases )) && (( $+aliases[(e)$word] )); then
        _zsh_patina_encode_string "$aliases[$word]"
        REPLY="a$REPLY"
    elif (( lookup_aliases )) && (( $+galiases[(e)$word] )); then
        _zsh_patina_encode_string "$galiases[$word]"
        REPLY="a$REPLY"
    elif (( $+functions[(e)$word] )); then
        REPLY=f
    elif (( $+builtins[(e)$word] )); then
        REPLY=b
    elif (( $+commands[(e)$word] )); then
        REPLY=c
    # $commands may stay stale until rehash after a new executable appears in
    # an existing PATH entry. Fall back to a targeted PATH lookup for this one
    # word instead of refreshing the whole command hash with each keystroke.
    elif whence -p -- "$word" >/dev/null 2>&1; then
        REPLY=c
    else
        REPLY=m
    fi
}

_zsh_patina_resolve_nameddir() {
    REPLY=
    if [[ -n "$1" && "$1" != *[^[:alnum:]_.-]* ]]; then
        eval "print -v REPLY -r -- ~${1}" 2>/dev/null
    fi
}

_zsh_patina_encode_string() {
    # fast path
    [[ $1 != *[%$'\n']* ]] && { REPLY="$1"; return }

    # encode % first so the % in %0A doesn't get double-encoded
    local s="${1//'%'/%25}"
    s="${s//$'\n'/%0A}"

    REPLY="$s"
}

_zsh_patina_decode_string() {
    # fast path
    [[ $1 != *%* ]] && { REPLY="$1"; return }

    # decode %0A first so that %250A becomes %0A (literal), not a newline
    local s="${1//'%0A'/$'\n'}"
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

    # Performance: Return immediately if there are bytes pending for input. This
    # can happen when pasting from the clipboard or when positioning the cursor
    # with Alt+Click/Option+Click, for example.
    (( KEYS_QUEUED_COUNT > 0 )) && return
    (( PENDING > 0 )) && return

    # remove tokens we have set earlier - do not clear the whole array as this
    # might reset syntax highlighting from other plugins (e.g. auto suggestions)
    region_highlight=( "${region_highlight[@]:#*memo=zsh_patina}" )

    # return immediately if both pre-buffer and buffer are empty
    [[ -z "$PREBUFFER" && -z "$BUFFER" ]] && return

    local socket_path="<{zsh_patina_runtime_dir}>/daemon.sock"
    if [[ ! -S "$socket_path" ]]; then
        # socket does not exist - daemon is not running
        return
    fi

    # Trim pre-buffer (remove single trailing \n). `print -r` will add it again
    # later anyhow
    local trimmed_prebuffer="${PREBUFFER%$'\n'}"

    # Count lines in pre-buffer. In a multi-line input at the secondary prompt,
    # the pre-buffer contains the lines before the one the cursor is currently
    # in.
    local pre_count=0
    if [[ -n "$trimmed_prebuffer" ]]; then
        # remove every character instead of '\n' and then get string length
        pre_count=$(( ${#${trimmed_prebuffer//[^$'\n']/}} + 1 ))
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

    {
        if [[ -z "$_ZSH_PATINA_ENCODED_PWD" ]]; then
            # Lazily set _ZSH_PATINA_ENCODED_PWD if it's empty. Doing this here
            # rather than right at activation, makes sure we get the actual
            # directory the user has started in and not the one from which
            # `zsh-patina activate` was called.
            _zsh_patina_encode_string "$PWD"
            _ZSH_PATINA_ENCODED_PWD=$REPLY
        fi

        {
            # build header
            local lns=$(( $pre_count + $count ))
            local header="VER=<{version}>"$'\n'"COL=$COLUMNS"$'\n'"ROW=$LINES"$'\n'"CUR=$CURSOR"$'\n'"PRL=$pre_count"$'\n'"LNS=$lns"$'\n'"PWD=$_ZSH_PATINA_ENCODED_PWD"$'\n'

            if (( $+REGION_ACTIVE )) && (( REGION_ACTIVE != 0 )); then
                _zsh_patina_encode_string "${${zle_highlight[(r)region:*]-}#*:}"
                header="${header}RGA=1"$'\n'"RGE=$MARK"$'\n'"RGH=$REPLY"$'\n'
            fi
            if (( $+SUFFIX_ACTIVE )) && (( SUFFIX_ACTIVE != 0 )); then
                _zsh_patina_encode_string "${${zle_highlight[(r)suffix:*]-}#*:}"
                header="${header}SFA=1"$'\n'"SFS=$SUFFIX_START"$'\n'"SFE=$SUFFIX_END"$'\n'"SFH=$REPLY"$'\n'
            fi
            if (( $+ISEARCHMATCH_ACTIVE )) && (( ISEARCHMATCH_ACTIVE != 0 )); then
                _zsh_patina_encode_string "${${zle_highlight[(r)isearch:*]-}#*:}"
                header="${header}ISA=1"$'\n'"ISS=$ISEARCHMATCH_START"$'\n'"ISE=$ISEARCHMATCH_END"$'\n'"ISH=$REPLY"$'\n'
            fi
            if (( $+YANK_ACTIVE )) && (( YANK_ACTIVE != 0 )); then
                _zsh_patina_encode_string "${${zle_highlight[(r)paste:*]-}#*:}"
                header="${header}YKA=1"$'\n'"YKS=$YANK_START"$'\n'"YKE=$YANK_END"$'\n'"YKH=$REPLY"$'\n'
            fi

            if [[ -o autocd ]]; then
                header="${header}ACD=1"$'\n'
            fi
            if [[ ! -o banghist ]]; then
                header="${header}BNG=0"$'\n'
            fi

            # send header
            print -r -- "$header"

            # send pre-buffer lines
            if (( pre_count != 0 )); then
                print -r -- "$trimmed_prebuffer"
            fi

            # send lines
            if (( count != 0 )); then
                print -r -- "$BUFFER"
            fi
        } >&"$fd" || {
            print -u2 "zsh-patina: Write to socket failed"
            return
        }

        # Must be declared here because we reuse them in the while loop.
        # Otherwise, their contents will be printed in the second loop iteration
        # (strange Zsh behaviour). As a matter of fact, declaring all variables
        # outside the while loop (outside the hot path), slightly increases
        # performance.
        local query_cmd query_lns query_las qline qi

        local new_regions=("${region_highlight[@]}") # preserve existing highlighting
        local line
        while IFS= read -r -u "$fd" line; do
            [[ -z "$line" ]] && continue

            if [[ "$line" == "?"* ]]; then
                # query block: read header fields until blank line
                query_cmd="${line#?CMD=}"
                query_lns=0
                query_las=1
                while IFS= read -r -u "$fd" qline; do
                    [[ -z "$qline" ]] && break
                    if [[ "$qline" == "LNS="* ]]; then
                        query_lns="${qline#LNS=}"
                    elif [[ "$qline" == "LAS="* ]]; then
                        query_las="${qline#LAS=}"
                    fi
                done

                if [[ "$query_cmd" == "CAL" ]]; then
                    for (( qi = 0; qi < query_lns; qi++ )); do
                        IFS= read -r -u "$fd" qline
                        _zsh_patina_decode_string "$qline"
                        _zsh_patina_resolve_callable "$REPLY" "$query_las"
                        print -r -u "$fd" -- "$REPLY"
                    done
                elif [[ "$query_cmd" == "NMD" ]]; then
                    for (( qi = 0; qi < query_lns; qi++ )); do
                        IFS= read -r -u "$fd" qline
                        _zsh_patina_decode_string "$qline"
                        _zsh_patina_resolve_nameddir "$REPLY"
                        _zsh_patina_encode_string "$REPLY"
                        print -r -u "$fd" -- "$REPLY"
                    done
                else
                    # unknown query type: drain body lines to keep socket in
                    # sync
                    for (( qi = 0; qi < query_lns; qi++ )); do
                        IFS= read -r -u "$fd" qline
                    done
                fi
            else
                new_regions+=("$line memo=zsh_patina")
            fi
        done

        # performance: set region_highlight once at the end rather than updating
        # it for every region
        region_highlight=("${new_regions[@]}")
    } always {
        # close socket connection
        exec {fd}>&-
    }

    # end=$EPOCHREALTIME
    # elapsed_ms=$(( (end - start) * 1000 ))
    # zle -M $elapsed_ms
    # printf "%.3f ms\n" $elapsed_ms
}

# store and update the current working directory in an encoded form
_zsh_patina_chpwd() {
    _zsh_patina_encode_string "$PWD"
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
