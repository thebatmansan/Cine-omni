use cine_previs::optics::*;
use glam::DVec4;
use ucg_schema::{CameraParams, Transform};

fn full_frame_35() -> CameraParams {
    let mut p = CameraParams::default();
    SensorPreset::FullFrame.apply(&mut p);
    p.focal_length_mm = 35.0;
    p.f_stop = 2.8;
    p.focus_distance_m = 3.0;
    p
}

#[test]
fn horizontal_fov_35mm_full_frame() {
    let p = full_frame_35();
    assert!(
        (horizontal_fov(&p) - 54.43).abs() < 0.02,
        "{}",
        horizontal_fov(&p)
    );
    assert!((focal_for_fov(36.0, horizontal_fov(&p)) - 35.0).abs() < 1e-9);
    // Super 35 crop factor ≈ 1.39 → 35 mm ≈ 49 mm equivalent.
    let s35 = SensorPreset::Super35.sensor();
    assert!((equivalent_focal_ff(35.0, s35.width_mm, s35.height_mm) - 48.7).abs() < 0.3);
}

#[test]
fn hyperfocal_and_dof_around_three_metres() {
    let h = hyperfocal_m(35.0, 2.8, 0.03);
    assert!((h - 14.62).abs() < 0.05, "{h}");
    let near = dof_near_m(35.0, 2.8, 0.03, 3.0);
    let far = dof_far_m(35.0, 2.8, 0.03, 3.0);
    assert!((near - 2.50).abs() < 0.01, "{near}");
    assert!((far - 3.77).abs() < 0.01, "{far}");
    assert!((total_dof_m(35.0, 2.8, 0.03, 3.0) - (far - near)).abs() < 1e-12);
    // Beyond the hyperfocal distance the far limit is infinite.
    assert!(dof_far_m(35.0, 2.8, 0.03, 20.0).is_infinite());
    assert!(total_dof_m(35.0, 2.8, 0.03, 20.0).is_infinite());
    // Default CoC follows the diagonal / 1500 rule.
    assert!((circle_of_confusion_mm(36.0, 24.0) - 0.02884).abs() < 1e-4);
    let (n, f) = dof_for(&full_frame_35());
    assert!(n < 3.0 && f > 3.0);
}

#[test]
fn projection_maps_frustum_corners_to_ndc() {
    let p = full_frame_35();
    let cam = look_at([2.0, 1.5, 6.0], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]);
    let fr = Frustum::from_camera(&cam, &p).unwrap();
    let vp = projection_matrix(&p) * view_matrix(&cam);
    let expect = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    for (i, c) in fr.near.iter().enumerate() {
        let clip = vp * DVec4::new(c.x, c.y, c.z, 1.0);
        let ndc = clip / clip.w;
        assert!((ndc.x - expect[i].0).abs() < 1e-6, "near x {ndc}");
        assert!((ndc.y - expect[i].1).abs() < 1e-6, "near y {ndc}");
        assert!((ndc.z + 1.0).abs() < 1e-6, "near z {ndc}");
    }
    for (i, c) in fr.far.iter().enumerate() {
        let clip = vp * DVec4::new(c.x, c.y, c.z, 1.0);
        let ndc = clip / clip.w;
        assert!((ndc.x - expect[i].0).abs() < 1e-6, "far x {ndc}");
        assert!((ndc.y - expect[i].1).abs() < 1e-6, "far y {ndc}");
        assert!((ndc.z - 1.0).abs() < 1e-6, "far z {ndc}");
    }
    // Centre of the frame projects to (0.5, 0.5).
    let centre = project_point(&p, &cam, [0.0, 1.0, 0.0]).unwrap();
    assert!((centre[0] - 0.5).abs() < 1e-9 && (centre[1] - 0.5).abs() < 1e-9);
    // Points inside are inside.
    assert!(fr.contains_point([0.0, 1.0, 0.0]));
    assert!(!fr.contains_point([2.0, 1.5, 7.0]));
}

#[test]
fn anamorphic_projection_widens_aspect() {
    let p = full_frame_35();
    let sph = projection_matrix(&p);
    let ana = projection_matrix_anamorphic(&p, AnamorphicSqueeze::X2);
    // x scale halves when the horizontal aperture doubles; y unchanged.
    assert!((ana.x_axis.x * 2.0 - sph.x_axis.x).abs() < 1e-12);
    assert!((ana.y_axis.y - sph.y_axis.y).abs() < 1e-12);
    assert!((AnamorphicSqueeze::X1_33.factor() - 1.33).abs() < 1e-12);
}

#[test]
fn exposure_and_shutter() {
    assert!((shutter_angle_to_seconds(172.8, 24.0) - 0.02).abs() < 1e-12);
    let p = CameraParams::default(); // ISO 800 f/2.8 180° 24fps
    assert!((ev_for(&p) - 5.556).abs() < 0.01);
    assert!((f_stop_to_t_stop(2.8, 1.0) - 2.8).abs() < 1e-12);
}

#[test]
fn view_matrix_is_inverse_of_transform() {
    let t = Transform {
        translate: [1.0, 2.0, 3.0],
        rotate: [0.0, 0.3826834, 0.0, 0.9238795],
        scale: [1.0; 3],
    };
    let m = t.to_mat4() * view_matrix(&t);
    for c in 0..4 {
        for r in 0..4 {
            let want = if c == r { 1.0 } else { 0.0 };
            assert!((m.col(c)[r] - want).abs() < 1e-6);
        }
    }
}
