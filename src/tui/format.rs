//! format.rs
//! 표시 폭 기준 문자열 맞춤: 끝 말줄임, 경로형 중간 말줄임, 오른쪽 정렬, 제어 문자 치환.

use ratatui::text::Span;

// 1. 글자·문자열 폭 -----------------------------------------------------------
pub fn char_width(letter: char) -> usize {
    let mut buffer = [0u8; 4];
    Span::raw(&*letter.encode_utf8(&mut buffer)).width()
}

pub fn text_width(text: &str) -> usize {
    Span::raw(text).width()
}

// 2. 끝 말줄임 ----------------------------------------------------------------
// 넘치면 말줄임, 모자라면 공백으로 정확히 width 열, 제어 문자는 공백
pub fn fit_display(text: &str, width: usize) -> String {
    let total = text_width(text);
    let limit = if total > width {
        width.saturating_sub(1)
    } else {
        width
    };
    let mut fitted = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let shown = if ch.is_control() { ' ' } else { ch };
        let shown_width = char_width(shown);
        if used + shown_width > limit {
            break;
        }
        fitted.push(shown);
        used += shown_width;
    }
    if total > width && width > 0 {
        fitted.push('…');
        used += 1;
    }
    while used < width {
        fitted.push(' ');
        used += 1;
    }
    fitted
}

// 3. 경로형 중간 말줄임 -------------------------------------------------------
// 폭을 넘으면 앞부분과 마지막 / 또는 \ 구간만 남기고 사이를 …, 구분자가 없거나 꼬리가 길면 끝 말줄임
pub fn fit_display_middle(text: &str, width: usize) -> String {
    if text_width(text) <= width {
        return text.to_string();
    }
    let tail_start = text.rfind(['/', '\\']);
    let tail = tail_start.map_or("", |index| &text[index..]);
    let tail_width = text_width(tail);
    if tail.is_empty() || tail_width + 2 > width {
        return fit_display(text, width).trim_end().to_string();
    }

    // 앞부분: 남은 폭 - 말줄임 1열
    let head_limit = width - tail_width - 1;
    let mut fitted = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let shown_width = char_width(ch);
        if used + shown_width > head_limit {
            break;
        }
        fitted.push(ch);
        used += shown_width;
    }
    fitted.push('…');
    fitted.push_str(tail);
    fitted
}

// 4. 오른쪽 정렬 --------------------------------------------------------------
// 표시 폭 기준 오른쪽 맞춤, 넘치면 말줄임 (정확히 width 열)
pub fn pad_left_display(text: &str, width: usize) -> String {
    let total = text_width(text);
    if total > width {
        return fit_display(text, width);
    }
    format!("{}{text}", " ".repeat(width - total))
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1. 끝 말줄임
    #[test]
    fn fit_pads_and_truncates_by_display_width() {
        assert_eq!(fit_display("abc", 5), "abc  ");
        assert_eq!(fit_display("abcdef", 4), "abc…");
        // 한글 한 글자는 2열
        assert_eq!(text_width(&fit_display("도구목록", 6)), 6);
        assert_eq!(fit_display("도구목록", 6), "도구… ");
        assert_eq!(fit_display("a\nb", 3), "a b");
        assert_eq!(fit_display("abc", 0), "");
    }

    // 2. 중간 말줄임
    #[test]
    fn middle_fit_keeps_head_and_last_path_segment() {
        assert_eq!(fit_display_middle("a/b.rs", 20), "a/b.rs");
        assert_eq!(
            fit_display_middle("C:/very/long/directory/tree/file.rs", 20),
            "C:/very/lon…/file.rs"
        );
        assert_eq!(fit_display_middle("abcdefghij", 6), "abcde…");
        for width in 3..30 {
            let fitted = fit_display_middle("C:/데이터/긴폴더/하위폴더/보고서.md", width);
            assert!(text_width(&fitted) <= width, "{width}");
        }
    }

    // 3. 오른쪽 정렬
    #[test]
    fn pad_left_aligns_by_display_width_and_truncates() {
        assert_eq!(pad_left_display("12", 5), "   12");
        assert_eq!(pad_left_display("한글", 6), "  한글");
        assert_eq!(pad_left_display("abcdef", 4), "abc…");
        assert_eq!(text_width(&pad_left_display("도구목록", 6)), 6);
        assert_eq!(pad_left_display("a", 0), "");
    }
}
