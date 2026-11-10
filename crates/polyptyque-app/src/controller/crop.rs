//! Rognage d'une source : l'éditeur (aperçu de la fenêtre entière, cadre à
//! poignées, saisie des marges, clavier) et son enregistrement avec la
//! source.
//!
//! Le cadre se règle en pixels de la fenêtre ; les calculs (format imposé,
//! bords de la fenêtre, taille minimale) sont ceux de
//! [`polyptyque_core::crop`]. Rien n'est enregistré avant « Appliquer ».

use polyptyque_core::crop::{Crop, CropRatio, Handle, Side};
use slint::{Image, ModelRc, SharedString, VecModel};
use tr::tr;

use super::App;
use crate::preview::{self, PreviewState, PreviewWorker};

/// Éditeur de rognage ouvert.
pub(super) struct CropEditor {
    source_id: String,
    source_name: String,
    /// Aperçu de la fenêtre entière (aucun si les fils ne démarrent pas).
    worker: Option<PreviewWorker>,
    crop: Crop,
    /// Rognage au moment où la souris a été enfoncée sur le cadre.
    drag_start: Crop,
    /// Taille de la fenêtre, connue à la première image.
    window: Option<(u32, u32)>,
}

impl App {
    /// Ouvre l'éditeur de rognage d'une source de la scène active.
    pub(super) fn open_crop(&self, id: &str) {
        if self.locked() {
            self.toast(&tr!("Le rognage ne se change pas pendant un enregistrement."), 2);
            return;
        }
        let Some(source) = self.active_source(id) else {
            return;
        };
        let index = self.config.borrow().active_scene().source_ids.iter().position(|s| s == id);
        let worker = preview::start_whole(&self.platform, &source);
        let choices: Vec<SharedString> = CropRatio::ALL.iter().map(|r| r.label().into()).collect();
        *self.crop.borrow_mut() = Some(CropEditor {
            source_id: source.id.clone(),
            source_name: source.name.clone(),
            worker,
            crop: source.crop,
            drag_start: source.crop,
            window: None,
        });
        self.with_bridge(|b| {
            if let Some(index) = index {
                b.set_selected_source(index as i32);
            }
            b.set_crop_source_id(source.id.as_str().into());
            b.set_crop_source_name(source.name.as_str().into());
            b.set_crop_image(Image::default());
            b.set_crop_has_image(false);
            b.set_crop_placeholder(tr!("Recherche de la fenêtre…").into());
            b.set_crop_can_open(false);
            b.set_crop_can_restore(false);
            b.set_crop_window_width(0);
            b.set_crop_window_height(0);
            b.set_crop_ratio_choices(ModelRc::new(VecModel::from(choices)));
            b.set_crop_handle(0);
            b.set_sheet("crop".into());
        });
        self.show_crop();
    }

    /// Ferme l'éditeur sans rien enregistrer (son aperçu s'arrête).
    pub(super) fn close_crop(&self) {
        self.crop.borrow_mut().take();
    }

    /// Marges et format de l'éditeur, vers l'interface.
    fn show_crop(&self) {
        let Some(crop) = self.crop.borrow().as_ref().map(|e| e.crop) else {
            return;
        };
        self.with_bridge(|b| {
            b.set_crop_left(crop.left as i32);
            b.set_crop_top(crop.top as i32);
            b.set_crop_right(crop.right as i32);
            b.set_crop_bottom(crop.bottom as i32);
            b.set_crop_ratio_index(crop.ratio.index() as i32);
        });
    }

    /// Change le rognage en cours d'édition : `f` reçoit l'éditeur et la
    /// taille de la fenêtre (rien tant qu'elle n'est pas connue).
    fn edit_crop(&self, f: impl FnOnce(&mut CropEditor, u32, u32)) {
        {
            let mut editor = self.crop.borrow_mut();
            let Some(editor) = editor.as_mut() else {
                return;
            };
            let Some((width, height)) = editor.window else {
                return;
            };
            f(editor, width, height);
        }
        self.show_crop();
    }

    /// Nouvelle image et nouvel état de l'aperçu de l'éditeur.
    pub(super) fn tick_crop(&self) {
        let (frame, state, size) = {
            let editor = self.crop.borrow();
            let Some(worker) = editor.as_ref().and_then(|e| e.worker.as_ref()) else {
                return;
            };
            let (frame, state) = worker.take_update();
            (frame, state, worker.content_size())
        };
        // Taille de la fenêtre connue ou changée : le cadre y est ramené.
        let resized = {
            let mut editor = self.crop.borrow_mut();
            match (editor.as_mut(), size) {
                (Some(editor), Some(size)) if editor.window != Some(size) => {
                    editor.window = Some(size);
                    editor.crop = editor.crop.normalized(size.0, size.1);
                    true
                }
                _ => false,
            }
        };
        if resized {
            if let Some((width, height)) = size {
                self.with_bridge(|b| {
                    b.set_crop_window_width(width as i32);
                    b.set_crop_window_height(height as i32);
                });
            }
            self.show_crop();
        }
        self.with_bridge(|b| {
            if let Some(frame) = frame {
                b.set_crop_image(Image::from_rgba8(frame));
                b.set_crop_has_image(true);
            }
            if let Some(state) = state {
                let placeholder = match &state {
                    PreviewState::Searching => tr!("Recherche de la fenêtre…"),
                    PreviewState::Live => String::new(),
                    PreviewState::Minimized => tr!("Fenêtre réduite : la rétablir pour la rogner."),
                    PreviewState::Missing => tr!("Fenêtre introuvable : l'ouvrir pour la rogner."),
                    PreviewState::Failed(e) => tr!("Aperçu impossible : {}", e),
                };
                // Une fenêtre réduite ne donne plus d'image : on le dit.
                if !matches!(state, PreviewState::Live) {
                    b.set_crop_has_image(false);
                }
                b.set_crop_placeholder(placeholder.into());
                b.set_crop_can_open(matches!(state, PreviewState::Missing));
                b.set_crop_can_restore(matches!(state, PreviewState::Minimized));
            }
        });
    }

    /// Souris enfoncée sur le cadre : les déplacements suivants partent
    /// d'ici.
    pub(super) fn crop_drag_start(&self) {
        if let Some(editor) = self.crop.borrow_mut().as_mut() {
            editor.drag_start = editor.crop;
        }
    }

    /// Glisser une poignée : décalage depuis l'appui, en pixels de la
    /// fenêtre.
    pub(super) fn crop_drag(&self, handle: i32, dx: i32, dy: i32) {
        let Some(handle) = Handle::from_index(handle) else {
            return;
        };
        self.edit_crop(|editor, width, height| {
            editor.crop = editor.drag_start.dragged(handle, dx as i64, dy as i64, width, height);
        });
    }

    /// Flèches du clavier : la poignée choisie bouge de `dx`, `dy` pixels.
    pub(super) fn crop_nudge(&self, dx: i32, dy: i32) {
        let handle = Handle::from_index(self.with_bridge_value(|b| b.get_crop_handle())).unwrap_or(Handle::Frame);
        self.edit_crop(|editor, width, height| {
            editor.crop = editor.crop.dragged(handle, dx as i64, dy as i64, width, height);
        });
    }

    /// Marge saisie pour un bord (0 gauche, 1 haut, 2 droite, 3 bas).
    pub(super) fn crop_margin_edited(&self, side: i32, value: i32) {
        let Some(side) = Side::from_index(side) else {
            return;
        };
        let value = value.max(0) as u32;
        self.edit_crop(|editor, width, height| {
            editor.crop = editor.crop.with_margin(side, value, width, height);
        });
    }

    /// Format choisi dans la liste.
    pub(super) fn crop_ratio_selected(&self, index: i32) {
        let ratio = CropRatio::from_index(index);
        let known = self.crop.borrow().as_ref().is_some_and(|e| e.window.is_some());
        if !known {
            // Fenêtre pas encore vue : le format sera appliqué à son arrivée.
            if let Some(editor) = self.crop.borrow_mut().as_mut() {
                editor.crop.ratio = ratio;
            }
            self.show_crop();
            return;
        }
        self.edit_crop(|editor, width, height| {
            editor.crop = editor.crop.with_ratio(ratio, width, height);
        });
    }

    /// Plus aucune marge (le format choisi reste).
    pub(super) fn crop_reset(&self) {
        let known = self.crop.borrow().as_ref().is_some_and(|e| e.window.is_some());
        if !known {
            if let Some(editor) = self.crop.borrow_mut().as_mut() {
                editor.crop = Crop { ratio: editor.crop.ratio, ..Crop::default() };
            }
            self.show_crop();
            return;
        }
        self.edit_crop(|editor, width, height| {
            editor.crop = editor.crop.reset(width, height);
        });
    }

    /// Enregistre le rognage avec la source : aperçus et enregistrements
    /// suivants l'utilisent, ainsi que toutes les scènes qui la partagent.
    pub(super) fn crop_apply(&self) {
        let Some(editor) = self.crop.borrow_mut().take() else {
            return;
        };
        self.with_bridge(|b| b.set_sheet("".into()));
        if self.locked() {
            self.toast(&tr!("Le rognage ne se change pas pendant un enregistrement."), 2);
            return;
        }
        let crop = match editor.window {
            Some((width, height)) => editor.crop.normalized(width, height),
            None => editor.crop,
        };
        let changed = {
            let mut config = self.config.borrow_mut();
            let Some(source) = config.sources.iter_mut().find(|s| s.id == editor.source_id) else {
                drop(config);
                self.toast(&tr!("Cette source a été supprimée entre-temps."), 2);
                return;
            };
            let changed = source.crop != crop;
            source.crop = crop;
            changed
        };
        if !changed {
            return;
        }
        log::info!("rognage de la source {} : {:?}", editor.source_id, crop);
        self.save();
        self.refresh_all();
        self.restart_previews();
        let message = if crop.is_none() {
            tr!("« {} » n'est plus rognée.", editor.source_name)
        } else {
            tr!("Rognage de « {} » enregistré.", editor.source_name)
        };
        self.toast(&message, 1);
    }
}
