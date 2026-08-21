use anyhow::{Result, anyhow};

pub fn highlight() -> Result<()> {
    Err(anyhow!(
        "This command is only available in a shell session where zsh-patina is \
        activated."
    ))
}
