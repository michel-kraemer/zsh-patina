use anyhow::{Context, Result, bail};
use rayon::ThreadPoolBuilder;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    process,
    sync::Arc,
};

use crate::highlighter::Highlighter;

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
    let mut line_count = 0;
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
            "line_count" => {
                line_count = value
                    .parse::<usize>()
                    .context("Unable to parse number of lines")?;
            }
            _ => {}
        }
    }

    // read lines
    let mut lines = String::new();
    let mut total_len = 0;
    let mut line_lengths = Vec::new();
    let mut cursor_line = 0;
    for i in 0..line_count {
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

    // write response
    let result = highlighter.highlight(&lines);
    for s in result.iter() {
        if s.end <= min || s.start >= max {
            continue;
        }
        stream
            .write_all(format!("{} {} fg={}\n", s.start, s.end, s.foreground_color).as_bytes())
            .context("Unable to send response")?;
    }

    Ok(())
}

pub fn start_daemon(data_dir: &Path) -> Result<()> {
    let pid_file = pid_path(data_dir);

    if let Some(pid) = read_pid(&pid_file)
        && pid_alive(pid)
    {
        // daemon is already running
        return Ok(());
    }

    // initialize highlighter
    let highlighter = Arc::new(Highlighter::new());

    // Make sure the data directory exists
    fs::create_dir_all(data_dir).context("Unable to create data directory")?;

    // Double-fork:
    //
    // Fork #1: the parent exits immediately so the `start` call returns at
    //          once. The child continues.
    //
    // setsid: the child becomes session leader, fully detached from the
    //         terminal and from ZSH's process group.
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
            return Ok(());
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
            return Ok(());
        }
    }

    // from here on, we are a true background daemon ...

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
    let pool = ThreadPoolBuilder::new().num_threads(0).build().unwrap();
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let highlighter = Arc::clone(&highlighter);
                pool.spawn(|| {
                    // Handle connection and ignore any errors. Errors can
                    // happen in two cases:
                    // * We are unable to read the input. In this case, ZSH will
                    //   generate an error message while the user is typing
                    //   ("broken pipe")
                    // * We are unable to highlight the command or send a
                    //   response. In this case, `stream` will be dropped and
                    //   ZSH will just continue without highlighting.
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

    Ok(())
}

pub fn stop_daemon(data_dir: &Path) -> Result<()> {
    let pid_file = pid_path(data_dir);
    if let Some(pid) = read_pid(&pid_file)
        && pid_alive(pid)
    {
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };

        let _ = fs::remove_file(pid_file);
        let _ = fs::remove_file(sock_path(data_dir));

        Ok(())
    } else {
        bail!("Daemon is not running")
    }
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
