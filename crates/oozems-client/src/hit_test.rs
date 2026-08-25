#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Point {
    pub x: f64,
    pub y: f64,
}

pub(crate) fn contains(
    rect: Rect,
    point: Point,
) -> bool {
    point.x >= rect.x
        && point.x < rect.x + rect.width
        && point.y >= rect.y
        && point.y < rect.y + rect.height
}

pub(crate) fn contains_inclusive(
    rect: Rect,
    point: Point,
) -> bool {
    point.x >= rect.x
        && point.x <= rect.x + rect.width
        && point.y >= rect.y
        && point.y <= rect.y + rect.height
}

#[cfg(test)]
mod tests {
    use super::Point;
    use super::Rect;
    use super::contains;
    use super::contains_inclusive;

    const RECT: Rect = Rect {
        x: 10.0,
        y: 20.0,
        width: 5.0,
        height: 5.0,
    };

    #[test]
    fn half_open_rectangles_exclude_right_and_bottom_edges() {
        assert!(contains(RECT, Point { x: 10.0, y: 20.0 }));
        assert!(!contains(RECT, Point { x: 15.0, y: 20.0 }));
        assert!(!contains(RECT, Point { x: 10.0, y: 25.0 }));
    }

    #[test]
    fn inclusive_rectangles_include_all_four_edges() {
        assert!(contains_inclusive(RECT, Point { x: 10.0, y: 20.0 }));
        assert!(contains_inclusive(RECT, Point { x: 15.0, y: 25.0 }));
    }
}
