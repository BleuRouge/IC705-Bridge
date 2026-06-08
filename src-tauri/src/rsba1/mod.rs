//! Implémentation Rust du protocole réseau RS-BA1 de l'Icom IC-705.
//!
//! Portée depuis kappanhang (Go). Voir `docs/RSBA1_protocol.md` pour la spec.
//! Trois sous-modules :
//! - [`stream`] : machinerie commune (handshake, keepalive pkt0/pkt7, retransmission) ;
//! - [`control`] : stream de contrôle (login, auth, négociation) sur le port 50001 ;
//! - [`serial`] : stream CI-V (TX/RX des trames) sur le port 50002.

pub mod control;
pub mod passcode;
pub mod serial;
pub mod stream;
