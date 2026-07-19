//! zstd helpers for raw/processed blobs at rest (SPEC C7: disk is tight).

use anyhow::{Context, Result};

const LEVEL: i32 = 7; // good ratio on JSON text, still fast on 1 vCPU

pub fn compress(bytes: &[u8]) -> Result<Vec<u8>> {
    zstd::encode_all(bytes, LEVEL).context("zstd compress")
}

#[allow(dead_code)] // readers arrive with P4 (extract) and the panel
pub fn decompress(bytes: &[u8]) -> Result<Vec<u8>> {
    zstd::decode_all(bytes).context("zstd decompress")
}

#[cfg(test)]
mod tests {
    #[test]
    fn roundtrip() {
        let data = "русский текст ".repeat(100);
        let packed = super::compress(data.as_bytes()).unwrap();
        assert!(packed.len() < data.len() / 4);
        assert_eq!(super::decompress(&packed).unwrap(), data.as_bytes());
    }
}
