//! Rognage d'une source : ce qui est retiré de chaque bord de la fenêtre,
//! en pixels de la fenêtre, et le format éventuellement imposé à la zone
//! gardée.
//!
//! Les marges sont comptées depuis les bords : une barre d'outils ou une
//! bordure retirée le reste quand la fenêtre change de taille. La zone
//! gardée ne descend jamais sous [`MIN_SIZE`] pixels, et garde son format
//! s'il est imposé (elle est alors réduite, centrée, à ce format).

use serde::{Deserialize, Serialize};
use tr::tr;

/// Plus petite zone gardée, en pixels de la fenêtre.
pub const MIN_SIZE: u32 = 16;

/// Format imposé à la zone gardée.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CropRatio {
    #[default]
    #[serde(rename = "free")]
    Free,
    #[serde(rename = "16:9")]
    R16x9,
    #[serde(rename = "4:3")]
    R4x3,
    #[serde(rename = "3:2")]
    R3x2,
    #[serde(rename = "1:1")]
    R1x1,
    #[serde(rename = "4:5")]
    R4x5,
    #[serde(rename = "9:16")]
    R9x16,
    #[serde(rename = "21:9")]
    R21x9,
}

impl CropRatio {
    /// Formats proposés, dans l'ordre de la liste.
    pub const ALL: [CropRatio; 8] = [
        CropRatio::Free,
        CropRatio::R16x9,
        CropRatio::R4x3,
        CropRatio::R3x2,
        CropRatio::R1x1,
        CropRatio::R4x5,
        CropRatio::R9x16,
        CropRatio::R21x9,
    ];

    /// Largeur et hauteur relatives ; `None` : format libre.
    pub fn value(self) -> Option<(u32, u32)> {
        match self {
            CropRatio::Free => None,
            CropRatio::R16x9 => Some((16, 9)),
            CropRatio::R4x3 => Some((4, 3)),
            CropRatio::R3x2 => Some((3, 2)),
            CropRatio::R1x1 => Some((1, 1)),
            CropRatio::R4x5 => Some((4, 5)),
            CropRatio::R9x16 => Some((9, 16)),
            CropRatio::R21x9 => Some((21, 9)),
        }
    }

    pub fn label(self) -> String {
        match self.value() {
            None => tr!("Format de rognage" => "Libre"),
            Some((w, h)) => format!("{w}:{h}"),
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|r| *r == self).unwrap_or(0)
    }

    pub fn from_index(index: i32) -> Self {
        usize::try_from(index).ok().and_then(|i| Self::ALL.get(i).copied()).unwrap_or_default()
    }
}

/// Poignée du cadre de rognage : le cadre entier (déplacé d'un bloc), un
/// coin ou un bord. Rangées dans le sens des aiguilles d'une montre, à
/// partir du coin en haut à gauche.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    Frame,
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl Handle {
    const ALL: [Handle; 9] = [
        Handle::Frame,
        Handle::TopLeft,
        Handle::Top,
        Handle::TopRight,
        Handle::Right,
        Handle::BottomRight,
        Handle::Bottom,
        Handle::BottomLeft,
        Handle::Left,
    ];

    /// Numéro donné par l'interface (0 : cadre entier, puis 1 à 8).
    pub fn from_index(index: i32) -> Option<Self> {
        usize::try_from(index).ok().and_then(|i| Self::ALL.get(i).copied())
    }

    fn moves_left(self) -> bool {
        matches!(self, Handle::TopLeft | Handle::Left | Handle::BottomLeft)
    }

    fn moves_right(self) -> bool {
        matches!(self, Handle::TopRight | Handle::Right | Handle::BottomRight)
    }

    fn moves_top(self) -> bool {
        matches!(self, Handle::TopLeft | Handle::Top | Handle::TopRight)
    }

    fn moves_bottom(self) -> bool {
        matches!(self, Handle::BottomLeft | Handle::Bottom | Handle::BottomRight)
    }
}

/// Bord de la fenêtre, pour saisir une marge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Top,
    Right,
    Bottom,
}

impl Side {
    /// Numéro donné par l'interface : 0 gauche, 1 haut, 2 droite, 3 bas.
    pub fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Side::Left),
            1 => Some(Side::Top),
            2 => Some(Side::Right),
            3 => Some(Side::Bottom),
            _ => None,
        }
    }
}

/// Rognage d'une source. Par défaut : rien n'est retiré, format libre.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Crop {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
    pub ratio: CropRatio,
}

/// Zone `(x, y, largeur, hauteur)` en nombres à virgule, pendant un calcul.
#[derive(Clone, Copy)]
struct Area {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Crop {
    /// Aucun rognage : la fenêtre entière, en format libre.
    pub fn is_none(&self) -> bool {
        *self == Crop::default()
    }

    /// Zone gardée `(x, y, largeur, hauteur)` dans une fenêtre de `width` ×
    /// `height` pixels : les marges, ramenées dans la fenêtre si elle est
    /// devenue trop petite, puis réduites au format imposé (centrées).
    pub fn region(&self, width: u32, height: u32) -> (u32, u32, u32, u32) {
        if width == 0 || height == 0 {
            return (0, 0, width, height);
        }
        let (x, w) = span(self.left, self.right, width);
        let (y, h) = span(self.top, self.bottom, height);
        match self.ratio.value() {
            None => (x, y, w, h),
            Some((a, b)) => {
                let (fw, fh) = fit_ratio(w, h, a, b);
                (x + (w - fw) / 2, y + (h - fh) / 2, fw, fh)
            }
        }
    }

    /// Rognage qui garde exactement cette zone de la fenêtre.
    fn from_region(x: u32, y: u32, w: u32, h: u32, width: u32, height: u32, ratio: CropRatio) -> Crop {
        Crop {
            left: x,
            top: y,
            right: width.saturating_sub(x + w),
            bottom: height.saturating_sub(y + h),
            ratio,
        }
    }

    /// Change de format : la plus grande zone à ce format, centrée dans la
    /// zone gardée jusqu'ici.
    pub fn with_ratio(&self, ratio: CropRatio, width: u32, height: u32) -> Crop {
        let free = Crop { ratio: CropRatio::Free, ..*self };
        let (x, y, w, h) = free.region(width, height);
        let (x, y, w, h) = Crop { ratio, ..Crop::default() }.region_within(x, y, w, h);
        Crop::from_region(x, y, w, h, width, height, ratio)
    }

    /// Plus aucune marge ; le format imposé reste (zone la plus grande).
    pub fn reset(&self, width: u32, height: u32) -> Crop {
        Crop { ratio: CropRatio::Free, ..Crop::default() }.with_ratio(self.ratio, width, height)
    }

    fn region_within(&self, x: u32, y: u32, w: u32, h: u32) -> (u32, u32, u32, u32) {
        let (rx, ry, rw, rh) = self.region(w, h);
        (x + rx, y + ry, rw, rh)
    }

    /// Les marges ramenées dans la fenêtre et au format (ce que montre
    /// l'éditeur).
    pub fn normalized(&self, width: u32, height: u32) -> Crop {
        let (x, y, w, h) = self.region(width, height);
        Crop::from_region(x, y, w, h, width, height, self.ratio)
    }

    /// Déplace une poignée de `dx`, `dy` pixels de la fenêtre (souris ou
    /// clavier) : le cadre entier glisse sans changer de taille, un bord ou
    /// un coin redimensionne la zone, en gardant le format imposé (le bord
    /// ou le coin opposé reste en place).
    pub fn dragged(&self, handle: Handle, dx: i64, dy: i64, width: u32, height: u32) -> Crop {
        if width == 0 || height == 0 {
            return *self;
        }
        let (x, y, w, h) = self.region(width, height);
        let (fw, fh) = (width as f64, height as f64);
        let start = Area { x: x as f64, y: y as f64, w: w as f64, h: h as f64 };
        let (dx, dy) = (dx as f64, dy as f64);
        let area = if handle == Handle::Frame {
            Area {
                x: (start.x + dx).clamp(0.0, fw - start.w),
                y: (start.y + dy).clamp(0.0, fh - start.h),
                ..start
            }
        } else {
            match self.ratio.value() {
                None => resize_free(start, handle, dx, dy, fw, fh),
                Some((a, b)) => resize_locked(start, handle, dx, dy, fw, fh, a as f64 / b as f64),
            }
        };
        let w = (area.w.round() as u32).clamp(1, width);
        let h = (area.h.round() as u32).clamp(1, height);
        let x = (area.x.round().max(0.0) as u32).min(width - w);
        let y = (area.y.round().max(0.0) as u32).min(height - h);
        Crop::from_region(x, y, w, h, width, height, self.ratio)
    }

    /// Marge saisie pour un bord : comme si ce bord était déplacé jusque-là.
    pub fn with_margin(&self, side: Side, value: u32, width: u32, height: u32) -> Crop {
        let current = self.normalized(width, height);
        let delta = |old: u32| value as i64 - old as i64;
        match side {
            Side::Left => current.dragged(Handle::Left, delta(current.left), 0, width, height),
            Side::Top => current.dragged(Handle::Top, 0, delta(current.top), width, height),
            Side::Right => current.dragged(Handle::Right, -delta(current.right), 0, width, height),
            Side::Bottom => current.dragged(Handle::Bottom, 0, -delta(current.bottom), width, height),
        }
    }
}

/// Position et longueur gardées sur un axe de `total` pixels, avec ces
/// marges : au moins [`MIN_SIZE`] pixels (ou toute la fenêtre si elle est
/// plus petite).
fn span(start: u32, end: u32, total: u32) -> (u32, u32) {
    let min = MIN_SIZE.min(total);
    let first = start.min(total - min);
    let last = total.saturating_sub(end).max(first + min).min(total);
    (first, last - first)
}

/// Plus grande zone au format `a`:`b` dans `w` × `h`.
fn fit_ratio(w: u32, h: u32, a: u32, b: u32) -> (u32, u32) {
    let (w64, h64, a, b) = (w as u64, h as u64, a as u64, b as u64);
    if w64 * b > h64 * a {
        // Trop large : toute la hauteur.
        (((h64 * a + b / 2) / b).clamp(1, w64) as u32, h)
    } else {
        (w, ((w64 * b + a / 2) / a).clamp(1, h64) as u32)
    }
}

fn resize_free(start: Area, handle: Handle, dx: f64, dy: f64, fw: f64, fh: f64) -> Area {
    let min = MIN_SIZE as f64;
    let (mut x0, mut y0) = (start.x, start.y);
    let (mut x1, mut y1) = (start.x + start.w, start.y + start.h);
    if handle.moves_left() {
        x0 = (x0 + dx).clamp(0.0, (x1 - min).max(0.0));
    }
    if handle.moves_right() {
        x1 = (x1 + dx).clamp((x0 + min).min(fw), fw);
    }
    if handle.moves_top() {
        y0 = (y0 + dy).clamp(0.0, (y1 - min).max(0.0));
    }
    if handle.moves_bottom() {
        y1 = (y1 + dy).clamp((y0 + min).min(fh), fh);
    }
    Area { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
}

/// Redimensionnement au format `ratio` (largeur / hauteur) : le coin ou le
/// bord opposé reste en place ; pour un bord, la zone reste centrée sur
/// l'autre axe (décalée si elle sortirait de la fenêtre).
fn resize_locked(start: Area, handle: Handle, dx: f64, dy: f64, fw: f64, fh: f64, ratio: f64) -> Area {
    let min_w = (MIN_SIZE as f64).max(MIN_SIZE as f64 * ratio).min(fw).min(fh * ratio);
    let horizontal = handle.moves_left() || handle.moves_right();
    let vertical = handle.moves_top() || handle.moves_bottom();
    // Côté fixe : à gauche du cadre si l'on tire à droite, etc.
    let anchor_x = if handle.moves_left() { start.x + start.w } else { start.x };
    let anchor_y = if handle.moves_top() { start.y + start.h } else { start.y };
    let room_x = if handle.moves_left() { anchor_x } else { fw - anchor_x };
    let room_y = if handle.moves_top() { anchor_y } else { fh - anchor_y };
    let grow_x = if handle.moves_left() { -dx } else { dx };
    let grow_y = if handle.moves_top() { -dy } else { dy };

    let wanted = match (horizontal, vertical) {
        // Coin : le pointeur ramené sur la diagonale du format.
        (true, true) => {
            let (px, py) = (start.w + grow_x, start.h + grow_y);
            (px * ratio + py) / (ratio * ratio + 1.0) * ratio
        }
        (true, false) => start.w + grow_x,
        _ => (start.h + grow_y) * ratio,
    };
    let max_w = match (horizontal, vertical) {
        (true, true) => room_x.min(room_y * ratio),
        (true, false) => room_x.min(fh * ratio),
        _ => fw.min(room_y * ratio),
    };
    let w = wanted.clamp(min_w.min(max_w), max_w);
    let h = w / ratio;
    let x = if horizontal {
        if handle.moves_left() { anchor_x - w } else { anchor_x }
    } else {
        (start.x + start.w / 2.0 - w / 2.0).clamp(0.0, fw - w)
    };
    let y = if vertical {
        if handle.moves_top() { anchor_y - h } else { anchor_y }
    } else {
        (start.y + start.h / 2.0 - h / 2.0).clamp(0.0, fh - h)
    };
    Area { x, y, w, h }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crop(left: u32, top: u32, right: u32, bottom: u32) -> Crop {
        Crop { left, top, right, bottom, ratio: CropRatio::Free }
    }

    #[test]
    fn no_crop_keeps_whole_window() {
        assert!(Crop::default().is_none());
        assert_eq!(Crop::default().region(1920, 1080), (0, 0, 1920, 1080));
    }

    #[test]
    fn margins_are_counted_from_edges() {
        let c = crop(10, 80, 20, 0);
        assert_eq!(c.region(1920, 1080), (10, 80, 1890, 1000));
        // Fenêtre agrandie : la barre du haut reste retirée.
        assert_eq!(c.region(2560, 1440), (10, 80, 2530, 1360));
    }

    #[test]
    fn too_small_window_keeps_minimum() {
        let c = crop(900, 0, 900, 0);
        let (x, _, w, _) = c.region(1000, 500);
        assert_eq!(w, MIN_SIZE);
        assert!(x + w <= 1000);
        // Fenêtre minuscule : elle entière.
        assert_eq!(c.region(8, 8), (0, 0, 8, 8));
    }

    #[test]
    fn ratio_shrinks_centered() {
        let c = Crop { ratio: CropRatio::R4x3, ..Crop::default() };
        assert_eq!(c.region(1920, 1080), (240, 0, 1440, 1080));
        let c = Crop { ratio: CropRatio::R9x16, ..Crop::default() };
        let (_, _, w, h) = c.region(1920, 1080);
        assert_eq!((w, h), (608, 1080));
    }

    #[test]
    fn choosing_ratio_fits_current_zone() {
        let c = crop(100, 100, 100, 100).with_ratio(CropRatio::R1x1, 1920, 1080);
        assert_eq!(c.region(1920, 1080), (520, 100, 880, 880));
        assert_eq!(c.ratio, CropRatio::R1x1);
        // Le rognage enregistré correspond exactement à la zone montrée.
        assert_eq!(c.normalized(1920, 1080), c);
    }

    #[test]
    fn frame_moves_without_resizing() {
        let c = crop(100, 100, 100, 100);
        let moved = c.dragged(Handle::Frame, 50, -30, 1920, 1080);
        assert_eq!(moved.region(1920, 1080), (150, 70, 1720, 880));
        // Arrêté par le bord de la fenêtre.
        let moved = c.dragged(Handle::Frame, -500, 0, 1920, 1080);
        assert_eq!(moved.region(1920, 1080), (0, 100, 1720, 880));
    }

    #[test]
    fn free_edges_and_corners() {
        let c = Crop::default();
        assert_eq!(c.dragged(Handle::Top, 0, 80, 1920, 1080).region(1920, 1080), (0, 80, 1920, 1000));
        assert_eq!(c.dragged(Handle::BottomRight, -20, -40, 1920, 1080).region(1920, 1080), (0, 0, 1900, 1040));
        // Un bord ne passe jamais l'autre.
        let squeezed = c.dragged(Handle::Left, 5000, 0, 1920, 1080);
        assert_eq!(squeezed.region(1920, 1080).2, MIN_SIZE);
    }

    #[test]
    fn locked_corner_keeps_ratio_and_anchor() {
        let c = Crop { ratio: CropRatio::R16x9, ..Crop::default() };
        let small = c.dragged(Handle::BottomRight, -320, -180, 1920, 1080);
        assert_eq!(small.region(1920, 1080), (0, 0, 1600, 900));
        // Pointeur déplacé sur un seul axe : la zone suit quand même.
        let (_, _, w, h) = c.dragged(Handle::BottomRight, -320, 0, 1920, 1080).region(1920, 1080);
        assert!(w < 1920 && w.abs_diff(h * 16 / 9) <= 1, "{w}×{h}");
        let left = small.dragged(Handle::TopLeft, 160, 500, 1920, 1080);
        let (x, y, w, h) = left.region(1920, 1080);
        // Le coin en bas à droite reste à (1600, 900), au pixel près.
        assert!((x + w).abs_diff(1600) <= 1 && (y + h).abs_diff(900) <= 1, "{x} {y} {w} {h}");
        assert!((w * 9).abs_diff(h * 16) <= 16, "{w}×{h}");
    }

    #[test]
    fn locked_edge_stays_centered() {
        let c = Crop { ratio: CropRatio::R4x3, ..Crop::default() };
        let (x, y, w, h) = c.dragged(Handle::Right, -240, 0, 1920, 1080).region(1920, 1080);
        assert_eq!((x, w), (240, 1200));
        assert_eq!((y, h), (90, 900));
    }

    #[test]
    fn locked_resize_never_leaves_window() {
        let c = Crop { ratio: CropRatio::R1x1, ..Crop::default() }.reset(1920, 1080);
        let grown = c.dragged(Handle::Right, 5000, 0, 1920, 1080);
        let (x, y, w, h) = grown.region(1920, 1080);
        assert!(x + w <= 1920 && y + h <= 1080);
        assert_eq!(w, h);
    }

    #[test]
    fn typed_margin_moves_that_edge() {
        let c = Crop::default().with_margin(Side::Right, 300, 1920, 1080);
        assert_eq!(c, crop(0, 0, 300, 0));
        let c = c.with_margin(Side::Bottom, 2000, 1920, 1080);
        assert_eq!(c.region(1920, 1080).3, MIN_SIZE);
    }

    #[test]
    fn ratio_round_trip_in_settings() {
        let c = Crop { left: 4, ratio: CropRatio::R21x9, ..Crop::default() };
        let text = toml::to_string(&c).unwrap();
        assert!(text.contains("ratio = \"21:9\""), "{text}");
        assert_eq!(toml::from_str::<Crop>(&text).unwrap(), c);
        assert_eq!(CropRatio::from_index(CropRatio::R3x2.index() as i32), CropRatio::R3x2);
        assert_eq!(CropRatio::from_index(99), CropRatio::Free);
    }
}
