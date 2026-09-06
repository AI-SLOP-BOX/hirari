#[derive(Default)]
pub struct SpatialOrchestrator;
impl SpatialOrchestrator {
    pub fn new() -> Self {
        Self
    }
    pub fn pan_714(&self, x: f32, y: f32, z: f32, input: f32, output: &mut [f32; 12]) {
        output.fill(0.0);
        let index = if y > 0.5 {
            1
        } else if x < -0.5 {
            0
        } else if x > 0.5 {
            2
        } else {
            1
        };
        output[index] = input * (1.0 - z.abs().min(1.0) * 0.25);
    }
}
