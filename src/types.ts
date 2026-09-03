/** États de connexion renvoyés par le backend (enum Rust `ConnState`). */
export type ConnState =
  | "disconnected"
  | "connecting"
  | "authenticated"
  | "civ_ready"
  | "error";

/** Instantané d'état (commande `get_status` / événement `status`). */
export interface StatusSnapshot {
  state: ConnState;
  message: string;
  host: string | null;
  api_running: boolean;
  api_port: number;
  api_url: string;
}

/** Résultat d'un envoi CI-V (commande `send_civ`). */
export interface CivResult {
  tx: string;
  response: string;
}

/** Une ligne du terminal CI-V. */
export interface LogEntry {
  id: number;
  ts: string;
  dir: "tx" | "rx" | "info" | "error";
  text: string;
}
