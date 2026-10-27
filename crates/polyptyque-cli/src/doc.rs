//! La liste des commandes de `docs/control.md` est générée depuis le
//! catalogue (descriptions anglaises) : ce test échoue si elle n'est plus à
//! jour. Mise à jour :
//!
//! ```text
//! POLYPTYQUE_DOC=maj cargo test -p polyptyque-cli doc
//! ```

use std::path::Path;

use polyptyque_core::control::catalog::{self, Command, Param, ParamType};

const START: &str = "<!-- commands:start -->";
const END: &str = "<!-- commands:end -->";

fn param_type(param: &Param) -> String {
    match param.ty {
        ParamType::String if !param.choices.is_empty() => {
            param.choices.iter().map(|c| format!("`{c}`")).collect::<Vec<_>>().join(" \\| ")
        }
        ParamType::Strings if !param.choices.is_empty() => {
            format!("list of {}", param.choices.iter().map(|c| format!("`{c}`")).collect::<Vec<_>>().join(", "))
        }
        ParamType::Strings => "list of strings".into(),
        ty => ty.name().into(),
    }
}

fn rows(out: &mut String, params: &[Param], prefix: &str) {
    for param in params {
        let name = format!("{prefix}{}", param.name);
        let required = if param.required { "yes" } else { "" };
        out.push_str(&format!(
            "| `{name}` | {} | {required} | {} |\n",
            param_type(param),
            param.description.replace('|', "\\|")
        ));
        rows(out, &param.fields, &format!("{name}."));
    }
}

fn section(command: &Command) -> String {
    let mut out = format!("### `{}`\n\n{}\n\n", command.method, command.description);
    let cli = command.method.replace('.', " ");
    if crate::mcp::NOT_TOOLS.contains(&command.method) {
        out.push_str("Command channel only (the command line and the MCP server send it themselves; `polyptyque-cli events` gives the live status).\n\n");
    } else {
        out.push_str(&format!("Command line: `polyptyque-cli {cli}` · MCP tool: `{}`\n\n", command.tool_name()));
    }
    if command.params.is_empty() {
        out.push_str("No parameters.\n\n");
    } else {
        out.push_str("| Parameter | Type | Required | Description |\n|---|---|---|---|\n");
        rows(&mut out, &command.params, "");
        out.push('\n');
    }
    out
}

fn generated() -> String {
    crate::i18n::set(true);
    let mut out = String::from("\n");
    for command in catalog::commands() {
        out.push_str(&section(&command));
    }
    out
}

#[test]
fn doc_lists_every_command() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/control.md");
    let text = std::fs::read_to_string(&path).expect("docs/control.md");
    let (Some(start), Some(end)) = (text.find(START), text.find(END)) else {
        panic!("marqueurs {START} et {END} absents de docs/control.md");
    };
    let current = &text[start + START.len()..end];
    let wanted = generated();
    if std::env::var("POLYPTYQUE_DOC").as_deref() == Ok("maj") {
        let updated = format!("{}{START}{wanted}{END}{}", &text[..start], &text[end + END.len()..]);
        std::fs::write(&path, updated).expect("écriture de docs/control.md");
        return;
    }
    assert!(
        current == wanted,
        "docs/control.md n'est plus à jour : POLYPTYQUE_DOC=maj cargo test -p polyptyque-cli doc"
    );
}
