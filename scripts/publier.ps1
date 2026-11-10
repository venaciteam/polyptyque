# Fabrique Polyptyque sur ce PC et publie la version sur GitHub (installateur, version portable,
# plugin Stream Deck, paquets de mise à jour Velopack). Remplace la publication par GitHub Actions.
#
#   .\scripts\publier.ps1             fabrique origin/main et publie la version de Cargo.toml
#   .\scripts\publier.ps1 -Essai      fabrique tout et copie les fichiers, sans rien envoyer
#   .\scripts\publier.ps1 -SansTests  saute cargo test et npm test (déconseillé pour une publication)
#
# Avant : commit de version sur dev (Cargo.toml + Cargo.lock, en-têtes de packaging\LISEZMOI.txt et
# packaging\README.txt, notes packaging\notes\vX.Y.Z.md en anglais puis en français), merge --no-ff de dev
# dans main, push des deux. Le script pose ensuite l'étiquette vX.Y.Z sur origin/main et publie.
#
# La fabrication se fait dans une copie propre sur le disque interne ($env:POLYPTYQUE_BUILD_DIR, par défaut
# %LOCALAPPDATA%\venacity-build\polyptyque) avec packaging\fabrication-locale.ps1 ; target (compilation,
# FFmpeg) y reste d'une version à l'autre. Fichiers produits : %LOCALAPPDATA%\venacity-build\sorties\polyptyque-<version>\.
param([switch]$Essai, [switch]$SansTests)

$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = [Text.UTF8Encoding]::new()
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:USERPROFILE\.dotnet\tools;$env:PATH"
$vpkVersion = '1.2.161'      # même version que la bibliothèque velopack de l'application
$packId = 'PolyptyqueApp'    # dossier d'installation %LOCALAPPDATA%\PolyptyqueApp
function Etape($t) { Write-Host "`n[*] $t" -ForegroundColor Cyan }
function Alerte($t) { Write-Host "    !! $t" -ForegroundColor Yellow }
function Arreter($t) { Write-Host "`n[!] $t" -ForegroundColor Red; exit 1 }
function Verifier($quoi) { if ($LASTEXITCODE -ne 0) { Arreter "$quoi a échoué (code $LASTEXITCODE)" } }

$Depot = Split-Path -Parent $PSScriptRoot
$Cible = if ($env:POLYPTYQUE_BUILD_DIR) { $env:POLYPTYQUE_BUILD_DIR } else { Join-Path $env:LOCALAPPDATA "venacity-build\polyptyque" }

Etape "Outils"
foreach ($c in "git", "gh", "cargo", "dotnet", "node", "npm") {
    if (-not (Get-Command $c -ErrorAction SilentlyContinue)) { Arreter "$c introuvable (prérequis : Rust, Build Tools C++, .NET 8, Node, gh)." }
}
$Nom = (git -C $Depot remote get-url origin) -replace '^.*github\.com[:/]', '' -replace '\.git$', ''
if (-not $Essai) { gh auth status *> $null; if ($LASTEXITCODE -ne 0) { Arreter "gh n'est pas connecté : gh auth login" } }

Etape "État du dépôt ($Depot)"
git -C $Depot fetch -q origin --tags; Verifier "git fetch"
$Commit = git -C $Depot rev-parse origin/main
$Probleme = @()
if (git -C $Depot status --porcelain) { $Probleme += "modifications non commitées dans le dépôt local" }
foreach ($b in "dev", "main") {
    if (git -C $Depot rev-parse -q --verify "refs/heads/$b") {
        $avance = git -C $Depot rev-list --count "origin/$b..$b"
        if ([int]$avance -gt 0) { $Probleme += "$b a $avance commit(s) non poussé(s)" }
    }
}
if ($Probleme) {
    $texte = $Probleme -join " ; "
    if ($Essai) { Alerte "$texte (ignoré en essai : la fabrication part de origin/main)" }
    else { Arreter "$texte. Pousser d'abord : la version publiée est origin/main." }
}
Write-Host "    commit fabriqué : $(git -C $Depot log -1 --format='%h %s' $Commit)"

Etape "Copie propre dans $Cible"
if (-not (Test-Path (Join-Path $Cible ".git"))) {
    New-Item -ItemType Directory -Force (Split-Path $Cible) | Out-Null
    gh repo clone $Nom $Cible -- --quiet -c core.autocrlf=false; Verifier "gh repo clone"
}
git -C $Cible config core.autocrlf false
git -C $Cible fetch -q origin --tags; Verifier "git fetch (copie)"
git -C $Cible checkout -q --force --detach $Commit; Verifier "git checkout"
git -C $Cible clean -q -ffdx -e target -e streamdeck/node_modules; Verifier "git clean"
Set-Location $Cible

Etape "Version"
$Version = ((cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).packages | Where-Object name -eq 'polyptyque-app').version
$Etiquette = "v$Version"
# Une version à tiret (« 1.0.0-beta.1 ») est une bêta : préversion GitHub, proposée seulement aux copies
# réglées sur « Versions stables et bêta ». Titre en anglais, langue de tête du dépôt.
$core, $suffix = $Version.Split('+')[0].Split('-', 2)
$Beta = [bool]$suffix
$n = $core.Split('.')
$Titre = "Polyptyque $($n[0]).$($n[1])"
if ($n[2] -ne '0') { $Titre += ".$($n[2])" }
if ($suffix -match '^beta\.(\d+)$') { $Titre += " beta $($Matches[1])" } elseif ($Beta) { $Titre += " $suffix" }
$Notes = "packaging\notes\$Etiquette.md"
Write-Host "    $Etiquette, page « $Titre », bêta : $Beta, notes : $(Test-Path $Notes)"
if (-not $Essai) {
    if (-not (Test-Path $Notes)) { Arreter "notes de version manquantes : $Notes" }
    git rev-parse -q --verify "refs/tags/$Etiquette" *> $null
    if ($LASTEXITCODE -eq 0 -and (git rev-parse "$Etiquette^{commit}") -ne $Commit) { Arreter "l'étiquette $Etiquette existe déjà sur un autre commit" }
}

if (-not $SansTests) {
    Etape "Tests (cargo test, npm test du plugin Stream Deck)"
    cargo test --locked -p polyptyque-core -p polyptyque-app; Verifier "cargo test"
}

Etape "Compilation et assemblage (packaging\fabrication-locale.ps1)"
& powershell -NoProfile -ExecutionPolicy Bypass -File packaging\fabrication-locale.ps1 -SansInstallateur; Verifier "fabrication-locale.ps1"

Etape "Installateur Velopack"
$installe = dotnet tool list -g | Select-String -Pattern "^vpk\s+$([regex]::Escape($vpkVersion))\b"
if (-not $installe) { dotnet tool update -g vpk --version $vpkVersion; Verifier "installation de vpk" }
$releases = 'target\releases'
if (Test-Path $releases) { Remove-Item -Recurse -Force $releases }
$pre = @(); if ($Beta) { $pre += '--pre' }
if (-not $Essai) {
    # Paquets de mise à jour partielle : télécharger la version précédente (la première stable n'en a pas).
    $precedente = @(gh release list --repo $Nom --exclude-drafts --exclude-pre-releases --limit 1 --json tagName | ConvertFrom-Json)
    Verifier "lecture des publications précédentes"
    if ($Beta -or $precedente.Count -gt 0) {
        vpk download github --repoUrl "https://github.com/$Nom" --token (gh auth token) --outputDir $releases @pre
        Verifier "téléchargement de la version précédente"
    }
}
$releaseNotes = @(); if (Test-Path $Notes) { $releaseNotes += '--releaseNotes', $Notes }
vpk pack --packId $packId --packVersion $Version --packDir dist\Polyptyque `
    --mainExe polyptyque.exe --packTitle Polyptyque --packAuthors Polyptyque `
    --icon assets\logo\polyptyque.ico --splashImage assets\logo\installation.gif `
    --runtime win10.0.19041-x64 @releaseNotes --outputDir $releases
Verifier "vpk pack"

if (-not $SansTests) {
    # Le paquet du plugin est fabriqué par fabrication-locale.ps1 (livré dans dist\Polyptyque).
    Etape "Tests du plugin Stream Deck"
    Push-Location streamdeck
    npm.cmd test; Verifier "npm test (Stream Deck)"
    Pop-Location
}

$Sortie = Join-Path (Split-Path $Cible) "sorties\polyptyque-$Version"
if (Test-Path $Sortie) { Remove-Item -Recurse -Force $Sortie }
New-Item -ItemType Directory -Force $Sortie | Out-Null
Copy-Item "$releases\$packId-win-Setup.exe" "$Sortie\Polyptyque-Setup.exe"
Copy-Item "$releases\$packId-win-Portable.zip" "$Sortie\Polyptyque-portable.zip"
Copy-Item livraison\city.vena.polyptyque.streamDeckPlugin "$Sortie\"
Get-ChildItem $Sortie | ForEach-Object { Write-Host ("    {0}  {1:N1} Mo" -f $_.Name, ($_.Length / 1MB)) }

if ($Essai) { Write-Host "`nEssai terminé, rien n'est envoyé : $Sortie" -ForegroundColor Green; exit 0 }

Etape "Publication $Etiquette sur $Nom"
git rev-parse -q --verify "refs/tags/$Etiquette" *> $null
if ($LASTEXITCODE -ne 0) { git tag $Etiquette $Commit; Verifier "git tag" }
git push origin "refs/tags/$Etiquette"; Verifier "git push de l'étiquette"
vpk upload github --repoUrl "https://github.com/$Nom" --token (gh auth token) `
    --outputDir $releases --tag $Etiquette --releaseName $Titre --publish @pre
Verifier "vpk upload"
# Installateur, version portable et plugin sous les noms du README. La mise à jour automatique n'utilise que
# les paquets (.nupkg) et releases.win.json, qui gardent leur nom.
gh release upload $Etiquette "$Sortie\Polyptyque-Setup.exe" "$Sortie\Polyptyque-portable.zip" "$Sortie\city.vena.polyptyque.streamDeckPlugin" --repo $Nom --clobber
Verifier "envoi des fichiers aux noms fixes"
foreach ($f in "$packId-win-Setup.exe", "$packId-win-Portable.zip") {
    gh release delete-asset $Etiquette $f --repo $Nom --yes; Verifier "suppression de $f"
}
if (-not $Beta) { gh release edit $Etiquette --latest --repo $Nom; Verifier "désignation de la dernière version" }
Write-Host "`nPubliée : https://github.com/$Nom/releases/tag/$Etiquette" -ForegroundColor Green
