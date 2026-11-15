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
use polyptyque_core::platform::{Platform, WindowHandle, WindowInfo};
use tr::tr;

/// Retrouve la fenêtre d'une source parmi `windows` (ordre d'empilement).
///
/// - Navigateur : la fenêtre principale du processus dont la ligne de
///   commande désigne le profil dédié. Le titre n'intervient pas.
/// - Application : même exécutable, même classe et même titre ; à défaut
///   même classe ; à défaut la seule fenêtre de ce programme.
pub fn find_window(platform: &dyn Platform, source: &Source, windows: &[WindowInfo]) -> Option<WindowInfo> {
    find_window_excluding(platform, source, windows, &Exclusions::default())
}

/// Fenêtres que les autres sources d'une session suivent ou attendent.
#[derive(Debug, Default, Clone)]
pub struct Exclusions {
    /// Déjà enregistrées par une autre source : jamais retenues.
    pub taken: HashSet<WindowHandle>,
    /// Même programme, même classe et même titre qu'une autre source : à
    /// elle d'abord. Retenues seulement si elles portent aussi exactement le
    /// titre de la source cherchée, jamais à défaut.
    pub reserved: HashSet<WindowHandle>,
}

impl Exclusions {
    /// Ce que `source` ne doit pas prendre parmi les sources `sources` d'une
    /// scène : les fenêtres `taken` (suivies par les autres), et à défaut de
    /// son titre exact celles qui correspondent exactement à une autre.
    pub fn among(source: &Source, sources: &[Source], taken: HashSet<WindowHandle>, windows: &[WindowInfo]) -> Self {
        let others: Vec<&Source> = sources.iter().filter(|s| s.id != source.id).collect();
        let mut exclusions = Exclusions { taken, ..Default::default() };
        exclusions.reserve_for(&others, windows);
        exclusions
    }

    /// Fenêtres de `windows` qui correspondent exactement à l'une des
    /// sources `others`.
    pub fn reserve_for(&mut self, others: &[&Source], windows: &[WindowInfo]) {
        for window in windows {
            if others.iter().any(|other| exact_match(other, window)) {
                self.reserved.insert(window.handle);
            }
        }
    }
}

/// La fenêtre porte exactement le programme, la classe et le titre d'une
/// source application.
fn exact_match(source: &Source, window: &WindowInfo) -> bool {
    let SourceKind::Application { exe_path, window_class, window_title, .. } = &source.kind else {
        return false;
    };
    (same_path(&window.exe, exe_path) || same_file_name(&window.exe, exe_path) || store_frame_of(exe_path, window))
        && &window.class == window_class
        && &window.title == window_title
}

/// Cadre des applications du Microsoft Store (UWP), et programme commun qui
/// le possède. Depuis le 2026-11-15, la plateforme attribue chaque cadre au
/// vrai processus de son application (`CalculatorApp.exe`, par exemple).
const STORE_FRAME_CLASS: &str = "ApplicationFrameWindow";
const STORE_FRAME_HOST: &str = "applicationframehost";

fn is_store_frame_host(exe: &Path) -> bool {
    file_stem(exe).as_deref() == Some(STORE_FRAME_HOST)
}

/// Source du Store enregistrée avant cette attribution : son programme est
/// `ApplicationFrameHost.exe`, et tout cadre du Store lui appartient, quel
/// que soit le programme auquel il est désormais attribué (comportement
/// d'avant).
fn store_frame_of(exe_path: &Path, window: &WindowInfo) -> bool {
    window.class == STORE_FRAME_CLASS && is_store_frame_host(exe_path)
}

/// Comme [`find_window`], sans jamais retenir une fenêtre déjà enregistrée
/// par une autre source de la session, ni à défaut celle qu'une autre
/// source attend (voir [`Exclusions`]) : deux sources d'un même programme
/// (deux VLC, deux applications Chrome) n'enregistrent jamais la même
/// fenêtre, ni l'une celle de l'autre.
///
/// Ordre de préférence pour une application, parmi les fenêtres libres de
/// ce programme : même classe et même titre ; puis même classe (la plus
/// haute dans l'ordre d'empilement, c'est-à-dire la dernière utilisée) ;
/// puis la seule fenêtre du programme. Une fois rattachée, une source garde
/// sa fenêtre (par sa poignée) tant qu'elle existe : ce choix ne se refait
/// qu'au démarrage ou après une perte.
pub fn find_window_excluding(
    platform: &dyn Platform,
    source: &Source,
    windows: &[WindowInfo],
    exclusions: &Exclusions,
) -> Option<WindowInfo> {
    let taken = &exclusions.taken;
    match &source.kind {
        SourceKind::Browser { profile_dir, browser, .. } => {
            let class = window_class(browser.family);
            let mut checked: HashMap<u32, bool> = HashMap::new();
            windows
                .iter()
                .filter(|w| w.class == class && !taken.contains(&w.handle))
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
            if is_store_frame_host(exe_path) {
                // Source du Store d'avant le 2026-11-15 : ses cadres portent
                // maintenant le programme de leur application.
                same_program = windows
                    .iter()
                    .filter(|w| store_frame_of(exe_path, w) || same_file_name(&w.exe, exe_path))
                    .collect();
            }
            let free: Vec<&WindowInfo> = same_program.iter().copied().filter(|w| !taken.contains(&w.handle)).collect();
            let unreserved = |w: &&&WindowInfo| !exclusions.reserved.contains(&w.handle);
            free.iter()
                .find(|w| &w.class == window_class && &w.title == window_title)
                .or_else(|| free.iter().filter(unreserved).find(|w| &w.class == window_class))
                .copied()
                // La seule fenêtre du programme, pas la seule restée libre :
                // une autre source peut suivre ses autres fenêtres.
                .or_else(|| {
                    (same_program.len() == 1 && free.len() == 1 && !exclusions.reserved.contains(&free[0].handle))
                        .then(|| free[0])
                })
                // Cadre du Store dont l'application n'a pas pu être reconnue
                // (réduit, en pause) : resté à ApplicationFrameHost.exe. Pris
                // seulement à son titre exact, en dernier recours.
                .or_else(|| {
                    (window_class == STORE_FRAME_CLASS)
                        .then(|| {
                            windows.iter().find(|w| {
                                w.class == STORE_FRAME_CLASS
                                    && is_store_frame_host(&w.exe)
                                    && &w.title == window_title
                                    && !taken.contains(&w.handle)
                            })
                        })
                        .flatten()
                })
                .cloned()
        }
    }
}

/// Pour le journal, quand la fenêtre d'un navigateur reste introuvable : le
/// navigateur tourne-t-il sur le profil, et quelles fenêtres de sa classe
/// existent (cachées comprises), avec ce qui a pu les écarter. Jamais de
/// titre ni de ligne de commande. `None` pour une application.
pub fn explain_missing(platform: &dyn Platform, source: &Source) -> Option<String> {
    let SourceKind::Browser { profile_dir, browser, .. } = &source.kind else {
        return None;
    };
    let class = window_class(browser.family);
    let mut lines = vec![format!(
        "navigateur actif sur le profil : {}",
        if profile_in_use(source) { "oui" } else { "non (ou inconnu)" }
    )];
    let mut command_lines: HashMap<u32, Option<String>> = HashMap::new();
    let windows = platform.window_diagnostics(class);
    if windows.is_empty() {
        lines.push(format!("aucune fenêtre {class}"));
    }
    for w in &windows {
        let mut flags = Vec::new();
        flags.push(if w.visible { "visible" } else { "cachée" });
        if w.cloaked {
            flags.push("masquée par Windows");
        }
        if w.owned {
            flags.push("rattachée à une autre fenêtre");
        }
        if w.tool_window {
            flags.push("fenêtre outil");
        }
        if !w.has_title {
            flags.push("sans titre");
        }
        if w.minimized {
            flags.push("réduite");
        }
        let profile = match command_lines.entry(w.pid).or_insert_with(|| platform.process_command_line(w.pid)) {
            None => "ligne de commande illisible",
            Some(line) if command_line_uses_profile(line, profile_dir) => "ce profil",
            Some(_) => "autre profil",
        };
        lines.push(format!("fenêtre {:#x}, processus {} : {} ; {profile}", w.handle, w.pid, flags.join(", ")));
    }
    Some(lines.join("\n  "))
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
    // Une fenêtre par source, avec les mêmes règles que l'enregistrement :
    // deux sources d'un même programme ne comptent jamais la même fenêtre.
    let mut taken: HashSet<WindowHandle> = HashSet::new();
    let mut find = |source: &Source, windows: &[WindowInfo]| {
        let exclusions = Exclusions::among(source, sources, taken.clone(), windows);
        let found = find_window_excluding(platform, source, windows, &exclusions);
        if let Some(window) = &found {
            taken.insert(window.handle);
        }
        found.is_some()
    };
    for source in sources {
        if find(source, &windows) {
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
            let found = find(source, &windows);
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
        SourceKind::Application { window_class, .. } if window_class == STORE_FRAME_CLASS => {
            // Une application du Store ne s'ouvre que par Windows (son
            // identifiant d'application, inconnu ici) : lancer directement
            // son programme (`…\WindowsApps\…`, ou ApplicationFrameHost.exe
            // pour une source ancienne) n'ouvre aucune fenêtre.
            anyhow::bail!(tr!("application du Microsoft Store, à ouvrir depuis le menu Démarrer"))
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
            vec![window(9, 9, "VLC", "Qt5QWindowIcon", r"C:\VLC\vlc.exe")]
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
        fn window_diagnostics(&self, _: &str) -> Vec<WindowDiagnostic> {
            let w = |handle, pid, visible, cloaked| WindowDiagnostic {
                handle,
                pid,
                visible,
                cloaked,
                owned: false,
                tool_window: false,
                has_title: true,
                minimized: false,
            };
            vec![w(1, 10, true, false), w(2, 11, true, true), w(3, 12, false, false)]
        }
        fn detect_browsers(&self) -> Vec<BrowserInstall> {
            vec![]
        }
        fn now_hns(&self) -> u64 {
            0
        }
        fn open_video_capture(
            &self,
            _: WindowHandle,
            _: u32,
            _: u32,
            _: bool,
            _: polyptyque_core::crop::Crop,
        ) -> Result<Box<dyn VideoCapture>> {
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
        Source { id: "x".into(), name: "x".into(), capture_cursor: true, relaunch: false, crop: Default::default(), kind }
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
    fn missing_browser_window_is_explained_without_titles() {
        let src = source(SourceKind::Browser {
            profile_dir: PathBuf::from(r"C:\p\abc"),
            start_url: None,
            browser: BrowserRef {
                name: "Firefox".into(),
                exe_path: PathBuf::from(r"C:\ff\firefox.exe"),
                family: BrowserFamily::Gecko,
            },
        });
        let report = explain_missing(&FakePlatform, &src).unwrap();
        assert!(report.contains("processus 10 : visible ; autre profil"), "{report}");
        assert!(report.contains("processus 11 : visible, masquée par Windows ; ce profil"), "{report}");
        assert!(report.contains("processus 12 : cachée ; ligne de commande illisible"), "{report}");

        let application = source(SourceKind::Application {
            exe_path: r"C:\x\app.exe".into(),
            window_class: "Classe".into(),
            window_title: "Titre".into(),
            launch_args: vec![],
        });
        assert!(explain_missing(&FakePlatform, &application).is_none());
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
    fn windows_of_other_sources_are_never_taken() {
        let exe = r"C:\VLC\vlc.exe";
        let windows = vec![
            window(1, 5, "film1.mkv - VLC", "Qt5QWindowIcon", exe),
            window(2, 6, "film2.mkv - VLC", "Qt5QWindowIcon", exe),
        ];
        let vlc = |title: &str| {
            source(SourceKind::Application {
                exe_path: exe.into(),
                window_class: "Qt5QWindowIcon".into(),
                window_title: title.into(),
                launch_args: vec![],
            })
        };
        // Titres changés : sans exclusion, les deux sources prendraient la 1.
        let first = vlc("autre1 - VLC");
        let second = vlc("autre2 - VLC");
        let none = Exclusions::default();
        assert_eq!(find_window_excluding(&FakePlatform, &first, &windows, &none).map(|w| w.handle), Some(1));
        let taken = Exclusions { taken: HashSet::from([1]), ..Default::default() };
        assert_eq!(find_window_excluding(&FakePlatform, &second, &windows, &taken).map(|w| w.handle), Some(2));
        // Titre exact d'une fenêtre déjà prise : jamais retenue.
        assert_eq!(
            find_window_excluding(&FakePlatform, &vlc("film1.mkv - VLC"), &windows, &taken).map(|w| w.handle),
            Some(2)
        );
        // Plus aucune fenêtre libre.
        let all = Exclusions { taken: HashSet::from([1, 2]), ..Default::default() };
        assert!(find_window_excluding(&FakePlatform, &second, &windows, &all).is_none());

        // Seule fenêtre du programme, d'une autre classe : retenue si libre,
        // jamais si une autre source la suit.
        let single = vec![window(3, 7, "Discord", "Autre", r"C:\Discord\Discord.exe")];
        let discord = source(SourceKind::Application {
            exe_path: r"C:\Discord\Discord.exe".into(),
            window_class: "Chrome_WidgetWin_1".into(),
            window_title: "Discord".into(),
            launch_args: vec![],
        });
        assert_eq!(find_window(&FakePlatform, &discord, &single).map(|w| w.handle), Some(3));
        let taken = Exclusions { taken: HashSet::from([3]), ..Default::default() };
        assert!(find_window_excluding(&FakePlatform, &discord, &single, &taken).is_none());
    }

    #[test]
    fn window_awaited_by_another_source_is_left_to_it() {
        let exe = r"C:\VLC\vlc.exe";
        let windows = vec![
            window(1, 5, "film2.mkv - VLC", "Qt5QWindowIcon", exe),
            window(2, 6, "film3.mkv - VLC", "Qt5QWindowIcon", exe),
        ];
        let vlc = |title: &str| {
            source(SourceKind::Application {
                exe_path: exe.into(),
                window_class: "Qt5QWindowIcon".into(),
                window_title: title.into(),
                launch_args: vec![],
            })
        };
        // La première source a changé de film ; la seconde attend « film2 »,
        // la plus haute : la première prend l'autre fenêtre, quel que soit
        // l'ordre de démarrage.
        let first = vlc("film1.mkv - VLC");
        let second = vlc("film2.mkv - VLC");
        let mut exclusions = Exclusions::default();
        exclusions.reserve_for(&[&second], &windows);
        assert_eq!(exclusions.reserved, HashSet::from([1]));
        assert_eq!(find_window_excluding(&FakePlatform, &first, &windows, &exclusions).map(|w| w.handle), Some(2));
        // Deux sources au même titre : la fenêtre reste à qui la prend.
        let mut same = Exclusions::default();
        same.reserve_for(&[&second], &windows);
        assert_eq!(find_window_excluding(&FakePlatform, &second, &windows, &same).map(|w| w.handle), Some(1));
        // Seule fenêtre du programme, attendue par une autre : pas à défaut.
        let alone = vec![windows[0].clone()];
        let mut reserved = Exclusions::default();
        reserved.reserve_for(&[&second], &alone);
        assert!(find_window_excluding(&FakePlatform, &first, &alone, &reserved).is_none());
    }

    #[test]
    fn prepared_windows_are_one_per_source() {
        let vlc = |id: &str| {
            let kind = SourceKind::Application {
                exe_path: r"C:\VLC\vlc.exe".into(),
                window_class: "Qt5QWindowIcon".into(),
                window_title: "VLC".into(),
                launch_args: vec![],
            };
            Source { id: id.into(), ..source(kind) }
        };
        let sources = [vlc("a"), vlc("b")];
        let mut found = Vec::new();
        let missing = prepare_windows(&FakePlatform, &sources, Duration::ZERO, |event| {
            if let PrepareEvent::Found(source) = event {
                found.push(source.id.clone());
            }
        });
        // Une seule fenêtre VLC (celle de FakePlatform) : une seule source la
        // compte, l'autre manque.
        assert_eq!(found, ["a"]);
        assert_eq!(missing, [sources[1].name.clone()]);
    }

    #[test]
    fn browser_window_of_another_source_is_skipped() {
        let windows = vec![
            window(1, 11, "Page", "MozillaWindowClass", r"C:\ff\firefox.exe"),
            window(2, 11, "Page", "MozillaWindowClass", r"C:\ff\firefox.exe"),
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
        let taken = Exclusions { taken: HashSet::from([1]), ..Default::default() };
        assert_eq!(find_window_excluding(&FakePlatform, &src, &windows, &taken).map(|w| w.handle), Some(2));
    }

    #[test]
    fn store_sources_match_frames_of_either_attribution() {
        let host = r"C:\Windows\System32\ApplicationFrameHost.exe";
        let calc = r"C:\Program Files\WindowsApps\Microsoft.WindowsCalculator_1_x64__8wekyb3d8bbwe\CalculatorApp.exe";
        let photos = r"C:\Program Files\WindowsApps\Microsoft.Windows.Photos_1_x64__8wekyb3d8bbwe\Photos.exe";
        let frame = "ApplicationFrameWindow";
        let store = |exe: &str, title: &str| {
            source(SourceKind::Application {
                exe_path: exe.into(),
                window_class: frame.into(),
                window_title: title.into(),
                launch_args: vec![],
            })
        };
        let windows = vec![window(1, 5, "Photos", frame, photos), window(2, 6, "Calculatrice", frame, calc)];
        // Source enregistrée avant l'attribution au vrai processus.
        let old = store(host, "Calculatrice");
        assert_eq!(find_window(&FakePlatform, &old, &windows).map(|w| w.handle), Some(2));
        // Titre changé : à défaut, même classe (comme avant).
        assert_eq!(find_window(&FakePlatform, &store(host, "Autre"), &windows).map(|w| w.handle), Some(1));
        // Sa fenêtre exacte reste réservée face à une autre source ancienne.
        let mut exclusions = Exclusions::default();
        exclusions.reserve_for(&[&old], &windows);
        assert_eq!(exclusions.reserved, HashSet::from([2]));
        assert_eq!(
            find_window_excluding(&FakePlatform, &store(host, "Autre"), &windows, &exclusions).map(|w| w.handle),
            Some(1)
        );
        // Cadre encore attribué à ApplicationFrameHost.exe (réduit).
        let unresolved = vec![window(3, 7, "Calculatrice", frame, host)];
        assert_eq!(find_window(&FakePlatform, &old, &unresolved).map(|w| w.handle), Some(3));
        // Source récente : ce cadre seulement à son titre exact.
        assert_eq!(find_window(&FakePlatform, &store(calc, "Calculatrice"), &unresolved).map(|w| w.handle), Some(3));
        assert!(find_window(&FakePlatform, &store(calc, "Autre"), &unresolved).is_none());
        let taken = Exclusions { taken: HashSet::from([3]), ..Default::default() };
        assert!(find_window_excluding(&FakePlatform, &store(calc, "Calculatrice"), &unresolved, &taken).is_none());
        // Une source récente ne prend jamais le cadre d'une autre application.
        assert!(find_window(&FakePlatform, &store(calc, "Calculatrice"), &windows[..1]).is_none());
        // Jamais de lancement direct d'une application du Store.
        assert!(launch(&old).is_err());
        assert!(launch(&store(calc, "Calculatrice")).is_err());
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
