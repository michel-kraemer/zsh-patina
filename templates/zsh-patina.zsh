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
    # an existing PATH entry.  Fall back to a targeted PATH lookup for this one
    # word instead of refreshing the whole command hash with each keystroke.
    elif whence -p -- "$word" >/dev/null 2>&1; then
        REPLY=c
    else
        REPLY=m
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

# State for the asynchronous request/response cycle. The request phase
# (_zsh_patina, registered on line-pre-redraw) sends the buffer state to the
# daemon and returns immediately. The response phase
# (_zsh_patina_async_response, registered via `zle -F`) reads the daemon's
# reply whenever the socket becomes readable, so zle's event loop is never
# blocked by network I/O.
typeset -g _zsh_patina_fd=
typeset -gi _zsh_patina_generation=0
typeset -gi _zsh_patina_request_gen=0
typeset -g _zsh_patina_buffer=
typeset -ga _zsh_patina_new_regions=()
typeset -g _zsh_patina_state=regions
typeset -g _zsh_patina_query_cmd=
typeset -gi _zsh_patina_query_lns=0
typeset -gi _zsh_patina_query_las=1
typeset -gi _zsh_patina_query_i=0

_zsh_patina() {
    # start=$EPOCHREALTIME

    # Cancel any previous in-flight request: unregister its fd handler, close
    # the socket and invalidate its results by bumping the generation counter.
    if [[ -n "$_zsh_patina_fd" ]]; then
        zle -F "$_zsh_patina_fd" 2>/dev/null
        exec {_zsh_patina_fd}>&- 2>/dev/null
        _zsh_patina_fd=
        _zsh_patina_buffer=
    fi
    (( ++_zsh_patina_generation ))

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
        exec {fd}>&-
        return
    }

    # Hand the socket over to zle's event loop. The response phase runs in
    # _zsh_patina_async_response whenever the fd becomes readable.
    _zsh_patina_fd=$fd
    _zsh_patina_request_gen=$_zsh_patina_generation
    _zsh_patina_buffer=
    _zsh_patina_new_regions=()
    _zsh_patina_state=regions
    zle -F "$fd" _zsh_patina_async_response

    # end=$EPOCHREALTIME
    # elapsed_ms=$(( (end - start) * 1000 ))
    # zle -M $elapsed_ms
    # printf "%.3f ms\n" $elapsed_ms
}

# Process one complete protocol line received from the daemon. This is a state
# machine because data may arrive in arbitrarily sized chunks: a query block
# can span several invocations of the fd handler.
_zsh_patina_process_line() {
    local fd=$1
    local line=$2

    case $_zsh_patina_state in
        regions)
            if [[ -z "$line" ]]; then
                return
            elif [[ "$line" == "?"* ]]; then
                # query block: header fields follow until a blank line
                _zsh_patina_query_cmd="${line#?CMD=}"
                _zsh_patina_query_lns=0
                _zsh_patina_query_las=1
                _zsh_patina_state=query_header
            else
                _zsh_patina_new_regions+=("$line memo=zsh_patina")
            fi
            ;;
        query_header)
            if [[ -z "$line" ]]; then
                _zsh_patina_query_i=0
                if (( _zsh_patina_query_lns == 0 )); then
                    _zsh_patina_state=regions
                else
                    _zsh_patina_state=query_body
                fi
            elif [[ "$line" == "LNS="* ]]; then
                _zsh_patina_query_lns="${line#LNS=}"
            elif [[ "$line" == "LAS="* ]]; then
                _zsh_patina_query_las="${line#LAS=}"
            fi
            ;;
        query_body)
            if [[ "$_zsh_patina_query_cmd" == "CAL" ]]; then
                _zsh_patina_decode_string "$line"
                _zsh_patina_resolve_callable "$REPLY" "$_zsh_patina_query_las"
                print -r -u "$fd" -- "$REPLY"
            fi
            # unknown query types only get their body lines drained to keep the
            # socket in sync
            (( ++_zsh_patina_query_i ))
            if (( _zsh_patina_query_i >= _zsh_patina_query_lns )); then
                _zsh_patina_state=regions
            fi
            ;;
    esac
}

# fd handler invoked by zle's event loop (`zle -F`) when the daemon socket is
# readable. $1 is the fd, $2 an error condition (e.g. "hup" on EOF).
_zsh_patina_async_response() {
    local fd=$1
    local err=$2

    # discard results from a request that has been superseded by a newer one
    if (( _zsh_patina_request_gen != _zsh_patina_generation )); then
        zle -F "$fd" 2>/dev/null
        exec {fd}>&- 2>/dev/null
        [[ "$_zsh_patina_fd" == "$fd" ]] && _zsh_patina_fd=
        return 0
    fi

    # Read all data that is immediately available. sysread never blocks here:
    # zle only calls this handler when the fd is readable, and further reads
    # are guarded by a zero-timeout zselect poll.
    local chunk eof=0
    while true; do
        if sysread -i "$fd" -s 65536 chunk 2>/dev/null; then
            _zsh_patina_buffer+=$chunk
            zselect -r "$fd" -t 0 2>/dev/null || break
        else
            # EOF: the daemon closes the connection after the last line
            eof=1
            break
        fi
    done

    # process complete lines from the buffer, keeping any partial tail
    local line
    while [[ "$_zsh_patina_buffer" == *$'\n'* ]]; do
        line=${_zsh_patina_buffer%%$'\n'*}
        _zsh_patina_buffer=${_zsh_patina_buffer#*$'\n'}
        # Skip empty lines to avoid infinite loop on trailing newlines
        [[ -z "$line" ]] && continue
        _zsh_patina_process_line "$fd" "$line"
    done

    # Apply highlighting after processing all currently available lines.
    # The generation check ensures we don't apply stale results.
    if (( _zsh_patina_request_gen == _zsh_patina_generation )) && (( ${#_zsh_patina_new_regions[@]} > 0 )); then
        region_highlight=( "${region_highlight[@]:#*memo=zsh_patina}" "${_zsh_patina_new_regions[@]}" )
        zle -R
    fi

    if (( eof )); then
        # flush a trailing partial line, if any
        if [[ -n "$_zsh_patina_buffer" ]]; then
            _zsh_patina_process_line "$fd" "$_zsh_patina_buffer"
            _zsh_patina_buffer=
        fi

        # close socket connection and unregister the handler
        zle -F "$fd" 2>/dev/null
        exec {fd}>&- 2>/dev/null
        _zsh_patina_fd=
        _zsh_patina_buffer=
        _zsh_patina_new_regions=()
        _zsh_patina_state=regions
    fi

    return 0
}

# store and update the current working directory in an encoded form
_zsh_patina_chpwd() {
    _zsh_patina_encode_string "$PWD"
    _ZSH_PATINA_ENCODED_PWD=$REPLY
}

if ! zmodload zsh/net/socket 2>/dev/null; then
    print -u2 "zsh-patina: failed to load zsh/net/socket module"
fi

# needed for the asynchronous response handler: sysread (zsh/system) and
# zero-timeout polling of the socket (zsh/zselect)
if ! zmodload zsh/system 2>/dev/null; then
    print -u2 "zsh-patina: failed to load zsh/system module"
fi
if ! zmodload zsh/zselect 2>/dev/null; then
    print -u2 "zsh-patina: failed to load zsh/zselect module"
fi

autoload -U add-zle-hook-widget add-zsh-hook
add-zle-hook-widget line-pre-redraw _zsh_patina

# Add hook for the current working directory but don't call `_zsh_patina_chpwd`
# right now. We will lazily initialize _ZSH_PATINA_ENCODED_PWD later.
add-zsh-hook chpwd _zsh_patina_chpwd
