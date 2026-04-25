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

use pretty_assertions::assert_eq;
use testcontainers::{
    GenericBuildableImage, GenericImage, ImageExt,
    core::{BuildImageOptions, WaitFor, wait::ExitWaitStrategy},
    runners::{AsyncBuilder, AsyncRunner},
};

/// Common setup code required by every test in this module
async fn setup() -> GenericImage {
    // emit bollard tracing events to stdout
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let profile = if cfg!(debug_assertions) {
        "dev"
    } else {
        "release"
    };

    // Build Docker image - may take a few minutes on the first run. It's a
    // shame that testcontainers doesn't support .dockerignore, so we have to
    // pass in all required files and directories manually. If the build fails
    // and you only get a cryptic error message, run the test with:
    //
    // RUST_LOG=debug cargo test -- --ignored --no-capture
    //
    // This will give you debug output from bollard. Unfortunately, all build
    // messages are base64 encoded. You have to decode them manually to find the
    // relevant error message.
    GenericBuildableImage::new("michelkraemer/zsh-patina-test", "latest")
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
}

/// Runs zsh-patina in a container and highlights the given buffer. Compares
/// `$region_highlight` to the expected result.
async fn run_highlight(
    image: &GenericImage,
    setup_commands: &[&str],
    buffer: &str,
    expected: &[&str],
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

    let container = image
        .clone()
        .with_wait_for(WaitFor::Exit(ExitWaitStrategy::default()))
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
    run_highlight(&image, &[], "ls -l", &["0 2 fg=cyan", "2 5 fg=magenta"]).await;
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
        &["0 2 fg=cyan", "2 5 fg=magenta"],
    )
    .await;

    // alias with a subshell
    run_highlight(
        &image,
        &["alias ll='(ls -l)'"],
        "ll -a",
        &["0 2 fg=cyan", "2 5 fg=magenta"],
    )
    .await;

    // alias referencing another alias
    run_highlight(
        &image,
        &["alias lla='ll -a'", "alias ll='ls -l'"],
        "lla && ll",
        &["0 3 fg=cyan", "4 6 fg=blue", "7 9 fg=cyan"],
    )
    .await;

    // alias referencing a command that does not exist
    run_highlight(&image, &["alias fb=foobar"], "fb", &["0 2 fg=red"]).await;

    // alias referencing two commands
    run_highlight(
        &image,
        &["alias foobar='ls -l && echo OK'"],
        "foobar",
        &["0 6 fg=cyan"],
    )
    .await;

    // alias referencing two commands, but the second one does not exist
    run_highlight(
        &image,
        &["alias foobar='ls -l && missing OK'"],
        "foobar",
        &["0 6 fg=red"],
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
        &["0 2 fg=red"],
    )
    .await;
}
