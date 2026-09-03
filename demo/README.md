# Démo opérationnelle

Ce dossier lance toute la démonstration avec une seule commande :

1. `uv` crée ou met à jour automatiquement l'environnement Python isolé ;
2. le lanceur ouvre IC705 Bridge (version installée, ou version source via `pnpm`) ;
3. il attend que l'utilisateur connecte l'IC-705 dans l'application ;
4. dès que le CI-V est prêt, il ouvre le moniteur waterfall Python existant.

## Prérequis

- un IC-705 joignable sur le même réseau, avec RS-BA1 configuré ;
- [`uv`](https://docs.astral.sh/uv/getting-started/installation/) ;
- soit IC705 Bridge déjà installé, soit les prérequis de développement du dépôt
  (`pnpm`, Rust et les dépendances Tauri).

Si l'application installée se trouve dans un emplacement inhabituel, définir la
variable d'environnement `IC705_BRIDGE_APP` avec le chemin de son exécutable (ou
du bundle `.app` sous macOS).

## Lancer la démo

Sur macOS ou Linux :

```bash
./demo/run_demo.sh
```

Sur Windows PowerShell :

```powershell
.\demo\run_demo.ps1
```

Dans la fenêtre IC705 Bridge, renseigner l'adresse IP et les identifiants RS-BA1
de l'IC-705, puis cliquer sur **Connect**. Laisser ensuite l'application ouverte
en arrière-plan : le moniteur waterfall se lance automatiquement dès que la
connexion CI-V est opérationnelle.

À la fermeture du moniteur, IC705 Bridge reste ouvert afin de pouvoir utiliser
**Disconnect** et rendre proprement le contrôle CI-V au poste.

## Options utiles

Le lanceur accepte notamment :

```bash
./demo/run_demo.sh --source              # toujours utiliser pnpm tauri dev
./demo/run_demo.sh --no-launch           # l'application est déjà ouverte
./demo/run_demo.sh --radio 0xA4 --rows 200
```

Utiliser `./demo/run_demo.sh --help` pour la liste complète.
