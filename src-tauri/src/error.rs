use std::fmt;

/// Erreurs du cœur réseau RS-BA1 / pont CI-V.
#[derive(Debug)]
pub enum BridgeError {
    /// Problème de socket / I/O réseau.
    Io(std::io::Error),
    /// La radio n'a pas répondu dans le délai imparti à une étape du handshake.
    Timeout(String),
    /// Identifiants (username / password) refusés par la radio.
    InvalidCredentials,
    /// La radio a refusé / interrompu l'authentification.
    AuthFailed(String),
    /// Aucune session active (envoi CI-V sans connexion).
    NotConnected,
    /// Trame CI-V invalide (saisie hexadécimale).
    InvalidFrame(String),
    /// Autre erreur de protocole.
    Protocol(String),
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BridgeError::Io(e) => write!(f, "erreur réseau : {e}"),
            BridgeError::Timeout(s) => write!(f, "délai dépassé : {s}"),
            BridgeError::InvalidCredentials => {
                write!(f, "identifiants invalides (username / password refusés)")
            }
            BridgeError::AuthFailed(s) => write!(f, "authentification échouée : {s}"),
            BridgeError::NotConnected => write!(f, "non connecté à l'IC-705"),
            BridgeError::InvalidFrame(s) => write!(f, "trame CI-V invalide : {s}"),
            BridgeError::Protocol(s) => write!(f, "erreur de protocole : {s}"),
        }
    }
}

impl std::error::Error for BridgeError {}

impl From<std::io::Error> for BridgeError {
    fn from(e: std::io::Error) -> Self {
        BridgeError::Io(e)
    }
}

/// Sérialisation pour les commandes Tauri (qui exigent un type sérialisable).
impl serde::Serialize for BridgeError {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, BridgeError>;
