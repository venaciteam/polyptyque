//! Rendu de contrôle hors bureau, sans lancer de capture ni ouvrir de
//! configuration réelle. Lancer le test ignoré avec --ignored --nocapture.

use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::{ComponentHandle, ModelRc, Rgb8Pixel, VecModel};

use crate::{AppWindow, AssistantItem, Bridge};

struct Offscreen(Rc<MinimalSoftwareWindow>);

impl Platform for Offscreen {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.0.clone())
    }
}

#[test]
#[ignore = "rendu de contrôle manuel de la feuille des assistants"]
fn render_assistants_sheet() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Offscreen(window.clone()))).unwrap();
    crate::i18n::set_language(crate::i18n::Language::French);
    let ui = AppWindow::new().unwrap();
    let bridge = ui.global::<Bridge>();
    let rows: Vec<_> = crate::assistants::Assistant::ALL
        .into_iter()
        .map(|assistant| AssistantItem {
            id: assistant.id().into(),
            name: assistant.name().into(),
            state: if assistant == crate::assistants::Assistant::ClaudeDesktop {
                "Configuration enregistrée"
            } else {
                "Détecté"
            }
            .into(),
            detail: if assistant == crate::assistants::Assistant::ClaudeDesktop {
                assistant.next_step()
            } else {
                "Prêt à configurer. Seule la connexion Polyptyque sera ajoutée à cet assistant.".into()
            }
            .into(),
            tone: 0,
            configured: assistant == crate::assistants::Assistant::ClaudeDesktop,
            needs_repair: false,
            can_connect: true,
            can_disconnect: assistant == crate::assistants::Assistant::ClaudeDesktop,
        })
        .collect();
    bridge.set_assistants(ModelRc::new(VecModel::from(rows)));
    bridge.set_sheet("assistants".into());
    ui.show().unwrap();
    window.set_size(slint::PhysicalSize::new(1280, 840));
    let mut pixels = vec![Rgb8Pixel::default(); 1280 * 840];
    window.draw_if_needed(|renderer| {
        renderer.render(&mut pixels, 1280);
    });
    assert!(pixels.windows(2).any(|pair| pair[0] != pair[1]));
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    let path = dir.join("connexion-assistants.ppm");
    let mut data = b"P6\n1280 840\n255\n".to_vec();
    for pixel in pixels {
        data.extend_from_slice(&[pixel.r, pixel.g, pixel.b]);
    }
    std::fs::write(&path, data).unwrap();
    println!("{}", path.display());
    ui.hide().unwrap();
}
