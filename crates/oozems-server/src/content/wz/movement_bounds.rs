use oozems_proto::v1::MapMovementBounds;

use super::Bounds;
use super::RawPlatform;

pub(super) fn build(
    footholds: &[RawPlatform],
    portal_xs: &[i32],
    bounds: Bounds,
) -> MapMovementBounds {
    let player_half_width = super::super::PLAYER_HALF_WIDTH.ceil() as i32;
    let vertical_range = coordinate_range(
        footholds
            .iter()
            .filter(|foothold| foothold.x1 == foothold.x2)
            .map(|foothold| foothold.x1),
    )
    .filter(|(left, right)| {
        left < right
            && portal_xs.iter().all(|portal_x| {
                left.saturating_add(player_half_width) <= *portal_x
                    && *portal_x <= right.saturating_sub(player_half_width)
            })
    });
    let foothold_range = coordinate_range(
        footholds
            .iter()
            .flat_map(|foothold| [foothold.x1, foothold.x2])
            .chain(portal_xs.iter().flat_map(|portal_x| {
                [
                    portal_x.saturating_sub(player_half_width),
                    portal_x.saturating_add(player_half_width),
                ]
            })),
    );
    let (left, right) = vertical_range
        .or(foothold_range)
        .unwrap_or((bounds.left, bounds.right));
    let left = left.clamp(bounds.left, bounds.right);
    let right = right.clamp(bounds.left, bounds.right);
    let inset = super::super::PLAYER_HALF_WIDTH.min((right - left) as f32 / 2.0);
    MapMovementBounds {
        left: (left - bounds.left) as f32 + inset,
        right: (right - bounds.left) as f32 - inset,
    }
}

fn coordinate_range(values: impl IntoIterator<Item = i32>) -> Option<(i32, i32)> {
    let mut values = values.into_iter();
    let first = values.next()?;
    Some(values.fold((first, first), |(minimum, maximum), value| {
        (minimum.min(value), maximum.max(value))
    }))
}

#[cfg(test)]
mod tests {
    use super::Bounds;
    use super::RawPlatform;
    use super::build;

    #[test]
    fn outer_vertical_footholds_define_movement_bounds() {
        let footholds = [
            RawPlatform {
                id: 1,
                x1: -100,
                y1: 300,
                x2: 200,
                y2: 300,
                layer: 0,
            },
            RawPlatform {
                id: 2,
                x1: -80,
                y1: 300,
                x2: -80,
                y2: 500,
                layer: 0,
            },
            RawPlatform {
                id: 3,
                x1: 180,
                y1: 300,
                x2: 180,
                y2: 500,
                layer: 0,
            },
        ];

        let movement = build(
            &footholds,
            &[],
            Bounds {
                left: -100,
                top: 0,
                right: 300,
                bottom: 600,
            },
        );

        assert_eq!(movement.left, 38.0);
        assert_eq!(movement.right, 262.0);
    }

    #[test]
    fn internal_vertical_footholds_cannot_exclude_portals() {
        let footholds = [
            RawPlatform {
                id: 1,
                x1: -100,
                y1: 300,
                x2: 900,
                y2: 300,
                layer: 0,
            },
            RawPlatform {
                id: 2,
                x1: 400,
                y1: 200,
                x2: 400,
                y2: 500,
                layer: 0,
            },
            RawPlatform {
                id: 3,
                x1: 600,
                y1: 200,
                x2: 600,
                y2: 500,
                layer: 0,
            },
        ];

        let movement = build(
            &footholds,
            &[0, 800],
            Bounds {
                left: -100,
                top: 0,
                right: 900,
                bottom: 600,
            },
        );

        assert_eq!(movement.left, 18.0);
        assert_eq!(movement.right, 982.0);
    }
}
