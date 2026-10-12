use glam::{Mat4, Vec3};

/// Orbit camera around a target point (Substance/Maya style controls).
#[derive(Clone, Debug)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub fov_y: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self { target: Vec3::ZERO, yaw: 0.6, pitch: 0.35, distance: 3.0, fov_y: 40f32.to_radians() }
    }
}

impl OrbitCamera {
    pub fn eye(&self) -> Vec3 {
        let dir = Vec3::new(
            self.pitch.cos() * self.yaw.sin(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.cos(),
        );
        self.target + dir * self.distance
    }

    pub fn view(&self) -> Mat4 {
        glam::camera::rh::view::look_at_mat4(self.eye(), self.target, Vec3::Y)
    }

    pub fn projection(&self, aspect: f32) -> Mat4 {
        let near = (self.distance * 0.01).max(1e-4);
        let far = self.distance * 100.0 + 10.0;
        glam::camera::rh::proj::directx::perspective(self.fov_y, aspect.max(1e-3), near, far)
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * 0.008;
        self.pitch = (self.pitch + dy * 0.008).clamp(-1.55, 1.55);
    }

    /// Pans in the view plane; `dx`/`dy` are in pixels, `viewport_h` in pixels.
    pub fn pan(&mut self, dx: f32, dy: f32, viewport_h: f32) {
        let view = self.view();
        let right = Vec3::new(view.x_axis.x, view.y_axis.x, view.z_axis.x);
        let up = Vec3::new(view.x_axis.y, view.y_axis.y, view.z_axis.y);
        let units_per_px = 2.0 * self.distance * (self.fov_y * 0.5).tan() / viewport_h.max(1.0);
        self.target += (-right * dx + up * dy) * units_per_px;
    }

    pub fn zoom(&mut self, factor: f32) {
        self.distance = (self.distance * factor).clamp(1e-3, 1e6);
    }

    /// Fits a bounding sphere into the view; `aspect` = width / height.
    pub fn frame(&mut self, center: Vec3, radius: f32, aspect: f32) {
        let half_y = self.fov_y * 0.5;
        let half_x = (half_y.tan() * aspect.max(1e-3)).atan();
        self.target = center;
        self.distance = radius / half_y.min(half_x).sin() * 1.05;
    }
}
