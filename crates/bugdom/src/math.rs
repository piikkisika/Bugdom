//! Angle helpers that gameplay code shares.
//!
//! Port of parts of original/src/QD3D/3DMath.c. Yaw angles follow the
//! original: an object with yaw 0 faces −Z, and yaw grows counter-clockwise
//! seen from above, the same as `Quat::from_rotation_y`.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use bevy::prelude::*;

/// Wraps an angle into `0.0..TAU`. Port of `MaskAngle`.
pub fn mask_angle(angle: f32) -> f32 {
    // Truncating division, then a single correction for negative angles,
    // exactly as the original does it.
    let mut wrapped = angle - (angle / TAU).trunc() * TAU;
    if angle < 0.0 {
        wrapped += TAU;
    }
    wrapped
}

/// The yaw that faces from one point to another in the XZ plane, or
/// `old_yaw` if the points are too close to tell.
/// Port of `CalcYAngleFromPointToPoint`.
pub fn yaw_from_point_to_point(old_yaw: f32, from: Vec2, to: Vec2) -> f32 {
    let d = from - to;
    if d.x.abs() < 0.0001 && d.y.abs() < 0.0001 {
        return old_yaw;
    }
    let yaw = TAU - mask_angle(d.y.atan2(d.x) - FRAC_PI_2);
    if yaw >= TAU - 0.0001 { 0.0 } else { yaw }
}

/// Turns `yaw` toward the target point by at most `max_turn` radians, the
/// short way round. Returns the new yaw (in `0.0..TAU`) and the angle that
/// is left to turn.
///
/// Port of `TurnObjectTowardTarget`, with the turn speed already multiplied
/// by the frame time.
pub fn turn_toward(yaw: f32, from: Vec2, target: Vec2, max_turn: f32) -> (f32, f32) {
    if (from.x - target.x).abs() < 1.0 && (from.y - target.y).abs() < 1.0 {
        return (yaw, 0.0);
    }
    let desired = yaw_from_point_to_point(yaw, from, target);
    let diff = desired - yaw;
    let turned = if diff > max_turn {
        if diff > PI {
            yaw - max_turn
        } else {
            yaw + max_turn
        }
    } else if diff < -max_turn {
        if diff < -PI {
            yaw + max_turn
        } else {
            yaw - max_turn
        }
    } else {
        desired
    };
    let turned = mask_angle(turned);
    (turned, (desired - turned).abs())
}

/// An approximate 2D distance: the longer axis plus 3/8 of the shorter one.
/// Port of `CalcQuickDistance`.
pub fn quick_distance(a: Vec2, b: Vec2) -> f32 {
    let d = (a - b).abs();
    d.max_element() + 0.375 * d.min_element()
}

/// The yaw of a rotation about Y, in `0.0..TAU`.
pub fn yaw_of(rotation: Quat) -> f32 {
    mask_angle(rotation.to_euler(EulerRot::YXZ).0)
}

/// The direction an object with this yaw faces, in the XZ plane.
pub fn yaw_forward(yaw: f32) -> Vec2 {
    Vec2::new(-yaw.sin(), -yaw.cos())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_angle_wraps_like_the_original() {
        assert!((mask_angle(TAU + 1.0) - 1.0).abs() < 1e-5);
        assert!((mask_angle(-1.0) - (TAU - 1.0)).abs() < 1e-6);
        assert_eq!(mask_angle(0.5), 0.5);
    }

    #[test]
    fn yaw_toward_a_point_matches_bevy_rotation() {
        for yaw in [0.0, 0.5, 2.0, 4.0, 6.0] {
            let forward = yaw_forward(yaw);
            let bevy = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;
            assert!(forward.abs_diff_eq(bevy.xz(), 1e-6));
            let found = yaw_from_point_to_point(0.0, Vec2::ZERO, forward * 100.0);
            assert!((found - yaw).abs() < 1e-4, "{yaw} → {found}");
        }
    }

    #[test]
    fn turning_takes_the_short_way() {
        let target = yaw_forward(0.2) * 100.0;
        let (yaw, left) = turn_toward(6.0, Vec2::ZERO, target, 0.1);
        assert!((yaw - 6.1).abs() < 1e-5);
        assert!(left > 0.0);
        let (yaw, left) = turn_toward(0.15, Vec2::ZERO, target, 0.1);
        assert!((yaw - 0.2).abs() < 1e-5 && left < 1e-5);
    }
}
