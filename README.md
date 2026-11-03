# PhotoPort

Transfert des photos et vidéos d'un iPhone vers l'ordinateur, par câble USB.
Sans iTunes, sans iCloud, sans app sur l'iPhone. Windows, macOS, Linux.

- Importe seulement les **nouvelles** photos (historique local, jamais de doublon).
- Grille de miniatures : coche une par une, par mois, tout d'un coup, Maj+clic pour
  une plage, ou glisse la souris pour cocher une zone. Les nouvelles sont cochées par défaut.
- Aperçu en grand (double-clic) : photos, vidéos, Live Photos, flèches pour défiler.
- Tous les fichiers du DCIM sont importés : HEIC, JPG, PNG, GIF, RAW (DNG), vidéos
  MOV/MP4 (4K comprises). Filtres Photos / Vidéos / Live / RAW.
- Recherche par lieu ou par scène (« Guadeloupe », « plage », « vélo »…) : PhotoPort lit
  l'index que l'iPhone a déjà calculé avec sa propre IA. Rien n'est envoyé sur Internet.
- Rangement au choix, d'après la date de prise de vue : par mois (`AAAA/MM - Mois/`),
  par jour (`AAAA/AAAA-MM-JJ/`), par année (`AAAA/`) ou en bloc (sans sous-dossiers).
- Garde les **Live Photos** ensemble (photo + vidéo dans le même dossier).
- Copie sûre : fichier temporaire, vérification de taille, reprise si le câble
  est débranché, journal d'erreurs.
- Option HEIC en JPG (macOS et Linux pour l'instant), qualité réglable.
- Option « Supprimer de l'iPhone après transfert », désactivée par défaut.

## Installation

Télécharger : **https://github.com/nthclrd/photoport/releases/latest**

| Système | Fichier | À installer en plus |
|---------|---------|---------------------|
| Windows 10/11 | `PhotoPort_…_x64-setup.exe` | l'app gratuite **Apple Devices** ([Microsoft Store](https://apps.microsoft.com/detail/9NP83LWLPZ9K)), une seule fois |
| macOS 10.15+ | `PhotoPort_…_universal.dmg` | rien |
| Debian / Ubuntu | `PhotoPort_…_amd64.deb` | rien (`usbmuxd` vient avec) |
| Autres Linux | `PhotoPort_…_amd64.AppImage` | `usbmuxd` (`sudo dnf install usbmuxd`, `sudo pacman -S usbmuxd`…) |

PhotoPort ne contient pas le pilote Apple : sur Windows, c'est « Apple Devices »
qui le fournit. Si le pilote manque, l'app l'explique et ouvre la bonne page.

Première connexion : déverrouille l'iPhone, branche-le, touche **Faire confiance**.

## Ligne de commande

```bash
photoport info                       # nom, modèle, iOS, état de confiance
photoport scan                       # nombre de photos/vidéos et taille
photoport plan  ~/Images/iPhone      # ce que le prochain import copierait
photoport import ~/Images/iPhone     # importer (Ctrl-C annule proprement)
photoport import ~/Images/iPhone --delete-after   # puis supprimer de l'iPhone
photoport -j 8 import ~/Images/iPhone             # 8 connexions en parallèle
photoport import ~/Images/iPhone --layout day     # month (défaut), day, year, flat
```

Dans le dossier de destination, `.photoport/` contient l'historique
(`history.sqlite`) et le journal d'erreurs (`errors.log`). Ne pas le supprimer,
sinon tout sera réimporté.

## Développement

Prérequis : [Rust](https://rustup.rs) stable, [Node.js](https://nodejs.org) 20+.
Linux : `sudo apt install libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf usbmuxd`.

```bash
cargo test --workspace                 # tests du cœur avec un faux iPhone
cargo run -p photoport-cli -- info     # CLI avec un vrai iPhone branché

cd app
npm install
npm run tauri dev                      # app de bureau, rechargement à chaud
npm run tauri build                    # installeur pour l'OS courant
```

Tests sur de vraies photos (facultatif, aucun fichier n'est versionné) :

```bash
PHOTOPORT_SAMPLE_HEIC=/chemin/photo.HEIC cargo test -p photoport-core -- --ignored exif
PHOTOPORT_SAMPLE_VIDEO=/chemin/video.MOV cargo test -p photoport-core -- --ignored real_video
```

### Structure

```
crates/core   cœur Rust : accès iPhone (AFC), import, rangement, historique
crates/cli    outil en ligne de commande
app/          app de bureau Tauri 2 + Svelte
web/          page de téléchargement statique
docs/         release, signature, stratégie HEIC, réglages UI
```

Le cœur passe par le trait `PhotoSource` : un autre backend (Wi-Fi, albums,
photos iCloud) s'ajoute sans toucher au reste.

Publier une version : [docs/RELEASE.md](docs/RELEASE.md).

## En cas de problème

Envoie la sortie de `photoport info`, ton OS et le contenu de
`<destination>/.photoport/errors.log` s'il existe.

| Message | Que faire |
|---------|-----------|
| `usbmuxd service unreachable` | Windows : installer Apple Devices. Linux : installer et démarrer `usbmuxd`. |
| `no iPhone detected` | Câble de données (pas seulement charge), iPhone déverrouillé. |
| `iPhone not trusted` | Déverrouiller l'iPhone, toucher « Faire confiance ». Sinon débrancher, rebrancher. |

## Licence

MIT.
