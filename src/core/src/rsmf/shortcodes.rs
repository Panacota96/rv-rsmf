//! Unicode emoji → Relativity RSMF reaction shortcodes.
//!
//! Relativity renders reactions only for the shortcodes on its "RSMF supported emoji" page
//! (help.relativity.com, Server 2024–2026 / RelativityOne); anything else is shown as text. This table is a
//! checked subset of the reactions people actually use; each entry was confirmed present on that page on
//! 2026-10-27 (`TABLE_VERSION`). `:heart:` is NOT on the page: the red heart maps to the `<3` emoticon,
//! which the page lists as "red heart".

pub const TABLE_VERSION: &str = "relativity-supported-emoji checked 2026-10-27 (Server 2024 page)";

const TABLE: &[(&str, &str)] = &[
    ("👍", ":+1:"),
    ("👎", ":-1:"),
    ("❤️", "<3"),
    ("❤", "<3"),
    ("💔", "</3"),
    ("😂", ":joy:"),
    ("😮", ":open_mouth:"),
    ("😯", ":hushed:"),
    ("😢", ":cry:"),
    ("😭", ":sob:"),
    ("🙏", ":pray:"),
    ("🔥", ":fire:"),
    ("🎉", ":tada:"),
    ("👏", ":clap:"),
    ("😁", ":grin:"),
    ("🤔", ":thinking_face:"),
    ("😡", ":rage:"),
    ("😠", ":angry:"),
    ("💯", ":100:"),
    ("😍", ":heart_eyes:"),
    ("🤣", ":rolling_on_the_floor_laughing:"),
    ("👌", ":ok_hand:"),
    ("😊", ":blush:"),
    ("🤯", ":exploding_head:"),
    ("✅", ":white_check_mark:"),
    ("🙌", ":raised_hands:"),
    ("😆", ":laughing:"),
    ("😱", ":scream:"),
    ("🤝", ":handshake:"),
    ("😘", ":kissing_heart:"),
    ("😎", ":sunglasses:"),
    ("👀", ":eyes:"),
    ("🤷", ":shrug:"),
    ("😄", ":smile:"),
    ("🙂", ":slightly_smiling_face:"),
    ("😉", ":wink:"),
    ("😃", ":smiley:"),
    ("🤨", ":face_with_raised_eyebrow:"),
    ("😐", ":neutral_face:"),
    ("🤗", ":hugging_face:"),
    ("💪", ":muscle:"),
    ("🚀", ":rocket:"),
    ("💩", ":poop:"),
    ("💀", ":skull:"),
    ("😅", ":sweat_smile:"),
    ("🙃", ":upside_down_face:"),
    ("🤪", ":zany_face:"),
    ("💖", ":sparkling_heart:"),
    ("💜", ":purple_heart:"),
    ("💛", ":yellow_heart:"),
    ("💕", ":two_hearts:"),
];

/// Maps a reaction to what goes into the manifest. Returns (value, supported_by_relativity).
/// Values already written as shortcodes (`:+1:`) or emoticons (`<3`) are kept. Skin-tone modifiers and
/// variation selectors are ignored for the lookup. Unsupported values are kept as text, at most 30
/// characters (the schema's intended limit).
pub fn map(value: &str) -> (String, bool) {
    let v = value.trim();
    if (v.len() > 2 && v.starts_with(':') && v.ends_with(':')) || v == "<3" || v == "</3" {
        let known = TABLE.iter().any(|(_, s)| *s == v);
        return (truncate(v), known);
    }
    let base: String = v
        .chars()
        .filter(|c| !matches!(*c as u32, 0xFE0F | 0x1F3FB..=0x1F3FF))
        .collect();
    if let Some((_, s)) = TABLE.iter().find(|(e, _)| *e == base || *e == v) {
        return ((*s).to_string(), true);
    }
    (truncate(v), false)
}

fn truncate(s: &str) -> String {
    s.chars().take(30).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_reactions_map_and_others_stay_text() {
        assert_eq!(map("👍"), (":+1:".into(), true));
        assert_eq!(map("👍🏽"), (":+1:".into(), true), "skin tone ignored");
        assert_eq!(map("❤️"), ("<3".into(), true));
        assert_eq!(map(":fire:"), (":fire:".into(), true));
        assert_eq!(map("🦩"), ("🦩".into(), false));
        assert_eq!(map(&"x".repeat(40)).0.chars().count(), 30);
    }
}
