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
typeset -g _zsh_patina_last_sent=
typeset -ga _zsh_patina_applied_regions=()
typeset -g _zsh_patina_sent_buf=
typeset -g _zsh_patina_buf=
typeset -g _zsh_patina_prebuf=
typeset -gi _zsh_patina_cursor=0

_zsh_patina() {
    # Snapshot the zle buffer state here. These parameters are only valid in
    # widget context, so the response handler (_zsh_patina_async_response,
    # registered via `zle -F`) and _zsh_patina_send_request read these
    # snapshots instead of $BUFFER/$PREBUFFER/$CURSOR directly.
    _zsh_patina_buf=$BUFFER
    _zsh_patina_prebuf=$PREBUFFER
    _zsh_patina_cursor=$CURSOR

    # Re-entrancy guard: `zle -R` calls in the response handler redraw the
    # line and fire this hook again with an unchanged buffer text. The redraw
    # clears region_highlight, so re-inject the regions we already computed.
    # We only send a new request to the daemon when the buffer text changed.
    local cur_sent="${_zsh_patina_prebuf}|${_zsh_patina_buf}"
    if [[ "$cur_sent" == "$_zsh_patina_last_sent" ]]; then
        if (( ${#_zsh_patina_applied_regions[@]} > 0 )); then
            region_highlight=( "${region_highlight[@]:#*memo=zsh_patina}" "${_zsh_patina_applied_regions[@]}" )
            print "DBG $(date +%H:%M:%S.%N) REINJECT napp=${#_zsh_patina_applied_regions[@]} rh=${#region_highlight[@]}" >> /tmp/patina_dbg.log 2>/dev/null
        fi
        return
    fi

    # Clear the regions we stored, they are stale for the new buffer.
    _zsh_patina_applied_regions=()

    # Performance: Return immediately if there are bytes pending for input. This
    # can happen when pasting from the clipboard or when positioning the cursor
    # with Alt+Click/Option+Click, for example.
    if (( KEYS_QUEUED_COUNT > 0 )); then
            return
    fi

    # If a request is still in flight (the daemon is computing the highlight
    # for the previous buffer), do not tear its socket down. The daemon
    # answers within a few tens of milliseconds, which is longer than the
    # interval between two keystrokes, so cancelling on every keystroke would
    # discard every request before its response arrives. Instead, return
    # without updating _zsh_patina_last_sent; once the response arrives and
    # the fd is freed, the EOF handler clears _zsh_patina_last_sent so the
    # next hook sends a fresh request from widget context.
    if [[ -n "$_zsh_patina_fd" ]]; then
            return
    fi

    _zsh_patina_last_sent=$cur_sent
    _zsh_patina_send_request
}

# Send a request for the current buffer state to the daemon and register the
# socket with zle's event loop. Called from the line-pre-redraw hook. Reads the
# buffer from the snapshots set by _zsh_patina because $BUFFER/$PREBUFFER/
# $CURSOR are not valid in the zle -F callback context.
_zsh_patina_send_request() {
    # remove tokens we have set earlier - do not clear the whole array as this
    # might reset syntax highlighting from other plugins (e.g. auto suggestions)
    region_highlight=( "${region_highlight[@]:#*memo=zsh_patina}" )

    # return immediately if both pre-buffer and buffer are empty
    [[ -z "$_zsh_patina_prebuf" && -z "$_zsh_patina_buf" ]] && return

    local socket_path="<{zsh_patina_runtime_dir}>/daemon.sock"
    if [[ ! -S "$socket_path" ]]; then
        return
    fi

    # Trim pre-buffer (remove single trailing \n). `print -r` will add it again
    # later anyhow
    local trimmed_prebuffer="${_zsh_patina_prebuf%$'\n'}"

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
    if [[ -n "$_zsh_patina_buf" ]]; then
        count=$(( ${#${_zsh_patina_buf//[^$'\n']/}} + 1 ))
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
        local header="VER=<{version}>"$'\n'"COL=$COLUMNS"$'\n'"ROW=$LINES"$'\n'"CUR=$_zsh_patina_cursor"$'\n'"PRL=$pre_count"$'\n'"LNS=$lns"$'\n'"PWD=$_ZSH_PATINA_ENCODED_PWD"$'\n'

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
            print -r -- "$_zsh_patina_buf"
        fi
    } >&"$fd" || {
        print -u2 "zsh-patina: Write to socket failed"
        exec {fd}>&-
        return
    }

    # Hand the socket over to zle's event loop. The response phase runs in
    # _zsh_patina_async_response whenever the fd becomes readable. The handler
    # is registered with `zle -Fw` so it runs in widget context: $BUFFER and
    # friends are valid, `zle -R` from the handler fires line-pre-redraw (so
    # our re-inject guard and zsh-autocomplete's region_highlight snapshot both
    # see the regions we just applied), and registering another `zle -F` from
    # inside the handler is safe (this is exactly how zsh-autocomplete's own
    # async machinery works).
    _zsh_patina_fd=$fd
    _zsh_patina_request_gen=$_zsh_patina_generation
    _zsh_patina_sent_buf="${_zsh_patina_prebuf}|${_zsh_patina_buf}"
    _zsh_patina_buffer=
    _zsh_patina_new_regions=()
    _zsh_patina_state=regions
    zle -Fw "$fd" _zsh_patina_async_response

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
    print "DBG $(date +%H:%M:%S.%N) PL state=$_zsh_patina_state line=|$line|" >> /tmp/patina_dbg.log 2>/dev/null

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
                print "DBG $(date +%H:%M:%S.%N) REGION-ADD nreg=${#_zsh_patina_new_regions[@]}" >> /tmp/patina_dbg.log 2>/dev/null
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
                print "DBG $(date +%H:%M:%S.%N) CAL-ANS name=|$line| reply=|$REPLY|" >> /tmp/patina_dbg.log 2>/dev/null
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

    local chunk eof=0 line
    print "DBG $(date +%H:%M:%S.%N) CB fd=$fd err=$err sent=|${_zsh_patina_sent_buf}| buf=|${_zsh_patina_buf}|" >> /tmp/patina_dbg.log 2>/dev/null

    # Read and process the daemon's response in a single callback dispatch.
    #
    # The highlight protocol may require a CAL round-trip: the daemon sends a
    # callable-resolution query, blocks on read_line for the answer, then sends
    # the highlight regions and closes the connection. This splits the response
    # across two socket writes that can arrive in separate zle -F callbacks.
    #
    # Under zsh-autocomplete, zle does not reliably dispatch a second zle -F
    # callback during idle (between keystrokes). A readable but undelivered fd
    # then starves all subsequent line-pre-redraw hooks, freezing highlighting
    # and input. To avoid depending on a second dispatch, after processing the
    # available data (which answers any CAL query) we briefly block on the fd
    # with zselect so the daemon's remaining response arrives in this same
    # callback. The daemon answers in under a millisecond, so the wait almost
    # never reaches its timeout.
    while true; do
        # Read all data that is immediately available (non-blocking).
        while true; do
            if sysread -i "$fd" -s 65536 chunk 2>/dev/null; then
                _zsh_patina_buffer+=$chunk
                zselect -r "$fd" -t 0 2>/dev/null || break
            else
                eof=1
                break
            fi
        done
        (( eof )) && break

        # Process complete lines (this answers CAL queries).
        while [[ "$_zsh_patina_buffer" == *$'\n'* ]]; do
            line=${_zsh_patina_buffer%%$'\n'*}
            _zsh_patina_buffer=${_zsh_patina_buffer#*$'\n'}
            _zsh_patina_process_line "$fd" "$line"
        done

            zselect -r "$fd" -t 10 2>/dev/null || break
        done

    # process any remaining complete lines
    while [[ "$_zsh_patina_buffer" == *$'\n'* ]]; do
        line=${_zsh_patina_buffer%%$'\n'*}
        _zsh_patina_buffer=${_zsh_patina_buffer#*$'\n'}
        _zsh_patina_process_line "$fd" "$line"
    done

    # Apply highlighting after processing all currently available lines.
    # The generation check ensures we don't apply stale results.
    local _dbg_bm=no; [[ "${_zsh_patina_prebuf}|${_zsh_patina_buf}" == "$_zsh_patina_sent_buf" ]] && _dbg_bm=yes
    print "DBG $(date +%H:%M:%S.%N) APPLY-CHECK genok=$(( _zsh_patina_request_gen == _zsh_patina_generation )) bufmatch=$_dbg_bm nreg=${#_zsh_patina_new_regions[@]} rh=${#region_highlight[@]} sent=|$_zsh_patina_sent_buf| buf=|${_zsh_patina_buf}|" >> /tmp/patina_dbg.log 2>/dev/null
    if (( _zsh_patina_request_gen == _zsh_patina_generation )) && [[ "${_zsh_patina_prebuf}|${_zsh_patina_buf}" == "$_zsh_patina_sent_buf" ]] && (( ${#_zsh_patina_new_regions[@]} > 0 )); then
        print "DBG $(date +%H:%M:%S.%N) APPLY-FIRED nreg=${#_zsh_patina_new_regions[@]} rh_before=${#region_highlight[@]}" >> /tmp/patina_dbg.log 2>/dev/null
        _zsh_patina_applied_regions=( "${_zsh_patina_new_regions[@]}" )
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

        # If the buffer changed while this request was in flight, the response
        # was stale and its regions were not applied. Send a new request for
        # the current buffer. This handler runs in widget context (registered
        # via `zle -Fw`), so registering the new fd with `zle -Fw` here is
        # safe.
        if [[ "${_zsh_patina_prebuf}|${_zsh_patina_buf}" != "$_zsh_patina_sent_buf" ]]; then
            _zsh_patina_last_sent=
            _zsh_patina_send_request
        fi
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
# Widget wrapper around _zsh_patina_send_request. Called via `zle
# _zsh_patina_send_widget` from the EOF branch of _zsh_patina_async_response
# when the daemon's response was stale (the buffer changed while the request
# was in flight). Running the send through a widget is required because
# `zle WIDGET` executes in widget context, where `zle -F` registration is safe.
# Registering `zle -F` directly from a `zle -F` callback is NOT safe under
# zsh-autocomplete: the handler is never dispatched during idle and the
# readable-but-undelivered fd blocks the entire zle event loop, freezing input.
_zsh_patina_send_widget() {
    local cur_sent="${_zsh_patina_prebuf}|${_zsh_patina_buf}"
    # Do not re-send for the buffer that was just highlighted.
    [[ "$cur_sent" == "$_zsh_patina_last_sent" ]] && return
    _zsh_patina_last_sent=$cur_sent
    _zsh_patina_send_request
}

add-zle-hook-widget line-pre-redraw _zsh_patina
zle -N _zsh_patina_send_widget

# Add hook for the current working directory but don't call `_zsh_patina_chpwd`
# right now. We will lazily initialize _ZSH_PATINA_ENCODED_PWD later.
add-zsh-hook chpwd _zsh_patina_chpwd
