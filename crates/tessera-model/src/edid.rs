//! EDID capability parsing — a bounded subset for color management.
//!
//! Reads the CTA-861 extension's HDR Static Metadata and Colorimetry
//! data blocks: HDR transfer-function support (ST 2084 PQ, HLG) and
//! BT.2020 RGB colorimetry. Everything else (make/model/serial strings,
//! chromaticity coordinates, detailed timings) is deliberately out of
//! scope — display primaries for output transforms come from ICC
//! profiles, not EDID, which is not universally reliable.

/// Color-related display capabilities advertised through EDID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EdidColorCapabilities {
    /// ST 2084 (PQ) HDR static metadata support (CTA-861.3).
    pub hdr_pq: bool,
    /// Hybrid Log-Gamma HDR support (CTA-861.3).
    pub hdr_hlg: bool,
    /// BT.2020 RGB colorimetry support (CTA-861 colorimetry block).
    pub bt2020_rgb: bool,
}

impl EdidColorCapabilities {
    /// Any HDR transfer function the compositor can drive.
    pub fn supports_hdr(&self) -> bool {
        self.hdr_pq || self.hdr_hlg
    }
}

const EDID_BLOCK: usize = 128;
const CTA861_TAG: u8 = 0x02;
const CTA_EXTENDED_BLOCK_TAG: u8 = 7;
const CTA_EXT_COLORIMETRY: u8 = 0x05;
const CTA_EXT_HDR_STATIC_METADATA: u8 = 0x06;

/// Parse HDR/wide-gamut capabilities from a raw EDID blob (128-byte base
/// block plus extension blocks). Absent or malformed extensions yield
/// `false` flags — a display that cannot prove HDR is treated as SDR.
pub fn edid_color_capabilities(edid: &[u8]) -> EdidColorCapabilities {
    let mut caps = EdidColorCapabilities::default();
    if edid.len() < EDID_BLOCK {
        return caps;
    }
    let extension_count = (edid[126] as usize).min(edid.len() / EDID_BLOCK - 1);
    for index in 0..extension_count {
        let block = &edid[(index + 1) * EDID_BLOCK..(index + 2) * EDID_BLOCK];
        if block[0] != CTA861_TAG {
            continue;
        }
        parse_cta_blocks(block, &mut caps);
    }
    caps
}

/// Walk the CTA-861 data-block collection (between the 4-byte header and
/// the DTD offset) for extended data blocks carrying color information.
fn parse_cta_blocks(block: &[u8], caps: &mut EdidColorCapabilities) {
    let dtd_offset = block[2] as usize;
    if !(4..=EDID_BLOCK).contains(&dtd_offset) {
        return;
    }
    let mut at = 4usize;
    while at < dtd_offset {
        let header = block[at];
        let tag = header >> 5;
        let len = (header & 0x1F) as usize;
        let payload = &at + 1..at + 1 + len;
        if payload.end > dtd_offset {
            break;
        }
        if tag == CTA_EXTENDED_BLOCK_TAG && len >= 1 {
            let extended = block[payload.start];
            let data = &block[payload.start + 1..payload.end];
            match extended {
                CTA_EXT_HDR_STATIC_METADATA if !data.is_empty() => {
                    let eotfs = data[0];
                    caps.hdr_pq |= eotfs & 0x04 != 0; /* ST 2084 */
                    caps.hdr_hlg |= eotfs & 0x08 != 0; /* HLG */
                }
                CTA_EXT_COLORIMETRY if !data.is_empty() => {
                    caps.bt2020_rgb |= data[0] & 0x80 != 0; /* BT.2020 RGB */
                }
                _ => {}
            }
        }
        at += 1 + len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a 2-block EDID: base block with one extension, CTA-861
    /// extension whose data blocks are `datablocks`.
    fn synthetic_edid(datablocks: &[(u8, &[u8])]) -> Vec<u8> {
        let mut edid = vec![0u8; EDID_BLOCK * 2];
        edid[126] = 1; // one extension
        let cta = &mut edid[EDID_BLOCK..];
        cta[0] = CTA861_TAG;
        cta[1] = 3; // revision
        let mut at = 4usize;
        for (ext_tag, payload) in datablocks {
            cta[at] = (CTA_EXTENDED_BLOCK_TAG << 5) | ((payload.len() as u8 + 1) & 0x1F);
            cta[at + 1] = *ext_tag;
            cta[at + 2..at + 2 + payload.len()].copy_from_slice(payload);
            at += 2 + payload.len();
        }
        cta[2] = at as u8; // DTD offset ends the data-block collection
        edid
    }

    #[test]
    fn pq_and_bt2020_are_detected() {
        // HDR SMD: EOTF mask 0b1100 = PQ + HLG; colorimetry 0x80 = BT.2020 RGB.
        let edid = synthetic_edid(&[
            (CTA_EXT_HDR_STATIC_METADATA, &[0x0C, 0x01, 0, 0, 0]),
            (CTA_EXT_COLORIMETRY, &[0x80, 0x00]),
        ]);
        let caps = edid_color_capabilities(&edid);
        assert!(caps.hdr_pq);
        assert!(caps.hdr_hlg);
        assert!(caps.bt2020_rgb);
        assert!(caps.supports_hdr());
    }

    #[test]
    fn sdr_only_display_reports_no_hdr() {
        let edid = synthetic_edid(&[(CTA_EXT_HDR_STATIC_METADATA, &[0x01, 0x00, 0, 0, 0])]);
        let caps = edid_color_capabilities(&edid);
        assert!(!caps.supports_hdr());
        assert!(!caps.bt2020_rgb);
    }

    #[test]
    fn missing_or_garbage_edid_is_sdr() {
        assert_eq!(
            edid_color_capabilities(&[]),
            EdidColorCapabilities::default()
        );
        assert_eq!(
            edid_color_capabilities(&[0xAA; EDID_BLOCK]),
            EdidColorCapabilities::default()
        );
        // Extension count larger than the buffer must not read out of bounds.
        let mut edid = vec![0u8; EDID_BLOCK];
        edid[126] = 4;
        assert_eq!(
            edid_color_capabilities(&edid),
            EdidColorCapabilities::default()
        );
    }

    #[test]
    fn truncated_data_block_is_ignored() {
        let mut edid = synthetic_edid(&[(CTA_EXT_HDR_STATIC_METADATA, &[0x04, 0x01])]);
        // Corrupt the final block's length so it overruns the DTD offset.
        let cta = &mut edid[EDID_BLOCK..];
        let at = 4usize + 2 + 2; // start of second block... first block len 4
        let _ = at;
        cta[2] = 6; // DTD offset cuts inside the first data block
        let caps = edid_color_capabilities(&edid);
        assert!(!caps.hdr_pq);
    }
}
