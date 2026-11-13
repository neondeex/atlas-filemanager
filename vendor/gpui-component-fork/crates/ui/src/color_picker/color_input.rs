use super::Colorize;
use gpui::{Hsla, hsla};

/// Complete color values only. Partial/invalid edits stay in the input until
/// the user commits, without rewriting their text or moving the caret.
pub(super) fn parse(value: &str) -> Option<Hsla> {
    let value = value.trim();
    let hex = value.strip_prefix('#').unwrap_or(value);
    if hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        if matches!(hex.len(), 6 | 8) {
            return Hsla::parse_hex(hex).ok();
        }
        if matches!(hex.len(), 3 | 4) {
            let expanded: String = hex.chars().flat_map(|c| [c, c]).collect();
            return Hsla::parse_hex(&expanded).ok();
        }
    }
    let lower = value.to_ascii_lowercase();
    let (body, count) = if let Some(body) = lower.strip_prefix("hsla(") {
        (body.strip_suffix(')')?, 4)
    } else {
        (lower.strip_prefix("hsl(")?.strip_suffix(')')?, 3)
    };
    let parts: Vec<_> = body.split(',').map(str::trim).collect();
    if parts.len() != count {
        return None;
    }
    let hue = finite(parts[0].strip_suffix("deg").unwrap_or(parts[0]))?;
    let saturation = percent(parts[1])?;
    let lightness = percent(parts[2])?;
    let alpha = if count == 4 {
        if parts[3].ends_with('%') {
            percent(parts[3])?
        } else {
            finite(parts[3])?
        }
    } else {
        1.0
    };
    (0.0..=1.0)
        .contains(&alpha)
        .then(|| hsla(hue.rem_euclid(360.0) / 360.0, saturation, lightness, alpha))
}

fn finite(value: &str) -> Option<f32> {
    let value = value.trim().parse::<f32>().ok()?;
    value.is_finite().then_some(value)
}

fn percent(value: &str) -> Option<f32> {
    let value = finite(value.strip_suffix('%')?)? / 100.0;
    (0.0..=1.0).contains(&value).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_pasted_hex_with_alpha_and_whitespace() {
        let color = parse("  #0F111759\n").unwrap();
        assert!((color.alpha - 89.0 / 255.0).abs() < 0.0001);
        assert_eq!(parse("0F111759"), Some(color));
    }

    #[test]
    fn preserves_exact_hsla_components() {
        let color = parse("hsla(224, 21%, 7%, 0.5)").unwrap();
        assert!((color.hue.into_positive_degrees() - 224.0).abs() < 0.0001);
        assert_eq!(color.saturation, 0.21);
        assert_eq!(color.lightness, 0.07);
        assert_eq!(color.alpha, 0.5);
        assert_eq!(parse("HSL(224deg, 21%, 7%)").unwrap().alpha, 1.0);
        assert_eq!(parse("hsla(224, 21%, 7%, 50%)"), Some(color));
    }

    #[test]
    fn incomplete_and_nonfinite_inputs_do_not_commit_a_color() {
        for value in [
            "",
            "#",
            "#0F11175",
            "a😀b",
            "#中文",
            "hsla(224,",
            "hsla(NaN, 21%, 7%, 0.5)",
            "hsla(224, 101%, 7%, 0.5)",
            "hsla(224, 21%, 7%, 2)",
        ] {
            assert!(parse(value).is_none(), "{value}");
        }
    }
}
