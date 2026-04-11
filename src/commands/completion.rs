use std::{fs::File, io};

use anyhow::{Context, Result};
use clap::CommandFactory;
use clap_complete::Shell;

use crate::Args;

/// Generates shell completions for zsh-patina. If `output_file` is `None`, the
/// script is printed to stdout. Otherwise, it is written to the specified file.
pub fn completion(output_file: Option<&str>) -> Result<()> {
    let mut writer: Box<dyn io::Write> = match output_file {
        Some(path) => {
            Box::new(File::create(path).with_context(|| format!("Unable to create file {path:?}"))?)
        }
        None => Box::new(io::stdout()),
    };

    clap_complete::aot::generate(Shell::Zsh, &mut Args::command(), "zsh-patina", &mut writer);

    Ok(())
}
