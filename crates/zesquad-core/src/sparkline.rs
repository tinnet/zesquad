//! Small line charts drawn as SVG paths on the server, so pages need no script for them.

/// A line through some values over a stretch of time, fitted into a `width` × `height` box.
#[derive(Debug, Clone, PartialEq)]
pub struct Sparkline {
    /// SVG path data, `M x,y L x,y ...`.
    pub path: String,
    /// Where the latest value is, to mark it.
    pub last_x: f64,
    pub last_y: f64,
}

/// Draws `points` (time, value) from `from` to `to`: time runs left to right, so gaps in the
/// data show as long segments, and the values fill the height between their own lowest and
/// highest (a flat line sits in the middle). `pad` keeps the line's ends and stroke off the
/// edges. Points outside the stretch are clamped to it. `None` without points.
#[must_use]
pub fn sparkline(
    points: &[(i64, u64)],
    from: i64,
    to: i64,
    width: f64,
    height: f64,
    pad: f64,
) -> Option<Sparkline> {
    let mut points = points.to_vec();
    points.sort_by_key(|&(at, _)| at);
    let lo = points.iter().map(|&(_, v)| v).min()?;
    let hi = points.iter().map(|&(_, v)| v).max()?;
    let span = (to - from).max(1);
    // Precision only matters to a pixel, so the casts losing some is fine.
    #[allow(clippy::cast_precision_loss)]
    let point = |at: i64, value: u64| {
        let t = (at.clamp(from, to) - from) as f64 / span as f64;
        let x = pad + t * (width - 2.0 * pad);
        let y = if hi == lo {
            height / 2.0
        } else {
            pad + (hi - value) as f64 / (hi - lo) as f64 * (height - 2.0 * pad)
        };
        (round(x), round(y))
    };
    let coords: Vec<(f64, f64)> = points.iter().map(|&(at, v)| point(at, v)).collect();
    let path = coords
        .iter()
        .map(|(x, y)| format!("{x},{y}"))
        .collect::<Vec<_>>()
        .join(" L");
    let (last_x, last_y) = *coords.last()?;
    Some(Sparkline {
        path: format!("M{path}"),
        last_x,
        last_y,
    })
}

/// To a tenth of a pixel, which keeps the path short.
fn round(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn draws_time_left_to_right_and_high_values_up() {
        let line = sparkline(&[(10, 5), (0, 1), (5, 3)], 0, 10, 100.0, 20.0, 2.0).unwrap();
        assert_eq!(line.path, "M2,18 L50,10 L98,2");
        assert_eq!((line.last_x, line.last_y), (98.0, 2.0));
    }

    #[test]
    fn a_flat_line_sits_in_the_middle() {
        let line = sparkline(&[(0, 7), (5, 7)], 0, 10, 100.0, 20.0, 2.0).unwrap();
        assert_eq!(line.path, "M2,10 L50,10");
    }

    #[test]
    fn nothing_to_draw_without_points() {
        assert_eq!(sparkline(&[], 0, 10, 100.0, 20.0, 2.0), None);
    }

    proptest! {
        #[test]
        fn stays_inside_the_box(
            points in prop::collection::vec((-100i64..1_000, 0u64..1_000_000), 1..50),
            from in -50i64..500,
            len in 0i64..1_000,
        ) {
            let line = sparkline(&points, from, from + len, 96.0, 24.0, 2.0).unwrap();
            let coords: Vec<&str> = line.path.split(['M', 'L', ' ']).filter(|s| !s.is_empty()).collect();
            prop_assert_eq!(coords.len(), points.len());
            for c in coords {
                let (x, y) = c.split_once(',').unwrap();
                let (x, y): (f64, f64) = (x.parse().unwrap(), y.parse().unwrap());
                prop_assert!((2.0..=94.0).contains(&x), "x {x}");
                prop_assert!((2.0..=22.0).contains(&y), "y {y}");
            }
        }
    }
}
