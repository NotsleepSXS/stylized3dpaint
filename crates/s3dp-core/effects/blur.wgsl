fn effect(c: Ctx) -> vec4<f32> {
    return blur_world(c, p_intensity() * radius());
}
