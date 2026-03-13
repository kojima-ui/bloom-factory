//! Cryptographic utilities for JioSaavn plugin.
//! Handles decryption of stream URLs using DES-ECB.

use anyhow::{anyhow, Result};
use base64::{engine::general_purpose, Engine as _};
use des::cipher::{generic_array::GenericArray, BlockDecrypt, KeyInit};
use des::Des;

const DES_KEY: &[u8; 8] = b"38346591";

/// Decrypts the encrypted media URL from JioSaavn.
///
/// The process is:
/// 1. Base64 decode.
/// 2. DES-ECB decrypt using key "38346591".
/// 3. UTF-8 decode.
/// 4. Replace .mp4.* with .mp4 and .m4a.* with .m4a.
/// 5. Replace http: with https:.
pub fn decode_media_url(input: &str) -> Result<String> {
    // 1. Base64 decode
    let encrypted_bytes = general_purpose::STANDARD
        .decode(input)
        .map_err(|e| anyhow!("Base64 decode failed: {}", e))?;

    // 2. DES-ECB decrypt
    let cipher = Des::new(GenericArray::from_slice(DES_KEY));

    // DES block size is 8 bytes.
    if encrypted_bytes.len() % 8 != 0 {
        return Err(anyhow!("Invalid encrypted data length"));
    }

    let mut decrypted_bytes = encrypted_bytes;

    // Process each block
    for chunk in decrypted_bytes.chunks_mut(8) {
        let block = GenericArray::from_mut_slice(chunk);
        cipher.decrypt_block(block);
    }

    // PKCS#7 unpadding
    let decrypted_len = decrypted_bytes.len();
    if decrypted_len > 0 {
        let last_byte = decrypted_bytes[decrypted_len - 1];
        if last_byte > 0 && last_byte <= 8 {
            let pad_len = last_byte as usize;
            if decrypted_len >= pad_len {
                let padding_start = decrypted_len - pad_len;
                let is_valid_padding = decrypted_bytes[padding_start..]
                    .iter()
                    .all(|&b| b == last_byte);
                if is_valid_padding {
                    decrypted_bytes.truncate(padding_start);
                }
            }
        }
    }

    // 3. UTF-8 decode
    let decoded_str =
        String::from_utf8(decrypted_bytes).map_err(|e| anyhow!("UTF-8 decode failed: {}", e))?;

    // 4. Replacements
    let decoded_str = clean_extension(&decoded_str, ".mp4");
    let decoded_str = clean_extension(&decoded_str, ".m4a");

    // 5. https replacement
    let final_url = decoded_str.replace("http:", "https:");

    Ok(final_url)
}

fn clean_extension(input: &str, ext: &str) -> String {
    if let Some(idx) = input.find(ext) {
        if idx + ext.len() < input.len() {
            return input[..idx + ext.len()].to_string();
        }
    }
    input.to_string()
}
