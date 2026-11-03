# HEIC vers JPG

## État actuel

| OS | Conversion | Outil |
|----|-----------|-------|
| macOS | oui | `sips`, livré avec le système |
| Linux | si installé | `heif-convert` (`sudo apt install libheif-examples`) |
| Windows | pas encore | prévu avec libheif embarqué |

Options dans l'app : *Garder l'original* (défaut), *JPG uniquement*, *HEIC + JPG*,
qualité JPG de 50 à 100 (défaut 92). Le code est dans `crates/core/src/heic.rs`.

- La date EXIF est conservée : vérifié sur une vraie photo d'iPhone
  (test `exif_date_survives_conversion`, lancé à la main).
- Si la conversion échoue, l'original HEIC est gardé et l'erreur va dans
  `errors.log`. Le transfert lui-même compte comme réussi.
- Les vidéos ne sont jamais converties.

## Pourquoi pas libheif-rs tout de suite

`libheif-rs` lie la bibliothèque C `libheif` (et `libde265` pour le décodage HEVC) :
- Linux : `libheif-dev`, simple.
- macOS : il faut compiler pour arm64 et x86_64, puis assembler le `.dmg` universel.
- Windows : pas de paquet officiel, compilation via vcpkg + MSVC, DLL à embarquer.
- Licences : LGPL-3. Liaison dynamique obligatoire et DLL remplaçable par l'utilisateur.

## Pour Windows (V1.1)

Deux pistes :
1. **libheif + libde265 via vcpkg** dans le workflow release, DLL embarquées à
   côté de l'exe, module `heic` appelé en FFI. Fonctionne hors ligne, sans rien
   installer. Coût : build plus long, conformité LGPL à documenter.
2. **WIC (Windows Imaging Component)** via la crate `windows`. Pas de dépendance à
   embarquer, mais exige les extensions « HEIF Image Extensions » (gratuite) et
   « HEVC Video Extensions » (payante, 0,99 €) du Microsoft Store. Moins bonne
   expérience pour des non-informaticiens.

Recommandation : piste 1.

En attendant, sur Windows : les HEIC s'ouvrent dans l'app Photos. Autre option,
régler l'iPhone sur *Réglages > Appareil photo > Formats > Le plus compatible* :
les nouvelles photos seront prises directement en JPG.
