#!/usr/bin/env bash
#
# Assemble Nomad.app : exécutable SwiftPM (release) + démon nomad embarqué,
# puis produit un .zip prêt à publier. Utilisable en CI comme en local.
#
# Usage :
#   make-app.sh <version> <chemin-du-démon-nomad> [<répertoire-de-sortie>]
#
# Exemple local (après `cargo build` à la racine) :
#   apps/macos/Packaging/make-app.sh v0.0.0 target/debug/nomad
#
# La progression va sur stderr ; la **seule** sortie stdout est le chemin du zip
# produit (pour être capturée : ASSET="$(make-app.sh ...)").
#
# NB : signature ad-hoc uniquement. Un vrai Developer ID + notarisation
# (étape 9) sont nécessaires pour une installation sans avertissement Gatekeeper.
set -euo pipefail

version="${1:?version requise (ex. v0.2.0)}"
daemon="${2:?chemin du binaire nomad requis}"
outdir="${3:-dist}"

here="$(cd "$(dirname "$0")" && pwd)"
pkg="$here/.."

log() { echo "$@" >&2; }

if [[ ! -x "$daemon" ]]; then
    log "erreur : démon introuvable ou non exécutable : $daemon"
    exit 1
fi

log "==> compilation de l'app (swift build -c release)"
swift build -c release --package-path "$pkg" >&2
exe="$pkg/.build/release/Nomad"
[[ -x "$exe" ]] || { log "erreur : exécutable introuvable : $exe"; exit 1; }

mkdir -p "$outdir"
outdir="$(cd "$outdir" && pwd)"
app="$outdir/Nomad.app"

log "==> assemblage de $app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$exe" "$app/Contents/MacOS/Nomad"
cp "$daemon" "$app/Contents/Resources/nomad"
chmod +x "$app/Contents/MacOS/Nomad" "$app/Contents/Resources/nomad"
sed "s/__VERSION__/${version#v}/g" "$here/Info.plist" > "$app/Contents/Info.plist"

log "==> signature ad-hoc"
codesign --force --deep --sign - "$app" >&2 2>&1 || log "  (signature ad-hoc échouée, on continue)"

zip="$outdir/Nomad-${version}-macos-arm64.zip"
log "==> archive $zip"
( cd "$outdir" && ditto -c -k --keepParent "Nomad.app" "$(basename "$zip")" )
rm -rf "$app"

echo "$zip"
