use anyhow::{Context, Result, bail};
use askama::Template;
use rayon::ThreadPoolBuilder;
use std::{
    fs,
    io::{BufRead, BufReader, Write, stdout},
    os::{
        fd::AsRawFd,
        unix::net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process,
    sync::Arc,
};

use crate::{
    Config,
    check::check_config,
    highlighting::{DynamicStyle, Highlighter, Span, SpanStyle, StaticStyle},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Parent,
    Child,
    Daemon,
}

#[derive(Template)]
#[template(path = "zsh-patina.zsh")]
struct ActivateTemplate {
    zsh_patina_path: String,
}

fn pid_path(data_dir: &Path) -> PathBuf {
    data_dir.join("daemon.pid")
}

fn sock_path(data_dir: &Path) -> PathBuf {
    data_dir.join("daemon.sock")
}

/// Read the PID from the PID file. Returns `None` if the file does not exist or
/// contains garbage.
fn read_pid(pid_file: &Path) -> Option<u32> {
    fs::read_to_string(pid_file).ok()?.trim().parse().ok()
}

/// Check whether a process with the given PID is currently alive.
fn pid_alive(pid: u32) -> bool {
    // kill(pid, 0) returns 0 if the process exists and we have permission to
    // signal it
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Convert a static style to the format that Zsh's `region_highlight` uses
fn format_static_style(style: &StaticStyle) -> String {
    let mut result = String::new();
    if let Some(fg) = &style.foreground_color {
        result.push_str(&format!("fg={}", fg));
    }
    if let Some(bg) = &style.background_color {
        if !result.is_empty() {
            result.push(',');
        }
        result.push_str(&format!("bg={}", bg));
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

fn encode_string(input: &str) -> String {
    // Fast path: no encoding needed
    if !input
        .bytes()
        .any(|b| matches!(b, b'%' | b' ' | b'\t' | b'\n' | b'\r' | b'\x0C'))
    {
        return input.to_owned();
    }

    let mut out = String::with_capacity(input.len() + input.len() / 4);
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

fn handle_connection(mut stream: UnixStream, highlighter: Arc<Highlighter>) -> Result<()> {
    let mut reader = BufReader::new(&stream);

    // read number of lines
    let mut header = String::new();
    reader
        .read_line(&mut header)
        .context("Unable to read header")?;
    let mut term_cols = 1000;
    let mut term_rows = 1000;
    let mut cursor = 0;
    let mut pre_buffer_line_count = 0;
    let mut buffer_line_count = 0;
    let mut pwd = None;
    for h in header.split_ascii_whitespace() {
        let (key, value) = h
            .split_once("=")
            .context("Unable to split header key-value pair")?;
        match key {
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
            "pwd" => {
                pwd = Some(decode_string(value));
            }
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

    // read lines
    let mut total_len = 0;
    let mut line_lengths = Vec::new();
    let mut cursor_line = 0;
    for i in 0..buffer_line_count {
        let mut line = String::new();
        reader.read_line(&mut line).context("Unable to read line")?;
        lines.push_str(&line);

        // this is O(n) but necessary in case the command contains
        // multi-byte characters
        let line_len = line.chars().count();

        // determine in which line we are currently
        if (total_len..total_len + line_len).contains(&cursor) {
            cursor_line = i;
        }

        line_lengths.push(line_len);
        total_len += line_len;
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
    let result = highlighter.highlight(&lines, pwd.as_deref(), |range| {
        // skip spans in the pre-buffer
        if range.end <= pre_buffer_total_len {
            return false;
        }

        // subtract pre-buffer offset
        let start = range.start.saturating_sub(pre_buffer_total_len);
        let end = range.end.saturating_sub(pre_buffer_total_len);

        // skip spans outside the current terminal window
        start < max && end > min
    })?;

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
                            encode_string(&parsed_callable),
                            encode_string(&all_fss)
                        ))
                    }
                }
            },
        };

        if let Some(message) = message {
            stream
                .write_all(message.as_bytes())
                .context("Unable to send response")?;
        }
    }

    Ok(())
}

pub fn activate(data_dir: &Path, config: &Config) -> Result<()> {
    check_config(config)?;

    if start_daemon_internal(data_dir, config)? == Role::Parent {
        let exe = std::env::current_exe()?;

        let template = ActivateTemplate {
            zsh_patina_path: exe.to_str().unwrap().to_string(),
        };

        let mut s = stdout().lock();
        s.write_all(template.render().unwrap().as_bytes())?;
        s.flush()?;
    }

    Ok(())
}

pub fn start_daemon(data_dir: &Path, config: &Config) -> Result<()> {
    start_daemon_internal(data_dir, config)?;
    Ok(())
}

fn start_daemon_internal(data_dir: &Path, config: &Config) -> Result<Role> {
    let pid_file = pid_path(data_dir);

    if let Some(pid) = read_pid(&pid_file)
        && pid_alive(pid)
    {
        // daemon is already running
        return Ok(Role::Parent);
    }

    // initialize highlighter
    let highlighter = Arc::new(Highlighter::new(&config.highlighting)?);

    // highlight something to make sure everything is loaded
    highlighter.highlight("echo Welcome to zsh-patina!", None, |_| true)?;

    // Make sure the data directory exists
    fs::create_dir_all(data_dir).context("Unable to create data directory")?;

    // Double-fork:
    //
    // Fork #1: the parent exits immediately so the `start` call returns at
    //          once. The child continues.
    //
    // setsid: the child becomes session leader, fully detached from the
    //         terminal and from Zsh's process group.
    //
    // Fork #2: the session-leader child forks again and exits.  The grandchild
    //          can never accidentally re-acquire a controlling terminal (POSIX
    //          guarantee).
    //
    // The grandchild is then adopted by PID 1 (init/systemd) and runs as a true
    // background daemon.

    // fork #1
    match unsafe { libc::fork() } {
        -1 => {
            bail!("fork #1 failed");
        }
        0 => {
            // child: continue below
        }
        _ => {
            // parent: return immediately
            return Ok(Role::Parent);
        }
    }

    // become session leader
    unsafe { libc::setsid() };

    // fork #2
    match unsafe { libc::fork() } {
        -1 => {
            bail!("fork #2 failed");
        }
        0 => {
            // grandchild
        }
        _ => {
            // intermediate child: exit
            return Ok(Role::Child);
        }
    }

    // from here on, we are a true background daemon ...

    // close all file descriptors so we're really decoupled from the parent
    // process
    unsafe {
        let devnull = std::fs::File::open("/dev/null").unwrap();
        libc::dup2(devnull.as_raw_fd(), libc::STDIN_FILENO);
        libc::dup2(devnull.as_raw_fd(), libc::STDOUT_FILENO);
        libc::dup2(devnull.as_raw_fd(), libc::STDERR_FILENO);
    }

    // write our PID so that `stop` and `status` can find us
    let my_pid = process::id();
    fs::write(&pid_file, format!("{my_pid}\n"))
        .with_context(|| format!("Unable to write PID file {pid_file:?}"))?;

    // clean up leftover socket
    let socket_path = sock_path(data_dir);
    let _ = fs::remove_file(&socket_path); // ignore errors

    // bind the Unix domain socket
    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("Unable to bind socket {socket_path:?}"))?;

    // accept connections
    let pool = ThreadPoolBuilder::new().num_threads(4).build().unwrap();
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let highlighter = Arc::clone(&highlighter);
                pool.spawn(|| {
                    // Handle connection and ignore any errors. Errors can
                    // happen in two cases:
                    // * We are unable to read the input. In this case, Zsh will
                    //   generate an error message while the user is typing
                    //   ("broken pipe")
                    // * We are unable to highlight the command or send a
                    //   response. In this case, `stream` will be dropped and
                    //   Zsh will just continue without highlighting.
                    let _ = handle_connection(stream, highlighter);
                });
            }
            _ => {
                break;
            }
        }
    }

    let _ = fs::remove_file(pid_file);
    let _ = fs::remove_file(socket_path);

    Ok(Role::Daemon)
}

pub fn stop_daemon(data_dir: &Path) -> Result<()> {
    let pid_file = pid_path(data_dir);
    if let Some(pid) = read_pid(&pid_file)
        && pid_alive(pid)
    {
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };

        let _ = fs::remove_file(pid_file);
        let _ = fs::remove_file(sock_path(data_dir));
    }
    Ok(())
}

pub fn status_daemon(data_dir: &Path) -> Result<()> {
    let pid_file = pid_path(data_dir);
    if let Some(pid) = read_pid(&pid_file)
        && pid_alive(pid)
    {
        println!("Daemon is running. PID {pid}.");
        Ok(())
    } else {
        bail!("Daemon is stopped");
    }
}
