# Publier une version

## Release sans signature

1. Mettre le même numéro de version dans `app/src-tauri/tauri.conf.json`,
   `app/src-tauri/Cargo.toml` et `Cargo.toml`.
2. Commiter, puis créer et pousser le tag :
   ```bash
   git tag v0.2.0
   git push origin v0.2.0
   ```
3. Le workflow **Release** construit les installeurs des 3 OS (environ 20 min) et
   crée un **brouillon** de release.
4. Sur GitHub, ouvrir Releases, vérifier les fichiers, cliquer **Publish**.

| OS | Fichier | Usage |
|----|---------|-------|
| Windows | `PhotoPort_X.Y.Z_x64-setup.exe` | installeur conseillé, sans droits admin |
| Windows | `PhotoPort_X.Y.Z_x64_fr-FR.msi` | déploiement en entreprise |
| macOS | `PhotoPort_X.Y.Z_universal.dmg` | Mac Intel et Apple Silicon |
| Linux | `PhotoPort_X.Y.Z_amd64.deb` | Debian, Ubuntu, Mint (installe `usbmuxd`) |
| Linux | `PhotoPort_X.Y.Z_amd64.AppImage` | toutes distributions (`usbmuxd` à part) |

Sans signature, avertissement au premier lancement :
- **Windows** (SmartScreen) : « Informations complémentaires », puis « Exécuter quand même ».
- **macOS** : clic droit sur l'app, « Ouvrir ». Sur macOS 15 et plus : Réglages
  système, Confidentialité et sécurité, « Ouvrir quand même ».

## Signature Windows (plus tard)

Supprime l'avertissement SmartScreen. Options :
1. **Azure Trusted Signing** (environ 10 $/mois, identité vérifiée requise).
2. **Certificat OV** (Sectigo, Certum : offre « Open Source » bon marché). La
   réputation SmartScreen se construit avec le temps.
3. **Certificat EV** : réputation immédiate, plus cher.

Mise en place : `bundle.windows.certificateThumbprint` ou `signCommand` dans
`tauri.conf.json`, certificat dans les secrets GitHub.
Doc : https://v2.tauri.app/distribute/sign/windows/

## Signature et notarisation macOS (plus tard)

1. **Apple Developer Program** (99 $/an).
2. Certificat **Developer ID Application**, exporté en `.p12`.
3. **Mot de passe d'app** sur appleid.apple.com.
4. Secrets GitHub (Settings, Secrets and variables, Actions) :

| Secret | Contenu |
|--------|---------|
| `APPLE_CERTIFICATE` | `.p12` en base64 (`base64 -i cert.p12`) |
| `APPLE_CERTIFICATE_PASSWORD` | mot de passe du `.p12` |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: Nom (TEAMID)` |
| `APPLE_ID` | e-mail du compte Apple |
| `APPLE_PASSWORD` | mot de passe d'app |
| `APPLE_TEAM_ID` | identifiant d'équipe |

5. Décommenter les lignes `APPLE_*` de `.github/workflows/release.yml`.

Doc : https://v2.tauri.app/distribute/sign/macos/

## Mise à jour automatique (plus tard)

Clé propre à Tauri, indépendante de la signature de code.

1. Générer la paire de clés une fois :
   ```bash
   cd app
   npx tauri signer generate -w ~/.tauri/photoport.key
   ```
2. Coller `~/.tauri/photoport.key.pub` dans `app/src-tauri/tauri.updater.conf.json`
   (`pubkey`).
3. Secrets `TAURI_SIGNING_PRIVATE_KEY` et `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
4. Dans `release.yml` : décommenter `TAURI_SIGNING_*`, et passer `args` à
   `${{ matrix.args }} --features updater --config src-tauri/tauri.updater.conf.json`.

**Ne jamais perdre la clé privée** : sans elle, plus de mise à jour possible pour
les utilisateurs existants.
