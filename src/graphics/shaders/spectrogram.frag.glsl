#version 450
/**
 * @brief Vulkan Spectrogram Shader.
 */

layout(location = 0) in vec2 texCoord;
layout(location = 0) out vec4 outColor;

layout(binding = 0) uniform sampler2D specSampler;

void main() {
    float intensity = texture(specSampler, texCoord).r;
    
    // SPECTRAL COLORMAP (Vulkan Edition)
    vec3 low = vec3(0.02, 0.0, 0.15);  // Deep Space
    vec3 mid = vec3(0.9, 0.2, 0.0);    // Magma
    vec3 high = vec3(1.0, 0.95, 0.2);  // Supernova
    
    // Gain Scaling: Scale intensity for visual vibrance (1.5x amplification)
    vec3 color = mix(low, mid, clamp(intensity * 1.5, 0.0, 1.0));
    
    // Saturation Threshold: Transition to supernova highlights above 60% intensity
    // Slope (2.5) ensures a sharp transition for peak clarity
    color = mix(color, high, clamp((intensity - 0.6) * 2.5, 0.0, 1.0));
    
    outColor = vec4(color, 1.0);
}
