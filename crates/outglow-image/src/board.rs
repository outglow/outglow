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

pub fn by_model(model: &str, hw_revision: &str) -> Option<&'static Known> {
    BOARDS.iter().find(|b| b.model == model && b.hw_revision == hw_revision)
}

pub fn tag(image: &[u8]) -> Option<String> {
    let at = find(image, TAG).map(|i| i + TAG.len()).or_else(|| find(image, LEGACY_TAG).map(|i| i + LEGACY_TAG.len()))?;
    let rest = &image[at..];
    let end = rest.iter().position(|&c| !(c.is_ascii_alphanumeric() || c == b'_' || c == b'-' || c == b'.'))
        .unwrap_or(rest.len());
    let name = std::str::from_utf8(&rest[..end]).ok()?;
    let (model, rev) = name.rsplit_once('-')?;
    let ok = !model.is_empty()
        && rev.len() >= 3
        && rev.as_bytes()[0].is_ascii_digit()
        && rev.as_bytes()[1] == b'.'
        && rev[2..].bytes().all(|c| c.is_ascii_digit());
    ok.then(|| name.to_string())
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
}
