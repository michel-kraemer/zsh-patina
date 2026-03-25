use anyhow::Result;

/// Print all scopes that can be used in a theme for highlighting (sorted
/// alphabetically)
pub fn list_scopes() -> Result<()> {
    let scopes = include!(concat!(env!("OUT_DIR"), "/scopes.rs"));
    for t in scopes {
        println!("{t}");
    }
    Ok(())
}
