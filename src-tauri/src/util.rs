//! Petites aides de conversion hexadécimale pour les trames CI-V.

use crate::error::{BridgeError, Result};

/// Formate des octets en hex majuscule séparé par des espaces : `FE FE A4 E0 03 FD`.
pub fn to_hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

/// Parse une saisie hexadécimale tolérante (espaces, virgules, `0x`, casse libre)
/// en octets. Renvoie une erreur lisible si la saisie est invalide.
pub fn parse_hex(input: &str) -> Result<Vec<u8>> {
    // Normalisation : on ne garde que les caractères hexadécimaux.
    let cleaned: String = input
        .replace("0x", " ")
        .replace("0X", " ")
        .chars()
        .filter(|c| !matches!(c, ' ' | '\t' | '\n' | '\r' | ',' | ';' | ':' | '-'))
        .collect();

    if cleaned.is_empty() {
        return Err(BridgeError::InvalidFrame("trame vide".into()));
    }
    if cleaned.len() % 2 != 0 {
        return Err(BridgeError::InvalidFrame(format!(
            "nombre impair de chiffres hexadécimaux ({})",
            cleaned.len()
        )));
    }
    if let Some(bad) = cleaned.chars().find(|c| !c.is_ascii_hexdigit()) {
        return Err(BridgeError::InvalidFrame(format!("caractère invalide : '{bad}'")));
    }

    let bytes = (0..cleaned.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&cleaned[i..i + 2], 16).unwrap())
        .collect::<Vec<u8>>();
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spaced_frame() {
        assert_eq!(parse_hex("FE FE A4 E0 03 FD").unwrap(), vec![0xFE, 0xFE, 0xA4, 0xE0, 0x03, 0xFD]);
    }

    #[test]
    fn parses_compact_and_mixed_case() {
        assert_eq!(parse_hex("fefea4e003fd").unwrap(), vec![0xFE, 0xFE, 0xA4, 0xE0, 0x03, 0xFD]);
    }

    #[test]
    fn rejects_odd_and_invalid() {
        assert!(parse_hex("FE F").is_err());
        assert!(parse_hex("FE GG").is_err());
        assert!(parse_hex("").is_err());
    }

    #[test]
    fn roundtrip() {
        assert_eq!(to_hex(&[0xFE, 0xFE, 0xA4, 0xE0, 0x03, 0xFD]), "FE FE A4 E0 03 FD");
    }
}
