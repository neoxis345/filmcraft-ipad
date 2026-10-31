<p align="center">
  <img src="assets/app-icon/hicolor/256x256/apps/ai.storyteller.filmcraft.png" width="128" height="128" alt="FilmCraft icon">
</p>

<h1 align="center">FilmCraft pour iPad</h1>

<p align="center">
  Un logiciel de montage vidéo complet, gratuit et open source, écrit en Rust, qui tourne sur iPad.<br>
  <em>A free, open-source, pure-Rust non-linear video editor that runs on iPad.</em>
</p>

Ce dépôt est une version modifiée de [FilmCraft](https://github.com/storytold/filmcraft), le logiciel de
montage de l'équipe ArtCraft. Il ajoute l'application iPadOS. **Cette version n'est ni réalisée, ni
approuvée, ni prise en charge par l'équipe ArtCraft** : signalez ses problèmes ici, pas chez eux.

## Ce que fait l'application

- L'interface complète de FilmCraft : timeline, moniteurs, effets, étalonnage, titres, audio, export.
- Le tactile, l'Apple Pencil, la souris et le trackpad, le clavier physique.
- Le décodage et l'encodage matériels (VideoToolbox), y compris les vidéos Dolby Vision de l'iPhone.
- Une mémoire et une qualité de lecture adaptées à chaque iPad, à sa température et au mode économie d'énergie.

**Vie privée :** l'application ne contient aucun code réseau, aucune télémétrie et aucun compte. Les projets et
les médias restent sur l'iPad. Elle n'ouvre le micro que pour enregistrer une voix off, à votre demande.

## iPad pris en charge

iPadOS 18 ou plus récent, puce A12 ou plus récente recommandée : du 3 Go (iPad 8 et 9, iPad mini 5, iPad Air 3)
aux iPad Pro M5, de 8,3 à 13 pouces.

## Installer depuis les sources

Il faut un Mac avec Xcode, [Rust](https://rustup.rs) et [XcodeGen](https://github.com/yonaskolb/XcodeGen), et un
iPad en mode développeur, branché au Mac :

```bash
git clone https://github.com/neoxis345/filmcraft-ipad
cd filmcraft-ipad
FILMCRAFT_BUNDLE_ID=org.exemple.filmcraft packaging/ios/deploy.sh
```

Avec un identifiant Apple gratuit, l'installation dure 7 jours : relancez `deploy.sh` pour la renouveler.
Le détail (architecture, tests sans iPad, TestFlight) est dans [packaging/ios/README.md](packaging/ios/README.md).

## Limites

- Pas d'export en arrière-plan : iPadOS arrête le GPU et l'encodeur matériel, gardez l'application à l'écran.
- Pas de modules externes, ni de transcription Whisper dans cette version.

## Pour les développeurs

Les règles du projet sont dans [AGENTS.md](AGENTS.md), l'architecture dans
[docs/architecture.md](docs/architecture.md). Les tests se lancent avec
[cargo-nextest](https://nexte.st) (certains supposent un processus par test) :

```bash
cargo nextest run --workspace
```

## Licence

Double licence [MIT](LICENSE-MIT) ou [Apache-2.0](LICENSE-APACHE), au choix.
Copyright (c) 2026 ArtCraft Team and the FilmCraft contributors. Mentions requises : [NOTICE](NOTICE) ;
fichiers tiers : [ATTRIBUTION.md](ATTRIBUTION.md).

Basé sur FilmCraft par l'équipe ArtCraft. Le nom et les logos ArtCraft sont des marques de l'équipe ArtCraft ;
ils ne sont pas couverts par cette licence et ont été retirés de cette version. FilmCraft est une réalisation
indépendante, sans lien avec Adobe : elle ne contient aucun code, icône, image, préréglage ou LUT propriétaire.
