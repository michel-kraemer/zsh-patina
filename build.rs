use std::{collections::HashSet, env, fs, path::PathBuf};

use anyhow::{Context, Result};
use syntect::{
    dumps::dump_to_uncompressed_file,
    parsing::{
        SyntaxDefinition, SyntaxSetBuilder,
        syntax_definition::{MatchPattern, Pattern},
    },
};

fn collect_scopes(definition: &SyntaxDefinition) -> HashSet<String> {
    let mut scopes = HashSet::new();

    scopes.insert(definition.scope.to_string());

    for context in definition.contexts.values() {
        for s in &context.meta_scope {
            scopes.insert(s.build_string());
        }
        for s in &context.meta_content_scope {
            scopes.insert(s.build_string());
        }

        for pattern in &context.patterns {
            if let Pattern::Match(match_pat) = pattern {
                collect_from_match(match_pat, &mut scopes);
            }
        }
    }

    scopes
}

fn collect_from_match(match_pattern: &MatchPattern, scopes: &mut HashSet<String>) {
    for s in &match_pattern.scope {
        scopes.insert(s.build_string());
    }
    if let Some(captures) = &match_pattern.captures {
        for (_, capture_scopes) in captures {
            for s in capture_scopes {
                scopes.insert(s.build_string());
            }
        }
    }
}

fn main() -> Result<()> {
    let out_dir = env::var_os("OUT_DIR").unwrap();

    // get all possible scopes from the Sublime syntax
    let syntax_yaml =
        fs::read_to_string("assets/Packages/ShellScript/Bash.sublime-syntax").unwrap();
    let syntax_definition = SyntaxDefinition::load_from_str(&syntax_yaml, true, None).unwrap();
    let scopes = collect_scopes(&syntax_definition);
    let mut scopes = scopes.into_iter().collect::<Vec<_>>();

    // add dynamic scopes
    scopes.push("dynamic.path.directory.complete.shell".to_string());
    scopes.push("dynamic.path.directory.partial.shell".to_string());
    scopes.push("dynamic.path.file.complete.shell".to_string());
    scopes.push("dynamic.path.file.partial.shell".to_string());
    scopes.push("dynamic.callable.alias.shell".to_string());
    scopes.push("dynamic.callable.builtin.shell".to_string());
    scopes.push("dynamic.callable.command.shell".to_string());
    scopes.push("dynamic.callable.function.shell".to_string());
    scopes.push("dynamic.callable.missing.shell".to_string());

    // add scope for history expansions
    scopes.push("meta.group.expansion.history.shell".to_string());

    // sort scopes
    scopes.sort_unstable();

    // dump scopes to a file
    let scopes_dest_path = PathBuf::from(&out_dir).join("scopes.rs");
    fs::write(
        scopes_dest_path,
        format!(
            "[{}]",
            scopes
                .into_iter()
                .map(|mut s| {
                    s.insert(0, '"');
                    s.push('"');
                    s
                })
                .collect::<Vec<_>>()
                .join(",")
        ),
    )?;

    // load shell syntax into a syntax set and dump it to a file
    let mut syntax_set_builder = SyntaxSetBuilder::new();
    syntax_set_builder.add_from_folder("assets/Packages/ShellScript", true)?;
    let syntax_set = syntax_set_builder.build();
    let syntax_dest_path = PathBuf::from(&out_dir).join("syntax_set.packdump");
    dump_to_uncompressed_file(&syntax_set, syntax_dest_path)
        .context("Unable to dump syntax to file")?;

    Ok(())
}
