# Publier une version (build cross-platform + mise à jour auto)

IC705 Bridge se distribue via les **Releases GitHub** : un tag de version
déclenche un workflow qui construit les installeurs macOS / Windows / Linux et
les publie automatiquement. L'app installée vérifie ensuite les mises à jour au
démarrage et se met à jour toute seule.

```
git tag v0.1.1  →  GitHub Actions  →  Release avec les 3 installeurs + latest.json
                                          │
                          app installée ──┘ (vérifie au lancement, télécharge, redémarre)
```

---

## 1. Configuration unique (à faire une seule fois)

Le workflow signe les artefacts de mise à jour avec une **clé privée minisign**
(indépendante de toute signature OS — c'est ce qui permet à l'updater de vérifier
l'authenticité d'une mise à jour). Cette clé a été générée dans
`.tauri/ic705bridge_updater.key` (ce dossier est **gitignoré** : la clé privée
ne doit jamais être commitée). La clé **publique** correspondante est déjà
inscrite dans `src-tauri/tauri.conf.json` (`plugins.updater.pubkey`).

Il faut donner la clé privée au CI via un **secret GitHub** :

```bash
# Depuis la racine du dépôt, avec gh authentifié (gh auth login) :
gh secret set TAURI_SIGNING_PRIVATE_KEY < .tauri/ic705bridge_updater.key

# La clé a été générée SANS mot de passe ; ce secret reste donc vide :
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --body ""
```

> 🔐 **Sauvegarde la clé privée** (`.tauri/ic705bridge_updater.key`) hors du
> dépôt, dans un endroit sûr. Si tu la perds, les apps déjà déployées ne
> pourront plus se mettre à jour (il faudra rediffuser une version avec une
> nouvelle clé publique, donc une réinstallation manuelle).

`GITHUB_TOKEN` est fourni automatiquement par Actions — rien à configurer.

---

## 2. Publier une nouvelle version

1. **Incrémente le numéro de version** dans les trois fichiers (gardez-les
   identiques) :
   - `src-tauri/tauri.conf.json` → `"version"`
   - `package.json` → `"version"`
   - `src-tauri/Cargo.toml` → `version`
2. Commit sur `main` :
   ```bash
   git commit -am "Release v0.1.1"
   git push origin main
   ```
3. Crée et pousse le tag (préfixe `v`) :
   ```bash
   git tag v0.1.1
   git push origin v0.1.1
   ```
4. Le workflow [`.github/workflows/release.yml`](../.github/workflows/release.yml)
   construit et publie la Release. Compter ~15-25 min (3 OS en parallèle).

La Release contient :
- macOS : `.dmg` (universel Intel + Apple Silicon),
- Windows : `.exe` (installeur NSIS) et `.msi`,
- Linux : `.AppImage` et `.deb`,
- `latest.json` : le manifeste lu par l'updater.

---

## 3. Comment la mise à jour automatique fonctionne

- L'endpoint est configuré dans `tauri.conf.json` :
  `https://github.com/BleuRouge/IC705-Bridge/releases/latest/download/latest.json`.
- Au démarrage, l'app compare sa version à celle du manifeste. Si une version
  plus récente existe, une **bannière** propose la mise à jour ; l'utilisateur
  clique, l'app télécharge (signature vérifiée), installe et redémarre.
- **Déploiement intranet** : si les postes n'ont pas accès à GitHub, héberge le
  contenu de la Release (installeurs + `latest.json`) sur l'intranet et remplace
  l'URL `endpoints` par celle de l'intranet, puis republie une version.

---

## 4. Installation côté utilisateur (binaires non signés)

Les binaires ne sont **pas signés** (pas de certificat Apple/Windows). Au premier
lancement, le système affiche un avertissement, à contourner une seule fois :

- **macOS** : clic droit sur l'app → **Ouvrir** → **Ouvrir** (ou
  `Réglages Système > Confidentialité et sécurité > Ouvrir quand même`).
- **Windows** : SmartScreen → **Informations complémentaires** → **Exécuter
  quand même**.
- **Linux** (AppImage) : `chmod +x IC705\ Bridge_*.AppImage` puis l'exécuter ;
  ou installer le `.deb` avec `sudo apt install ./IC705\ Bridge_*.deb`.

Les mises à jour suivantes passent par l'updater intégré, sans réinstallation
manuelle.

---

## 5. Builds locaux (debug / itération)

```bash
pnpm install
pnpm tauri dev      # lancement en développement (la CSP de prod n'y est PAS appliquée)
```

### Tester l'app packagée sans rien signer

Pour juste vérifier l'app de production (ex. valider la CSP), on construit le
DMG seul : il n'est pas une cible « updater », donc aucune clé n'est requise.

```bash
pnpm tauri build --bundles dmg
open "src-tauri/target/release/bundle/dmg/IC705 Bridge_0.1.0_aarch64.dmg"
```

### Build de production complet (avec artefact + signature updater)

`createUpdaterArtifacts: true` fait signer l'artefact updater : un build complet
exige donc la **clé privée minisign** en variable d'environnement (la nôtre a été
générée **sans mot de passe**, cf. §1) :

```bash
export TAURI_SIGNING_PRIVATE_KEY="$(cat .tauri/ic705bridge_updater.key)"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
pnpm tauri build
```

Sans ces variables, le build échoue à la fin sur
`A public key has been found, but no private key`.

> **macOS — DMG :** l'emballage `.dmg` pilote le Finder via AppleScript. Au
> premier build, macOS demande d'autoriser le terminal à contrôler le Finder
> (Réglages Système → Confidentialité et sécurité → Automatisation). Si c'est
> refusé, le DMG échoue (`bundle_dmg.sh`) — réautoriser, ou `tccutil reset
> AppleEvents`, puis relancer. Le `.app` lui est déjà construit dans tous les cas.

Depuis un Mac, `pnpm tauri build` ne produit que les bundles macOS. Les
installeurs Windows et Linux ne peuvent être produits que par le CI (ou des
machines/VMs dédiées) — c'est précisément le rôle du workflow de release.
