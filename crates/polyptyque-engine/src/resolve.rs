//! Reconnaissance et lancement des fenêtres des sources.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use polyptyque_core::browsers::{command_line_uses_profile, file_stem, launch_args, window_class, GECKO_USER_JS};
use polyptyque_core::config::{BrowserFamily, Source, SourceKind};
use polyptyque_core::platform::{Platform, WindowInfo};
use tr::tr;

/// Retrouve la fenêtre d'une source parmi `windows` (ordre d'empilement).
///
/// - Navigateur : la fenêtre principale du processus dont la ligne de
///   commande désigne le profil dédié. Le titre n'intervient pas.
/// - Application : même exécutable, même classe et même titre ; à défaut
///   même classe ; à défaut la seule fenêtre de ce programme.
pub fn find_window(platform: &dyn Platform, source: &Source, windows: &[WindowInfo]) -> Option<WindowInfo> {
    match &source.kind {
        SourceKind::Browser { profile_dir, browser, .. } => {
            let class = window_class(browser.family);
            let mut checked: HashMap<u32, bool> = HashMap::new();
            windows
                .iter()
                .filter(|w| w.class == class)
                .find(|w| {
                    *checked.entry(w.pid).or_insert_with(|| {
                        platform
                            .process_command_line(w.pid)
                            .is_some_and(|line| command_line_uses_profile(&line, profile_dir))
                    })
                })
                .cloned()
        }
        SourceKind::Application { exe_path, window_class, window_title, .. } => {
            let mut same_program: Vec<&WindowInfo> = windows.iter().filter(|w| same_path(&w.exe, exe_path)).collect();
            if same_program.is_empty() {
                // Le programme a pu être mis à jour et changer de dossier
                // (Discord, par exemple) : on se rabat sur le nom du fichier.
                same_program = windows.iter().filter(|w| same_file_name(&w.exe, exe_path)).collect();
            }
            same_program
                .iter()
                .find(|w| &w.class == window_class && &w.title == window_title)
                .or_else(|| same_program.iter().find(|w| &w.class == window_class))
                .or_else(|| (same_program.len() == 1).then(|| &same_program[0]))
                .map(|w| (*w).clone())
        }
    }
}

/// Fenêtres des navigateurs des sources (leurs profils dédiés), à fermer en
/// quittant Polyptyque. Les applications n'en font jamais partie.
///
/// Un navigateur ne restaure au lancement suivant que la dernière fenêtre
/// fermée de son profil : pour chaque profil, la fenêtre de la source (celle
/// que retient [`find_window`]) vient donc en dernier.
pub fn browser_windows(platform: &dyn Platform, sources: &[Source], windows: &[WindowInfo]) -> Vec<WindowInfo> {
    let mut command_lines: HashMap<u32, Option<String>> = HashMap::new();
    let mut taken: HashSet<isize> = HashSet::new();
    let mut found = Vec::new();
    for source in sources {
        let SourceKind::Browser { profile_dir, browser, .. } = &source.kind else {
            continue;
        };
        let class = window_class(browser.family);
        let mut profile: Vec<WindowInfo> = windows
            .iter()
            .filter(|w| w.class == class && !taken.contains(&w.handle))
            .filter(|w| {
                command_lines
                    .entry(w.pid)
                    .or_insert_with(|| platform.process_command_line(w.pid))
                    .as_deref()
                    .is_some_and(|line| command_line_uses_profile(line, profile_dir))
            })
            .cloned()
            .collect();
        if profile.len() > 1 {
            profile.rotate_left(1);
        }
        taken.extend(profile.iter().map(|w| w.handle));
        found.extend(profile);
    }
    found
}

/// Étapes de la préparation des fenêtres avant un enregistrement.
pub enum PrepareEvent<'a> {
    /// La fenêtre est déjà ouverte.
    Found(&'a Source),
    /// Le programme est lancé, en attente de sa fenêtre.
    Launching(&'a Source),
    LaunchFailed(&'a Source, String),
    /// La fenêtre lancée vient d'apparaître.
    Opened(&'a Source),
}

/// Retrouve ou lance la fenêtre de chaque source, puis attend au plus
/// `wait` celles qui ont été lancées. Renvoie les noms des sources encore
/// introuvables (elles seront rattachées dès leur apparition).
pub fn prepare_windows(
    platform: &dyn Platform,
    sources: &[Source],
    wait: Duration,
    mut on_event: impl FnMut(PrepareEvent),
) -> Vec<String> {
    let windows = platform.list_windows();
    let mut waiting: Vec<&Source> = Vec::new();
    let mut missing = Vec::new();
    for source in sources {
        if find_window(platform, source, &windows).is_some() {
            on_event(PrepareEvent::Found(source));
        } else if source.should_launch() {
            // Déjà en train de s'ouvrir (lancé il y a peu, ou Firefox encore
            // actif sur son profil) : on attend sa fenêtre sans le relancer.
            match launch_once(source) {
                Ok(_) => {
                    on_event(PrepareEvent::Launching(source));
                    waiting.push(source);
                }
                Err(e) => {
                    on_event(PrepareEvent::LaunchFailed(source, format!("{e:#}")));
                    missing.push(source.name.clone());
                }
            }
        } else {
            missing.push(source.name.clone());
        }
    }
    let deadline = Instant::now() + wait;
    while !waiting.is_empty() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(500));
        let windows = platform.list_windows();
        waiting.retain(|source| {
            let found = find_window(platform, source, &windows).is_some();
            if found {
                on_event(PrepareEvent::Opened(source));
            }
            !found
        });
    }
    missing.extend(waiting.iter().map(|s| s.name.clone()));
    missing
}

/// Délai pendant lequel une source qui vient d'être lancée n'est pas
/// relancée : son programme peut mettre quelques secondes à afficher sa
/// fenêtre.
const LAUNCH_GUARD: Duration = Duration::from_secs(20);

/// Sources lancées récemment, avec l'heure du lancement.
static RECENT_LAUNCHES: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());

/// Lance le programme d'une source, sauf s'il est déjà en train de s'ouvrir :
/// lancé il y a moins de 20 s (sa fenêtre n'est pas encore là), ou Firefox
/// encore actif sur son profil. Un second lancement ouvrirait une fenêtre de
/// trop, ou afficherait « Firefox est déjà ouvert ». Renvoie faux si le
/// programme n'a pas été relancé pour cette raison.
pub fn launch_once(source: &Source) -> Result<bool> {
    if profile_in_use(source) {
        return Ok(false);
    }
    {
        let mut recent = RECENT_LAUNCHES.lock().unwrap_or_else(|e| e.into_inner());
        recent.retain(|(_, at)| at.elapsed() < LAUNCH_GUARD);
        if recent.iter().any(|(id, _)| *id == source.id) {
            return Ok(false);
        }
        recent.push((source.id.clone(), Instant::now()));
    }
    if let Err(e) = launch(source) {
        RECENT_LAUNCHES.lock().unwrap_or_else(|e| e.into_inner()).retain(|(id, _)| *id != source.id);
        return Err(e);
    }
    Ok(true)
}

/// Lance le programme d'une source : le navigateur sur son profil dédié, ou
/// l'application avec ses arguments.
pub fn launch(source: &Source) -> Result<()> {
    match &source.kind {
        SourceKind::Browser { profile_dir, start_url, browser } => {
            let first_launch = prepare_profile(profile_dir, browser.family)?;
            let url = if first_launch { start_url.as_deref() } else { None };
            spawn_detached(&browser.exe_path, &launch_args(browser.family, profile_dir, url))
        }
        SourceKind::Application { exe_path, launch_args, .. } => {
            let args: Vec<OsString> = launch_args.iter().map(OsString::from).collect();
            spawn_detached(exe_path, &args)
        }
    }
}

/// Vrai si un Firefox tourne encore sur le profil de cette source (en cours
/// de fermeture ou de redémarrage, par exemple). Le relancer afficherait
/// alors « Firefox est déjà ouvert ». Sous Windows, Firefox garde
/// `parent.lock` ouvert sans partage tant qu'il tourne : l'ouvrir échoue.
pub fn profile_in_use(source: &Source) -> bool {
    /// `ERROR_SHARING_VIOLATION`.
    const SHARING_VIOLATION: i32 = 32;
    let SourceKind::Browser { profile_dir, browser, .. } = &source.kind else {
        return false;
    };
    if browser.family != BrowserFamily::Gecko {
        // Chrome et ses dérivés ouvrent simplement une nouvelle fenêtre.
        return false;
    }
    match fs::File::open(profile_dir.join("parent.lock")) {
        Ok(_) => false,
        Err(e) => e.raw_os_error() == Some(SHARING_VIOLATION),
    }
}

/// Crée le dossier du profil s'il n'existe pas et y dépose les préférences.
/// Renvoie vrai s'il s'agit du tout premier lancement.
pub fn prepare_profile(profile_dir: &Path, family: BrowserFamily) -> Result<bool> {
    let first_launch = !profile_dir.exists();
    fs::create_dir_all(profile_dir).with_context(|| tr!("création du profil {}", profile_dir.display()))?;
    if family == BrowserFamily::Gecko {
        let user_js = profile_dir.join("user.js");
        if !user_js.exists() {
            fs::write(&user_js, GECKO_USER_JS).with_context(|| tr!("écriture de {}", user_js.display()))?;
        }
    }
    Ok(first_launch)
}

fn spawn_detached(exe: &Path, args: &[OsString]) -> Result<()> {
    let mut command = Command::new(exe);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Some(dir) = exe.parent().filter(|d| d.exists()) {
        command.current_dir(dir);
    }
    command.spawn().with_context(|| tr!("lancement de {}", exe.display()))?;
    Ok(())
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

fn same_file_name(a: &Path, b: &Path) -> bool {
    match (file_stem(a), file_stem(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_source_is_not_launched_twice_while_opening() {
        let window = WindowInfo {
            handle: 1,
            pid: 1,
            title: "Essai".into(),
            class: "Essai".into(),
            exe: PathBuf::from("/bin/true"),
        };
        let source = Source::application("Essai", &window, true, true);
        assert!(launch_once(&source).unwrap());
        // Toujours en train de s'ouvrir : pas de second lancement.
        assert!(!launch_once(&source).unwrap());
        // Un lancement raté ne bloque pas l'essai suivant.
        let missing = WindowInfo { exe: PathBuf::from("/introuvable/programme"), ..window };
        let broken = Source::application("Cassée", &missing, true, true);
        assert!(launch_once(&broken).is_err());
        assert!(launch_once(&broken).is_err());
    }
    use polyptyque_core::config::BrowserRef;
    use polyptyque_core::platform::*;
    use std::path::PathBuf;

    struct FakePlatform;

    impl Platform for FakePlatform {
        fn list_windows(&self) -> Vec<WindowInfo> {
            vec![]
        }
        fn is_window_alive(&self, _: WindowHandle) -> bool {
            true
        }
        fn is_window_minimized(&self, _: WindowHandle) -> bool {
            false
        }
        fn process_command_line(&self, pid: u32) -> Option<String> {
            match pid {
                10 => Some(r#"firefox.exe -profile "C:\p\autre""#.into()),
                11 => Some(r#"firefox.exe -profile "C:\p\abc" -no-remote"#.into()),
                _ => None,
            }
        }
        fn detect_browsers(&self) -> Vec<BrowserInstall> {
            vec![]
        }
        fn now_hns(&self) -> u64 {
            0
        }
        fn open_video_capture(&self, _: WindowHandle, _: u32, _: u32, _: bool) -> Result<Box<dyn VideoCapture>> {
            unimplemented!()
        }
        fn open_audio_capture(&self, _: u32, _: u32, _: u16) -> Result<Box<dyn AudioCapture>> {
            unimplemented!()
        }
        fn list_microphones(&self) -> Vec<AudioDevice> {
            vec![]
        }
        fn open_microphone_capture(&self, _: Option<&str>, _: u32, _: u16) -> Result<Box<dyn AudioCapture>> {
            unimplemented!()
        }
        fn create_stream_pipe(&self, _: &str) -> Result<Box<dyn StreamPipe>> {
            unimplemented!()
        }
        fn prepare_encoder_process(&self, _: &mut Command) {}
        fn begin_recording_session(&self) {}
        fn end_recording_session(&self) {}
        fn free_disk_space(&self, _: &Path) -> Option<u64> {
            None
        }
    }

    fn window(handle: isize, pid: u32, title: &str, class: &str, exe: &str) -> WindowInfo {
        WindowInfo { handle, pid, title: title.into(), class: class.into(), exe: PathBuf::from(exe) }
    }

    fn source(kind: SourceKind) -> Source {
        Source { id: "x".into(), name: "x".into(), capture_cursor: true, relaunch: false, kind }
    }

    #[test]
    fn browser_is_found_by_profile_not_title() {
        let windows = vec![
            window(1, 10, "Page — Mozilla Firefox", "MozillaWindowClass", r"C:\ff\firefox.exe"),
            window(2, 11, "Page — Mozilla Firefox", "MozillaWindowClass", r"C:\ff\firefox.exe"),
        ];
        let src = source(SourceKind::Browser {
            profile_dir: PathBuf::from(r"C:\p\abc"),
            start_url: None,
            browser: BrowserRef {
                name: "Firefox".into(),
                exe_path: PathBuf::from(r"C:\ff\firefox.exe"),
                family: BrowserFamily::Gecko,
            },
        });
        assert_eq!(find_window(&FakePlatform, &src, &windows).map(|w| w.handle), Some(2));
    }

    #[test]
    fn browser_windows_to_close_keep_the_source_window_last() {
        let windows = vec![
            window(1, 11, "Seconde fenêtre", "MozillaWindowClass", r"C:\ff\firefox.exe"),
            window(2, 10, "Autre profil", "MozillaWindowClass", r"C:\ff\firefox.exe"),
            window(3, 11, "Fenêtre de la source", "MozillaWindowClass", r"C:\ff\firefox.exe"),
            window(4, 12, "Discord", "Chrome_WidgetWin_1", r"C:\Discord\Discord.exe"),
        ];
        let browser = source(SourceKind::Browser {
            profile_dir: PathBuf::from(r"C:\p\abc"),
            start_url: None,
            browser: BrowserRef {
                name: "Firefox".into(),
                exe_path: PathBuf::from(r"C:\ff\firefox.exe"),
                family: BrowserFamily::Gecko,
            },
        });
        let application = source(SourceKind::Application {
            exe_path: r"C:\Discord\Discord.exe".into(),
            window_class: "Chrome_WidgetWin_1".into(),
            window_title: "Discord".into(),
            launch_args: vec![],
        });
        // La même source dans deux scènes : ses fenêtres ne sont comptées qu'une fois.
        let sources = [browser.clone(), application, browser];
        let handles: Vec<isize> =
            browser_windows(&FakePlatform, &sources, &windows).iter().map(|w| w.handle).collect();
        // `find_window` retient la première dans l'ordre d'empilement (1) :
        // elle passe en dernier.
        assert_eq!(handles, vec![3, 1]);
    }

    #[test]
    fn application_matching_tiers() {
        let exe = r"C:\Discord\app-1.0.2\Discord.exe";
        let windows = vec![
            window(1, 5, "Autre", "Chrome_WidgetWin_1", r"C:\Discord\app-1.0.2\Discord.exe"),
            window(2, 5, "#général - Discord", "Chrome_WidgetWin_1", r"C:\Discord\app-1.0.2\Discord.exe"),
        ];
        let exact = source(SourceKind::Application {
            exe_path: exe.into(),
            window_class: "Chrome_WidgetWin_1".into(),
            window_title: "#général - Discord".into(),
            launch_args: vec![],
        });
        assert_eq!(find_window(&FakePlatform, &exact, &windows).map(|w| w.handle), Some(2));

        // Titre changé : même classe, première fenêtre dans l'ordre d'empilement.
        let renamed = source(SourceKind::Application {
            exe_path: exe.into(),
            window_class: "Chrome_WidgetWin_1".into(),
            window_title: "#autre-salon - Discord".into(),
            launch_args: vec![],
        });
        assert_eq!(find_window(&FakePlatform, &renamed, &windows).map(|w| w.handle), Some(1));

        // Programme mis à jour dans un autre dossier.
        let updated = source(SourceKind::Application {
            exe_path: r"C:\Discord\app-1.0.1\Discord.exe".into(),
            window_class: "Chrome_WidgetWin_1".into(),
            window_title: "#général - Discord".into(),
            launch_args: vec![],
        });
        assert_eq!(find_window(&FakePlatform, &updated, &windows).map(|w| w.handle), Some(2));
    }

    #[test]
    fn unrelated_application_is_not_matched() {
        let windows = vec![window(1, 5, "Jeu", "UnityWndClass", r"C:\Jeux\jeu.exe")];
        let src = source(SourceKind::Application {
            exe_path: r"C:\Autre\programme.exe".into(),
            window_class: "Classe".into(),
            window_title: "Titre".into(),
            launch_args: vec![],
        });
        assert!(find_window(&FakePlatform, &src, &windows).is_none());
    }
}
