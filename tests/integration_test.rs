//! The tests in this module require Docker. They are ignored by default. Run
//! them with:
//!
//! ```shell
//! cargo test -- --ignored
//! ```
//!
//! Be aware that the first run may take a few minutes as the Docker image is
//! built.
use std::path::Path;
use std::sync::LazyLock;

use pretty_assertions::assert_eq;
use tempfile::NamedTempFile;
use testcontainers::{
    GenericBuildableImage, GenericImage, ImageExt,
    core::{
        BuildImageOptions, Mount, WaitFor, logs::consumer::logging_consumer::LoggingConsumer,
        wait::ExitWaitStrategy,
    },
    runners::{AsyncBuilder, AsyncRunner},
};
use tokio::fs;
use tokio::sync::OnceCell;

const DYNAMIC_CALLABLE_ALIAS: &str = "dynamic.callable.alias.shell";
const DYNAMIC_CALLABLE_BUILTIN: &str = "dynamic.callable.builtin.shell";
const DYNAMIC_CALLABLE_COMMAND: &str = "dynamic.callable.command.shell";
const DYNAMIC_CALLABLE_MISSING: &str = "dynamic.callable.missing.shell";
const DYNAMIC_PATH_DIRECTORY_COMPLETE: &str = "dynamic.path.directory.complete.shell";
const DYNAMIC_PATH_FILE_COMPLETE: &str = "dynamic.path.file.complete.shell";
const ARGUMENTS: &str = "meta.function-call.arguments.shell";
const KEYWORD_TIME: &str = "keyword.control.flow.time.shell";
const OPERATOR_LOGICAL_AND: &str = "keyword.operator.logical.and.shell";
const STRING_QUOTED_DOUBLE: &str = "string.quoted.double.shell";
const PUNCTUATION_PARAMETER: &str = "punctuation.definition.parameter.shell";
const PUNCTUATION_STRING_BEGIN: &str = "punctuation.definition.string.begin.shell";
const PUNCTUATION_STRING_END: &str = "punctuation.definition.string.end.shell";
const PARAMETER: &str = "variable.parameter.option.shell";
const FUNCTION: &str = "variable.function.shell";
const TILDE: &str = "variable.language.tilde.shell";

static TEST_THEME: LazyLock<toml::Table> = LazyLock::new(|| {
    include_str!(concat!(env!("OUT_DIR"), "/test_theme.toml"))
        .parse()
        .expect("test_theme.toml must be valid TOML")
});
static IMAGE: OnceCell<GenericImage> = OnceCell::const_new();

/// Look up scopes in the test theme and return a region_highlight entry for the
/// given range, combining their foreground and background styles if needed.
fn h<const N: usize>(start: usize, end: usize, scopes: [&str; N]) -> String {
    let styles = scopes
        .map(|scope| {
            let value = TEST_THEME
                .get(scope)
                .unwrap_or_else(|| panic!("scope '{scope}' not found in test_theme.toml"));
            match value {
                toml::Value::Integer(n) => format!("fg={n}"),
                toml::Value::Table(t) => {
                    let bg = t["background"]
                        .as_integer()
                        .expect("background must be an integer");
                    format!("bg={bg}")
                }
                _ => panic!("unexpected TOML value type for scope '{scope}'"),
            }
        })
        .join(",");
    format!("{start} {end} {styles}")
}

/// Format a command line with the given spans applying xterm 256 color codes
/// (using short forms where possible)
fn xterm_256color<const N: usize>(command_line: &str, spans: [(usize, usize, &str); N]) -> Vec<u8> {
    #[derive(PartialEq, Eq, PartialOrd, Ord)]
    #[repr(u8)]
    enum Event {
        EndBg = 0,
        EndFg = 1,
        StartFg(i64) = 2,
        StartBg(i64) = 3,
    }

    let mut events = Vec::new();
    for (start, end, scope) in spans {
        let value = TEST_THEME
            .get(scope)
            .unwrap_or_else(|| panic!("scope '{scope}' not found in test_theme.toml"));
        match value {
            toml::Value::Integer(n) => {
                events.push((start, Event::StartFg(*n)));
                events.push((end, Event::EndFg));
            }
            toml::Value::Table(t) => {
                let n = t["background"]
                    .as_integer()
                    .expect("background must be an integer");
                events.push((start, Event::StartBg(n)));
                events.push((end, Event::EndBg));
            }
            _ => panic!("unexpected TOML value type for scope '{scope}'"),
        }
    }

    // sort by index, then close all end events first (bg before fg) and then
    // open all start events (fg before bg)
    events.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    let bytes = command_line.as_bytes();
    let mut result = Vec::new();
    let mut pos = 0;

    for (i, e) in events {
        while pos < i {
            result.push(bytes[pos]);
            pos += 1;
        }

        match e {
            Event::EndBg => result.extend(b"\x1B[49m"),
            Event::EndFg => result.extend(b"\x1B[39m"),
            Event::StartFg(n) => {
                let f = match n {
                    0..=7 => format!("\x1B[3{}m", n),      // normal colors
                    8..=15 => format!("\x1B[9{}m", n - 8), // bright colors
                    _ => format!("\x1B[38;5;{n}m"),        // 256-color form
                };
                result.extend(f.as_bytes());
            }
            Event::StartBg(n) => {
                let f = match n {
                    0..=7 => format!("\x1B[4{}m", n),       // normal colors
                    8..=15 => format!("\x1B[10{}m", n - 8), // bright colors
                    _ => format!("\x1B[48;5;{n}m"),         // 256-color form
                };
                result.extend(f.as_bytes());
            }
        }
    }

    result.extend(&bytes[pos..]);

    result
}

/// Common setup code required by every test in this module
async fn setup() -> &'static GenericImage {
    IMAGE.get_or_init(build_image).await
}

async fn build_image() -> GenericImage {
    let _ = env_logger::try_init();

    if std::env::var_os("USE_PREBUILT_IMAGE").is_none() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let profile = if cfg!(debug_assertions) {
            "dev"
        } else {
            "release"
        };

        // Build Docker image - may take a few minutes on the first run. It's a
        // shame that testcontainers doesn't support .dockerignore, so we have
        // to pass in all required files and directories manually. If the build
        // fails and you only get a cryptic error message, run the test with:
        //
        // RUST_LOG=debug cargo test -- --ignored --no-capture
        //
        // This will give you debug output from bollard. Unfortunately, all
        // build messages are base64 encoded. You have to decode them manually
        // to find the relevant error message.
        GenericBuildableImage::new(format!("michelkraemer/zsh-patina-test-{profile}"), "latest")
            .with_dockerfile(manifest_dir.join("tests/Dockerfile"))
            .with_file(manifest_dir.join("Cargo.toml"), "Cargo.toml")
            .with_file(manifest_dir.join("Cargo.lock"), "Cargo.lock")
            .with_file(manifest_dir.join("build.rs"), "build.rs")
            .with_file(manifest_dir.join("src"), "src")
            .with_file(manifest_dir.join("assets"), "assets")
            .with_file(manifest_dir.join("templates"), "templates")
            .with_file(manifest_dir.join("themes"), "themes")
            .with_file(manifest_dir.join("askama.toml"), "askama.toml")
            .build_image_with(BuildImageOptions::new().with_build_arg("PROFILE", profile))
            .await
            .expect("failed to build Docker image")
    } else {
        // use existing pre-built image on CI server
        GenericImage::new("michelkraemer/zsh-patina-test-dev", "latest")
    }
}

/// Runs zsh-patina in a container and highlights the given buffer. Compares
/// `$region_highlight` to the expected result. There may be two separate pre-
/// and post- setup steps that run before and after sourcing `zsh-patina
/// activate`, respectively. This method will somewhat emulate human user
/// behavior, in entering most of the buffer first, then "typing out" its last
/// character afterwards.
async fn run_highlight(setup_pre: &[&str], setup_post: &[&str], buffer: &str, expected: &[String]) {
    let before_activate = if setup_pre.is_empty() {
        String::new()
    } else {
        format!("{}; ", setup_pre.join("; "))
    };
    let after_activate = if setup_post.is_empty() {
        String::new()
    } else {
        format!("{}; ", setup_post.join("; "))
    };

    // emulate human keystrokes triggering successive highlightings
    let previous_buffer = buffer
        .char_indices()
        .next_back()
        .map(|(idx, _)| &buffer[..idx])
        .unwrap_or("");

    // Trigger highlighting repeatedly in a loop until the daemon has fully
    // started and $region_highlight is actually filled
    let highlight_loop = "CURSOR=${#BUFFER}; for i in {1..300}; do _zsh_patina; [[ ${#region_highlight[@]} -gt 0 ]] && break; sleep 0.1; done;";
    let zsh_script = format!(
        r#"{before_activate}
        eval "$(zsh-patina activate)"
        typeset -g -a region_highlight  # ensure region_highlight exists globally
        BUFFER="{previous_buffer}"; {highlight_loop}
        {after_activate}
        BUFFER="{buffer}"; {highlight_loop}
        printf '%s\n' "${{region_highlight[@]}}""#
    );

    let (stdout_bytes, exit_code) = run_container(zsh_script).await;

    let stdout = std::str::from_utf8(&stdout_bytes)
        .expect("stdout is not valid UTF-8")
        .to_string();

    let lines = stdout
        .lines()
        .map(|l| {
            l.strip_suffix(" memo=zsh_patina")
                .expect("region_highlight entry must end with ` memo=zsh_patina'")
        })
        .collect::<Vec<_>>();

    assert_eq!(lines, expected);
    assert_eq!(exit_code, 0);
}

/// Runs the `highlight` subcommand in a container using the given arguments and
/// stdin. Return stdout and the subcommand's exit code.
async fn run_highlight_subcommand(
    setup_pre: &[&str],
    args: &[&str],
    stdin: &str,
) -> (Vec<u8>, i64) {
    let before_activate = if setup_pre.is_empty() {
        String::new()
    } else {
        format!("{}; ", setup_pre.join("; "))
    };

    // Wait for the daemon to come up by polling until `zsh-patina status`
    // succeeds. Without this, the `highlight` subcommand will render the
    // unhighlighted input (see the command's documentation).
    let wait_daemon =
        "for i in {1..300}; do zsh-patina status >/dev/null 2>&1 && break; sleep 0.1; done;";

    let zsh_script = format!(
        r#"{before_activate}
        eval "$(zsh-patina activate)"
        {wait_daemon}
        export TERM=xterm-256color  # make sure the command's output is colored
        zsh-patina highlight {} <<'EOF'
{stdin}
EOF"#,
        args.join(" ")
    );

    run_container(zsh_script).await
}

/// Runs the `highlight` subcommand in a container using the given arguments and
/// stdin. Compares stdout with the given expected bytes.
async fn assert_highlight_subcommand(
    setup_pre: &[&str],
    args: &[&str],
    stdin: &str,
    expected: &[u8],
) {
    let (stdout_bytes, exit_code) = run_highlight_subcommand(setup_pre, args, stdin).await;

    if stdout_bytes != expected {
        // print stdout in human-readable form for better debugging
        eprintln!(
            "stdout: {}",
            stdout_bytes
                .iter()
                .map(|&b| if b >= 32 {
                    (b as char).to_string()
                } else {
                    format!("\\x{b:0>2x}")
                })
                .collect::<Vec<_>>()
                .join("")
        );
    }
    assert_eq!(stdout_bytes, expected);
    assert_eq!(exit_code, 0);
}

/// Runs a given zsh script in a Docker image. Mounts a zsh-patina configuration
/// and the test theme into the container.
async fn run_container(zsh_script: String) -> (Vec<u8>, i64) {
    let image = setup().await;

    let config = "[highlighting]\ntheme = \"file:/root/.config/zsh-patina/test_theme.toml\"";
    let config_file = NamedTempFile::new().expect("Unable to create temporary config file");
    fs::write(&config_file, config)
        .await
        .expect("Unable to write to temporary config file");

    let container = image
        .clone()
        .with_wait_for(WaitFor::Exit(ExitWaitStrategy::default()))
        .with_mount(Mount::bind_mount(
            config_file.path().to_string_lossy(),
            "/root/.config/zsh-patina/config.toml",
        ))
        .with_mount(Mount::bind_mount(
            concat!(env!("OUT_DIR"), "/test_theme.toml"),
            "/root/.config/zsh-patina/test_theme.toml",
        ))
        .with_cmd(["unbuffer", "zsh", "-c", &zsh_script])
        .with_log_consumer(LoggingConsumer::new())
        .start()
        .await
        .expect("failed to start container");

    let stdout = container
        .stdout_to_vec()
        .await
        .expect("failed to read stdout");
    let exit_code = container
        .exit_code()
        .await
        .expect("failed to read exit code")
        .unwrap();

    (stdout, exit_code)
}

/// Test if a simple `ls -l` command is highlighted correctly
#[tokio::test]
#[ignore]
async fn ls_with_option() {
    run_highlight(
        &[],
        &[],
        "ls -l",
        &[
            h(0, 2, [DYNAMIC_CALLABLE_COMMAND]),
            h(2, 4, [PUNCTUATION_PARAMETER]),
            h(4, 5, [PARAMETER]),
        ],
    )
    .await;
}

/// Test if a named directory created with `hash -d` is resolved by Zsh.
#[tokio::test]
#[ignore]
async fn named_directory() {
    run_highlight(
        &[
            "mkdir -p /tmp/named",
            "touch /tmp/named/test.txt",
            "printf '#!/bin/sh\n' > /tmp/named/test.sh",
            "chmod u+x /tmp/named/test.sh",
            "hash -d named=/tmp/named",
        ],
        &[],
        "ls ~named/test.txt",
        &[
            h(0, 2, [DYNAMIC_CALLABLE_COMMAND]),
            h(2, 3, [ARGUMENTS]),
            h(3, 4, [TILDE, DYNAMIC_PATH_FILE_COMPLETE]),
            h(4, 18, [ARGUMENTS, DYNAMIC_PATH_FILE_COMPLETE]),
        ],
    )
    .await;

    run_highlight(
        &[
            "mkdir -p /tmp/named",
            "printf '#!/bin/sh\n' > /tmp/named/test.sh",
            "chmod +x /tmp/named/test.sh",
            "hash -d named=/tmp/named",
        ],
        &[],
        "~named/test.sh",
        &[
            h(0, 1, [TILDE, DYNAMIC_CALLABLE_COMMAND]),
            h(1, 14, [FUNCTION, DYNAMIC_CALLABLE_COMMAND]),
        ],
    )
    .await;

    run_highlight(
        &[],
        &[],
        "ls ~missing/test.txt",
        &[
            h(0, 2, [DYNAMIC_CALLABLE_COMMAND]),
            h(2, 3, [ARGUMENTS]),
            h(3, 4, [TILDE]),
            h(4, 20, [ARGUMENTS]),
        ],
    )
    .await;
}

/// Test if aliases are resolved correctly
#[tokio::test]
#[ignore]
async fn resolve_alias() {
    // simple alias
    run_highlight(
        &["alias ll='ls -l'"],
        &[],
        "ll -a",
        &[
            h(0, 2, [DYNAMIC_CALLABLE_ALIAS]),
            h(2, 4, [PUNCTUATION_PARAMETER]),
            h(4, 5, [PARAMETER]),
        ],
    )
    .await;

    // alias with a subshell
    run_highlight(
        &["alias ll='(ls -l)'"],
        &[],
        "ll -a",
        &[
            h(0, 2, [DYNAMIC_CALLABLE_ALIAS]),
            h(2, 4, [PUNCTUATION_PARAMETER]),
            h(4, 5, [PARAMETER]),
        ],
    )
    .await;

    // alias referencing another alias
    run_highlight(
        &["alias lla='ll -a'", "alias ll='ls -l'"],
        &[],
        "lla && ll",
        &[
            h(0, 3, [DYNAMIC_CALLABLE_ALIAS]),
            h(4, 6, [OPERATOR_LOGICAL_AND]),
            h(7, 9, [DYNAMIC_CALLABLE_ALIAS]),
        ],
    )
    .await;

    // alias referencing a command that does not exist
    run_highlight(
        &["alias fb=foobar"],
        &[],
        "fb",
        &[h(0, 2, [DYNAMIC_CALLABLE_MISSING])],
    )
    .await;

    // alias referencing two commands
    run_highlight(
        &["alias foobar='ls -l && echo OK'"],
        &[],
        "foobar",
        &[h(0, 6, [DYNAMIC_CALLABLE_ALIAS])],
    )
    .await;

    // alias referencing two commands, but the second one does not exist
    run_highlight(
        &["alias foobar='ls -l && missing OK'"],
        &[],
        "foobar",
        &[h(0, 6, [DYNAMIC_CALLABLE_MISSING])],
    )
    .await;

    // cycle: alias referencing another alias referencing the first one again
    run_highlight(
        &[
            "alias fb='foobar --option'",
            "alias foobar='fb --another-option'",
        ],
        &[],
        "fb",
        &[h(0, 2, [DYNAMIC_CALLABLE_MISSING])],
    )
    .await;

    // self-referencing alias (not a cycle!)
    run_highlight(
        &["alias grep='grep --color'"],
        &[],
        "grep",
        &[h(0, 4, [DYNAMIC_CALLABLE_ALIAS])],
    )
    .await;

    // valid: grep points to the alias g, and g then points to the command grep
    // invalid: g points to the alias grep, and grep then points to the missing command g
    run_highlight(
        &["alias grep='g --color'", "alias g='grep'"],
        &[],
        "grep && g",
        &[
            h(0, 4, [DYNAMIC_CALLABLE_ALIAS]),
            h(5, 7, [OPERATOR_LOGICAL_AND]),
            h(8, 9, [DYNAMIC_CALLABLE_MISSING]),
        ],
    )
    .await;

    // valid: the alias grep points to the command grep
    // valid: g points to the alias grep, which points to the command grep
    run_highlight(
        &["alias grep='grep --color'", "alias g='grep'"],
        &[],
        "grep && g",
        &[
            h(0, 4, [DYNAMIC_CALLABLE_ALIAS]),
            h(5, 7, [OPERATOR_LOGICAL_AND]),
            h(8, 9, [DYNAMIC_CALLABLE_ALIAS]),
        ],
    )
    .await;
}

/// Test if a command created in an existing PATH entry after activation is
/// highlighted correctly.
#[tokio::test]
#[ignore]
async fn command_created_after_activation_in_existing_path_entry() {
    run_highlight(
        &["mkdir -p /tmp/bin", "export PATH=/tmp/bin:$PATH"],
        &[
            "printf '#!/bin/sh\n' > /tmp/bin/freshcmd",
            "chmod +x /tmp/bin/freshcmd",
        ],
        "freshcmd",
        &[h(0, 8, [DYNAMIC_CALLABLE_COMMAND])],
    )
    .await;
}

/// Use the `highlight` subcommand to highlight a simple command
#[tokio::test]
#[ignore]
async fn highlight_subcommand_simple_command() {
    assert_highlight_subcommand(
        &[],
        &[],
        "time ls",
        &xterm_256color(
            "time ls\n",
            [(0, 4, KEYWORD_TIME), (5, 7, DYNAMIC_CALLABLE_COMMAND)],
        ),
    )
    .await;
}

/// Use the `highlight` subcommand to highlight a command with a directory
#[tokio::test]
#[ignore]
async fn highlight_subcommand_command_with_dir() {
    assert_highlight_subcommand(
        &[],
        &[],
        "ls /test",
        &xterm_256color(
            "ls /test\n",
            [(0, 2, DYNAMIC_CALLABLE_COMMAND), (2, 8, ARGUMENTS)],
        ),
    )
    .await;

    assert_highlight_subcommand(
        &["mkdir /test"],
        &[],
        "ls /test",
        &xterm_256color(
            "ls /test\n",
            [
                (0, 2, DYNAMIC_CALLABLE_COMMAND),
                (2, 3, ARGUMENTS),
                (3, 8, ARGUMENTS),
                (3, 8, DYNAMIC_PATH_DIRECTORY_COMPLETE),
            ],
        ),
    )
    .await;
}

/// Use the `highlight` subcommand and highlight a command line from a file
#[tokio::test]
#[ignore]
async fn highlight_subcommand_from_file() {
    assert_highlight_subcommand(
        &["echo 'echo \"Hello world\"' > /tmp/command-line.zsh"],
        &["/tmp/command-line.zsh"],
        "",
        &xterm_256color(
            "echo \"Hello world\"\n",
            [
                (0, 4, DYNAMIC_CALLABLE_BUILTIN),
                (4, 5, ARGUMENTS),
                (5, 6, PUNCTUATION_STRING_BEGIN),
                (6, 17, STRING_QUOTED_DOUBLE),
                (17, 18, PUNCTUATION_STRING_END),
            ],
        ),
    )
    .await;

    assert_highlight_subcommand(
        &["echo 'echo OK' > -h"],
        &["--", "-h"],
        "",
        &xterm_256color(
            "echo OK\n",
            [(0, 4, DYNAMIC_CALLABLE_BUILTIN), (4, 7, ARGUMENTS)],
        ),
    )
    .await;
}

/// Test that the `highlight` subcommand fails if an input file does not exist
#[tokio::test]
#[ignore]
async fn highlight_subcommand_nonexistent_input_file() {
    let (stdout, exit_code) = run_highlight_subcommand(&[], &["/tmp/command-line.zsh"], "").await;
    assert_eq!(exit_code, 1);
    assert_eq!(
        std::str::from_utf8(&stdout).unwrap(),
        "\x1B[31;1mzsh-patina:\x1B[0m Failed to read file: '/tmp/command-line.zsh'\n",
    );
}

/// Test that the `highlight` subcommand fails if the input file is a directory
#[tokio::test]
#[ignore]
async fn highlight_subcommand_input_is_dir() {
    let (stdout, exit_code) = run_highlight_subcommand(&["mkdir /temp"], &["/temp"], "").await;
    assert_eq!(exit_code, 1);
    assert_eq!(
        std::str::from_utf8(&stdout).unwrap(),
        "\x1B[31;1mzsh-patina:\x1B[0m Failed to read file: '/temp'\n",
    );
}

/// Test that the `highlight` subcommand shows the help
#[tokio::test]
#[ignore]
async fn highlight_subcommand_help() {
    let (stdout, exit_code) = run_highlight_subcommand(&[], &["-h"], "").await;
    assert_eq!(exit_code, 0);
    assert!(std::str::from_utf8(&stdout).unwrap().contains("Usage:"));

    let (stdout, exit_code) = run_highlight_subcommand(&[], &["--help"], "").await;
    assert_eq!(exit_code, 0);
    assert!(std::str::from_utf8(&stdout).unwrap().contains("Usage:"));

    // show help even if there is a file given
    let (stdout, exit_code) = run_highlight_subcommand(&[], &["-h", "file.txt"], "").await;
    assert_eq!(exit_code, 0);
    assert!(std::str::from_utf8(&stdout).unwrap().contains("Usage:"));

    // show help even if there is a file given
    let (stdout, exit_code) = run_highlight_subcommand(&[], &["-h", "--", "file.txt"], "").await;
    assert_eq!(exit_code, 0);
    assert!(std::str::from_utf8(&stdout).unwrap().contains("Usage:"));

    let (stdout, exit_code) = run_highlight_subcommand(&[], &["-a"], "").await;
    assert_eq!(exit_code, 1);
    assert!(
        std::str::from_utf8(&stdout)
            .unwrap()
            .contains("unexpected argument"),
    );

    let (stdout, exit_code) = run_highlight_subcommand(&[], &["file1", "file2"], "").await;
    assert_eq!(exit_code, 1);
    assert!(
        std::str::from_utf8(&stdout)
            .unwrap()
            .contains("unexpected argument"),
    );

    let (stdout, exit_code) = run_highlight_subcommand(&[], &["file1", "--", "file2"], "").await;
    assert_eq!(exit_code, 1);
    assert!(
        std::str::from_utf8(&stdout)
            .unwrap()
            .contains("unexpected argument"),
    );
}
