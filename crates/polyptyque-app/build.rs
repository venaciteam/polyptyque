include!("../../packaging/ressources_windows.rs");

fn main() {
    // Thème sombre « Fluent » pour les contrôles standard (listes, champs…).
    // Catalogues de traduction intégrés à l'exécutable (voir src/i18n.rs),
    // sans contexte par défaut : un même texte a la même entrée dans
    // l'interface et dans les messages Rust.
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent-dark".into())
        .with_bundled_translations("lang")
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    slint_build::compile_with_config("ui/app.slint", config).expect("compilation de l'interface");
    // Description affichée par le Gestionnaire des tâches : le nom seul, le
    // même dans toutes les langues de l'interface.
    ressources_windows("Polyptyque", "polyptyque");
}
