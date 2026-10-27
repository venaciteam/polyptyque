// Ressources Windows communes aux exécutables (inclus par leur build.rs) :
// icône et description visibles dans l'Explorateur (Propriétés > Détails),
// et icône de l'enregistrement en cours (ressource 2 : point rouge, pour la
// zone de notification).
// Langue neutre : un même exécutable pour le français et l'anglais.

/// Intègre l'icône et la description dans l'exécutable. Sans effet hors
/// d'une compilation Windows faite sous Windows (l'outil `rc.exe` est requis).
fn ressources_windows(description: &str, fichier: &str) {
    let racine = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let icone = racine.join("assets/logo/polyptyque.ico");
    let icone_enregistrement = racine.join("assets/logo/polyptyque-enregistrement.ico");
    println!("cargo:rerun-if-changed={}", icone.display());
    println!("cargo:rerun-if-changed={}", icone_enregistrement.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") || !cfg!(windows) {
        return;
    }
    let version = env!("CARGO_PKG_VERSION");
    let mut nombres: Vec<u16> = version.split(['.', '-', '+']).take(3).map(|n| n.parse().unwrap_or(0)).collect();
    nombres.resize(3, 0);
    let virgules = format!("{},{},{},0", nombres[0], nombres[1], nombres[2]);
    let chemin_icone = icone.display().to_string().replace('\\', "\\\\");
    let chemin_enregistrement = icone_enregistrement.display().to_string().replace('\\', "\\\\");
    let rc = format!(
        r#"#pragma code_page(65001)
1 ICON "{chemin_icone}"
2 ICON "{chemin_enregistrement}"
1 VERSIONINFO
FILEVERSION {virgules}
PRODUCTVERSION {virgules}
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "000004B0"
        BEGIN
            VALUE "CompanyName", "Polyptyque"
            VALUE "FileDescription", "{description}"
            VALUE "FileVersion", "{version}"
            VALUE "InternalName", "{fichier}"
            VALUE "LegalCopyright", "GPL-3.0-or-later"
            VALUE "OriginalFilename", "{fichier}.exe"
            VALUE "ProductName", "Polyptyque"
            VALUE "ProductVersion", "{version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x0, 1200
    END
END
"#
    );
    let sortie = std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("polyptyque.rc");
    std::fs::write(&sortie, rc).expect("écriture des ressources Windows");
    embed_resource::compile(&sortie, embed_resource::NONE).manifest_optional().expect("ressources Windows");
}
