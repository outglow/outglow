use crate::find;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Known {
    pub name: &'static str,
    pub model: &'static str,
    pub hw_revision: &'static str,
    pub ble_product: u16,
    pub kind: &'static str,
    pub image_type: u16,
}

include!("boards_table.rs");

pub const TAG: &[u8] = b"outglow-board:";
pub const LEGACY_TAG: &[u8] = b"open-eglo-board:";
pub const KINDS: [&str; 5] = ["outglow-rgbtw", "outglow-rgbw", "outglow-dimm", "outglow-tw", "outglow-ebf"];

pub fn known(name: &str) -> Option<&'static Known> {
    BOARDS.iter().find(|b| b.name == name)
}

pub fn tag(image: &[u8]) -> Option<String> {
    let at = find(image, TAG).map(|i| i + TAG.len()).or_else(|| find(image, LEGACY_TAG).map(|i| i + LEGACY_TAG.len()))?;
    let rest = &image[at..];
    let model = rest.iter().position(|&c| !(c.is_ascii_alphanumeric() || c == b'_')).unwrap_or(rest.len());
    let rev = rest.get(model..model + 5)?;
    let ok = model > 0
        && rev[0] == b'-'
        && rev[1].is_ascii_digit()
        && rev[2] == b'.'
        && rev[3].is_ascii_digit()
        && rev[4].is_ascii_digit();
    ok.then(|| String::from_utf8_lossy(&rest[..model + 5]).into_owned())
}

pub fn kind(image: &[u8]) -> Option<&'static str> {
    for prefix in [&b"outglow-"[..], &b"open-eglo-"[..]] {
        let mut from = 0;
        while let Some(i) = find(&image[from..], prefix) {
            let rest = &image[from + i + prefix.len()..];
            if let Some(k) = KINDS.iter().find(|k| rest.starts_with(&k.as_bytes()["outglow-".len()..])) {
                return Some(k);
            }
            from += i + 1;
        }
    }
    None
}

#[derive(Clone, Debug, PartialEq)]
pub enum Identity {
    Outglow { board: String },
    Original { model: String, hw_revision: String },
}

impl Identity {
    pub fn model_and_revision(&self) -> (String, String) {
        match self {
            Identity::Outglow { board } => {
                let (m, r) = board.rsplit_once('-').unwrap_or((board, ""));
                (m.into(), r.into())
            }
            Identity::Original { model, hw_revision } => (model.clone(), hw_revision.clone()),
        }
    }
}

fn revisions(image: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for i in 0..image.len().saturating_sub(3) {
        let w = &image[i..i + 4];
        if w[0] == b'4' && w[1] == b'.' && w[2].is_ascii_digit() && w[3].is_ascii_digit() {
            let before = if i > 0 { image[i - 1] } else { 0 };
            let after = image.get(i + 4).copied().unwrap_or(0);
            if !(before.is_ascii_digit() || before == b'.' || after.is_ascii_digit() || after == b'.') {
                let s = String::from_utf8_lossy(w).into_owned();
                if !out.contains(&s) {
                    out.push(s);
                }
            }
        }
    }
    out.sort();
    out
}

pub fn identify(image: &[u8]) -> Result<Identity, String> {
    if let Some(board) = tag(image) {
        return Ok(Identity::Outglow { board });
    }
    let mut models: Vec<&str> = BOARDS.iter().map(|b| b.model).collect();
    models.sort_by_key(|m| std::cmp::Reverse(m.len()));
    models.dedup();
    let model = models.into_iter().find(|m| {
        let mut needle = m.as_bytes().to_vec();
        needle.push(0);
        find(image, &needle).is_some()
    });
    let revs = revisions(image);
    match (model, revs.as_slice()) {
        (Some(m), [r]) => Ok(Identity::Original { model: m.into(), hw_revision: r.clone() }),
        _ => Err(format!("cannot tell the board of the image: model {model:?}, revisions {revs:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tag_of_an_outglow_image() {
        let img = b"\x00\x01outglow-board:ESMLFzm_w6_Dimm-4.63\x00outglow-dimm\x00";
        assert_eq!(tag(img).as_deref(), Some("ESMLFzm_w6_Dimm-4.63"));
        assert_eq!(kind(img), Some("outglow-dimm"));
        assert_eq!(identify(img), Ok(Identity::Outglow { board: "ESMLFzm_w6_Dimm-4.63".into() }));
        assert!(known("ESMLFzm_w6_Dimm-4.63").is_some_and(|b| b.ble_product == 0x20a7 && b.image_type == 0x0390));
    }

    #[test]
    fn the_tag_ends_with_its_revision_whatever_follows_it_in_flash() {
        assert_eq!(tag(b"outglow-board:EGLO_ZM_RGB_W-4.62the radio link").as_deref(), Some("EGLO_ZM_RGB_W-4.62"));
        assert_eq!(tag(b"outglow-board:ESMLFzm_w6_TW-4.6312").as_deref(), Some("ESMLFzm_w6_TW-4.63"));
        assert_eq!(tag(b"outglow-board:EGLO_ZM_RGB_W-4.6"), None);
        assert_eq!(tag(b"outglow-board:-4.62"), None);
    }

    #[test]
    fn the_first_kind_in_the_image() {
        assert_eq!(kind(b"x outglow-rgbtw\0"), Some("outglow-rgbtw"));
        assert_eq!(kind(b"x outglow-board:A-4.60 outglow-dimmoutglow-tw"), Some("outglow-dimm"));
        assert_eq!(kind(b"x open-eglo-board:A-4.60 open-eglo-tw\0"), Some("outglow-tw"));
        assert_eq!(tag(b"x open-eglo-board:ESMLFzm_w6_Dimm-4.63\0").as_deref(), Some("ESMLFzm_w6_Dimm-4.63"));
    }

    #[test]
    fn an_original_image_by_model_and_single_revision() {
        let img = b"..ESMLFzm_w6_TW\0..v3.0.2.. 4.63\0..14.633..";
        assert_eq!(identify(img), Ok(Identity::Original { model: "ESMLFzm_w6_TW".into(), hw_revision: "4.63".into() }));
        assert!(identify(b"ESMLFzm_w6_TW\0 4.60 4.63").is_err());
    }

    #[test]
    fn the_model_and_revision_of_either_identity() {
        let o = Identity::Outglow { board: "EGLO_ZM_RGB_TW-4.62".into() };
        assert_eq!(o.model_and_revision(), ("EGLO_ZM_RGB_TW".into(), "4.62".into()));
        let o = Identity::Outglow { board: "nodash".into() };
        assert_eq!(o.model_and_revision(), ("nodash".into(), "".into()));
        let s = Identity::Original { model: "ESMLFzm_w6_TW".into(), hw_revision: "4.63".into() };
        assert_eq!(s.model_and_revision(), ("ESMLFzm_w6_TW".into(), "4.63".into()));
    }

    #[test]
    fn a_revision_is_4_dot_two_digits_standing_alone() {
        assert_eq!(revisions(b"4.63"), ["4.63"]);
        assert_eq!(revisions(b"4.63 x4.60;4.69\0"), ["4.60", "4.63", "4.69"]);
        assert_eq!(revisions(b"4.62 4.62"), ["4.62"]);
        for not in [&b"14.63"[..], b"4.631", b".4.63", b"4.63.", b"x5.63", b"4x63", b"4.x3", b"4.6x", b" ab12 ", b"4.6", b""] {
            assert_eq!(revisions(not), Vec::<String>::new(), "{}", String::from_utf8_lossy(not));
        }
        assert_eq!(revisions(b"ab 4.631"), Vec::<String>::new());
    }

    #[test]
    fn an_original_image_needs_a_known_model() {
        assert!(identify(b"Unknown_Model\0 4.63").is_err());
        assert_eq!(
            identify(b"ESMLFzm_w6_TW 4.63"),
            Err("cannot tell the board of the image: model None, revisions [\"4.63\"]".into())
        );
        assert_eq!(identify(b"x EGLO_ZM_TW_CLP\0 4.60"), Ok(Identity::Original { model: "EGLO_ZM_TW_CLP".into(), hw_revision: "4.60".into() }));
        assert_eq!(identify(b"x EGLO_ZM_TW\0 4.60"), Ok(Identity::Original { model: "EGLO_ZM_TW".into(), hw_revision: "4.60".into() }));
    }

    #[test]
    fn no_kind_in_an_image_without_one() {
        assert_eq!(kind(b"outglow-board:X-4.60 outglow-xyz open-eglo-"), None);
        assert_eq!(tag(b"outglow-board:ESMLFzm_w6_TW-4.6"), None);
        assert_eq!(tag(b"outglow-board:ESMLFzm_w6_TW+4.63"), None);
        assert_eq!(tag(b"outglow-board:ESMLFzm_w6_TW-x.63"), None);
        assert_eq!(tag(b"outglow-board:ESMLFzm_w6_TW-4x63"), None);
        assert_eq!(tag(b"outglow-board:ESMLFzm_w6_TW-4.x3"), None);
        assert_eq!(tag(b"outglow-board:ESMLFzm_w6_TW-4.6x"), None);
        assert_eq!(tag(b"no tag"), None);
    }
}
