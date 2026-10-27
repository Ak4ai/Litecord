//! Servers often decorate channel and category names with "fancy text":
//! letters from the Unicode Mathematical Alphanumeric Symbols block (𝙶𝚊𝚖𝚎, 𝘋𝘪𝘤𝘢𝘴).
//! Neither the bundled Inter nor the renderer's fallback has those glyphs, so
//! they showed up as empty boxes or blanks. Channel names, author names and the
//! text of messages and embeds map them back to the plain letters they stand for,
//! which Inter renders; everything else is left untouched.

use std::borrow::Cow;

const MATH_LETTERS_START: u32 = 0x1D400;
const MATH_LETTERS_END: u32 = 0x1D6A3;
const MATH_DOTLESS_I: u32 = 0x1D6A4;
const MATH_DOTLESS_J: u32 = 0x1D6A5;
const MATH_DIGITS_START: u32 = 0x1D7CE;
const MATH_DIGITS_END: u32 = 0x1D7FF;

/// The plain letter or digit a styled one stands for.
pub fn plain_letter(c: char) -> Option<char> {
    let cp = c as u32;
    match cp {
        // 13 styles of 52 letters each: A-Z then a-z.
        MATH_LETTERS_START..=MATH_LETTERS_END => {
            let offset = ((cp - MATH_LETTERS_START) % 52) as u8;
            Some(if offset < 26 { (b'A' + offset) as char } else { (b'a' + offset - 26) as char })
        }
        MATH_DOTLESS_I => Some('i'),
        MATH_DOTLESS_J => Some('j'),
        // 5 styles of 10 digits each.
        MATH_DIGITS_START..=MATH_DIGITS_END => Some((b'0' + ((cp - MATH_DIGITS_START) % 10) as u8) as char),
        _ => letterlike_hole(c),
    }
}

/// Letters that predate the math block live in Letterlike Symbols, and the
/// math block leaves holes where they are, so styled text mixes both.
fn letterlike_hole(c: char) -> Option<char> {
    Some(match c {
        'ℎ' => 'h', 'ℊ' => 'g', 'ℯ' => 'e', 'ℴ' => 'o',
        'ℬ' => 'B', 'ℰ' => 'E', 'ℱ' => 'F', 'ℋ' => 'H', 'ℐ' => 'I',
        'ℒ' => 'L', 'ℳ' => 'M', 'ℛ' => 'R',
        'ℭ' => 'C', 'ℌ' => 'H', 'ℑ' => 'I', 'ℜ' => 'R', 'ℨ' => 'Z',
        'ℂ' => 'C', 'ℍ' => 'H', 'ℕ' => 'N', 'ℙ' => 'P', 'ℚ' => 'Q', 'ℝ' => 'R', 'ℤ' => 'Z',
        _ => return None,
    })
}

/// Returns the name with styled math letters replaced by plain ones.
/// Borrows when there is nothing to replace, which is almost every name.
pub fn plain_display_name(name: &str) -> Cow<'_, str> {
    if !name.chars().any(|c| plain_letter(c).is_some()) {
        return Cow::Borrowed(name);
    }
    Cow::Owned(name.chars().map(|c| plain_letter(c).unwrap_or(c)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styled_letters_become_plain_letters() {
        assert_eq!(plain_display_name("┌𝙶𝚊𝚖𝚎-𝙳𝚎𝚟𝚎𝚕𝚘𝚙𝚖𝚎𝚗𝚝┐"), "┌Game-Development┐");
        assert_eq!(plain_display_name("𝘋𝘪𝘤𝘢𝘴"), "Dicas");
        assert_eq!(plain_display_name("𝐁𝐨𝐥𝐝 𝟏𝟐𝟑"), "Bold 123");
        assert_eq!(plain_display_name("𝑃𝑙𝑎𝑛𝑐𝑘 ℎ"), "Planck h");
        assert_eq!(plain_display_name("📜-𝗥𝗨𝗟𝗘𝗦"), "📜-RULES");
        assert_eq!(plain_display_name("♡ ━━━ 𝗔𝗥𝗖𝗛𝗜𝗩𝗘𝗗 ━━━ ♡"), "♡ ━━━ ARCHIVED ━━━ ♡");
    }

    #[test]
    fn ordinary_names_are_borrowed_unchanged() {
        let name = "ᴍᴀʀᴠᴇʟ ʀɪᴠᴀʟs ┊ geral 🎮 ação";
        assert!(matches!(plain_display_name(name), Cow::Borrowed(n) if n == name));
    }
}
