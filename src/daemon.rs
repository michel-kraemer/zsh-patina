use anyhow::{Context, Result, bail};
use askama::Template;
use rayon::ThreadPoolBuilder;
use std::{
    fs::{self, Permissions},
    io::{BufRead, BufReader, Write, stdout},
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
    time::Duration,
};

use crate::{
    commands::check_config,
    config::Config,
    highlighting::{DynamicStyle, Highlighter, HighlighterBuilder, Span, SpanStyle, StaticStyle},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Parent,
    Child,
    Daemon,
}

/// The version of the communication protocol between the Zsh client and the
/// highlighting daemon. Increase this version number whenever there has been
/// a breaking change.
const PROTOCOL_VERSION: &str = "1";

#[derive(Template)]
#[template(path = "zsh-patina.zsh")]
struct ActivateTemplate {
    zsh_patina_path: String,
    zsh_patina_runtime_dir: String,
    version: &'static str,
}

fn pid_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("daemon.pid")
}

fn sock_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("daemon.sock")
}

/// Read the PID from the PID file. Returns `None` if the file does not exist or
/// contains garbage.
fn read_pid(pid_file: &Path) -> Option<u32> {
    fs::read_to_string(pid_file).ok()?.trim().parse().ok()
}

/// Check whether a process with the given PID is currently alive.
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
fn decode_string(s: &str) -> String {
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

fn encode_string(input: String) -> String {
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

/// Add a region with a Zsh `zle_highlight` style if the region is active. The
/// region is defined by `start` and `end`, and the style is defined by
/// `zle_highlight` (e.g. `underline`). If the region is active but
/// `zle_highlight` is empty, the `default_value` will be used.
fn add_zle_highlight(
    active: Option<&str>,
    start: Option<usize>,
    end: Option<usize>,
    zle_highlight: Option<String>,
    default_value: &str,
    stream: &mut UnixStream,
) -> Result<()> {
    if let Some(active) = active
        && active != "0"
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
        stream
            .write_all(format!("{from} {to} {style}\n").as_bytes())
            .context("Unable to send response")?;
    }
    Ok(())
}

fn handle_connection(mut stream: UnixStream, highlighter: Arc<Highlighter>) -> Result<()> {
    let mut reader = BufReader::new(&stream);

    // read number of lines
    let mut header = String::new();
    reader
        .read_line(&mut header)
        .context("Unable to read header")?;

    let mut client_version = None;

    let mut term_cols = 1000;
    let mut term_rows = 1000;
    let mut cursor = 0;

    let mut pre_buffer_line_count = 0;
    let mut buffer_line_count = 0;

    let mut pwd = None;
    let mut cmd = None;

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

            "pwd" => pwd = Some(decode_string(value)),
            "cmd" => cmd = Some(value),

            "region_active" => region_active = Some(value),
            "mark" => {
                mark = Some(
                    value
                        .parse::<usize>()
                        .context("Unable to parse mark position")?,
                );
            }
            "zle_highlight_region" => zle_highlight_region = Some(decode_string(value)),

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
            "zle_highlight_suffix" => zle_highlight_suffix = Some(decode_string(value)),

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
            "zle_highlight_isearch" => zle_highlight_isearch = Some(decode_string(value)),

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
            "zle_highlight_paste" => zle_highlight_paste = Some(decode_string(value)),

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
    if client_version.is_none_or(|v| v != PROTOCOL_VERSION) {
        // Return immediately. This will close the connection with an empty
        // response.
        log::warn!(
            "Client version is {client_version:?}. Expected protocol version is {PROTOCOL_VERSION}."
        );
        return Ok(());
    }

    // handle "hello" command — respond with daemon version
    if cmd == Some("hello") {
        stream
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
    let result = highlighter.highlight(
        &lines,
        Some(pre_buffer_total_len + cursor),
        pwd.as_deref(),
        |range| {
            // skip spans in the pre-buffer
            if range.end <= pre_buffer_total_len {
                return false;
            }

            // subtract pre-buffer offset
            let start = range.start.saturating_sub(pre_buffer_total_len);
            let end = range.end.saturating_sub(pre_buffer_total_len);

            // skip spans outside the current terminal window
            start < max && end > min
        },
    )?;

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
                        .filter_map(|c| {
                            let fss = format!("{}:{}", c.0, format_static_style(&c.1));
                            if fss.is_empty() { None } else { Some(fss) }
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
                            encode_string(parsed_callable),
                            encode_string(all_fss)
                        ))
                    }
                }
            },
        };

        if let Some(message) = message {
            log::trace!("Writing response: {message}");
            stream
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
        &mut stream,
    )?;
    add_zle_highlight(
        suffix_active,
        suffix_start,
        suffix_end,
        zle_highlight_suffix,
        "bold",
        &mut stream,
    )?;
    add_zle_highlight(
        isearch_active,
        isearch_start,
        isearch_end,
        zle_highlight_isearch,
        "underline",
        &mut stream,
    )?;
    add_zle_highlight(
        yank_active,
        yank_start,
        yank_end,
        zle_highlight_paste,
        "standout",
        &mut stream,
    )?;

    Ok(())
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
            version: PROTOCOL_VERSION,
        };

        let mut s = stdout().lock();
        s.write_all(template.render().unwrap().as_bytes())?;
        s.flush()?;
    }

    if already_running {
        // Check the currently running daemon's version. Restart the it if the
        // versions don't match.
        let socket_path = sock_path(runtime_dir);
        let mut stream = UnixStream::connect(&socket_path)?;

        let timeout = Duration::from_secs(2);
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;

        let header = format!(
            "ver={PROTOCOL_VERSION} cmd=hello buffer_line_count=0 pre_buffer_line_count=0\n"
        );
        stream.write_all(header.as_bytes())?;

        let mut response = String::new();
        let mut reader = BufReader::new(&stream);
        reader.read_line(&mut response)?;

        let daemon_version = response
            .split_ascii_whitespace()
            .find_map(|kv| kv.strip_prefix("ver="));
        let our_version = env!("CARGO_PKG_VERSION");

        if daemon_version.is_none_or(|v| v != our_version) {
            // restart daemon
            stop_daemon(runtime_dir);
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
    let pid_file = pid_path(runtime_dir);

    if let Some(pid) = read_pid(&pid_file)
        && pid_alive(pid)
    {
        if no_daemon {
            println!("Daemon is already running. PID {pid}.");
        }

        // daemon is already running
        return Ok((Role::Parent, true));
    }

    // Make sure the data directory exists
    fs::create_dir_all(runtime_dir).context("Unable to create data directory")?;

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

        // close all file descriptors so we're really decoupled from the parent
        // process
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
    let my_pid = process::id();
    fs::write(&pid_file, format!("{my_pid}\n"))
        .with_context(|| format!("Unable to write PID file {pid_file:?}"))?;

    // Set read/write permissions and protect PID file from being deleted by
    // periodic cleanup (https://specifications.freedesktop.org/basedir/latest/).
    fs::set_permissions(&pid_file, Permissions::from_mode(0o1600))
        .with_context(|| format!("Unable to set permissions of {pid_file:?}"))?;

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
        let _ = init_highlighter.highlight("echo Welcome to zsh-patina!", None, None, |_| true);
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

    let _ = fs::remove_file(pid_file);
    let _ = fs::remove_file(socket_path);

    Ok((Role::Daemon, false))
}

pub fn stop_daemon(runtime_dir: &Path) {
    let pid_file = pid_path(runtime_dir);
    if let Some(pid) = read_pid(&pid_file)
        && pid_alive(pid)
    {
        // SAFETY: `pid` is known to be running. SIGTERM is a valid signal
        // number.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };

        let _ = fs::remove_file(pid_file);
        let _ = fs::remove_file(sock_path(runtime_dir));
    }
}

pub fn is_daemon_running(runtime_dir: &Path) -> Option<u32> {
    let pid_file = pid_path(runtime_dir);
    if let Some(pid) = read_pid(&pid_file)
        && pid_alive(pid)
    {
        Some(pid)
    } else {
        None
    }
}

pub fn status_daemon(runtime_dir: &Path) -> Result<()> {
    if let Some(pid) = is_daemon_running(runtime_dir) {
        println!("Daemon is running. PID {pid}.");
        Ok(())
    } else {
        bail!("Daemon is stopped.");
    }
}
