import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { listen } from "@tauri-apps/api/event";
import { currentMonitor, getCurrentWindow } from "@tauri-apps/api/window";
import type { Update } from "@tauri-apps/plugin-updater";
import type { CivResult, ConnState, LogEntry, StatusSnapshot } from "./types";
import { checkForUpdate, installUpdate, type UpdateProgress } from "./updater";
import "./App.css";

type Tab = "connection" | "terminal";

const STATE_LABELS: Record<ConnState, string> = {
  disconnected: "Déconnecté",
  connecting: "Connexion…",
  authenticated: "Authentifié",
  civ_ready: "Tunnel CI-V prêt",
  error: "Erreur",
};

function nowTs(): string {
  return new Date().toLocaleTimeString("fr-FR", { hour12: false }) +
    "." + String(new Date().getMilliseconds()).padStart(3, "0");
}

let logSeq = 0;

/** Plafond du terminal : borne mémoire + nombre de nœuds DOM sous flux continu. */
const MAX_LOG = 2000;
/** Fenêtre de regroupement des trames avant rendu (~10 Hz). */
const LOG_FLUSH_MS = 100;

/** Taille nominale, réduite automatiquement si l'écran de travail est plus petit. */
const WINDOW_WIDTH = 980;
const WINDOW_HEIGHT = 720;
const WINDOW_MARGIN = 32;

/** Clés localStorage (on ne mémorise jamais le mot de passe). */
const LS_HOST = "ic705.host";
const LS_USER = "ic705.username";

/** Trames CI-V d'exemple (remplissent la saisie, l'étudiant les lit/édite). */
const PRESETS: { label: string; frame: string }[] = [
  { label: "Fréquence", frame: "FE FE A4 E0 03 FD" },
  { label: "Mode", frame: "FE FE A4 E0 04 FD" },
  { label: "S-mètre", frame: "FE FE A4 E0 15 02 FD" },
];

function App() {
  const [tab, setTab] = useState<Tab>("connection");
  const [status, setStatus] = useState<StatusSnapshot>({
    state: "disconnected",
    message: "Déconnecté",
    host: null,
    api_running: false,
    api_url: "http://127.0.0.1:8765",
  });

  // Champs de connexion (host/username mémorisés du dernier essai réussi).
  const [host, setHost] = useState(() => localStorage.getItem(LS_HOST) ?? "192.168.1.200");
  const [username, setUsername] = useState(() => localStorage.getItem(LS_USER) ?? "");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);

  // Terminal CI-V
  const [frame, setFrame] = useState("FE FE A4 E0 03 FD");
  const [sending, setSending] = useState(false);
  const [log, setLog] = useState<LogEntry[]>([]);
  const pendingLog = useRef<LogEntry[]>([]);
  const flushTimer = useRef<number | null>(null);

  // Mise à jour de l'application
  const [update, setUpdate] = useState<Update | null>(null);
  const [updateProgress, setUpdateProgress] = useState<UpdateProgress | null>(null);

  const connected = status.state === "civ_ready";

  // Le gestionnaire de fenêtres ne contraint pas toujours la taille initiale
  // aux petits écrans (vidéoprojecteur, mise à l'échelle Windows, dock macOS).
  // Ajuste une fois la fenêtre à la zone réellement disponible.
  useEffect(() => {
    async function fitWindowToScreen() {
      try {
        const monitor = await currentMonitor();
        if (!monitor) return;
        const availableWidth = monitor.workArea.size.width / monitor.scaleFactor - WINDOW_MARGIN;
        const availableHeight = monitor.workArea.size.height / monitor.scaleFactor - WINDOW_MARGIN;
        await getCurrentWindow().setSize(new LogicalSize(
          Math.min(WINDOW_WIDTH, availableWidth),
          Math.min(WINDOW_HEIGHT, availableHeight),
        ));
      } catch {
        // En aperçu web (hors Tauri) ces API ne sont pas disponibles.
      }
    }
    void fitWindowToScreen();
  }, []);

  // Bufferise les lignes et ne les pousse dans l'état que par lots : sous flux
  // continu (scope / transceive), un setLog + scroll par trame gelait l'UI.
  function addLog(dir: LogEntry["dir"], text: string) {
    pendingLog.current.push({ id: ++logSeq, ts: nowTs(), dir, text });
    if (flushTimer.current !== null) return;
    flushTimer.current = window.setTimeout(() => {
      flushTimer.current = null;
      const batch = pendingLog.current;
      pendingLog.current = [];
      setLog((l) => {
        const next = l.concat(batch);
        return next.length > MAX_LOG ? next.slice(next.length - MAX_LOG) : next;
      });
    }, LOG_FLUSH_MS);
  }

  function clearLog() {
    pendingLog.current = [];
    setLog([]);
  }

  // Abonnements aux événements backend + état initial.
  useEffect(() => {
    invoke<StatusSnapshot>("get_status").then(setStatus).catch(() => {});

    const unStatus = listen<StatusSnapshot>("status", (e) => setStatus(e.payload));
    const unCiv = listen<string>("civ-rx", (e) => addLog("rx", e.payload));

    return () => {
      unStatus.then((f) => f());
      unCiv.then((f) => f());
    };
  }, []);

  // Vide le buffer en attente au démontage (évite un setState post-démontage).
  useEffect(() => {
    return () => {
      if (flushTimer.current !== null) window.clearTimeout(flushTimer.current);
    };
  }, []);

  // Vérifie la présence d'une mise à jour au démarrage (silencieux si à jour
  // ou en dev où l'updater n'est pas joignable).
  useEffect(() => {
    checkForUpdate()
      .then((u) => {
        if (u) setUpdate(u);
      })
      .catch(() => {});
  }, []);

  async function onInstallUpdate() {
    if (!update) return;
    try {
      // Déconnecter la radio AVANT le redémarrage de mise à jour : le relaunch
      // ne repasse pas par la fermeture de fenêtre, la radio garderait sinon
      // une session pendante (reconnexion refusée après l'update).
      await invoke("disconnect").catch(() => {});
      await installUpdate(update, setUpdateProgress);
      // L'app redémarre ; ce code n'est normalement pas atteint.
    } catch (err) {
      setUpdateProgress({ phase: "error", message: String(err) });
    }
  }

  async function onConnect() {
    setBusy(true);
    try {
      const s = await invoke<StatusSnapshot>("connect", { host, username, password });
      setStatus(s);
      // Mémorise l'hôte/username de l'essai réussi (jamais le mot de passe).
      localStorage.setItem(LS_HOST, host.trim());
      localStorage.setItem(LS_USER, username);
      addLog("info", `Connecté à ${host.trim()}`);
      setTab("terminal");
    } catch (err) {
      addLog("error", String(err));
    } finally {
      setBusy(false);
    }
  }

  async function onDisconnect() {
    setBusy(true);
    try {
      const s = await invoke<StatusSnapshot>("disconnect");
      setStatus(s);
      addLog("info", "Déconnecté");
    } catch (err) {
      addLog("error", String(err));
    } finally {
      setBusy(false);
    }
  }

  async function onSend() {
    const f = frame.trim();
    if (!f || sending) return;
    setSending(true);
    addLog("tx", f);
    try {
      // Les trames RX s'affichent en temps réel via l'événement `civ-rx`.
      // Le backend lève une erreur explicite si aucune réponse corrélée
      // n'arrive, ce qui évite un double affichage ici.
      await invoke<CivResult>("send_civ", { frame: f });
    } catch (err) {
      addLog("error", String(err));
    } finally {
      setSending(false);
    }
  }

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          <div>
            <div className="title">IC705 Bridge</div>
            <div className="subtitle">Passerelle CI-V · Icom IC-705</div>
          </div>
        </div>
        <StatePill state={status.state} />
      </header>

      {update && (
        <UpdateBanner
          version={update.version}
          progress={updateProgress}
          onInstall={onInstallUpdate}
          onDismiss={() => setUpdate(null)}
        />
      )}

      <nav className="tabs">
        <button className={tab === "connection" ? "tab active" : "tab"} onClick={() => setTab("connection")}>
          Connection
        </button>
        <button
          className={tab === "terminal" ? "tab active" : "tab"}
          onClick={() => setTab("terminal")}
          disabled={!connected}
          title={connected ? "" : "Connecte-toi d'abord"}
        >
          CI-V Terminal
        </button>
      </nav>

      <main className="content">
        {tab === "connection" ? (
          <ConnectionTab
            host={host} setHost={setHost}
            username={username} setUsername={setUsername}
            password={password} setPassword={setPassword}
            busy={busy} status={status}
            onConnect={onConnect} onDisconnect={onDisconnect}
          />
        ) : (
          <TerminalTab
            frame={frame} setFrame={setFrame}
            log={log} clear={clearLog}
            onSend={onSend} connected={connected}
            sending={sending}
          />
        )}
      </main>
    </div>
  );
}

function StatePill({ state }: { state: ConnState }) {
  return <span className={`pill pill-${state}`}>{STATE_LABELS[state]}</span>;
}

function UpdateBanner(props: {
  version: string;
  progress: UpdateProgress | null;
  onInstall: () => void;
  onDismiss: () => void;
}) {
  const { version, progress } = props;
  const busy =
    progress?.phase === "downloading" || progress?.phase === "installing";

  let detail = `Version ${version} disponible.`;
  if (progress?.phase === "downloading") {
    const pct = progress.total
      ? Math.round((progress.downloaded / progress.total) * 100)
      : null;
    detail = pct !== null ? `Téléchargement… ${pct}%` : "Téléchargement…";
  } else if (progress?.phase === "installing") {
    detail = "Installation… l'app va redémarrer.";
  } else if (progress?.phase === "error") {
    detail = `Échec de la mise à jour : ${progress.message}`;
  }

  return (
    <div className="update-banner">
      <span className="update-text">⬆ {detail}</span>
      <div className="update-actions">
        <button className="btn primary small" onClick={props.onInstall} disabled={busy}>
          {busy ? "…" : "Mettre à jour"}
        </button>
        <button className="btn small" onClick={props.onDismiss} disabled={busy}>
          Plus tard
        </button>
      </div>
    </div>
  );
}

function ConnectionTab(props: {
  host: string; setHost: (v: string) => void;
  username: string; setUsername: (v: string) => void;
  password: string; setPassword: (v: string) => void;
  busy: boolean; status: StatusSnapshot;
  onConnect: () => void; onDisconnect: () => void;
}) {
  const { host, setHost, username, setUsername, password, setPassword, busy, status } = props;
  const connected = status.state === "civ_ready";
  const connecting = status.state === "connecting";

  return (
    <div className="panel">
      <h2>Connexion à l'IC-705</h2>
      <p className="hint">Renseigne l'IP et les identifiants RS-BA1 de la radio (réseau Wi-Fi).</p>

      <form className="form" onSubmit={(e) => { e.preventDefault(); if (!connected) props.onConnect(); }}>
        <label>
          <span>Host / IP IC-705</span>
          <input value={host} onChange={(e) => setHost(e.target.value)} placeholder="192.168.1.200" disabled={connected} />
        </label>
        <label>
          <span>Username</span>
          <input value={username} onChange={(e) => setUsername(e.target.value)} autoComplete="username" disabled={connected} />
        </label>
        <label>
          <span>Password</span>
          <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} autoComplete="current-password" disabled={connected} />
        </label>
        <div className="ports">
          <label><span>Control port</span><input value="50001" disabled /></label>
          <label><span>CI-V port</span><input value="50002" disabled /></label>
        </div>

        <div className="actions">
          {!connected ? (
            <button type="submit" className="btn primary" disabled={busy || connecting}>
              {connecting ? "Connexion…" : "Connect"}
            </button>
          ) : (
            <button type="button" className="btn danger" onClick={props.onDisconnect} disabled={busy}>
              Disconnect
            </button>
          )}
        </div>
      </form>

      <div className="status-board">
        <StatusLine ok={status.state !== "disconnected" && status.state !== "error" && status.state !== "connecting"} label="Network connected" />
        <StatusLine ok={connected} label="Authenticated" />
        <StatusLine ok={connected} label="CI-V tunnel ready" />
        <StatusLine ok={status.api_running} label={`Local API running at ${status.api_url}`} />
        {status.state === "error" && <div className="error-msg">⚠ {status.message}</div>}
      </div>
    </div>
  );
}

function StatusLine({ ok, label }: { ok: boolean; label: string }) {
  return (
    <div className={ok ? "sline ok" : "sline"}>
      <span className="mark">{ok ? "✓" : "○"}</span> {label}
    </div>
  );
}

function TerminalTab(props: {
  frame: string; setFrame: (v: string) => void;
  log: LogEntry[]; clear: () => void;
  onSend: () => void; connected: boolean; sending: boolean;
}) {
  const { frame, setFrame, log, connected, sending } = props;
  const consoleRef = useRef<HTMLDivElement>(null);

  // Autoscroll « collant » : ne suit le bas que si l'utilisateur y est déjà,
  // pour ne pas le ramener de force quand il remonte inspecter une trame.
  useEffect(() => {
    const el = consoleRef.current;
    if (!el) return;
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
    if (nearBottom) el.scrollTop = el.scrollHeight;
  }, [log]);

  return (
    <div className="panel terminal">
      <h2>CI-V Terminal</h2>
      <p className="hint">
        Saisis une trame CI-V brute en hexadécimal, par ex. <code>FE FE A4 E0 03 FD</code>.
      </p>

      <div className="console" ref={consoleRef}>
        {log.length === 0 && <div className="empty">Aucune trame pour le moment.</div>}
        {log.map((e) => (
          <div key={e.id} className={`line ${e.dir}`}>
            <span className="time">{e.ts}</span>
            <span className="tag">{e.dir.toUpperCase()}</span>
            <span className="data">{e.text}</span>
          </div>
        ))}
      </div>

      <div className="presets">
        <span className="presets-label">Exemples :</span>
        {PRESETS.map((p) => (
          <button
            key={p.label}
            type="button"
            className="chip"
            onClick={() => setFrame(p.frame)}
            disabled={!connected}
            title={p.frame}
          >
            {p.label}
          </button>
        ))}
      </div>

      <form className="sendbar" onSubmit={(e) => { e.preventDefault(); props.onSend(); }}>
        <input
          value={frame}
          onChange={(e) => setFrame(e.target.value)}
          placeholder="FE FE A4 E0 03 FD"
          spellCheck={false}
          disabled={!connected}
        />
        <button type="submit" className="btn primary" disabled={!connected || sending}>
          {sending ? "…" : "Send"}
        </button>
        <button type="button" className="btn" onClick={props.clear}>Clear</button>
      </form>
      {!connected && <div className="warn">Connecte-toi à l'IC-705 dans l'onglet Connection.</div>}
    </div>
  );
}

export default App;
