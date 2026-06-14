import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
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

function App() {
  const [tab, setTab] = useState<Tab>("connection");
  const [status, setStatus] = useState<StatusSnapshot>({
    state: "disconnected",
    message: "Déconnecté",
    host: null,
    api_running: false,
    api_url: "http://127.0.0.1:8765",
  });

  // Champs de connexion
  const [host, setHost] = useState("192.168.1.200");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);

  // Terminal CI-V
  const [frame, setFrame] = useState("FE FE A4 E0 03 FD");
  const [log, setLog] = useState<LogEntry[]>([]);
  const pendingLog = useRef<LogEntry[]>([]);
  const flushTimer = useRef<number | null>(null);

  // Mise à jour de l'application
  const [update, setUpdate] = useState<Update | null>(null);
  const [updateProgress, setUpdateProgress] = useState<UpdateProgress | null>(null);

  const connected = status.state === "civ_ready";

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
      addLog("info", `Connecté à ${host}`);
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
    if (!f) return;
    addLog("tx", f);
    try {
      // L'envoi déclenche les trames RX, affichées en temps réel via l'événement
      // `civ-rx`. On ignore donc la réponse agrégée renvoyée ici (évite les doublons).
      await invoke<CivResult>("send_civ", { frame: f });
    } catch (err) {
      addLog("error", String(err));
    }
  }

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          <span className="logo">📻</span>
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
  onSend: () => void; connected: boolean;
}) {
  const { frame, setFrame, log, connected } = props;
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

      <form className="sendbar" onSubmit={(e) => { e.preventDefault(); props.onSend(); }}>
        <input
          value={frame}
          onChange={(e) => setFrame(e.target.value)}
          placeholder="FE FE A4 E0 03 FD"
          spellCheck={false}
          disabled={!connected}
        />
        <button type="submit" className="btn primary" disabled={!connected}>Send</button>
        <button type="button" className="btn" onClick={props.clear}>Clear</button>
      </form>
      {!connected && <div className="warn">Connecte-toi à l'IC-705 dans l'onglet Connection.</div>}
    </div>
  );
}

export default App;
