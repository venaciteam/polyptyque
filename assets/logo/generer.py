"""Génère les images de marque de Polyptyque à partir d'une seule
description du logo : icône Windows (.ico), images PNG, animation de
l'installateur.

Utilisation (Python 3 et Pillow) :
    python assets/logo/generer.py

Le logo : trois panneaux, comme un polyptyque. Le panneau central est
plein, les deux autres translucides, sur un carré arrondi en dégradé.
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ICI = Path(__file__).resolve().parent
RACINE = ICI.parent.parent

# Couleurs (reprises du thème de l'interface, ui/theme.slint).
DEGRADE_HAUT = (138, 152, 255)
DEGRADE_BAS = (92, 106, 235)
FOND_SOMBRE = (14, 16, 20)
TEXTE = (232, 234, 238)
TEXTE_DISCRET = (138, 146, 160)
SURECHANTILLONNAGE = 8


def barres(taille):
    """Rectangles des trois panneaux, alignés sur les pixels de l'image
    finale pour rester nets en petite taille."""
    largeur = max(2, round(taille * 0.15))
    ecart = max(1, round(taille * 0.065))
    hauteur = max(6, round(taille * 0.56))
    x0 = round((taille - (3 * largeur + 2 * ecart)) / 2)
    y0 = round((taille - hauteur) / 2)
    return [(x0 + i * (largeur + ecart), y0, largeur, hauteur) for i in range(3)]


def icone(taille, allumees=(False, True, False)):
    """Icône carrée ; `allumees` dit quels panneaux sont pleins."""
    s = SURECHANTILLONNAGE
    grand = taille * s
    fond = Image.new("RGBA", (grand, grand))
    trace = ImageDraw.Draw(fond)
    for y in range(grand):
        t = y / grand
        couleur = tuple(round(a + (b - a) * t) for a, b in zip(DEGRADE_HAUT, DEGRADE_BAS))
        trace.line([(0, y), (grand, y)], fill=couleur + (255,))
    masque = Image.new("L", (grand, grand), 0)
    ImageDraw.Draw(masque).rounded_rectangle([0, 0, grand - 1, grand - 1], radius=round(grand * 0.22), fill=255)
    image = Image.new("RGBA", (grand, grand), (0, 0, 0, 0))
    image.paste(fond, (0, 0), masque)

    calque = Image.new("RGBA", (grand, grand), (0, 0, 0, 0))
    dessin = ImageDraw.Draw(calque)
    for (x, y, l, h), plein in zip(barres(taille), allumees):
        opacite = 255 if plein else 150
        dessin.rounded_rectangle(
            [x * s, y * s, (x + l) * s - 1, (y + h) * s - 1], radius=l * s * 0.3, fill=(255, 255, 255, opacite)
        )
    image = Image.alpha_composite(image, calque)
    return image.resize((taille, taille), Image.LANCZOS)


# Point rouge de l'enregistrement en cours (thème : Theme.recording).
ENREGISTREMENT = (255, 77, 90)


def icone_enregistrement(taille):
    """Icône de la zone de notification pendant un enregistrement : le logo
    avec un point rouge en bas à droite, cerclé de sombre pour rester
    visible sur une barre des tâches claire comme foncée."""
    s = SURECHANTILLONNAGE
    grand = taille * s
    image = icone(taille).resize((grand, grand), Image.LANCZOS)
    diametre = max(7, round(taille * 0.5)) * s
    bord = max(1, round(taille * 0.07)) * s
    x0 = grand - diametre
    y0 = grand - diametre
    dessin = ImageDraw.Draw(image)
    dessin.ellipse([x0, y0, grand - 1, grand - 1], fill=FOND_SOMBRE + (255,))
    dessin.ellipse([x0 + bord, y0 + bord, grand - 1 - bord, grand - 1 - bord], fill=ENREGISTREMENT + (255,))
    return image.resize((taille, taille), Image.LANCZOS)


def police(nom, taille):
    for dossier in ("/usr/share/fonts/opentype/inter", "C:/Windows/Fonts"):
        chemin = Path(dossier) / nom
        if chemin.exists():
            return ImageFont.truetype(str(chemin), taille)
    return ImageFont.load_default(taille)


def animation_installation(chemin):
    """Image de l'installateur : les panneaux s'allument tour à tour."""
    largeur, hauteur = 480, 280
    titre = police("Inter-SemiBold.otf", 30)
    sous_titre = police("Inter-Regular.otf", 15)
    etats = [
        (False, False, False),
        (True, False, False),
        (True, True, False),
        (True, True, True),
        (True, True, True),
        (False, True, False),
    ]
    images = []
    for etat in etats:
        image = Image.new("RGBA", (largeur, hauteur), FOND_SOMBRE + (255,))
        logo = icone(96, etat)
        image.alpha_composite(logo, ((largeur - 96) // 2, 46))
        dessin = ImageDraw.Draw(image)
        for texte, fonte, couleur, y in (
            ("Polyptyque", titre, TEXTE, 160),
            ("Installation en cours…", sous_titre, TEXTE_DISCRET, 204),
        ):
            boite = dessin.textbbox((0, 0), texte, font=fonte)
            dessin.text(((largeur - (boite[2] - boite[0])) / 2, y), texte, font=fonte, fill=couleur)
        images.append(image.convert("RGB").quantize(colors=128, method=Image.Quantize.MEDIANCUT))
    images[0].save(chemin, save_all=True, append_images=images[1:], duration=[260, 260, 260, 700, 260, 520], loop=0)


def main():
    tailles = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    icones = [icone(t) for t in tailles]
    icones[-1].save(ICI / "polyptyque.ico", sizes=[(t, t) for t in tailles], append_images=icones[:-1])
    icone_enregistrement_fichier()
    icone(512).save(ICI / "polyptyque-512.png")
    icone(256).save(RACINE / "crates" / "polyptyque-app" / "ui" / "icons" / "polyptyque.png")
    animation_installation(ICI / "installation.gif")
    print("images générées dans", ICI)


def icone_enregistrement_fichier():
    """Icône d'enregistrement (ressource 2 des exécutables)."""
    tailles = [16, 20, 24, 32, 40, 48, 64, 256]
    icones = [icone_enregistrement(t) for t in tailles]
    icones[-1].save(ICI / "polyptyque-enregistrement.ico", sizes=[(t, t) for t in tailles], append_images=icones[:-1])


if __name__ == "__main__":
    main()
