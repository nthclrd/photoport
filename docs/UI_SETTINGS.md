# UI settings — running list

Options the Tauri app must expose (étape 10). Written down early so later
code wires the right knobs.

## Destination
- Dossier de destination (bouton « Parcourir »).
- Rangement : par mois `AAAA/MM - NomDuMois/` (défaut), par jour `AAAA/AAAA-MM-JJ/`,
  par année `AAAA/`, en bloc (dossier unique). Mémorisé.

## Sélection
- Grille de miniatures (JPEG que iOS garde dans `/PhotoData/Thumbnails/V2/…/5005.JPG`,
  servies par le protocole `thumb://`), groupée par mois, plus récent en haut.
- Une Live Photo = une case. Déjà importé = grisé, non cochable, masquable.
- Tout sélectionner / tout désélectionner / tout le mois / Maj+clic plage.
- Les nouvelles photos arrivent cochées ; les choix faits restent entre deux scans.

## Format & qualité (demandé par l'utilisateur étape 2)
- **HEIC → JPG** : radio `Garder original` / `JPG uniquement` / `Les deux`.
  Actif sur macOS et Linux (avec heif-convert), grisé avec explication sur Windows.
- **Qualité JPG** : curseur 50-100, défaut 92. Désactivé si HEIC gardé seul.
- Choix mémorisés entre deux lancements (dossier, format, qualité).
- **Vidéos** : pas de transcodage V1. Si demandé plus tard : radio
  `Garder original` / `H.264 MP4` + curseur bitrate.
- Préserver EXIF/XMP dans la conversion.

## Transfert
- Toggle « Supprimer de l'iPhone après transfert » (défaut OFF, confirm).
- Toggle « Live Photos : garder le MOV associé » (défaut ON).
- Parallélisme AFC : auto / 1 / 2 / 4 (défaut auto après mesure).

## Divers
- Langue : fr / en (défaut système).
- Thème : auto / clair / sombre.
- Mise à jour auto : on/off (plugin updater).

## Recherche intelligente
- Source : index de recherche de l'app Photos (`/PhotoData/Caches/search/psi.sqlite`)
  joint à `Photos.sqlite` (UUID -> fichier DCIM). Copiés une fois (~120 Mo), index
  compact gardé dans le cache de l'app, rebâti seulement si l'iPhone l'a modifié.
- Formats non documentés par Apple : en cas d'échec, message « Recherche indisponible ».

## Performance des miniatures
- iPhone 16 Pro mesuré : ~25 miniatures/s avec 8 connexions AFC (16 est plus lent).
- Chargement par ordre de proximité avec le centre de l'écran, 8 en vol, rien hors de vue.
- Cache disque des miniatures (max 300 Mo, ramené à 200 Mo) : réouverture instantanée.

## Aperçu vidéo
- Lecture en avance : 3 lecteurs gardent le fichier ouvert sur l'iPhone et remplissent
  ~32 Mo devant la position de lecture (cache mémoire max 160 Mo, une vidéo à la fois).
- Débit limité par l'iPhone : ~35 Mo/s observé pendant un import, ~2 Mo/s observé plus tard (cause non confirmée, iPhone verrouillé ?).
- Barre de progression pendant l'inventaire du DCIM (`photoport://scan`).
