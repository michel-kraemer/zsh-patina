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
    core::{BuildImageOptions, Mount, WaitFor, wait::ExitWaitStrategy},
    runners::{AsyncBuilder, AsyncRunner},
};
use tokio::fs;

const DYNAMIC_CALLABLE_ALIAS: &str = "dynamic.callable.alias.shell";
const DYNAMIC_CALLABLE_COMMAND: &str = "dynamic.callable.command.shell";
const DYNAMIC_CALLABLE_MISSING: &str = "dynamic.callable.missing.shell";
const OPERATOR_LOGICAL_AND: &str = "keyword.operator.logical.and.shell";
const PUNCTUATION_PARAMETER: &str = "punctuation.definition.parameter.shell";
const PARAMETER: &str = "variable.parameter.option.shell";

static TEST_THEME: LazyLock<toml::Table> = LazyLock::new(|| {
    include_str!(concat!(env!("OUT_DIR"), "/test_theme.toml"))
        .parse()
        .expect("test_theme.toml must be valid TOML")
});

/// Look up a scope name in the test theme and return a region_highlight entry
/// string for the given start/end positions. Foreground-only scopes produce
/// `"<start> <end> fg=<n>"`, background-only scopes produce
/// `"<start> <end> bg=<n>"`.
fn h(start: usize, end: usize, scope: &str) -> String {
    let value = TEST_THEME
        .get(scope)
        .unwrap_or_else(|| panic!("scope '{scope}' not found in test_theme.toml"));
    match value {
        toml::Value::Integer(n) => format!("{start} {end} fg={n}"),
        toml::Value::Table(t) => {
            let bg = t["background"]
                .as_integer()
                .expect("background must be an integer");
            format!("{start} {end} bg={bg}")
        }
        _ => panic!("unexpected TOML value type for scope '{scope}'"),
    }
}

/// Common setup code required by every test in this module
async fn setup() -> GenericImage {
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
/// `$region_highlight` to the expected result.
async fn run_highlight(
    image: &GenericImage,
    setup_commands: &[&str],
    buffer: &str,
    expected: &[String],
) {
    let setup = if setup_commands.is_empty() {
        String::new()
    } else {
        format!("{}; ", setup_commands.join("; "))
    };
    let zsh_script = format!(
        r#"eval "$(zsh-patina activate)"; {setup}
        BUFFER="{buffer}"; CURSOR=${{#BUFFER}}; for i in {{1..50}}; do _zsh_patina; [[ ${{#region_highlight[@]}} -gt 0 ]] && break; sleep 0.1; done;
        printf '%s\n' "${{region_highlight[@]}}""#
    );

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
        .start()
        .await
        .expect("failed to start container");

    let stdout_bytes = container
        .stdout_to_vec()
        .await
        .expect("failed to read stdout");
    let stdout = std::str::from_utf8(&stdout_bytes).expect("stdout is not valid UTF-8");

    let lines = stdout
        .lines()
        .map(|l| {
            l.strip_suffix(" memo=zsh_patina")
                .expect("region_highlight entry must end with ` memo=zsh_patina'")
        })
        .collect::<Vec<_>>();

    assert_eq!(lines, expected);
}

/// Test if a simple `ls -l` command is highlighted correctly
#[tokio::test]
#[ignore]
async fn ls_with_option() {
    let image = setup().await;
    run_highlight(
        &image,
        &[],
        "ls -l",
        &[
            h(0, 2, DYNAMIC_CALLABLE_COMMAND),
            h(2, 4, PUNCTUATION_PARAMETER),
            h(4, 5, PARAMETER),
        ],
    )
    .await;
}

/// Test if aliases are resolved correctly
#[tokio::test]
#[ignore]
async fn resolve_alias() {
    let image = setup().await;

    // simple alias
    run_highlight(
        &image,
        &["alias ll='ls -l'"],
        "ll -a",
        &[
            h(0, 2, DYNAMIC_CALLABLE_ALIAS),
            h(2, 4, PUNCTUATION_PARAMETER),
            h(4, 5, PARAMETER),
        ],
    )
    .await;

    // alias with a subshell
    run_highlight(
        &image,
        &["alias ll='(ls -l)'"],
        "ll -a",
        &[
            h(0, 2, DYNAMIC_CALLABLE_ALIAS),
            h(2, 4, PUNCTUATION_PARAMETER),
            h(4, 5, PARAMETER),
        ],
    )
    .await;

    // alias referencing another alias
    run_highlight(
        &image,
        &["alias lla='ll -a'", "alias ll='ls -l'"],
        "lla && ll",
        &[
            h(0, 3, DYNAMIC_CALLABLE_ALIAS),
            h(4, 6, OPERATOR_LOGICAL_AND),
            h(7, 9, DYNAMIC_CALLABLE_ALIAS),
        ],
    )
    .await;

    // alias referencing a command that does not exist
    run_highlight(
        &image,
        &["alias fb=foobar"],
        "fb",
        &[h(0, 2, DYNAMIC_CALLABLE_MISSING)],
    )
    .await;

    // alias referencing two commands
    run_highlight(
        &image,
        &["alias foobar='ls -l && echo OK'"],
        "foobar",
        &[h(0, 6, DYNAMIC_CALLABLE_ALIAS)],
    )
    .await;

    // alias referencing two commands, but the second one does not exist
    run_highlight(
        &image,
        &["alias foobar='ls -l && missing OK'"],
        "foobar",
        &[h(0, 6, DYNAMIC_CALLABLE_MISSING)],
    )
    .await;

    // cycle: alias referencing another alias referencing the first one again
    run_highlight(
        &image,
        &[
            "alias fb='foobar --option'",
            "alias foobar='fb --another-option'",
        ],
        "fb",
        &[h(0, 2, DYNAMIC_CALLABLE_MISSING)],
    )
    .await;

    // self-referencing alias (not a cycle!)
    run_highlight(
        &image,
        &["alias grep='grep --color'"],
        "grep",
        &[h(0, 4, DYNAMIC_CALLABLE_ALIAS)],
    )
    .await;

    // valid: grep points to the alias g, and g then points to the command grep
    // invalid: g points to the alias grep, and grep then points to the missing command g
    run_highlight(
        &image,
        &["alias grep='g --color'", "alias g='grep'"],
        "grep && g",
        &[
            h(0, 4, DYNAMIC_CALLABLE_ALIAS),
            h(5, 7, OPERATOR_LOGICAL_AND),
            h(8, 9, DYNAMIC_CALLABLE_MISSING),
        ],
    )
    .await;

    // valid: the alias grep points to the command grep
    // valid: g points to the alias grep, which points to the command grep
    run_highlight(
        &image,
        &["alias grep='grep --color'", "alias g='grep'"],
        "grep && g",
        &[
            h(0, 4, DYNAMIC_CALLABLE_ALIAS),
            h(5, 7, OPERATOR_LOGICAL_AND),
            h(8, 9, DYNAMIC_CALLABLE_ALIAS),
        ],
    )
    .await;
}
