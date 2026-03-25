use anyhow::Result;

use crate::{config::Config, theme::Theme};

pub fn check_config(config: &Config) -> Result<()> {
    // At this point, it has already been checked if the program's main
    // configuration is parseable. Otherwise, we would not have a `config`
    // object.

    // check if we can load the custom theme and if it's syntax is OK
    let _ = Theme::load(&config.highlighting.theme)?;

    Ok(())
}
