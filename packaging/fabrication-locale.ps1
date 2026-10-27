# Fabrication locale sous Windows : mêmes étapes que .github/workflows/build.yml
# (compilation, FFmpeg, assemblage, installateur Velopack), sans publication.
# Plus rapide que GitHub pour tester une version : la compilation reprend là où
# elle s'était arrêtée et FFmpeg n'est téléchargé qu'une fois.
#
# Prérequis : Rust (rustup), Visual Studio Build Tools (C++), .NET 8 ou plus.
# Résultat : livraison\Polyptyque-Setup.exe et livraison\Polyptyque-portable.zip.
#
#   powershell -ExecutionPolicy Bypass -File packaging\fabrication-locale.ps1
#   ... -SansInstallateur   : compilation et assemblage seulement (dist\Polyptyque)

param([switch]$SansInstallateur)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:USERPROFILE\.dotnet\tools;$env:PATH"

# Mêmes valeurs que le workflow.
$ffmpegZip = 'ffmpeg-n8.1-latest-win64-gpl-8.1'
$ffmpegUrl = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/$ffmpegZip.zip"
$vpkVersion = '1.2.161'
$packId = 'PolyptyqueApp'

Write-Host '== Compilation'
cargo build --release -p polyptyque-app -p polyptyque-cli
if ($LASTEXITCODE -ne 0) { throw 'compilation échouée' }

# FFmpeg gardé d'une fabrication à l'autre dans target\ (ignoré par Git).
$ffmpegDir = "target\ffmpeg\$ffmpegZip"
if (-not (Test-Path "$ffmpegDir\bin\ffmpeg.exe")) {
    Write-Host '== Téléchargement de FFmpeg'
    New-Item -ItemType Directory -Force -Path target\ffmpeg | Out-Null
    Invoke-WebRequest -Uri $ffmpegUrl -OutFile target\ffmpeg\ffmpeg.zip
    Expand-Archive target\ffmpeg\ffmpeg.zip -DestinationPath target\ffmpeg -Force
    Remove-Item target\ffmpeg\ffmpeg.zip
}

Write-Host '== Assemblage'
$dist = 'dist\Polyptyque'
if (Test-Path $dist) { Remove-Item -Recurse -Force $dist }
New-Item -ItemType Directory -Force -Path $dist | Out-Null
Copy-Item target\release\polyptyque.exe $dist\
Copy-Item target\release\polyptyque-cli.exe $dist\
Copy-Item "$ffmpegDir\bin\ffmpeg.exe" $dist\
Copy-Item "$ffmpegDir\LICENSE.txt" "$dist\FFMPEG-LICENSE.txt"
Copy-Item LICENSE "$dist\LICENSE.txt"
Copy-Item packaging\LISEZMOI.txt $dist\
Copy-Item packaging\README.txt $dist\
if ($SansInstallateur) {
    Write-Host "Programme assemblé dans $dist"
    return
}

Write-Host '== Installateur'
$installed = dotnet tool list -g | Select-String -Pattern "^vpk\s+$([regex]::Escape($vpkVersion))\b"
if (-not $installed) {
    dotnet tool update -g vpk --version $vpkVersion
    if ($LASTEXITCODE -ne 0) { throw 'installation de vpk échouée' }
}
$version = ((cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).packages |
    Where-Object name -eq 'polyptyque-app').version
$notes = "packaging\notes\v$version.md"
$releaseNotes = @()
if (Test-Path $notes) { $releaseNotes += '--releaseNotes', $notes }
$releases = 'target\releases'
if (Test-Path $releases) { Remove-Item -Recurse -Force $releases }
vpk pack --packId $packId --packVersion $version --packDir $dist `
    --mainExe polyptyque.exe --packTitle Polyptyque --packAuthors Polyptyque `
    --icon assets\logo\polyptyque.ico --splashImage assets\logo\installation.gif `
    --runtime win10.0.19041-x64 @releaseNotes --outputDir $releases
if ($LASTEXITCODE -ne 0) { throw 'vpk pack a échoué' }
New-Item -ItemType Directory -Force -Path livraison | Out-Null
Copy-Item "$releases\$packId-win-Setup.exe" livraison\Polyptyque-Setup.exe
Copy-Item "$releases\$packId-win-Portable.zip" livraison\Polyptyque-portable.zip
Write-Host "Version $version : livraison\Polyptyque-Setup.exe et livraison\Polyptyque-portable.zip"
