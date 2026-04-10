use clap::CommandFactory;
use clap_complete::Shell;

use crate::Args;

/// Generate completions and print them to stdout
pub fn completion() {
    clap_complete::aot::generate(
        Shell::Zsh,
        &mut Args::command(),
        "zsh-patina",
        &mut std::io::stdout(),
    );
}
