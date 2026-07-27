pub(crate) fn frame_selected(frame: usize, selector: Option<&str>) -> bool {
    let Some(selector) = selector.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };
    selector.split(',').any(|part| {
        let part = part.trim();
        if let Some((start, end)) = part.split_once("..=") {
            return parse_frame(start).zip(parse_frame(end)).is_some_and(
                |(start, end)| (start..=end).contains(&frame),
            );
        }
        if let Some((start, end)) = part.split_once("..") {
            return parse_frame(start)
                .zip(parse_frame(end))
                .is_some_and(|(start, end)| (start..end).contains(&frame));
        }
        parse_frame(part) == Some(frame)
    })
}

fn parse_frame(value: &str) -> Option<usize> {
    value.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::frame_selected;

    #[test]
    fn accepts_single_frames_lists_and_ranges() {
        assert!(frame_selected(12, Some("12")));
        assert!(frame_selected(12, Some("4, 12, 30")));
        assert!(frame_selected(12, Some("10..15")));
        assert!(frame_selected(15, Some("10..=15")));
        assert!(!frame_selected(15, Some("10..15")));
        assert!(!frame_selected(12, Some("1, 2..8")));
        assert!(frame_selected(12, None));
    }
}
