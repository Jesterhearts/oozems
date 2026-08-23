use oozems_proto::v1::Platform;

pub(super) fn attached_platform(
    platforms: &[Platform],
    foothold_id: u32,
    x: f32,
    reference_y: f32,
) -> Option<(i32, f32)> {
    let attached = platforms
        .iter()
        .find(|platform| foothold_id != 0 && platform.id == foothold_id)
        .and_then(|platform| {
            platform_surface_at_x(platform, x).map(|surface| (platform.layer, surface))
        });
    if attached.is_some() {
        return attached;
    }
    platforms
        .iter()
        .filter_map(|platform| {
            let surface = platform_surface_at_x(platform, x)?;
            Some((platform.layer, surface, (surface - reference_y).abs()))
        })
        .min_by(|left, right| left.2.total_cmp(&right.2))
        .map(|(layer, surface, _)| (layer, surface))
}

fn platform_surface_at_x(
    platform: &Platform,
    x: f32,
) -> Option<f32> {
    let minimum_x = platform.x.min(platform.end_x);
    let maximum_x = platform.x.max(platform.end_x);
    if !(minimum_x..=maximum_x).contains(&x) {
        return None;
    }
    let delta_x = platform.end_x - platform.x;
    if delta_x.abs() < f32::EPSILON {
        return None;
    }
    let progress = (x - platform.x) / delta_x;
    Some(platform.y + progress * (platform.end_y - platform.y))
}
