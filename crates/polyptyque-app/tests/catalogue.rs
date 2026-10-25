//! Catalogues de traduction (voir `src/i18n.rs` et « Langues et formats »
//! dans le cahier des charges).
//!
//! Vérifie, en lisant les sources elles-mêmes :
//! - que chaque texte à traduire (`@tr(…)` dans `ui/`, `tr!(…)` dans les
//!   crates `polyptyque-app`, `-core`, `-engine` et `-windows`) a son entrée
//!   dans le catalogue anglais, et qu'aucune entrée n'y traîne sans texte ;
//! - une fois la version anglaise écrite ([`ENGLISH_COMPLETE`]), que chaque
//!   entrée est traduite, avec les mêmes paramètres `{…}` que le français ;
//! - que le catalogue français reprend les pluriels de l'interface (Slint
//!   applique sinon la règle anglaise : « 0 fichiers »).
//!
//! Mise à jour des deux catalogues (nouveaux textes ajoutés sans traduction,
//! textes disparus retirés et affichés) :
//!
//! ```text
//! POLYPTYQUE_CATALOGUE=maj cargo test -p polyptyque-app --test catalogue
//! ```
//!
//! La fabrication de GitHub ne lance aucun test : celui-ci se lance en local
//! avant chaque publication.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// Passe à `true` quand la version anglaise est écrite : dès lors, un texte
/// sans traduction fait échouer le test.
const ENGLISH_COMPLETE: bool = true;

/// Crates dont les messages Rust passent par `tr!`. Le mode commande
/// (`polyptyque-cli`) partage le catalogue de l'interface.
const RUST_CRATES: [&str; 5] =
    ["polyptyque-app", "polyptyque-cli", "polyptyque-core", "polyptyque-engine", "polyptyque-windows"];

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Key {
    context: Option<String>,
    msgid: String,
    plural: Option<String>,
}

struct Message {
    key: Key,
    /// Emplacements (`fichier:ligne`, relatifs à `crates/`).
    origins: Vec<String>,
    /// Le texte a des paramètres : ses `{…}` sont à remplacer, pas à
    /// afficher tels quels.
    has_arguments: bool,
    /// Texte de l'interface Slint (concerné par le catalogue français).
    in_slint: bool,
}

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn lang_file(language: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("lang").join(language).join("LC_MESSAGES/polyptyque-app.po")
}

fn files_with_extension(dir: &Path, extension: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            files_with_extension(&path, extension, out);
        } else if path.extension().is_some_and(|e| e == extension) {
            out.push(path);
        }
    }
}

/// Lit une chaîne littérale qui commence à `text[0] == '"'`. Renvoie son
/// contenu et le nombre d'octets lus.
fn string_literal(text: &str, rust: bool) -> Result<(String, usize), String> {
    let mut chars = text.char_indices();
    if chars.next().map(|(_, c)| c) != Some('"') {
        return Err("une chaîne littérale \"…\" est attendue".into());
    }
    let mut out = String::new();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Ok((out, i + 1)),
            '\\' => match chars.next().map(|(_, c)| c) {
                Some('n') => out.push('\n'),
                Some('t') if rust => out.push('\t'),
                Some('r') if rust => out.push('\r'),
                Some('0') if rust => out.push('\0'),
                Some('\'') if rust => out.push('\''),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('u') => {
                    let rest: String = chars.by_ref().map(|(_, c)| c).take_while(|&c| c != '}').collect();
                    let code = rest.trim_start_matches('{');
                    let value =
                        u32::from_str_radix(code, 16).map_err(|_| format!("échappement \\u invalide : {rest}"))?;
                    out.push(char::from_u32(value).ok_or("caractère invalide")?);
                }
                // Rust : fin de ligne échappée, les espaces suivants sont ignorés.
                Some('\n') if rust => {
                    let rest = &text[i + 2..];
                    let skip = rest.len() - rest.trim_start().len();
                    for _ in 0..rest[..skip].chars().count() {
                        chars.next();
                    }
                }
                Some('{') if !rust => {
                    return Err("pas d'interpolation \\{…} dans @tr : passer la valeur en paramètre".into())
                }
                other => return Err(format!("échappement non pris en charge : \\{other:?}")),
            },
            c => out.push(c),
        }
    }
    Err("chaîne non terminée".into())
}

fn skip_spaces(text: &str) -> usize {
    text.len() - text.trim_start().len()
}

/// Lit les arguments d'un appel `@tr(` ou `tr!(` qui commence à `text`
/// (juste après la parenthèse) : `"texte"`, `"contexte" => "texte"`,
/// `"singulier" | "pluriel" % n`, suivis ou non de paramètres.
fn parse_call(text: &str, rust: bool) -> Result<(Key, bool), String> {
    let mut pos = skip_spaces(text);
    let (first, len) = string_literal(&text[pos..], rust)?;
    pos += len;
    pos += skip_spaces(&text[pos..]);
    let (context, msgid) = if text[pos..].starts_with("=>") {
        pos += 2;
        pos += skip_spaces(&text[pos..]);
        let (msgid, len) = string_literal(&text[pos..], rust)?;
        pos += len;
        pos += skip_spaces(&text[pos..]);
        (Some(first), msgid)
    } else {
        (None, first)
    };
    let plural = if text[pos..].starts_with('|') {
        pos += 1;
        pos += skip_spaces(&text[pos..]);
        let (plural, len) = string_literal(&text[pos..], rust)?;
        pos += len;
        pos += skip_spaces(&text[pos..]);
        if !text[pos..].starts_with('%') {
            return Err("« % n » attendu après le pluriel".into());
        }
        Some(plural)
    } else {
        None
    };
    let has_arguments = plural.is_some() || text[pos..].starts_with(',');
    Ok((Key { context, msgid, plural }, has_arguments))
}

/// Tous les textes à traduire, dans l'ordre des fichiers puis des lignes.
fn scan_sources() -> Vec<Message> {
    let crates = crates_dir();
    let mut files: Vec<(PathBuf, bool)> = Vec::new();
    let mut slint = Vec::new();
    files_with_extension(&crates.join("polyptyque-app/ui"), "slint", &mut slint);
    files.extend(slint.into_iter().map(|p| (p, false)));
    for name in RUST_CRATES {
        let mut rust = Vec::new();
        files_with_extension(&crates.join(name).join("src"), "rs", &mut rust);
        files.extend(rust.into_iter().map(|p| (p, true)));
    }

    let mut messages: Vec<Message> = Vec::new();
    let mut index: HashMap<Key, usize> = HashMap::new();
    let mut errors = Vec::new();
    for (path, rust) in files {
        let text = fs::read_to_string(&path).unwrap();
        let marker = if rust { "tr!(" } else { "@tr(" };
        let relative = path.strip_prefix(&crates).unwrap().display().to_string();
        for (start, _) in text.match_indices(marker) {
            // `xtr!(`, `str!(`… : pas notre macro.
            if rust && text[..start].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
            if text[line_start..start].contains("//") {
                continue;
            }
            let line = text[..start].matches('\n').count() + 1;
            let origin = format!("{relative}:{line}");
            match parse_call(&text[start + marker.len()..], rust) {
                Ok((key, has_arguments)) => match index.get(&key) {
                    Some(&i) => {
                        let message = &mut messages[i];
                        message.origins.push(origin);
                        message.has_arguments |= has_arguments;
                        message.in_slint |= !rust;
                    }
                    None => {
                        index.insert(key.clone(), messages.len());
                        messages.push(Message { key, origins: vec![origin], has_arguments, in_slint: !rust });
                    }
                },
                Err(e) => errors.push(format!("{origin} : {e}")),
            }
        }
    }
    assert!(errors.is_empty(), "textes à traduire illisibles :\n{}", errors.join("\n"));
    messages
}

fn entry_key(entry: &rspolib::POEntry) -> Key {
    Key { context: entry.msgctxt.clone(), msgid: entry.msgid.clone(), plural: entry.msgid_plural.clone() }
}

fn read_catalog(language: &str) -> (String, Vec<rspolib::POEntry>) {
    let path = lang_file(language);
    let text = fs::read_to_string(&path).unwrap();
    // En-tête : commentaires et métadonnées, jusqu'à la première ligne vide.
    let header = text.split("\n\n").next().unwrap().to_string();
    let file = rspolib::pofile(path.as_path()).unwrap_or_else(|e| panic!("{} illisible : {e}", path.display()));
    (header, file.entries.into_iter().filter(|e| !e.obsolete).collect())
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\t', "\\t")
}

fn write_entry(out: &mut String, key: &Key, comment: Option<&str>, origins: &[String], msgstr: &[String]) {
    out.push('\n');
    if let Some(comment) = comment.filter(|c| !c.is_empty()) {
        for line in comment.lines() {
            let _ = writeln!(out, "# {line}");
        }
    }
    if !origins.is_empty() {
        let _ = writeln!(out, "#: {}", origins.join(" "));
    }
    if let Some(context) = &key.context {
        let _ = writeln!(out, "msgctxt \"{}\"", escape(context));
    }
    let _ = writeln!(out, "msgid \"{}\"", escape(&key.msgid));
    match &key.plural {
        Some(plural) => {
            let _ = writeln!(out, "msgid_plural \"{}\"", escape(plural));
            for i in 0..2 {
                let _ = writeln!(out, "msgstr[{i}] \"{}\"", escape(msgstr.get(i).map_or("", String::as_str)));
            }
        }
        None => {
            let _ = writeln!(out, "msgstr \"{}\"", escape(msgstr.first().map_or("", String::as_str)));
        }
    }
}

fn translations(entry: &rspolib::POEntry) -> Vec<String> {
    if entry.msgid_plural.is_some() {
        entry.msgstr_plural.clone()
    } else {
        entry.msgstr.clone().into_iter().collect()
    }
}

/// Paramètres d'un texte : `{}` numérotés dans l'ordre (`0`, `1`…), `{1}`
/// par son numéro (une traduction peut changer l'ordre), `{nom}` par son
/// nom.
fn placeholders(text: &str) -> BTreeSet<String> {
    let mut next = 0;
    let mut found = BTreeSet::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let Some(end) = rest[start..].find('}') else { break };
        let name = rest[start + 1..start + end].trim();
        if name.is_empty() {
            found.insert(next.to_string());
            next += 1;
        } else {
            found.insert(name.to_string());
        }
        rest = &rest[start + end + 1..];
    }
    found
}

#[test]
fn catalogues() {
    let messages = scan_sources();
    assert!(messages.len() > 10, "aucun texte à traduire trouvé : la lecture des sources est cassée");
    let update = std::env::var("POLYPTYQUE_CATALOGUE").as_deref() == Ok("maj");

    let (en_header, en_entries) = read_catalog("en");
    let (fr_header, fr_entries) = read_catalog("fr");
    let scanned: HashSet<&Key> = messages.iter().map(|m| &m.key).collect();

    if update {
        let existing: HashMap<Key, &rspolib::POEntry> = en_entries.iter().map(|e| (entry_key(e), e)).collect();
        let mut out = en_header.clone();
        out.push('\n');
        for message in &messages {
            let entry = existing.get(&message.key);
            let msgstr = entry.map(|e| translations(e)).unwrap_or_default();
            write_entry(&mut out, &message.key, entry.and_then(|e| e.tcomment.as_deref()), &message.origins, &msgstr);
        }
        for entry in en_entries.iter().filter(|e| !scanned.contains(&entry_key(e))) {
            eprintln!("retiré du catalogue anglais (plus dans les sources) : {:?}", entry.msgid);
        }
        fs::write(lang_file("en"), out).unwrap();

        // Catalogue français : entrées des composants de Slint conservées,
        // pluriels de l'interface recopiés.
        let mut out = fr_header.clone();
        out.push('\n');
        for entry in fr_entries.iter().filter(|e| !scanned.contains(&entry_key(e))) {
            write_entry(&mut out, &entry_key(entry), entry.tcomment.as_deref(), &[], &translations(entry));
        }
        for message in messages.iter().filter(|m| m.in_slint && m.key.plural.is_some()) {
            let forms = vec![message.key.msgid.clone(), message.key.plural.clone().unwrap()];
            write_entry(
                &mut out,
                &message.key,
                Some("Pluriel de l'interface, recopié du code."),
                &message.origins,
                &forms,
            );
        }
        fs::write(lang_file("fr"), out).unwrap();
        eprintln!("catalogues mis à jour : {} textes", messages.len());
        return;
    }

    let mut problems = Vec::new();
    let catalog: HashMap<Key, &rspolib::POEntry> = en_entries.iter().map(|e| (entry_key(e), e)).collect();
    for message in &messages {
        let Some(entry) = catalog.get(&message.key) else {
            problems.push(format!("{} : absent du catalogue anglais : {:?}", message.origins[0], message.key.msgid));
            continue;
        };
        let forms = translations(entry);
        let translated = !forms.is_empty() && forms.iter().all(|f| !f.is_empty());
        if !translated {
            if ENGLISH_COMPLETE {
                problems.push(format!("{} : pas de version anglaise : {:?}", message.origins[0], message.key.msgid));
            }
            continue;
        }
        if message.has_arguments {
            let mut expected = placeholders(&message.key.msgid);
            if let Some(plural) = &message.key.plural {
                expected.extend(placeholders(plural));
            }
            for form in &forms {
                let found = placeholders(form);
                // Au pluriel, le singulier anglais peut taire le nombre
                // (« One file ») : chaque forme utilise une partie des
                // paramètres. Sinon, exactement les mêmes.
                let ok = if message.key.plural.is_some() { found.is_subset(&expected) } else { found == expected };
                if !ok {
                    problems.push(format!(
                        "{} : paramètres différents entre {:?} et {:?}",
                        message.origins[0], message.key.msgid, form
                    ));
                }
            }
        }
    }
    for entry in en_entries.iter().filter(|e| !scanned.contains(&entry_key(e))) {
        problems.push(format!("catalogue anglais : entrée sans texte dans les sources : {:?}", entry.msgid));
    }
    let fr: HashSet<Key> = fr_entries.iter().map(entry_key).collect();
    for message in messages.iter().filter(|m| m.in_slint && m.key.plural.is_some()) {
        if !fr.contains(&message.key) {
            problems.push(format!(
                "{} : pluriel absent du catalogue français : {:?}",
                message.origins[0], message.key.msgid
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "{} problème(s) de traduction (mise à jour : POLYPTYQUE_CATALOGUE=maj cargo test -p polyptyque-app --test catalogue) :\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// Le catalogue tel que Polyptyque le lit à l'exécution (texte intégré à
/// l'exécutable) traduit bien chaque entrée : un fichier correct ne suffit
/// pas si la lecture en mémoire échoue (cas de rspolib 0.1 avec des octets).
#[test]
fn runtime_catalog_translates_every_entry() {
    use tr::Translator;
    let text = fs::read_to_string(lang_file("en")).unwrap();
    let catalog = polyptyque_core::i18n::Catalog::parse(&text).unwrap();
    let (_, entries) = read_catalog("en");
    let mut wrong = Vec::new();
    for entry in &entries {
        let context = entry.msgctxt.as_deref();
        match &entry.msgid_plural {
            Some(plural) => {
                for (n, form) in [(1u64, 0usize), (2, 1)] {
                    let expected = entry.msgstr_plural.get(form).cloned().unwrap_or_default();
                    let got = catalog.ntranslate(n, &entry.msgid, plural, context);
                    if !expected.is_empty() && got != expected {
                        wrong.push(format!("{:?} (n = {n}) : {got:?} au lieu de {expected:?}", entry.msgid));
                    }
                }
            }
            None => {
                let expected = entry.msgstr.clone().unwrap_or_default();
                let got = catalog.translate(&entry.msgid, context);
                if !expected.is_empty() && got != expected {
                    wrong.push(format!("{:?} : {got:?} au lieu de {expected:?}", entry.msgid));
                }
            }
        }
    }
    assert!(
        !entries.is_empty() && catalog.len() == entries.len(),
        "{} entrées lues sur {}",
        catalog.len(),
        entries.len()
    );
    assert!(wrong.is_empty(), "traductions perdues à la lecture :\n{}", wrong.join("\n"));
}
