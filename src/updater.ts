// Vérification et installation des mises à jour via le plugin updater Tauri.
//
// L'app interroge l'endpoint configuré (Releases GitHub) au démarrage. Si une
// version plus récente existe, elle est téléchargée, installée, puis l'app
// redémarre. La signature minisign est vérifiée par le plugin (clé publique
// dans tauri.conf.json) — un artefact non signé / falsifié est rejeté.

import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export type UpdateProgress =
  | { phase: "checking" }
  | { phase: "available"; version: string; notes?: string }
  | { phase: "downloading"; downloaded: number; total?: number }
  | { phase: "installing" }
  | { phase: "uptodate" }
  | { phase: "error"; message: string };

/**
 * Vérifie la présence d'une mise à jour. Renvoie l'objet `Update` si une
 * nouvelle version est disponible, sinon `null`. Ne télécharge rien.
 */
export async function checkForUpdate(): Promise<Update | null> {
  // En dev (pas d'endpoint joignable ou app non empaquetée), `check` peut lever :
  // on renvoie alors `null` plutôt que de casser le démarrage.
  return await check();
}

/**
 * Télécharge et installe la mise à jour fournie, en signalant la progression,
 * puis redémarre l'application.
 */
export async function installUpdate(
  update: Update,
  onProgress: (p: UpdateProgress) => void,
): Promise<void> {
  let downloaded = 0;
  let total: number | undefined;

  onProgress({ phase: "downloading", downloaded: 0 });
  await update.downloadAndInstall((event) => {
    switch (event.event) {
      case "Started":
        total = event.data.contentLength;
        onProgress({ phase: "downloading", downloaded: 0, total });
        break;
      case "Progress":
        downloaded += event.data.chunkLength;
        onProgress({ phase: "downloading", downloaded, total });
        break;
      case "Finished":
        onProgress({ phase: "installing" });
        break;
    }
  });

  await relaunch();
}
