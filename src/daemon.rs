use anyhow::{Context, Result, bail};
use askama::Template;
use rayon::ThreadPoolBuilder;
use rustc_hash::{FxHashMap, FxHashSet};
use std::{
    borrow::Cow,
    fs::{self, File, OpenOptions, Permissions, TryLockError},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write, stdout},
    os::{
        fd::AsRawFd,
        unix::{
            fs::PermissionsExt,
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    process,
    sync::Arc,
    thread,
    time::Duration,
};

use crate::{
    commands::check_config,
    config::Config,
    highlighting::{
        CallableType, DynamicStyle, Highlighter, HighlighterBuilder, HighlightingRequest, Span,
        SpanStyle, StaticStyle,
    },
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Parent,
    Child,
    Daemon,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Command {
    Hello,
    Highlight,
}

#[derive(Template)]
#[template(path = "zsh-patina.zsh")]
struct ActivateTemplate {
    zsh_patina_path: String,
    zsh_patina_runtime_dir: String,
    version: &'static str,
}

#[deprecated = "This function is only needed for backwards compatibility. It will be removed in a future release."]
fn pid_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("daemon.pid")
}

fn lock_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("daemon.lock")
}

fn sock_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("daemon.sock")
}

/// Read the PID from the PID file. Returns `None` if the file does not exist or
/// contains garbage.
fn read_pid_legacy(pid_file: &Path) -> Option<u32> {
    fs::read_to_string(pid_file).ok()?.trim().parse().ok()
}

// Total amount of time we are willing to wait for another daemon process to
// finish a short-lived initialization step. This is only relevant when multiple
// shells start around the same time; the startup race is prevented by the lock
// file, but the winner may not have completed these steps yet.
const LOCK_WAIT_TIMEOUT: Duration = Duration::from_millis(1000);

// Polling interval used while waiting for the daemon to finish initialization.
const LOCK_WAIT_INTERVAL: Duration = Duration::from_millis(50);

fn read_pid(pid_file: &mut File) -> Result<u32> {
    let deadline = std::time::Instant::now() + LOCK_WAIT_TIMEOUT;
    loop {
        pid_file.seek(SeekFrom::Start(0))?;
        let mut pid = String::new();
        pid_file.read_to_string(&mut pid)?;
        let pid = pid.trim();
        if !pid.is_empty() {
            return pid.parse().context("Could not parse PID from lock file");
        }
        if std::time::Instant::now() >= deadline {
            bail!("Daemon is running but current PID could not be read");
        }
        thread::sleep(LOCK_WAIT_INTERVAL);
    }
}

/// Connect to the daemon's Unix domain socket, retrying if the socket does not
/// exist yet. This handles the short window after a new daemon has acquired the
/// startup lock but has not finished binding the socket.
fn connect_with_retry(socket_path: &Path) -> Result<UnixStream> {
    let deadline = std::time::Instant::now() + LOCK_WAIT_TIMEOUT;
    loop {
        match UnixStream::connect(socket_path) {
            Ok(stream) => return Ok(stream),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if std::time::Instant::now() >= deadline {
                    return Err(e.into());
                }
                thread::sleep(LOCK_WAIT_INTERVAL);
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// Wait until the exclusive lock on `lock_file` can be acquired or the timeout
/// expires. Returns an error if the lock is still held when the timeout is
/// reached.
fn wait_for_lock_release(lock_file: &mut File) -> Result<()> {
    let deadline = std::time::Instant::now() + LOCK_WAIT_TIMEOUT;
    loop {
        match lock_file.try_lock() {
            Ok(()) => return Ok(()),
            Err(TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    bail!("Daemon did not release the lock file in time");
                }
                thread::sleep(LOCK_WAIT_INTERVAL);
            }
            Err(TryLockError::Error(e)) => return Err(e.into()),
        }
    }
}

/// Check whether a process with the given PID is currently alive.
#[deprecated = "This function is only needed for backwards compatibility. It will be removed in a future release."]
fn pid_alive(pid: u32) -> bool {
    // SAFETY: This is safe because we're only passing a valid PID and a signal
    // of 0, which does not actually send a signal. kill(pid, 0) returns 0 if
    // the process exists and we have permission to signal it.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Convert a static style to the format that Zsh's `region_highlight` uses
fn format_static_style(style: &StaticStyle) -> String {
    let mut result = String::new();
    if let Some(fg) = &style.foreground_color {
        result.push_str("fg=");
        result.push_str(fg);
    }
    if let Some(bg) = &style.background_color {
        if !result.is_empty() {
            result.push(',');
        }
        result.push_str("bg=");
        result.push_str(bg);
    }
    if style.bold {
        if !result.is_empty() {
            result.push(',');
        }
        result.push_str("bold");
    }
    if style.underline {
        if !result.is_empty() {
            result.push(',');
        }
        result.push_str("underline");
    }
    result
}

/// Decode a path that was encoded by our Zsh script with percent-encoding for
/// ASCII whitespace characters
#[deprecated = "Protocol version 1 will be removed in one of the next releases"]
fn decode_string_v1(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let decoded = match &bytes[i + 1..i + 3] {
                // the same characters are used by Rust's is_ascii_whitespace()
                b"20" => Some(' '),
                b"09" => Some('\t'),
                b"0A" => Some('\n'),
                b"0D" => Some('\r'),
                b"0C" => Some('\x0C'),
                b"25" => Some('%'),
                _ => None,
            };
            if let Some(c) = decoded {
                out.push(c);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

#[deprecated = "Protocol version 1 will be removed in one of the next releases"]
fn encode_string_v1(input: String) -> String {
    // Fast path: no encoding needed
    if !input
        .bytes()
        .any(|b| matches!(b, b'%' | b' ' | b'\t' | b'\n' | b'\r' | b'\x0C'))
    {
        return input;
    }

    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'%' => out.push_str("%25"),
            b' ' => out.push_str("%20"),
            b'\t' => out.push_str("%09"),
            b'\n' => out.push_str("%0A"),
            b'\r' => out.push_str("%0D"),
            b'\x0C' => out.push_str("%0C"),
            // Safe to cast: all encoded chars are ASCII, and multi-byte UTF-8
            // sequences (bytes >= 0x80) pass through unchanged, so valid UTF-8
            // in means valid UTF-8 out.
            _ => out.push(b as char),
        }
    }
    out
}

fn decode_string(s: &str) -> String {
    if !s.bytes().any(|b| b == b'%') {
        // fast path: nothing to decode
        return s.to_string();
    }

    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            match &bytes[i + 1..i + 3] {
                b"0A" => {
                    out.push('\n');
                    i += 3;
                    continue;
                }
                b"25" => {
                    out.push('%');
                    i += 3;
                    continue;
                }
                _ => {
                    // unknown %XX: pass through as literal text
                    out.push('%');
                    out.push(bytes[i + 1] as char);
                    out.push(bytes[i + 2] as char);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn encode_string(input: &str) -> Cow<'_, str> {
    if !input.bytes().any(|b| matches!(b, b'%' | b'\n')) {
        // fast path: nothing to encode
        return Cow::from(input);
    }

    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'%' => out.push_str("%25"),
            b'\n' => out.push_str("%0A"),
            _ => out.push(b as char),
        }
    }
    Cow::from(out)
}

/// Add a region with a Zsh `zle_highlight` style if the region is active. The
/// region is defined by `start` and `end`, and the style is defined by
/// `zle_highlight` (e.g. `underline`). If the region is active but
/// `zle_highlight` is empty, the `default_value` will be used.
fn add_zle_highlight<W: Write>(
    active: Option<bool>,
    start: Option<usize>,
    end: Option<usize>,
    zle_highlight: Option<String>,
    default_value: &str,
    writer: &mut W,
) -> Result<()> {
    if let Some(active) = active
        && active
        && let Some(start) = start
        && let Some(end) = end
    {
        let style = zle_highlight
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| default_value.to_string());
        let (from, to) = if start < end {
            (start, end)
        } else {
            (end, start)
        };
        writer
            .write_all(format!("{from} {to} {style}\n").as_bytes())
            .context("Unable to send response")?;
    }
    Ok(())
}

fn handle_connection(stream: UnixStream, highlighter: Arc<Highlighter>) -> Result<()> {
    // clone the stream so we can read and write simultaneously
    let writer = stream
        .try_clone()
        .context("Unable to clone socket for writing")?;
    let mut reader = BufReader::new(stream);

    // read protocol version (or header if protocol is v1)
    let mut first_line = String::new();
    reader
        .read_line(&mut first_line)
        .context("Unable to read header")?;
    if first_line.ends_with('\n') {
        first_line.pop();
    }

    let version = first_line.strip_prefix("VER=").unwrap_or("1");
    match version {
        "3" => handle_connection_v2(reader, writer, &highlighter, true),
        "2" => handle_connection_v2(reader, writer, &highlighter, false),
        "1" => handle_connection_v1(reader, writer, first_line, highlighter),
        _ => {
            // Return immediately. This will close the connection with an empty
            // response.
            log::error!(
                "Client protocol version is {version:?}. Expected protocol \
                version is \"1\", \"2\", or \"3\"."
            );
            Ok(())
        }
    }
}

#[deprecated = "Protocol version 1 will be removed in one of the next releases"]
#[allow(deprecated)]
fn handle_connection_v1<R: BufRead, W: Write>(
    mut reader: R,
    mut writer: W,
    header: String,
    highlighter: Arc<Highlighter>,
) -> Result<()> {
    let mut client_version = None;

    let mut term_cols = 1000;
    let mut term_rows = 1000;
    let mut cursor = 0;

    let mut pre_buffer_line_count = 0;
    let mut buffer_line_count = 0;

    let mut pwd = None;
    let mut cmd = None;
    let mut history_expansions_enabled = true;

    let mut region_active = None;
    let mut mark = None;
    let mut zle_highlight_region = None;

    let mut suffix_active = None;
    let mut suffix_start = None;
    let mut suffix_end = None;
    let mut zle_highlight_suffix = None;

    let mut isearch_active = None;
    let mut isearch_start = None;
    let mut isearch_end = None;
    let mut zle_highlight_isearch = None;

    let mut yank_active = None;
    let mut yank_start = None;
    let mut yank_end = None;
    let mut zle_highlight_paste = None;

    log::trace!("Received header: {}", header.trim_ascii_end());

    for h in header.split_ascii_whitespace() {
        let (key, value) = h
            .split_once("=")
            .context("Unable to split header key-value pair")?;
        match key {
            "ver" => client_version = Some(value),

            "term_cols" => {
                term_cols = value
                    .parse::<usize>()
                    .context("Unable to parse number of terminal columns")?;
            }
            "term_rows" => {
                term_rows = value
                    .parse::<usize>()
                    .context("Unable to parse number of terminal rows")?;
            }
            "cursor" => {
                cursor = value
                    .parse::<usize>()
                    .context("Unable to parse cursor position")?;
            }

            "pre_buffer_line_count" => {
                pre_buffer_line_count = value
                    .parse::<usize>()
                    .context("Unable to parse number of lines in pre-buffer")?;
            }
            "buffer_line_count" => {
                buffer_line_count = value
                    .parse::<usize>()
                    .context("Unable to parse number of lines in buffer")?;
            }

            "pwd" => pwd = Some(decode_string_v1(value)),
            "cmd" => cmd = Some(value),
            "banghist" => {
                history_expansions_enabled = value
                    .parse::<u8>()
                    .context("Unable to parse banghist option")?
                    > 0;
            }

            "region_active" => region_active = Some(value),
            "mark" => {
                mark = Some(
                    value
                        .parse::<usize>()
                        .context("Unable to parse mark position")?,
                );
            }
            "zle_highlight_region" => zle_highlight_region = Some(decode_string_v1(value)),

            "suffix_active" => suffix_active = Some(value),
            "suffix_start" => {
                suffix_start = Some(
                    value
                        .parse::<usize>()
                        .context("Unable to parse suffix start position")?,
                );
            }
            "suffix_end" => {
                suffix_end = Some(
                    value
                        .parse::<usize>()
                        .context("Unable to parse suffix end position")?,
                );
            }
            "zle_highlight_suffix" => zle_highlight_suffix = Some(decode_string_v1(value)),

            "isearch_active" => isearch_active = Some(value),
            "isearch_start" => {
                isearch_start = Some(
                    value
                        .parse::<usize>()
                        .context("Unable to parse isearch start position")?,
                );
            }
            "isearch_end" => {
                isearch_end = Some(
                    value
                        .parse::<usize>()
                        .context("Unable to parse isearch end position")?,
                );
            }
            "zle_highlight_isearch" => zle_highlight_isearch = Some(decode_string_v1(value)),

            "yank_active" => yank_active = Some(value),
            "yank_start" => {
                yank_start = Some(
                    value
                        .parse::<usize>()
                        .context("Unable to parse yank start position")?,
                );
            }
            "yank_end" => {
                yank_end = Some(
                    value
                        .parse::<usize>()
                        .context("Unable to parse yank end position")?,
                );
            }
            "zle_highlight_paste" => zle_highlight_paste = Some(decode_string_v1(value)),

            _ => {}
        }
    }

    // read pre-buffer lines
    let mut lines = String::new();
    let mut pre_buffer_total_len = 0;
    for _ in 0..pre_buffer_line_count {
        let mut line = String::new();
        reader.read_line(&mut line).context("Unable to read line")?;
        lines.push_str(&line);

        // this is O(n) but necessary in case the command contains
        // multi-byte characters
        let line_len = line.chars().count();
        pre_buffer_total_len += line_len;
    }

    log::trace!("{pre_buffer_line_count} pre-buffer lines read.");

    // read lines
    let mut total_len = 0;
    let mut line_lengths = Vec::new();
    let mut cursor_line = 0;
    let mut cursor_line_found = false;
    for i in 0..buffer_line_count {
        let mut line = String::new();
        reader.read_line(&mut line).context("Unable to read line")?;

        // this is O(n) but necessary in case the command contains
        // multi-byte characters
        let line_len = line.chars().count();

        // determine in which line we are currently (line_len contains trailing \n)
        if (total_len..total_len + line_len).contains(&cursor) {
            cursor_line = i;
            cursor_line_found = true;
        }

        if !cursor_line_found || i < cursor_line.saturating_add(term_rows) {
            lines.push_str(&line);
            line_lengths.push(line_len);
            total_len += line_len;
        } else {
            // no need to store lines that are outside the terminal window, but
            // we still need to read them from the client
        }
    }

    log::trace!("{buffer_line_count} buffer lines read.");

    // check if the client version matches ours
    if client_version.is_none_or(|v| v != "1") {
        // Return immediately. This will close the connection with an empty
        // response.
        log::warn!("Client version is {client_version:?}. Expected protocol version is \"1\".");
        return Ok(());
    }

    // handle "hello" command — respond with daemon version
    if cmd == Some("hello") {
        writer
            .write_all(format!("ver={}\n", env!("CARGO_PKG_VERSION")).as_bytes())
            .context("Unable to send version")?;
        return Ok(());
    }

    // Performance: Limit spans to a window around the cursor. This is necessary
    // to reduce the number of ranges sent back to the client. The window is
    // calculated based on the number of lines and columns in the terminal. We
    // try to cut off as much as possible. In practice, since we don't know
    // exactly where the cursor is on the screen, we will most likely still
    // include too much, but that's OK.
    let min = line_lengths[0..cursor_line.saturating_sub(term_rows)]
        .iter()
        .sum::<usize>()
        .max(cursor.saturating_sub(term_cols * term_rows));
    let max = line_lengths[0..line_lengths
        .len()
        .min(cursor_line.saturating_add(term_rows))]
        .iter()
        .sum::<usize>()
        .min(cursor.saturating_add(term_cols * term_rows));

    // perform highlighting
    let request = HighlightingRequest::default()
        .with_cursor(pre_buffer_total_len + cursor)
        .with_pwd(pwd.as_deref())
        .with_history_expansions(history_expansions_enabled)
        .with_predicate(|range| {
            // skip spans in the pre-buffer
            if range.end <= pre_buffer_total_len {
                return false;
            }

            // subtract pre-buffer offset
            let start = range.start.saturating_sub(pre_buffer_total_len);
            let end = range.end.saturating_sub(pre_buffer_total_len);

            // skip spans outside the current terminal window
            start < max && end > min
        });
    let result = highlighter.highlight(&lines, &request)?;

    // merge consecutive spans with the same style
    let mut merged: Vec<Span> = Vec::new();
    for mut span in result {
        // subtract pre-buffer offset
        span.start = span.start.saturating_sub(pre_buffer_total_len);
        span.end = span.end.saturating_sub(pre_buffer_total_len);

        if let Some(prev) = merged.last_mut()
            && prev.end == span.start
            && prev.style == span.style
        {
            prev.end = span.end;
        } else {
            merged.push(span);
        }
    }

    log::trace!("Highlighting result: {merged:?}");

    // handle "resolve" command - return list of dynamic callables
    if cmd == Some("resolve") {
        for s in merged {
            if let SpanStyle::Dynamic(DynamicStyle::Callable { parsed_callable }) = s.style {
                writer
                    .write_all(format!("{}\n", encode_string_v1(parsed_callable)).as_bytes())
                    .context("Unable to send response")?;
            }
        }
        return Ok(());
    }

    for s in merged {
        // write response
        let message = match s.style {
            SpanStyle::Static(static_style) => {
                let fss = format_static_style(&static_style);
                if fss.is_empty() {
                    None
                } else {
                    Some(format!("{} {} {}\n", s.start, s.end, fss))
                }
            }
            SpanStyle::Dynamic(dynamic_style) => match dynamic_style {
                DynamicStyle::Callable { parsed_callable } => {
                    let all_fss = highlighter
                        .callable_choices()
                        .iter()
                        .map(|c| {
                            let t = match c.0 {
                                CallableType::Alias => 'a',
                                CallableType::Builtin => 'b',
                                CallableType::Command => 'c',
                                CallableType::Function => 'f',
                                CallableType::Missing => 'm',
                                CallableType::Unknown => 'e',
                            };
                            format!("{t}:{}", format_static_style(c.1))
                        })
                        .collect::<Vec<_>>()
                        .join(";");
                    if all_fss.is_empty() {
                        None
                    } else {
                        Some(format!(
                            "-DY{} {} {} {}\n",
                            s.start,
                            s.end,
                            encode_string_v1(parsed_callable),
                            encode_string_v1(all_fss)
                        ))
                    }
                }
                DynamicStyle::Nameddir { .. } => None,
            },
        };

        if let Some(message) = message {
            log::trace!("Writing response: {message}");
            writer
                .write_all(message.as_bytes())
                .context("Unable to send response")?;
        }
    }

    // apply zle_highlight styles
    add_zle_highlight(
        region_active.map(|a| a != "0"),
        mark,
        Some(cursor),
        zle_highlight_region,
        "standout",
        &mut writer,
    )?;
    add_zle_highlight(
        suffix_active.map(|a| a != "0"),
        suffix_start,
        suffix_end,
        zle_highlight_suffix,
        "bold",
        &mut writer,
    )?;
    add_zle_highlight(
        isearch_active.map(|a| a != "0"),
        isearch_start,
        isearch_end,
        zle_highlight_isearch,
        "underline",
        &mut writer,
    )?;
    add_zle_highlight(
        yank_active.map(|a| a != "0"),
        yank_start,
        yank_end,
        zle_highlight_paste,
        "standout",
        &mut writer,
    )?;

    Ok(())
}

fn handle_connection_v2<R: BufRead, W: Write>(
    mut reader: R,
    writer: W,
    highlighter: &Highlighter,
    supports_nameddirs: bool,
) -> Result<()> {
    let mut cmd = Command::Highlight;
    let mut body_line_count = 0;

    // read header
    let mut header_lines = Vec::new();
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .context("Unable to read header line")?;
        if line.ends_with('\n') {
            line.pop();
        }
        if line.is_empty() {
            // end of header
            break;
        }

        log::trace!("Received header: {}", line);

        if let Some(value) = line.strip_prefix("CMD=") {
            cmd = match value {
                "HLO" => Command::Hello,
                "HLT" => Command::Highlight,
                _ => bail!("Unknown command: {value}"),
            };
        } else if let Some(value) = line.strip_prefix("LNS=") {
            body_line_count = value
                .parse::<usize>()
                .context("Unable to parse number of lines in message body")?;
        } else {
            header_lines.push(line);
        }
    }

    // read body
    let mut body_lines = Vec::new();
    for _ in 0..body_line_count {
        let mut line = String::new();
        reader.read_line(&mut line).context("Unable to read line")?;
        body_lines.push(line);
    }

    log::trace!("{body_line_count} body lines read.");

    match cmd {
        Command::Hello => handle_hello(writer),
        Command::Highlight => handle_highlight(
            header_lines,
            body_lines,
            reader,
            writer,
            highlighter,
            supports_nameddirs,
        ),
    }
}

/// Handle "HLO" command. Respond with daemon version.
fn handle_hello<W>(mut writer: W) -> Result<()>
where
    W: Write,
{
    writer
        .write_all(format!("VER={}\n", env!("CARGO_PKG_VERSION")).as_bytes())
        .context("Unable to send version")?;
    Ok(())
}

/// Handle "HLT" command
fn handle_highlight<R, W>(
    header_lines: Vec<String>,
    body_lines: Vec<String>,
    mut reader: R,
    mut writer: W,
    highlighter: &Highlighter,
    supports_nameddirs: bool,
) -> Result<()>
where
    R: BufRead,
    W: Write,
{
    let mut term_cols = 1000;
    let mut term_rows = 1000;
    let mut cursor = 0;

    let mut pre_buffer_line_count = 0;

    let mut pwd = None;
    let mut autocd_enabled = false;
    let mut history_expansions_enabled = true;

    let mut region_active = None;
    let mut mark = None;
    let mut zle_highlight_region = None;

    let mut suffix_active = None;
    let mut suffix_start = None;
    let mut suffix_end = None;
    let mut zle_highlight_suffix = None;

    let mut isearch_active = None;
    let mut isearch_start = None;
    let mut isearch_end = None;
    let mut zle_highlight_isearch = None;

    let mut yank_active = None;
    let mut yank_start = None;
    let mut yank_end = None;
    let mut zle_highlight_paste = None;

    // parse header
    for line in header_lines {
        if let Some(value) = line.strip_prefix("COL=") {
            term_cols = value
                .parse::<usize>()
                .context("Unable to parse number of terminal columns")?;
        } else if let Some(value) = line.strip_prefix("ROW=") {
            term_rows = value
                .parse::<usize>()
                .context("Unable to parse number of terminal rows")?;
        } else if let Some(value) = line.strip_prefix("CUR=") {
            cursor = value
                .parse::<usize>()
                .context("Unable to parse cursor position")?;
        } else if let Some(value) = line.strip_prefix("PWD=") {
            pwd = Some(decode_string(value));
        } else if let Some(value) = line.strip_prefix("ACD=") {
            autocd_enabled = value
                .parse::<u8>()
                .context("Unable to parse autocd option")?
                > 0;
        } else if let Some(value) = line.strip_prefix("BNG=") {
            history_expansions_enabled = value
                .parse::<u8>()
                .context("Unable to parse banghist option")?
                > 0;
        } else if let Some(value) = line.strip_prefix("PRL=") {
            pre_buffer_line_count = value
                .parse::<usize>()
                .context("Unable to parse number of lines in pre-buffer")?;
        } else if let Some(value) = line.strip_prefix("RGA=") {
            region_active = Some(
                value
                    .parse::<u8>()
                    .context("Unable to parse region active flag")?
                    > 0,
            );
        } else if let Some(value) = line.strip_prefix("RGE=") {
            mark = Some(
                value
                    .parse::<usize>()
                    .context("Unable to parse region end position")?,
            );
        } else if let Some(value) = line.strip_prefix("RGH=") {
            zle_highlight_region = Some(decode_string(value));
        } else if let Some(value) = line.strip_prefix("SFA=") {
            suffix_active = Some(
                value
                    .parse::<u8>()
                    .context("Unable to parse suffix active flag")?
                    > 0,
            );
        } else if let Some(value) = line.strip_prefix("SFS=") {
            suffix_start = Some(
                value
                    .parse::<usize>()
                    .context("Unable to parse suffix start position")?,
            );
        } else if let Some(value) = line.strip_prefix("SFE=") {
            suffix_end = Some(
                value
                    .parse::<usize>()
                    .context("Unable to parse suffix end position")?,
            );
        } else if let Some(value) = line.strip_prefix("SFH=") {
            zle_highlight_suffix = Some(decode_string(value));
        } else if let Some(value) = line.strip_prefix("ISA=") {
            isearch_active = Some(
                value
                    .parse::<u8>()
                    .context("Unable to parse isearch active flag")?
                    > 0,
            );
        } else if let Some(value) = line.strip_prefix("ISS=") {
            isearch_start = Some(
                value
                    .parse::<usize>()
                    .context("Unable to parse isearch start position")?,
            );
        } else if let Some(value) = line.strip_prefix("ISE=") {
            isearch_end = Some(
                value
                    .parse::<usize>()
                    .context("Unable to parse isearch end position")?,
            );
        } else if let Some(value) = line.strip_prefix("ISH=") {
            zle_highlight_isearch = Some(decode_string(value));
        } else if let Some(value) = line.strip_prefix("YKA=") {
            yank_active = Some(
                value
                    .parse::<u8>()
                    .context("Unable to parse yank active flag")?
                    > 0,
            );
        } else if let Some(value) = line.strip_prefix("YKS=") {
            yank_start = Some(
                value
                    .parse::<usize>()
                    .context("Unable to parse yank start position")?,
            );
        } else if let Some(value) = line.strip_prefix("YKE=") {
            yank_end = Some(
                value
                    .parse::<usize>()
                    .context("Unable to parse yank end position")?,
            );
        } else if let Some(value) = line.strip_prefix("YKH=") {
            zle_highlight_paste = Some(decode_string(value));
        }
    }

    if pre_buffer_line_count > body_lines.len() {
        bail!("Pre-buffer line count is larger than body line count");
    }

    let buffer_line_count = body_lines.len() - pre_buffer_line_count;

    // read pre-buffer lines
    let mut body_iterator = body_lines.into_iter();
    let mut lines = String::new();
    let mut pre_buffer_total_len = 0;
    for _ in 0..pre_buffer_line_count {
        let line = body_iterator
            .next()
            .expect("pre_buffer_line_count is always less than or equal to body_lines.len()");
        lines.push_str(&line);
        pre_buffer_total_len += line.chars().count();
    }

    log::trace!("{pre_buffer_line_count} pre-buffer lines parsed.");

    // read buffer lines
    let mut total_len = 0;
    let mut line_lengths = Vec::new();
    let mut cursor_line = 0;
    let mut cursor_line_found = false;
    for i in 0..buffer_line_count {
        let line = body_iterator.next().expect(
            "buffer_line_count is always equal to body_lines.len() - pre_buffer_line_count",
        );

        let line_len = line.chars().count();

        // determine in which line we are currently (line_len contains trailing \n)
        if (total_len..total_len + line_len).contains(&cursor) {
            cursor_line = i;
            cursor_line_found = true;
        }

        if !cursor_line_found || i < cursor_line.saturating_add(term_rows) {
            lines.push_str(&line);
            line_lengths.push(line_len);
            total_len += line_len;
        } else {
            // no need to store lines that are outside the terminal window
            break;
        }
    }

    log::trace!("{buffer_line_count} buffer lines parsed.");

    // Performance: Limit spans to a window around the cursor. This is necessary
    // to reduce the number of ranges sent back to the client. The window is
    // calculated based on the number of lines and columns in the terminal. We
    // try to cut off as much as possible. In practice, since we don't know
    // exactly where the cursor is on the screen, we will most likely still
    // include too much, but that's OK.
    let min = line_lengths[0..cursor_line.saturating_sub(term_rows)]
        .iter()
        .sum::<usize>()
        .max(cursor.saturating_sub(term_cols * term_rows));
    let max = line_lengths[0..line_lengths
        .len()
        .min(cursor_line.saturating_add(term_rows))]
        .iter()
        .sum::<usize>()
        .min(cursor.saturating_add(term_cols * term_rows));

    // perform highlighting
    let request = HighlightingRequest::default()
        .with_cursor(pre_buffer_total_len + cursor)
        .with_pwd(pwd.as_deref())
        .with_autocd(autocd_enabled)
        .with_nameddirs(supports_nameddirs)
        .with_history_expansions(history_expansions_enabled)
        .with_predicate(|range| {
            // skip spans in the pre-buffer
            if range.end <= pre_buffer_total_len {
                return false;
            }

            // subtract pre-buffer offset
            let start = range.start.saturating_sub(pre_buffer_total_len);
            let end = range.end.saturating_sub(pre_buffer_total_len);

            // skip spans outside the current terminal window
            start < max && end > min
        });
    let mut result = highlighter.highlight(&lines, &request)?;

    // collect unique nameddirs that need to be resolved
    let nameddirs_to_resolve = result
        .iter()
        .filter_map(|span| match &span.style {
            SpanStyle::Dynamic(DynamicStyle::Nameddir { name, .. }) => Some(name.as_str()),
            _ => None,
        })
        .collect::<FxHashSet<_>>();

    // resolve nameddirs to their absolute paths
    let resolved_nameddirs = if supports_nameddirs && !nameddirs_to_resolve.is_empty() {
        resolve_nameddirs(
            &nameddirs_to_resolve.into_iter().collect::<Vec<_>>(),
            &mut reader,
            &mut writer,
        )?
    } else {
        FxHashMap::default()
    };

    // re-classify nameddir-style spans with their intended final style
    result.retain_mut(|span| {
        let SpanStyle::Dynamic(DynamicStyle::Nameddir {
            name,
            parsed_path,
            dynamic_type,
            base_style,
        }) = &span.style
        else {
            return true;
        };

        let mut path = parsed_path.clone();
        if let Some(directory) = resolved_nameddirs
            .get(name)
            .filter(|directory| !directory.is_empty())
        {
            path.replace_range(0..name.len() + 1, directory);
        }

        let Some(style) = highlighter.classify_dynamic(
            path,
            &(span.start..span.end),
            *dynamic_type,
            base_style.as_ref(),
            &request,
        ) else {
            return false;
        };

        span.style = style;

        true
    });

    // merge consecutive spans with the same style
    let mut merged: Vec<Span> = Vec::new();
    for mut span in result {
        // subtract pre-buffer offset
        span.start = span.start.saturating_sub(pre_buffer_total_len);
        span.end = span.end.saturating_sub(pre_buffer_total_len);

        if let Some(prev) = merged.last_mut()
            && prev.end == span.start
            && prev.style == span.style
        {
            prev.end = span.end;
        } else {
            merged.push(span);
        }
    }

    log::trace!("Highlighting result: {merged:?}");

    // collect unique callables that need to be resolved
    let mut callables_to_resolve: FxHashSet<&str> = FxHashSet::default();
    for span in &merged {
        if let SpanStyle::Dynamic(DynamicStyle::Callable { parsed_callable }) = &span.style {
            callables_to_resolve.insert(parsed_callable);
        }
    }

    // resolve callables to CallableTypes
    let resolved_callables = resolve_callables(
        &callables_to_resolve.into_iter().collect::<Vec<_>>(),
        true,
        &mut reader,
        &mut writer,
        highlighter,
        &pwd,
        FxHashSet::default(),
    )?;

    // write response
    for s in &merged {
        let message = match &s.style {
            SpanStyle::Static(static_style) => {
                let fss = format_static_style(static_style);
                if fss.is_empty() {
                    None
                } else {
                    Some(format!("{} {} {}\n", s.start, s.end, fss))
                }
            }
            SpanStyle::Dynamic(DynamicStyle::Callable { parsed_callable }) => resolved_callables
                .get(parsed_callable.as_str())
                .and_then(|callable_type| {
                    highlighter
                        .callable_choices()
                        .get(callable_type)
                        .or_else(|| highlighter.callable_choices().get(&CallableType::Unknown))
                        .map(format_static_style)
                        .filter(|s| !s.is_empty())
                        .map(|style| format!("{} {} {}\n", s.start, s.end, style))
                }),
            SpanStyle::Dynamic(DynamicStyle::Nameddir { .. }) => None,
        };

        if let Some(message) = message {
            log::trace!("Writing response: {message}");
            writer
                .write_all(message.as_bytes())
                .context("Unable to send response")?;
        }
    }

    // apply zle_highlight styles
    add_zle_highlight(
        region_active,
        mark,
        Some(cursor),
        zle_highlight_region,
        "standout",
        &mut writer,
    )?;
    add_zle_highlight(
        suffix_active,
        suffix_start,
        suffix_end,
        zle_highlight_suffix,
        "bold",
        &mut writer,
    )?;
    add_zle_highlight(
        isearch_active,
        isearch_start,
        isearch_end,
        zle_highlight_isearch,
        "underline",
        &mut writer,
    )?;
    add_zle_highlight(
        yank_active,
        yank_start,
        yank_end,
        zle_highlight_paste,
        "standout",
        &mut writer,
    )?;

    Ok(())
}

/// Resolve named directories to absolute paths by asking the client.
fn resolve_nameddirs<R, W>(
    names: &[&str],
    reader: &mut R,
    writer: &mut W,
) -> Result<FxHashMap<String, String>>
where
    R: BufRead,
    W: Write,
{
    writer
        .write_all(format!("?CMD=NMD\nLNS={}\n\n", names.len()).as_bytes())
        .context("Unable to send NMD query")?;
    for name in names {
        writeln!(writer, "{}", encode_string(name)).context("Unable to send NMD query body")?;
    }
    writer.flush().context("Unable to flush NMD query")?;

    let mut result = FxHashMap::default();
    for name in names {
        let mut answer = String::new();
        reader
            .read_line(&mut answer)
            .context("Unable to read NMD answer")?;
        let answer = decode_string(answer.trim_ascii_end());
        result.insert((*name).to_owned(), answer);
    }
    Ok(result)
}

/// Resolve a list of callables to their CallableTypes by asking the client. If
/// `lookup_aliases` is true, aliases will be resolved recursively until a
/// non-alias CallableType is found. The `visited` set is used to prevent
/// infinite recursion when resolving aliases.
fn resolve_callables<R, W>(
    callables_to_resolve: &[&str],
    lookup_aliases: bool,
    reader: &mut R,
    writer: &mut W,
    highlighter: &Highlighter,
    pwd: &Option<String>,
    visited: FxHashSet<&str>,
) -> Result<FxHashMap<String, CallableType>>
where
    R: BufRead,
    W: Write,
{
    if callables_to_resolve.is_empty() {
        return Ok(FxHashMap::default());
    }

    // send a single query for all callables
    let las = if !lookup_aliases { "LAS=0\n" } else { "" };
    writer
        .write_all(format!("?CMD=CAL\nLNS={}\n{las}\n", callables_to_resolve.len()).as_bytes())
        .context("Unable to send CAL query")?;
    for &name in callables_to_resolve {
        writer
            .write_all(format!("{}\n", encode_string(name)).as_bytes())
            .context("Unable to send CAL query body")?;
    }
    writer.flush().context("Unable to flush CAL query")?;

    // read one line per callable
    let mut answers = Vec::new();
    for name in callables_to_resolve {
        let mut answer = String::new();
        reader
            .read_line(&mut answer)
            .context("Unable to read CAL answer")?;
        if answer.ends_with('\n') {
            answer.pop();
        }
        answers.push((name, answer));
    }

    let mut result = FxHashMap::default();
    for (name, answer) in answers {
        let mut callable_type = match answer.bytes().next() {
            Some(b'a') => CallableType::Alias,
            Some(b'b') => CallableType::Builtin,
            Some(b'c') => CallableType::Command,
            Some(b'f') => CallableType::Function,
            Some(b'm') => CallableType::Missing,
            _ => CallableType::Unknown,
        };

        log::trace!("Callable `{name}' resolved to {callable_type:?}");

        // recursively resolve aliases
        if callable_type == CallableType::Alias {
            let resolved_alias = decode_string(&answer[1..]);

            log::trace!("Alias `{name}' resolved to `{resolved_alias}'");

            // apply highlighting to the resolved alias to extract all callables
            let request = HighlightingRequest::default().with_pwd(pwd.as_deref());
            let alias_spans = highlighter.highlight(&resolved_alias, &request)?;

            // check if all callables can be resolved
            'outer: for span in &alias_spans {
                if let SpanStyle::Dynamic(DynamicStyle::Callable { parsed_callable }) = &span.style
                {
                    // If this callable is already in the visited set, tell the
                    // client to only lookup builtins, functions, commands, but
                    // not aliases. This prevents infinite recursion and is also
                    // in line with what Zsh does when it executes an alias.
                    let las = !visited.contains(parsed_callable.as_str());

                    // ask client to resolve this callable
                    let mut visited = visited.clone();
                    visited.insert(name);
                    let alias_result = resolve_callables(
                        &[parsed_callable],
                        las,
                        reader,
                        writer,
                        highlighter,
                        pwd,
                        visited,
                    )?;

                    // if the callable resolves to `Missing`, mark the whole
                    // alias as `Missing` too
                    for ar in alias_result {
                        if ar.1 == CallableType::Missing {
                            callable_type = CallableType::Missing;
                            break 'outer;
                        }
                    }
                }
            }
        }

        result.insert(name.to_string(), callable_type);
    }

    Ok(result)
}

pub fn activate(runtime_dir: &Path, config: &Config) -> Result<()> {
    check_config(config)?;

    let (role, already_running) = start_daemon_internal(runtime_dir, config, false)?;
    if role == Role::Parent {
        let exe = std::env::current_exe()?;

        let template = ActivateTemplate {
            zsh_patina_path: exe.to_str().unwrap().to_string(),
            zsh_patina_runtime_dir: runtime_dir
                .to_str()
                .unwrap()
                .trim_end_matches('/')
                .to_string(),
            version: "3",
        };

        let mut s = stdout().lock();
        s.write_all(template.render().unwrap().as_bytes())?;
        s.flush()?;
    }

    if already_running {
        // Check the currently running daemon's version. Restart it if the
        // versions don't match.
        let socket_path = sock_path(runtime_dir);
        let mut stream = connect_with_retry(&socket_path)?;

        let timeout = Duration::from_secs(2);
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;

        stream.write_all(b"VER=3\nCMD=HLO\n\n")?;

        let mut response = String::new();
        let mut reader = BufReader::new(&stream);
        reader.read_line(&mut response)?;

        let daemon_version = response
            .split_ascii_whitespace()
            .find_map(|kv| kv.strip_prefix("VER="));
        let our_version = env!("CARGO_PKG_VERSION");

        if daemon_version.is_none_or(|v| v != our_version) {
            // restart daemon
            stop_daemon(runtime_dir)?;
            start_daemon(runtime_dir, config, false)?;
        }
    }

    Ok(())
}

pub fn start_daemon(runtime_dir: &Path, config: &Config, no_daemon: bool) -> Result<()> {
    start_daemon_internal(runtime_dir, config, no_daemon)?;
    Ok(())
}

fn start_daemon_internal(
    runtime_dir: &Path,
    config: &Config,
    no_daemon: bool,
) -> Result<(Role, bool)> {
    // legacy path for backwards compatibility
    {
        let pid_file = pid_path(runtime_dir);

        if let Some(pid) = read_pid_legacy(&pid_file) {
            if pid_alive(pid) {
                if no_daemon {
                    println!("Daemon is already running. PID {pid}.");
                }

                // legacy daemon is already running
                return Ok((Role::Parent, true));
            } else {
                // remove the stale legacy file and fall through
                let _ = fs::remove_file(pid_file);
            }
        }
    }

    // Make sure the data directory exists before we create/open the lock file
    fs::create_dir_all(runtime_dir).context("Unable to create data directory")?;

    // open lock file and acquire an exclusive lock on it
    let lock_file_path = lock_path(runtime_dir);
    let mut lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_file_path)?;
    match lock_file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            if no_daemon {
                let pid = read_pid(&mut lock_file)
                    .context("Daemon is already running but current PID could not be read")?;
                println!("Daemon is already running. PID {pid}.");
            }

            // daemon is already running
            return Ok((Role::Parent, true));
        }
        Err(TryLockError::Error(e)) => return Err(e.into()),
    }

    if !no_daemon {
        // Double-fork:
        //
        // Fork #1: the parent exits immediately so the `start` call returns at
        //          once. The child continues.
        //
        // setsid: the child becomes session leader, fully detached from the
        //         terminal and from Zsh's process group.
        //
        // Fork #2: the session-leader child forks again and exits. The
        //          grandchild can never accidentally re-acquire a controlling
        //          terminal (POSIX guarantee).
        //
        // The grandchild is then adopted by PID 1 (init/systemd) and runs as a
        // true background daemon.

        // fork #1
        // SAFETY: Forking is safe because we haven't created any threads yet
        // and we will exit as soon as possible
        match unsafe { libc::fork() } {
            -1 => {
                bail!("fork #1 failed");
            }
            0 => {
                // child: continue below
            }
            _ => {
                // parent: return immediately
                return Ok((Role::Parent, false));
            }
        }

        // become session leader
        // SAFETY: No preconditions — setsid() is always safe to call.
        unsafe { libc::setsid() };

        // fork #2
        // SAFETY: Forking is safe because we haven't created any threads yet
        // and we will exit as soon as possible
        match unsafe { libc::fork() } {
            -1 => {
                bail!("fork #2 failed");
            }
            0 => {
                // grandchild
            }
            _ => {
                // intermediate child: exit
                return Ok((Role::Child, false));
            }
        }

        // from here on, we are a true background daemon ...

        // Close all file descriptors so we're really decoupled from the parent
        // process. Keep the lock file open!
        // SAFETY: there is only one thread so far, so `close_open_fds` is safe
        // to call.
        unsafe {
            close_fds::close_open_fds(3, &[lock_file.as_raw_fd()]);
        }

        // Redirect stdin, stdout, stderr to /dev/null to further decouple from
        // the parent.
        // SAFETY: `devnull` was just successfully opened so its fd is valid.
        // stdin/stdout/stderr are valid target fds by definition. `devnull` is
        // dropped after this block; the dup'd fds are independent copies so
        // closing the original does not affect them.
        unsafe {
            let devnull = std::fs::File::open("/dev/null").unwrap();
            libc::dup2(devnull.as_raw_fd(), libc::STDIN_FILENO);
            libc::dup2(devnull.as_raw_fd(), libc::STDOUT_FILENO);
            libc::dup2(devnull.as_raw_fd(), libc::STDERR_FILENO);
        }
    }

    // write our PID so that `stop` and `status` can find us
    lock_file.set_len(0)?;
    lock_file.seek(SeekFrom::Start(0))?;
    writeln!(lock_file, "{}", process::id())?;
    lock_file.flush()?;

    // Set read/write permissions and protect lock file from being deleted by
    // periodic cleanup (https://specifications.freedesktop.org/basedir/latest/).
    lock_file
        .set_permissions(Permissions::from_mode(0o1600))
        .with_context(|| format!("Unable to set permissions of {lock_file_path:?}"))?;

    // clean up leftover socket
    let socket_path = sock_path(runtime_dir);
    let _ = fs::remove_file(&socket_path); // ignore errors

    let pool = ThreadPoolBuilder::new().num_threads(4).build().unwrap();

    // initialize highlighter
    let highlighter = Arc::new(HighlighterBuilder::new(&config.highlighting).build()?);

    // highlight something to make sure everything is loaded - do this in a
    // background task to not delay the main thread
    let init_highlighter = Arc::clone(&highlighter);
    pool.spawn(move || {
        let _ = init_highlighter.highlight(
            "echo Welcome to zsh-patina!",
            &HighlightingRequest::default(),
        );
    });

    // bind the Unix domain socket
    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("Unable to bind socket {socket_path:?}"))?;

    // Set read/write permissions and protect socket from being deleted by
    // periodic cleanup (https://specifications.freedesktop.org/basedir/latest/).
    fs::set_permissions(&socket_path, Permissions::from_mode(0o1600))
        .with_context(|| format!("Unable to set permissions of {socket_path:?}"))?;

    log::info!("Listening for connections on {socket_path:?} ...");

    // accept connections
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                // This is a safe guard against bugs. It is extremely unlikely
                // that a single message will take longer than 1 second to be
                // sent from the client to the server (or vice versa) but in
                // case something goes wrong during communication (e.g. the
                // client sends a higher line count in the header than it
                // actually sends lines), we won't block indefinitely.
                stream.set_read_timeout(Some(Duration::from_secs(1)))?;
                stream.set_write_timeout(Some(Duration::from_secs(1)))?;

                let highlighter = Arc::clone(&highlighter);
                pool.spawn(|| {
                    log::debug!("New connection ...");

                    // Handle connection and ignore any errors. Errors can
                    // happen in two cases:
                    // * We are unable to read the input. In this case, Zsh will
                    //   generate an error message while the user is typing
                    //   ("broken pipe")
                    // * We are unable to highlight the command or send a
                    //   response. In this case, `stream` will be dropped and
                    //   Zsh will just continue without highlighting.
                    let e = handle_connection(stream, highlighter);

                    match e {
                        Ok(_) => log::debug!("Connection successfully handled."),
                        Err(e) => {
                            log::error!("Failed to handle connection.");
                            log::error!("{e}");
                        }
                    }
                });
            }
            _ => {
                break;
            }
        }
    }

    // always remove socket before lock file to avoid a possible race condition
    let _ = fs::remove_file(socket_path);
    let _ = fs::remove_file(lock_file_path);

    Ok((Role::Daemon, false))
}

#[deprecated = "This function is only needed for backwards compatibility. It will be removed in a future release."]
#[allow(deprecated)]
fn stop_daemon_legacy(runtime_dir: &Path) -> bool {
    let pid_file = pid_path(runtime_dir);
    if let Some(pid) = read_pid_legacy(&pid_file)
        && pid_alive(pid)
    {
        // SAFETY: `pid` is known to be running. SIGTERM is a valid signal
        // number.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };

        let _ = fs::remove_file(sock_path(runtime_dir));
        let _ = fs::remove_file(pid_file);

        true
    } else {
        false
    }
}

pub fn stop_daemon(runtime_dir: &Path) -> Result<()> {
    if stop_daemon_legacy(runtime_dir) {
        return Ok(());
    }

    // open lock file and try to acquire an exclusive lock
    let lock_file_path = lock_path(runtime_dir);
    let mut lock_file = match OpenOptions::new()
        .read(true)
        .write(true) // required to obtain exclusive lock
        .open(&lock_file_path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // lock file does not exist - daemon is not running
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };

    match lock_file.try_lock() {
        Ok(()) => {
            // lock file exists but it is not locked, which means it is stale
            // and the daemon is not running
            Ok(())
        }

        Err(TryLockError::WouldBlock) => {
            // daemon is running - read PID
            let pid = read_pid(&mut lock_file)?;

            // SAFETY: `pid` is known to be running. SIGTERM is a valid signal
            // number.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };

            // Remove the socket immediately so no new client connects to a
            // daemon that is about to shut down. Keep the lock file on disk:
            // the daemon still holds the lock and will remove the file when it
            // exits. Leaving it in place also blocks any concurrent starter
            // from creating a new lock inode until the old daemon is gone.
            let _ = fs::remove_file(sock_path(runtime_dir));

            // wait for the daemon to release the lock file
            wait_for_lock_release(&mut lock_file)?;

            Ok(())
        }

        Err(TryLockError::Error(e)) => Err(e.into()),
    }
}

#[deprecated = "This function is only needed for backwards compatibility. It will be removed in a future release."]
#[allow(deprecated)]
fn is_daemon_running_legacy(runtime_dir: &Path) -> Option<u32> {
    let pid_file = pid_path(runtime_dir);
    if let Some(pid) = read_pid_legacy(&pid_file)
        && pid_alive(pid)
    {
        Some(pid)
    } else {
        None
    }
}

pub fn is_daemon_running(runtime_dir: &Path) -> Result<Option<u32>> {
    if let Some(pid) = is_daemon_running_legacy(runtime_dir) {
        return Ok(Some(pid));
    }

    // open lock file and try to acquire an exclusive lock
    let lock_file_path = lock_path(runtime_dir);
    let mut lock_file = match OpenOptions::new()
        .read(true)
        .write(true) // required to obtain exclusive lock
        .open(&lock_file_path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // lock file does not exist - daemon is not running
            return Ok(None);
        }
        Err(e) => return Err(e.into()),
    };

    match lock_file.try_lock() {
        Ok(()) => {
            // lock file exists but it is not locked, which means it is stale and
            // the daemon is not running
            Ok(None)
        }
        Err(TryLockError::WouldBlock) => {
            // daemon is running - return PID
            read_pid(&mut lock_file).map(Some)
        }
        Err(TryLockError::Error(e)) => Err(e.into()),
    }
}

pub fn status_daemon(runtime_dir: &Path) -> Result<()> {
    if let Some(pid) = is_daemon_running(runtime_dir)? {
        println!("Daemon is running. PID {pid}.");
        Ok(())
    } else {
        bail!("Daemon is stopped.");
    }
}
