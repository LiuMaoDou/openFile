//! Query segmentation is opt-in. Literal queries never change semantics.
use jieba_rs::Jieba;
use std::sync::OnceLock;
fn jieba() -> &'static Jieba {
    static JIEBA: OnceLock<Jieba> = OnceLock::new();
    JIEBA.get_or_init(Jieba::new)
}
fn han(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x323af)
}
fn delimiter(c: char) -> bool {
    c.is_whitespace() || "_/\\.,;:!?()[]{}<>\"'，。；：！？（）【】《》、".contains(c)
}
// Bounded runs avoid a document-sized Jieba DAG, even in minified files.
fn visit(text: &str, mut emit: impl FnMut(&str)) {
    let mut start = 0;
    let mut kind = None;
    let flush = |s: &str, chinese: bool, emit: &mut dyn FnMut(&str)| {
        if chinese {
            for word in jieba().cut(s, false) {
                emit(word.word);
            }
        } else if !s.is_empty() {
            emit(s);
        }
    };
    for (at, ch) in text.char_indices() {
        if delimiter(ch) {
            if let Some(k) = kind.take() {
                flush(&text[start..at], k, &mut emit);
            }
            start = at + ch.len_utf8();
        } else {
            let next = han(ch);
            if let Some(k) = kind {
                if k != next || (next && at - start >= 4096) {
                    flush(&text[start..at], k, &mut emit);
                    start = at;
                }
            } else {
                start = at;
            }
            kind = Some(next);
        }
    }
    if let Some(k) = kind {
        flush(&text[start..], k, &mut emit);
    }
}
pub(crate) fn terms(text: &str) -> Vec<String> {
    let mut terms = Vec::new();
    visit(text, |s| {
        let s = s.to_ascii_lowercase();
        if !terms.contains(&s) {
            terms.push(s);
        }
    });
    if terms.is_empty() && !text.trim().is_empty() {
        terms.push(text.trim().to_ascii_lowercase());
    }
    terms
}
fn encode(word: &str, output: &mut String) {
    const HEX: &[u8] = b"0123456789abcdef";
    for b in word.bytes() {
        let b = b.to_ascii_lowercase();
        output.push(HEX[(b >> 4) as usize] as char);
        output.push(HEX[(b & 15) as usize] as char);
    }
}
pub(crate) fn index_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_mul(3));
    visit(text, |word| {
        encode(word, &mut out);
        out.push(' ');
    });
    out
}
pub(crate) fn expression(terms: &[String]) -> String {
    terms
        .iter()
        .map(|word| {
            let mut out = String::new();
            encode(word, &mut out);
            format!("\"{out}\"")
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chinese_and_identifiers_keep_literal_symbols() {
        assert_eq!(
            terms("项目合同_最终版 AB-123 C++"),
            vec!["项目", "合同", "最终版", "ab-123", "c++"]
        );
        assert_eq!(terms("合同 合同"), vec!["合同"]);
        assert!(index_text("合同 AB-123 C++").len() <= "合同 AB-123 C++".len() * 3);
    }
}
