//! Aide lisible du mode commande, dans la langue de Polyptyque.

use polyptyque_core::control::catalog::{self, Command, Param, ParamType};
use polyptyque_core::control::protocol::Reason;
use tr::tr;

/// Mots de la commande : « recording.start » → « recording start ».
pub fn words(command: &Command) -> String {
    command.method.replace('.', " ")
}

fn placeholder(param: &Param) -> String {
    match param.ty {
        ParamType::Boolean => String::new(),
        ParamType::Integer => tr!(" <nombre>"),
        ParamType::Strings if !param.choices.is_empty() => format!(" <{}>", param.choices.join(",")),
        ParamType::Strings => tr!(" <texte> (répétable)"),
        ParamType::Object => " <json>".to_string(),
        ParamType::String if !param.choices.is_empty() => format!(" <{}>", param.choices.join("|")),
        ParamType::String => tr!(" <texte>"),
    }
}

fn param_lines(out: &mut String, params: &[Param], prefix: &str) {
    for param in params {
        let name = format!("{prefix}{}", param.name.replace('_', "-"));
        let required = if param.required { tr!(" (obligatoire)") } else { String::new() };
        out.push_str(&format!("    --{name}{}{required}\n        {}\n", placeholder(param), param.description));
        if !param.fields.is_empty() {
            param_lines(out, &param.fields, &format!("{name}."));
        }
    }
}

/// Aide d'une commande.
pub fn command(command: &Command) -> String {
    let mut out = format!("polyptyque-cli {}\n\n{}\n", words(command), command.description);
    if !command.params.is_empty() {
        out.push('\n');
        out.push_str(&tr!("Paramètres :"));
        out.push('\n');
        param_lines(&mut out, &command.params, "");
    }
    out
}

/// Aide générale.
pub fn general() -> String {
    let mut out = tr!(
        "Polyptyque {} · mode commande\n\nPilote le Polyptyque ouvert sur cet ordinateur (fenêtre affichée ou en arrière-plan), si « Autoriser le pilotage par d'autres programmes » est activé dans ses réglages. Chaque commande écrit sa réponse en JSON.\n",
        env!("CARGO_PKG_VERSION")
    );
    out.push('\n');
    out.push_str(&tr!("Utilisation :"));
    out.push('\n');
    for line in [
        tr!("polyptyque-cli <commande> [--paramètre valeur…] [--pretty]"),
        tr!("polyptyque-cli help <commande>"),
        tr!("polyptyque-cli call <méthode> '<json>'"),
        "polyptyque-cli commands".to_string(),
        "polyptyque-cli events".to_string(),
        "polyptyque-cli app launch".to_string(),
        "polyptyque-cli mcp".to_string(),
    ] {
        out.push_str(&format!("  {line}\n"));
    }
    out.push('\n');
    out.push_str(&tr!("Commandes :"));
    out.push('\n');
    let commands = catalog::commands();
    let width = commands.iter().map(|c| words(c).chars().count()).max().unwrap_or(0) + 2;
    for command in &commands {
        let first = command.description.split_inclusive(". ").next().unwrap_or(&command.description).trim_end();
        out.push_str(&format!("  {:width$}{first}\n", words(command)));
    }
    out.push_str(&format!("  {:width$}{}\n", "app launch", tr!("Lance Polyptyque en arrière-plan s'il est fermé.")));
    out.push_str(&format!("  {:width$}{}\n", "events", tr!("État en direct : une ligne JSON par changement.")));
    out.push_str(&format!(
        "  {:width$}{}\n",
        "mcp",
        tr!("Serveur MCP (entrée et sortie standard), pour les assistants IA.")
    ));
    out.push_str(&format!("  {:width$}{}\n", "commands", tr!("Catalogue des commandes, en JSON.")));
    out.push('\n');
    out.push_str(&tr!("Codes de retour :"));
    out.push_str("\n  0  ");
    out.push_str(&tr!("réussi"));
    out.push('\n');
    let mut codes: Vec<(i32, Vec<&str>)> = Vec::new();
    for reason in Reason::ALL {
        match codes.iter_mut().find(|(code, _)| *code == reason.exit_code()) {
            Some((_, names)) => names.push(reason.as_str()),
            None => codes.push((reason.exit_code(), vec![reason.as_str()])),
        }
    }
    codes.sort();
    for (code, names) in codes {
        out.push_str(&format!("  {code}  {}\n", names.join(", ")));
    }
    out.push('\n');
    out.push_str(&tr!(
        "Ce que la fenêtre ferait confirmer (supprimer, arrêter un enregistrement programmé, quitter…) renvoie « confirmation_required » avec la question : la poser à la personne, puis relancer avec --confirm.\nRéférence complète : https://github.com/venaciteam/Polyptyque/blob/main/docs/control.md"
    ));
    out.push('\n');
    out
}
