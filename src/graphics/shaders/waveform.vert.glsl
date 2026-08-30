#version 450

/**
 * @brief VulkanWaveform: High-performance GLSL shader for cross-platform audio visualization.
 * Uses Storage Buffer Objects (SSBO) for fast peak data access.
 */

layout(location = 0) in float2 inPos;
layout(location = 0) out vec4 outColor;

layout(binding = 0) buffer WaveformData {
    float peakPairs[]; // Interleaved [min, max, min, max...]
};

layout(push_constant) uniform PushConstants {
    vec2 viewRange;
    vec4 drawColor;
} pc;

void main() {
    uint vid = gl_VertexIndex;
    uint pairIdx = vid / 2;
    
    float x = float(vid) / (pc.viewRange.y - pc.viewRange.x);
    float y = (vid % 2 == 0) ? peakPairs[pairIdx * 2] : peakPairs[pairIdx * 2 + 1];
    
    // NDC Transformation: Map [0, 1] range to OpenGL/Vulkan [-1, 1] screen space.
    // Factor 2.0 scales width to full viewport; -1.0 centers it.
    gl_Position = vec4(x * 2.0 - 1.0, y, 0.0, 1.0);
    outColor = pc.drawColor;
}
