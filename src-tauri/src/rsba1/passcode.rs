//! Encodage username / password du protocole RS-BA1.
//!
//! Porté à l'identique depuis kappanhang (`passcode.go`). La radio attend les
//! identifiants brouillés via une table de substitution indexée de 32 à 126.

/// Table de substitution : index = `p - 32`, pour `p` dans `32..=126`.
/// Valeurs reprises exactement de `passcode.go`.
const TABLE: [u8; 95] = [
    0x47, 0x5d, 0x4c, 0x42, 0x66, 0x20, 0x23, 0x46, 0x4e, 0x57, 0x45, 0x3d, // 32..=43
    0x67, 0x76, 0x60, 0x41, 0x62, 0x39, 0x59, 0x2d, 0x68, 0x7e, 0x7c, 0x65, // 44..=55
    0x7d, 0x49, 0x29, 0x72, 0x73, 0x78, 0x21, 0x6e, 0x5a, 0x5e, 0x4a, 0x3e, // 56..=67
    0x71, 0x2c, 0x2a, 0x54, 0x3c, 0x3a, 0x63, 0x4f, 0x43, 0x75, 0x27, 0x79, // 68..=79
    0x5b, 0x35, 0x70, 0x48, 0x6b, 0x56, 0x6f, 0x34, 0x32, 0x6c, 0x30, 0x61, // 80..=91
    0x6d, 0x7b, 0x2f, 0x4b, 0x64, 0x38, 0x2b, 0x2e, 0x50, 0x40, 0x3f, 0x55, // 92..=103
    0x33, 0x37, 0x25, 0x77, 0x24, 0x26, 0x74, 0x6a, 0x28, 0x53, 0x4d, 0x69, // 104..=115
    0x22, 0x5c, 0x44, 0x31, 0x36, 0x58, 0x3b, 0x7a, 0x51, 0x5f, 0x52, // 116..=126
];

/// Encode une chaîne (username ou password) en 16 octets pour la radio.
pub fn passcode(s: &str) -> [u8; 16] {
    let mut res = [0u8; 16];
    for (i, &c) in s.as_bytes().iter().enumerate().take(16) {
        let mut p = c as i32 + i as i32;
        if p > 126 {
            p = 32 + p % 127;
        }
        let idx = p - 32;
        if (0..TABLE.len() as i32).contains(&idx) {
            res[i] = TABLE[idx as usize];
        }
        // Hors plage -> 0 (équivalent au zero-value de la map Go).
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_expected_bounds() {
        // 32 -> 0x47, 126 -> 0x52 (premières/dernières entrées de la table kappanhang).
        assert_eq!(TABLE[0], 0x47);
        assert_eq!(TABLE[94], 0x52);
    }

    #[test]
    fn encodes_to_16_bytes() {
        let out = passcode("test");
        assert_eq!(out.len(), 16);
        // 't'=116, i=0 -> p=116 -> TABLE[116-32] = sequence[116] = 0x22
        assert_eq!(out[0], 0x22);
        // 'e'=101, i=1 -> p=102 -> sequence[102] = 0x3f
        assert_eq!(out[1], 0x3f);
    }
}
