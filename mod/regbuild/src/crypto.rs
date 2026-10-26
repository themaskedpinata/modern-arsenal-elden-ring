//! AES-256-CBC of the Elden Ring regulation.bin: file = 16-byte IV + ciphertext (SoulsFormats `SFUtil.DecryptERRegulation`).
//!
//! The vanilla file and WitchyBND's output both carry an all-zero IV, so a build is deterministic; it must stay zero.
//! Reading: decrypt without padding (vanilla has none: its DCX is already 16-aligned) and trust the DCX sizes. Writing:
//! WitchyBND's game-proven framing: the DCX zero-padded to 16, then one PKCS7 block (16 x 0x10), IV 0.

use aes::Aes256;
use aes::cipher::block_padding::NoPadding;
use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};

/// SoulsFormats' `erRegulationKey` (verified by a successful decrypt of the 2.7.2.0 vanilla file to `DCX\0`).
pub const ER_REGULATION_KEY: [u8; 32] = [
    0x99, 0xBF, 0xFC, 0x36, 0x6A, 0x6B, 0xC8, 0xC6, 0xF5, 0x82, 0x7D, 0x09, 0x36, 0x02, 0xD6, 0x76, 0xC4, 0x28, 0x92, 0xA0,
    0x1C, 0x20, 0x7F, 0xB0, 0x24, 0xD3, 0xAF, 0x4E, 0x49, 0x3F, 0xEF, 0x99,
];

/// The IV of every regulation this builder writes (== vanilla's and WitchyBND's).
pub const IV: [u8; 16] = [0; 16];

/// Decrypts `file` (IV + ciphertext); the plaintext keeps any padding (the DCX sizes delimit the data).
pub fn decrypt(file: &[u8]) -> Result<Vec<u8>, String> {
    if file.len() < 32 || !(file.len() - 16).is_multiple_of(16) {
        return Err(format!("regulation.bin: {} bytes is not IV + whole AES blocks", file.len()));
    }
    let (iv, body) = file.split_at(16);
    let mut buf = body.to_vec();
    cbc::Decryptor::<Aes256>::new(&ER_REGULATION_KEY.into(), iv.into())
        .decrypt_padded_mut::<NoPadding>(&mut buf)
        .map_err(|_| "regulation.bin: AES decrypt failed".to_string())?;
    Ok(buf)
}

/// The IV of an encrypted file (diagnostics: the harness reports it).
pub fn iv_of(file: &[u8]) -> Option<[u8; 16]> {
    file.get(..16).and_then(|s| s.try_into().ok())
}

/// Encrypts a DCX: zero padding to 16, one PKCS7 block, AES-256-CBC with IV 0; returns IV + ciphertext.
pub fn encrypt(dcx: &[u8]) -> Vec<u8> {
    let mut buf = dcx.to_vec();
    buf.resize(dcx.len().div_ceil(16) * 16, 0);
    buf.extend_from_slice(&[16u8; 16]);
    let n = buf.len();
    cbc::Encryptor::<Aes256>::new(&ER_REGULATION_KEY.into(), &IV.into())
        .encrypt_padded_mut::<NoPadding>(&mut buf, n)
        .expect("whole blocks");
    let mut out = Vec::with_capacity(16 + n);
    out.extend_from_slice(&IV);
    out.extend_from_slice(&buf);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aes_256_cbc_known_answer() {
        // NIST SP 800-38A F.2.5 (CBC-AES256.Encrypt), block 1, with our framing taken apart by hand.
        let key: [u8; 32] = hex("603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4").try_into().unwrap();
        let iv: [u8; 16] = hex("000102030405060708090a0b0c0d0e0f").try_into().unwrap();
        let mut b = hex("6bc1bee22e409f96e93d7e117393172a");
        cbc::Encryptor::<Aes256>::new(&key.into(), &iv.into()).encrypt_padded_mut::<NoPadding>(&mut b, 16).unwrap();
        assert_eq!(b, hex("f58c4c04d6e5f1ba779eabfb5f7bfbd6"));
    }

    #[test]
    fn round_trip_and_framing() {
        for len in [0usize, 1, 15, 16, 17, 76, 1000] {
            let dcx: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
            let f = encrypt(&dcx);
            assert_eq!(&f[..16], &IV, "zero IV");
            let pad = len.div_ceil(16) * 16;
            assert_eq!(f.len(), 16 + pad + 16, "zero pad + one PKCS7 block");
            let p = decrypt(&f).unwrap();
            assert_eq!(&p[..len], &dcx[..]);
            assert!(p[len..pad].iter().all(|&b| b == 0));
            assert!(p[pad..].iter().all(|&b| b == 16));
            assert_eq!(encrypt(&dcx), f, "deterministic");
        }
        assert!(decrypt(&[0u8; 20]).is_err());
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }
}
